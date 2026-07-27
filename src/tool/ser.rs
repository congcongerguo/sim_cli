//! Echo server tool — 在 TUI 内部运行一个 TCP JSON echo server。
//!
//! 用法（在 ser tab 中）：
//!   start  — 启动 echo server
//!   stop   — 停止 server
//!
//! 另一个 conn tab 用 `con tcp` 连接过来，`send` 发 JSON ping，
//! 本 server 会原样 echo 回去，同时把收到的每条消息显示在日志和 state panel 中。
//!
//! ## 与外部 echo_server bin 的关系
//! `src/bin/echo_server.rs` 是一个独立可执行文件（用于手动测试），
//! 本文件把它做成了 Tool，可以在 TUI 内用 `start`/`stop` 控制。

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::json_framer::JsonFramer;
use crate::message::{LogLevel, Message};

use super::{cmd, msg, Cmd, Tool, ToolState};

const MAX_PENDING: usize = 16 * 1024 * 1024;
const READ_CHUNK: usize = 4096;

// ── server → tool 事件 ──────────────────────────────────────────────────

enum SerEvent {
    Listening { addr: String },
    Connected { peer: SocketAddr },
    Disconnected { peer: SocketAddr },
    /// 收到一个完整 JSON 消息，已 echo 回客户端。
    Recv { peer: SocketAddr, json: String },
    Error { peer: Option<SocketAddr>, msg: String },
}

// ── SerTool ─────────────────────────────────────────────────────────────

pub struct SerTool {
    state: State,
    shutdown: Option<oneshot::Sender<()>>,
    ev_rx: mpsc::Receiver<SerEvent>,
    ev_tx: mpsc::Sender<SerEvent>,
    addr: String,
    conns: Vec<SocketAddr>,
    recv_count: u64,
    latest_recv: Option<serde_json::Value>,
}

enum State {
    Idle,
    Listening { addr: String },
}

impl SerTool {
    pub fn new(def: &'static super::registry::ToolDef) -> Self {
        let (ev_tx, ev_rx) = mpsc::channel(64);
        Self {
            state: State::Idle,
            shutdown: None,
            ev_rx,
            ev_tx,
            addr: def.tcp_addr().to_string(),
            conns: Vec::new(),
            recv_count: 0,
            latest_recv: None,
        }
    }
}

impl Tool for SerTool {
    fn commands(&self) -> Vec<Cmd> {
        vec![
            cmd("start", "start echo server"),
            cmd("stop", "stop echo server"),
        ]
    }

    fn handle(&mut self, cmd: &str, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "start" => {
                if matches!(self.state, State::Listening { .. }) {
                    return vec![msg("server already running", LogLevel::Warn)];
                }
                let (shutdown_tx, shutdown_rx) = oneshot::channel();
                let ev_tx = self.ev_tx.clone();
                let addr = self.addr.clone();

                tokio::spawn(run_server(addr, ev_tx, shutdown_rx));

                self.shutdown = Some(shutdown_tx);
                vec![msg(
                    &format!("starting echo server on {}...", self.addr),
                    LogLevel::Notice,
                )]
            }
            "stop" => {
                if self.shutdown.take().is_some() {
                    self.state = State::Idle;
                    self.conns.clear();
                    vec![msg("server stopped", LogLevel::Notice)]
                } else {
                    vec![msg("server not running", LogLevel::Warn)]
                }
            }
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        let mut msgs = Vec::new();
        while let Ok(ev) = self.ev_rx.try_recv() {
            match ev {
                SerEvent::Listening { addr } => {
                    self.state = State::Listening { addr };
                }
                SerEvent::Connected { peer } => {
                    self.conns.push(peer);
                    msgs.push(msg(
                        &format!("[+] client connected: {peer}"),
                        LogLevel::Notice,
                    ));
                }
                SerEvent::Disconnected { peer } => {
                    self.conns.retain(|p| *p != peer);
                    msgs.push(msg(
                        &format!("[-] client disconnected: {peer}"),
                        LogLevel::Info,
                    ));
                }
                SerEvent::Recv { peer, json } => {
                    self.recv_count += 1;
                    let n = self.recv_count;
                    // 尝试解析 JSON 以便在 state panel 中展示。
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                        self.latest_recv = Some(v);
                    }
                    msgs.push(Message::System {
                        text: format!("<- echo #{n} from {peer}: {json}"),
                        level: LogLevel::Debug,
                    });
                }
                SerEvent::Error { peer, msg: err } => {
                    if let Some(p) = peer {
                        self.conns.retain(|x| *x != p);
                    }
                    msgs.push(msg(&format!("server error: {err}"), LogLevel::Error));
                }
            }
        }
        msgs
    }

    fn snapshot(&self) -> ToolState {
        match &self.state {
            State::Idle => ToolState {
                active: false,
                badge: Some("ser: off".into()),
                ..Default::default()
            },
            State::Listening { addr } => {
                let mut fields = vec![
                    ("addr".into(), addr.clone()),
                    ("clients".into(), self.conns.len().to_string()),
                ];
                if self.recv_count > 0 {
                    fields.push(("recv".into(), self.recv_count.to_string()));
                }
                if let Some(ref v) = self.latest_recv {
                    crate::ui::state_panel::flatten_json("", v, &mut fields);
                }
                ToolState {
                    active: true,
                    badge: Some(format!("ser: {addr} ({} client/s)", self.conns.len())),
                    fields,
                }
            }
        }
    }

    fn tick_ms(&self) -> u64 {
        50
    }
}

