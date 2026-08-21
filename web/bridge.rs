//! 参考实现:tiny_http 桥 —— 浏览器 ⇄(SSE + POST)⇄ 本桥 ⇄(Unix socket)⇄ sim_cli。
//!
//! 这是给你**同板的 tiny_http web 服务**用的参考代码,可整段拷进去(或作为
//! 一个模块)。它让 sim_cli 本体**一行 web 代码都不用加**(体积零增长):web 相关
//! 的东西全在你这个本来就付了 tiny_http 代价的服务里。
//!
//! 依赖(你的 web 服务 Cargo.toml):
//! ```toml
//! tiny_http = "0.12"
//! # SSE 不需要 SHA-1 / md-5 / base64 —— 纯 HTTP。
//! ```
//!
//! 数据流(与经过测试的 web/dev-bridge.py 完全一致):
//! - `GET  /`        → 返回内嵌的 index.html(`include_str!`,单二进制)
//! - `GET  /events`  → 连一条到 sim_cli 的 socket;把它推来的每行 ServerMsg JSON
//!                     转成一条 SSE `data:` 事件;首个 `session` 事件带回 sid
//! - `POST /command?sid=...` → 按 sid 找到对应 socket,把请求体(ClientMsg JSON)写进去
//!
//! 每个浏览器 SSE 连接 = 一条独立 sim_cli 连接 = 一个独立 ViewSession
//! (各自过滤/滚动);命令按 sid 落到同一条连接上。
//!
//! 注意:本文件是**参考起点**,未随 sim_cli 一起编译。请按你的服务结构适配
//! (错误处理、鉴权、TLS、日志、线程模型等)。协议行为以 dev-bridge.py 为准。

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use tiny_http::{Header, Response, Server, StatusCode};

/// sim_cli 后端 socket 路径(与 `sim_cli --serve` 的默认一致)。
const SIM_CLI_SOCK: &str = "/tmp/sim_cli.sock";
/// 本桥监听地址。
const BIND: &str = "0.0.0.0:8080";

/// sid → 到 sim_cli 的 socket(写句柄)。SSE 连接建;POST 用。
type Sessions = Arc<Mutex<HashMap<String, UnixStream>>>;

pub fn run() -> io::Result<()> {
    let server = Server::http(BIND).map_err(|e| io::Error::other(e.to_string()))?;
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    // 线程/连接:SSE 会长时间占用一个线程,故每请求一个线程,避免固定池被 SSE 占满。
    for request in server.incoming_requests() {
        let sessions = sessions.clone();
        thread::spawn(move || {
            if let Err(e) = handle(request, sessions) {
                eprintln!("bridge: {e}");
            }
        });
    }
    Ok(())
}

fn handle(request: tiny_http::Request, sessions: Sessions) -> io::Result<()> {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("/");
    match (request.method().as_str(), path) {
        ("GET", "/") | ("GET", "/index.html") => serve_index(request),
        ("GET", "/events") => serve_events(request, sessions),
        ("POST", "/command") => serve_command(request, sessions, &url),
        _ => request.respond(Response::empty(StatusCode(404))),
    }
}

/// 返回内嵌的单文件前端。`include_str!` 把它编进你的服务二进制。
fn serve_index(request: tiny_http::Request) -> io::Result<()> {
    // 路径按本文件相对位置;拷进你的项目后改成你的实际路径。
    let html = include_str!("index.html");
    let resp = Response::from_string(html).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
    );
    request.respond(resp)
}

/// SSE:一条到 sim_cli 的连接,把其 NDJSON 帧转发为事件流。
fn serve_events(request: tiny_http::Request, sessions: Sessions) -> io::Result<()> {
    let sock = match UnixStream::connect(SIM_CLI_SOCK) {
        Ok(s) => s,
        Err(e) => {
            let _ = request.respond(Response::from_string(format!("backend: {e}")).with_status_code(502));
            return Ok(());
        }
    };
    let sid = new_sid();

    // 写句柄进 map(供 POST 用);读句柄进 SSE body。二者是同一 socket 的独立克隆。
    let write_handle = sock.try_clone()?;
    sessions.lock().unwrap().insert(sid.clone(), write_handle);

    // 声明全量、跟随底部的视口,后端便持续推 window。
    {
        let mut w = sock.try_clone()?;
        let _ = w.write_all(br#"{"type":"view","count":0,"follow":true}"#);
        let _ = w.write_all(b"\n");
    }

    let body = SseBody {
        reader: BufReader::new(sock),
        pending: Vec::new(),
        sid: sid.clone(),
        sent_header: false,
        sessions,
    };
    let resp = Response::new(
        StatusCode(200),
        vec![
            Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..]).unwrap(),
            Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..]).unwrap(),
        ],
        body,
        None, // 长度未知 → chunked;SSE 一直开着
        None,
    );
    request.respond(resp)
}

/// 把 sim_cli 的 NDJSON 行重帧成 SSE 事件的 `Read` 适配器。
struct SseBody {
    reader: BufReader<UnixStream>,
    pending: Vec<u8>,
    sid: String,
    sent_header: bool,
    sessions: Sessions,
}

impl Read for SseBody {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(out.len());
                out[..n].copy_from_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return Ok(n);
            }
            if !self.sent_header {
                self.sent_header = true;
                self.pending = format!("event: session\ndata: {}\n\n", self.sid).into_bytes();
                continue;
            }
            let mut line = String::new();
            let n = self.reader.read_line(&mut line)?;
            if n == 0 {
                return Ok(0); // 后端关闭 → 结束事件流
            }
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            self.pending = format!("data: {line}\n\n").into_bytes();
        }
    }
}

impl Drop for SseBody {
    fn drop(&mut self) {
        self.sessions.lock().unwrap().remove(&self.sid);
    }
}

/// POST /command?sid=...:把请求体(ClientMsg JSON)写进该 sid 的 socket。
fn serve_command(mut request: tiny_http::Request, sessions: Sessions, url: &str) -> io::Result<()> {
    let sid = query_param(url, "sid").unwrap_or_default();
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body)?;

    let sock = sessions.lock().unwrap().get(&sid).and_then(|s| s.try_clone().ok());
    match sock {
        Some(mut s) => {
            let _ = s.write_all(body.trim().as_bytes());
            let _ = s.write_all(b"\n");
            request.respond(Response::empty(StatusCode(204)))
        }
        None => request.respond(Response::from_string("unknown session").with_status_code(409)),
    }
}

// ── 小工具 ────────────────────────────────────────────────────────────────

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 无依赖的 sid:纳秒时间 + 自增计数,拼成十六进制。生产可换 uuid。
fn new_sid() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:x}{c:x}")
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(v.to_string());
            }
        }
    }
    None
}