// ── 后台 task ───────────────────────────────────────────────────────────

async fn run_server(
    addr: String,
    ev_tx: mpsc::Sender<SerEvent>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            let _ = ev_tx
                .send(SerEvent::Error {
                    peer: None,
                    msg: format!("bind failed: {e}"),
                })
                .await;
            return;
        }
    };
    let _ = ev_tx
        .send(SerEvent::Listening {
            addr: listener.local_addr().unwrap().to_string(),
        })
        .await;

    loop {
        tokio::select! {
            result = listener.accept() => match result {
                Ok((sock, peer)) => {
                    let _ = ev_tx.send(SerEvent::Connected { peer }).await;
                    let tx = ev_tx.clone();
                    tokio::spawn(handle_client(sock, peer, tx));
                }
                Err(e) => {
                    let _ = ev_tx.send(SerEvent::Error {
                        peer: None,
                        msg: format!("accept error: {e}"),
                    }).await;
                }
            },
            _ = &mut shutdown => break,
        }
    }
}

async fn handle_client(
    sock: TcpStream,
    peer: SocketAddr,
    ev_tx: mpsc::Sender<SerEvent>,
) {
    let (mut read, mut write) = sock.into_split();
    let mut framer = JsonFramer::new();
    let mut chunk = [0u8; READ_CHUNK];

    loop {
        let n = match read.read(&mut chunk).await {
            Ok(0) => {
                let _ = ev_tx.send(SerEvent::Disconnected { peer }).await;
                return;
            }
            Ok(n) => n,
            Err(e) => {
                let _ = ev_tx
                    .send(SerEvent::Error {
                        peer: Some(peer),
                        msg: format!("read: {e}"),
                    })
                    .await;
                return;
            }
        };

        framer.feed(&chunk[..n]);
        if framer.pending_bytes() > MAX_PENDING {
            let _ = ev_tx
                .send(SerEvent::Error {
                    peer: Some(peer),
                    msg: format!("buffer exceeded {MAX_PENDING} bytes"),
                })
                .await;
            return;
        }

        while let Some(msg) = framer.next_message() {
            // 原样 echo 回客户端
            if let Err(e) = write.write_all(&msg).await {
                let _ = ev_tx
                    .send(SerEvent::Error {
                        peer: Some(peer),
                        msg: format!("write: {e}"),
                    })
                    .await;
                return;
            }
            if let Err(e) = write.flush().await {
                let _ = ev_tx
                    .send(SerEvent::Error {
                        peer: Some(peer),
                        msg: format!("flush: {e}"),
                    })
                    .await;
                return;
            }
            // 通知 tool
            if let Ok(text) = String::from_utf8(msg) {
                let _ = ev_tx.send(SerEvent::Recv { peer, json: text }).await;
            }
        }
    }
}
