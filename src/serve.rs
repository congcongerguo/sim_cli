//! 无头后端服务(daemon)。`sim_cli --serve` 起一个常驻进程,监听 Unix
//! socket,用 NDJSON 线协议(见 [`crate::protocol`])对外提供服务。
//!
//! ## 架构
//! ```text
//!   backend::run  ──ViewState(watch)──▶  每连接任务  ──Window/Shared──▶ 客户端
//!        ▲                                   │
//!        └────────── Command(mpsc) ──────────┘  (Input/TagSwitch/Permission)
//! ```
//! - 复用现有 [`crate::backend::run`]:后端逻辑、Tool、路由完全不变。
//! - **共享状态**([`Shared`])广播给所有连接;**视口内容**([`Window`])按
//!   每连接的 [`ViewSession`] 单独计算(过滤 / 滚动 / grep 各看各的)。
//! - 过滤复用 [`crate::filter`],grep 复用 [`crate::msg_log::scan`](搜的是
//!   **服务端**板子上的落盘归档 —— 正确的位置)。
//!
//! 仅在 `serve` feature 且 Unix 平台下编入。

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
#[cfg(unix)]
use tokio::net::UnixListener;
use tokio::sync::{mpsc, watch};

use crate::backend::{Command, Mode, ViewState};
use crate::filter::Filter;
use crate::log_buffer::msg_line_count;
use crate::message::TimedMessage;
use crate::protocol::{
    ClientMsg, CmdDto, MsgDto, ModalDto, PROTOCOL_VERSION, Shared, ToolInfoDto, ToolStateDto,
    ViewReq, Window,
};

/// 默认 Unix socket 路径。可用 `SIM_CLI_SOCK` 覆盖。
#[cfg(unix)]
pub fn default_sock_path() -> String {
    std::env::var("SIM_CLI_SOCK").unwrap_or_else(|_| "/tmp/sim_cli.sock".to_string())
}

/// 默认 TCP 监听地址(仅回环,不对外)。可用 `SIM_CLI_TCP` 覆盖。
/// (unix 平台默认走 Unix socket,此函数仅在非 unix 回退时用到。)
#[cfg_attr(unix, allow(dead_code))]
pub fn default_tcp_addr() -> String {
    std::env::var("SIM_CLI_TCP").unwrap_or_else(|_| "127.0.0.1:7879".to_string())
}

/// 服务/连接端点。Unix socket 仅本机、零网络暴露(推荐,unix 平台);TCP
/// 跨平台(Windows 只能用它),对外时须前置 TLS。协议与传输解耦,换端点不改协议。
#[derive(Clone, Debug)]
pub enum Endpoint {
    /// Unix domain socket 路径(仅 unix 平台)。
    #[cfg(unix)]
    Unix(String),
    /// TCP 地址,如 `127.0.0.1:7879`。
    Tcp(String),
}

impl Endpoint {
    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(p) => format!("unix:{p}"),
            Endpoint::Tcp(a) => format!("tcp:{a}"),
        }
    }
}

/// 启动无头服务:创建后端,监听端点,为每个连接派生一个任务。
pub async fn run(endpoint: Endpoint) -> Result<()> {
    // 复用现有后端:Command 进 / ViewState 出。
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let (view_tx, view_rx) = watch::channel(ViewState::initial());
    tokio::spawn(crate::backend::run(cmd_rx, view_tx));

    eprintln!("sim_cli serving on {} (protocol v{PROTOCOL_VERSION})", endpoint.describe());

    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => {
            // 陈旧 socket 文件会导致 bind 失败;先清掉。
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path)?;
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let (r, w) = stream.into_split();
                        spawn_conn(r, w, &cmd_tx, &view_rx);
                    }
                    Err(e) => eprintln!("sim_cli: accept error: {e}"),
                }
            }
        }
        Endpoint::Tcp(addr) => {
            let listener = TcpListener::bind(&addr).await?;
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        let (r, w) = stream.into_split();
                        spawn_conn(r, w, &cmd_tx, &view_rx);
                    }
                    Err(e) => eprintln!("sim_cli: accept error: {e}"),
                }
            }
        }
    }
}

/// 为一条连接(已拆分的读写半)派生处理任务。传输无关。
fn spawn_conn<R, W>(
    read_half: R,
    write_half: W,
    cmd_tx: &mpsc::Sender<Command>,
    view_rx: &watch::Receiver<ViewState>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let cmd_tx = cmd_tx.clone();
    let view_rx = view_rx.clone();
    tokio::spawn(async move {
        if let Err(e) = handle_conn(read_half, write_half, cmd_tx, view_rx).await {
            eprintln!("sim_cli: connection ended: {e}");
        }
    });
}

/// 单连接处理:读客户端命令、按本连接的 [`ViewSession`] 推送共享状态与视口。
/// 传输无关:接受已拆分的读写半(Unix / TCP 皆可)。
async fn handle_conn<R, W>(
    read_half: R,
    mut write_half: W,
    cmd_tx: mpsc::Sender<Command>,
    mut view_rx: watch::Receiver<ViewState>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send,
{
    // 读端独立任务:解析 NDJSON → ClientMsg,投递到本连接主循环。
    let (client_tx, mut client_rx) = mpsc::channel::<ClientMsg>(64);
    tokio::spawn(async move {
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ClientMsg>(&line) {
                Ok(msg) => {
                    if client_tx.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    // 非法输入:回错误帧由主循环发,这里只发一个哨兵不方便,
                    // 简化为丢弃并让主循环无感(协议错误不该拖垮连接)。
                    let _ = e;
                }
            }
        }
    });

    let mut session = ViewSession::default();

    // 握手 + 首帧。
    send(&mut write_half, &crate::protocol::ServerMsg::Hello {
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: PROTOCOL_VERSION,
    })
    .await?;

    let mut last_shared: Option<Shared> = None;
    let mut last_window_json: Option<String> = None;

    // 先推一帧当前状态。clone 出快照,避免把 watch 的 borrow 跨 await 持有。
    let view0 = view_rx.borrow().clone();
    push_frames(&mut write_half, &view0, &session, &mut last_shared, &mut last_window_json).await?;

    loop {
        tokio::select! {
            changed = view_rx.changed() => {
                if changed.is_err() {
                    break; // 后端已退出
                }
                let view = view_rx.borrow().clone();
                push_frames(&mut write_half, &view, &session, &mut last_shared, &mut last_window_json).await?;
            }
            maybe = client_rx.recv() => {
                let Some(msg) = maybe else { break }; // 读端关闭
                match msg {
                    ClientMsg::Input { text } => {
                        // 客户端 "exit" 只断开自己,不杀后端(daemon 必须常驻)。
                        if text.trim() == "exit" {
                            break;
                        }
                        let _ = cmd_tx.send(Command::Input(text)).await;
                    }
                    ClientMsg::TagSwitch { name } => {
                        let _ = cmd_tx.send(Command::TagSwitch(name)).await;
                    }
                    ClientMsg::Permission { choice } => {
                        let _ = cmd_tx.send(Command::Permission(choice.into())).await;
                    }
                    ClientMsg::View(req) => {
                        session.set_view(req);
                        // 视口参数变化 → 立即重算本连接窗口。
                        let view = view_rx.borrow().clone();
                        push_window(&mut write_half, &view, &session, &mut last_window_json).await?;
                    }
                    ClientMsg::Grep { expr } => {
                        session.set_grep(&expr);
                        let view = view_rx.borrow().clone();
                        push_window(&mut write_half, &view, &session, &mut last_window_json).await?;
                    }
                    ClientMsg::Ungrep => {
                        session.clear_grep();
                        let view = view_rx.borrow().clone();
                        push_window(&mut write_half, &view, &session, &mut last_window_json).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// 推送共享状态(变化时)+ 视口内容(变化时)。
async fn push_frames(
    w: &mut (impl AsyncWriteExt + Unpin),
    view: &ViewState,
    session: &ViewSession,
    last_shared: &mut Option<Shared>,
    last_window_json: &mut Option<String>,
) -> Result<()> {
    let shared = build_shared(view);
    if last_shared.as_ref() != Some(&shared) {
        send(w, &crate::protocol::ServerMsg::Shared(shared.clone())).await?;
        *last_shared = Some(shared);
    }
    push_window(w, view, session, last_window_json).await
}

/// 推送视口内容(仅当与上次不同 —— 抑制空闲期的重复推送)。
async fn push_window(
    w: &mut (impl AsyncWriteExt + Unpin),
    view: &ViewState,
    session: &ViewSession,
    last_window_json: &mut Option<String>,
) -> Result<()> {
    let window = session.build(view);
    let json = serde_json::to_string(&crate::protocol::ServerMsg::Window(window))?;
    if last_window_json.as_deref() != Some(json.as_str()) {
        w.write_all(json.as_bytes()).await?;
        w.write_all(b"\n").await?;
        w.flush().await?;
        *last_window_json = Some(json);
    }
    Ok(())
}

async fn send(w: &mut (impl AsyncWriteExt + Unpin), msg: &crate::protocol::ServerMsg) -> Result<()> {
    let json = serde_json::to_string(msg)?;
    w.write_all(json.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

/// 从 [`ViewState`] 抽取共享状态 DTO。
fn build_shared(view: &ViewState) -> Shared {
    Shared {
        tools: view
            .tools
            .iter()
            .map(|t| ToolInfoDto { name: t.name.clone(), active: t.active, available: t.available })
            .collect(),
        active_index: view.active_index,
        active_cmds: view.active_cmds.iter().map(CmdDto::from_cmd).collect(),
        state: ToolStateDto {
            fields: view.state.fields.clone(),
            active: view.state.active,
            badge: view.state.badge.clone(),
        },
        mode: match view.mode {
            Mode::Plan => "plan".to_string(),
            Mode::Normal => "normal".to_string(),
        },
        streaming: view.streaming,
        modal: view.modal.as_ref().map(|m| ModalDto {
            tool_name: m.tool_name.clone(),
            args_preview: m.args_preview.clone(),
        }),
        should_quit: view.should_quit,
    }
}

// ── 每连接视口会话 ────────────────────────────────────────────────────────

/// 单个连接的视口状态:过滤 / grep / 视口请求。滚动/窗口逻辑复用
/// [`crate::scroll`],过滤复用 [`crate::filter`]。
#[derive(Default)]
pub struct ViewSession {
    req: ViewReq,
    include: Option<Filter>,
    exclude: Option<Filter>,
    filter_error: Option<String>,
    exclude_error: Option<String>,
    /// grep 结果:(表达式, 命中消息)。`Some` 时替代实时缓冲。
    grep: Option<(String, Vec<TimedMessage>)>,
    grep_error: Option<String>,
}

impl ViewSession {
    /// 应用一条视口请求:重新解析 include/exclude 过滤,记录 start/count/follow。
    pub fn set_view(&mut self, req: ViewReq) {
        // include
        self.include = None;
        self.filter_error = None;
        if let Some(src) = req.filter.as_deref().filter(|s| !s.trim().is_empty()) {
            match Filter::parse(src) {
                Ok(f) => self.include = Some(f),
                Err(e) => self.filter_error = Some(e),
            }
        }
        // exclude
        self.exclude = None;
        self.exclude_error = None;
        if let Some(src) = req.exclude.as_deref().filter(|s| !s.trim().is_empty()) {
            match Filter::parse(src) {
                Ok(f) => self.exclude = Some(f),
                Err(e) => self.exclude_error = Some(e),
            }
        }
        self.req = req;
    }

    /// 在**服务端**归档里 grep(搜的是板子上的落盘历史)。
    pub fn set_grep(&mut self, expr: &str) {
        const GREP_LIMIT: usize = 5000;
        if expr.trim().is_empty() {
            self.clear_grep();
            return;
        }
        match Filter::parse(expr) {
            Ok(f) => {
                let lines = crate::msg_log::scan(|l| f.matches_text(l), GREP_LIMIT);
                let msgs = lines.iter().map(|l| grep_line_to_msg(l)).collect();
                self.grep = Some((f.src().to_string(), msgs));
                self.grep_error = None;
            }
            Err(e) => self.grep_error = Some(e),
        }
    }

    pub fn clear_grep(&mut self) {
        self.grep = None;
        self.grep_error = None;
    }

    /// 依据当前会话与最新 [`ViewState`] 计算本连接的视口。
    pub fn build(&self, view: &ViewState) -> Window {
        let tab = view
            .tools
            .get(view.active_index)
            .map(|t| t.name.clone())
            .unwrap_or_default();

        // grep 视图:整段替换实时缓冲。
        if let Some((expr, results)) = &self.grep {
            let total: u64 = results.iter().map(|tm| msg_line_count(&tm.msg)).sum();
            let (start, messages) = window_slice(results, 0, &self.req, total);
            return Window {
                tab,
                start,
                messages,
                total,
                evicted: 0,
                follow: self.req.follow,
                filter: None,
                exclude: None,
                filter_error: None,
                exclude_error: None,
                matched: None,
                grep: Some((expr.clone(), results.len())),
                grep_error: self.grep_error.clone(),
            };
        }

        // 过滤视图:筛过的消息构成自洽视图(无驱逐前缀)。
        if self.include.is_some() || self.exclude.is_some() {
            let filtered: Vec<TimedMessage> = view
                .messages
                .iter()
                .filter(|tm| {
                    self.include.as_ref().is_none_or(|f| f.matches_msg(&tm.msg))
                        && !self.exclude.as_ref().is_some_and(|f| f.matches_msg(&tm.msg))
                })
                .cloned()
                .collect();
            let total: u64 = filtered.iter().map(|tm| msg_line_count(&tm.msg)).sum();
            let (start, messages) = window_slice(&filtered, 0, &self.req, total);
            return Window {
                tab,
                start,
                messages,
                total,
                evicted: 0,
                follow: self.req.follow,
                filter: self.include.as_ref().map(|f| f.src().to_string()),
                exclude: self.exclude.as_ref().map(|f| f.src().to_string()),
                filter_error: self.filter_error.clone(),
                exclude_error: self.exclude_error.clone(),
                matched: Some((filtered.len(), view.messages.len())),
                grep: None,
                grep_error: None,
            };
        }

        // 未过滤的实时视图:直接用后端缓冲的几何。
        let total = view.buffer_total_lines;
        let (start, messages) = window_slice(&view.messages, view.evicted_lines, &self.req, total);
        Window {
            tab,
            start,
            messages,
            total,
            evicted: view.evicted_lines,
            follow: self.req.follow,
            filter: None,
            exclude: None,
            filter_error: self.filter_error.clone(),
            exclude_error: self.exclude_error.clone(),
            matched: None,
            grep: None,
            grep_error: self.grep_error.clone(),
        }
    }
}

/// 从消息列表切出请求的视口。
///
/// - `count == 0`:全量模式,返回全部消息(远端 TUI 复用本地滚动/渲染)。
/// - `count > 0`:按范围取,用 [`crate::scroll`] 计算可见行窗口,返回其覆盖的
///   消息(以及它们在绝对坐标里的起始行)。
fn window_slice(
    messages: &[TimedMessage],
    evicted_base: u64,
    req: &ViewReq,
    total: u64,
) -> (u64, Vec<MsgDto>) {
    if req.count == 0 {
        let dtos = messages.iter().map(MsgDto::from_timed).collect();
        return (evicted_base, dtos);
    }

    // 复用 scroll 的绝对↔视口换算(与 TUI 渲染同一套逻辑)。
    let rel_off = crate::scroll::viewport_offset(
        &crate::scroll::ScrollState { offset: req.start, follow_tail: req.follow },
        &crate::scroll::ScrollInput {
            viewport: req.count.min(u16::MAX as u32) as u16,
            total_lines: total,
            evicted_lines: evicted_base,
        },
    );
    let window_end = rel_off + req.count as u64;

    let mut out = Vec::new();
    let mut cum: u64 = 0;
    let mut abs_start = evicted_base;
    let mut started = false;
    for tm in messages {
        let lc = msg_line_count(&tm.msg);
        let (start, end) = (cum, cum + lc);
        cum = end;
        if lc == 0 || end <= rel_off {
            continue; // 视口之上
        }
        if start >= window_end {
            break; // 视口之下
        }
        if !started {
            abs_start = evicted_base + start;
            started = true;
        }
        out.push(MsgDto::from_timed(tm));
    }
    (abs_start, out)
}

/// 把一条落盘归档行("YYYY-MM-DD HH:MM:SS.mmm [tool] body")解析回展示消息,
/// 恢复时间戳。与前端 grep 逻辑一致。
fn grep_line_to_msg(line: &str) -> TimedMessage {
    use chrono::TimeZone;
    let time = line
        .get(..23)
        .and_then(|ts| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S%.3f").ok())
        .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single())
        .unwrap_or_else(chrono::Local::now);
    let text = line.get(24..).unwrap_or(line).to_string();
    TimedMessage {
        time,
        msg: crate::message::Message::System {
            text,
            level: crate::message::LogLevel::Info,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::message::{LogLevel, Message, TimedMessage};

    fn sys(text: &str) -> TimedMessage {
        TimedMessage {
            time: chrono::Local::now(),
            msg: Message::System { text: text.into(), level: LogLevel::Info },
        }
    }

    fn view_with(msgs: Vec<TimedMessage>) -> ViewState {
        let mut v = ViewState::initial();
        let total = msgs.iter().map(|tm| msg_line_count(&tm.msg)).sum();
        v.messages = Arc::new(msgs);
        v.buffer_total_lines = total;
        v.evicted_lines = 0;
        v
    }

    #[test]
    fn full_mode_returns_all_messages() {
        let view = view_with(vec![sys("a"), sys("b"), sys("c")]);
        let session = ViewSession::default(); // count=0 → full
        let w = session.build(&view);
        assert_eq!(w.messages.len(), 3);
        assert_eq!(w.total, 3);
        assert_eq!(w.evicted, 0);
    }

    #[test]
    fn include_filter_drops_non_matching() {
        let view = view_with(vec![sys("error x"), sys("ok y"), sys("error z")]);
        let mut session = ViewSession::default();
        session.set_view(ViewReq { filter: Some("/error/".into()), ..Default::default() });
        let w = session.build(&view);
        assert_eq!(w.messages.len(), 2);
        assert_eq!(w.matched, Some((2, 3)));
        assert_eq!(w.filter.as_deref(), Some("/error/"));
    }

    #[test]
    fn exclude_composes_with_include() {
        let view = view_with(vec![
            sys("request /api ok"),
            sys("request /health ok"),
            sys("debug noise"),
        ]);
        let mut session = ViewSession::default();
        session.set_view(ViewReq {
            filter: Some("/request/".into()),
            exclude: Some("/health/".into()),
            ..Default::default()
        });
        let w = session.build(&view);
        assert_eq!(w.messages.len(), 1, "only /api request survives");
    }

    #[test]
    fn bad_filter_reports_error_and_shows_all() {
        let view = view_with(vec![sys("a"), sys("b")]);
        let mut session = ViewSession::default();
        session.set_view(ViewReq { filter: Some("/(".into()), ..Default::default() });
        let w = session.build(&view);
        // Parse failed → no include filter applied, error surfaced.
        assert!(w.filter_error.is_some());
    }

    #[test]
    fn ranged_window_follow_tail_returns_last_screen() {
        // 10 single-line messages, viewport 3, follow tail → msg-7,8,9.
        let msgs: Vec<_> = (0..10).map(|i| sys(&format!("msg-{i}"))).collect();
        let view = view_with(msgs);
        let mut session = ViewSession::default();
        session.set_view(ViewReq { count: 3, follow: true, ..Default::default() });
        let w = session.build(&view);
        let texts: Vec<String> = w.messages.iter().map(|m| match &m.body {
            crate::protocol::MsgBody::System { text, .. } => text.clone(),
            _ => String::new(),
        }).collect();
        assert_eq!(texts, vec!["msg-7", "msg-8", "msg-9"]);
        assert_eq!(w.start, 7, "absolute start line of the window");
        assert_eq!(w.total, 10);
    }

    #[test]
    fn ranged_window_mid_scroll_returns_right_slice() {
        let msgs: Vec<_> = (0..10).map(|i| sys(&format!("msg-{i}"))).collect();
        let view = view_with(msgs);
        let mut session = ViewSession::default();
        session.set_view(ViewReq { count: 3, start: 4, follow: false, ..Default::default() });
        let w = session.build(&view);
        let texts: Vec<String> = w.messages.iter().map(|m| match &m.body {
            crate::protocol::MsgBody::System { text, .. } => text.clone(),
            _ => String::new(),
        }).collect();
        assert_eq!(texts, vec!["msg-4", "msg-5", "msg-6"]);
        assert_eq!(w.start, 4);
    }

    #[test]
    fn grep_replaces_live_view() {
        let view = view_with(vec![sys("live only")]);
        let mut session = ViewSession::default();
        // Inject grep results directly (bypassing disk scan).
        session.grep = Some(("/x/".into(), vec![sys("hit 1"), sys("hit 2")]));
        let w = session.build(&view);
        assert_eq!(w.messages.len(), 2);
        assert_eq!(w.grep, Some(("/x/".to_string(), 2)));
    }

    /// 端到端最小闭环:真后端 + 一条 socket 连接,握手 + 发命令 + 收窗口。
    /// 用 `UnixStream::pair()` 免去监听器,专测线协议与桥接。
    #[tokio::test]
    async fn end_to_end_handshake_command_window() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::time::{Duration, timeout};

        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
        let (view_tx, view_rx) = watch::channel(ViewState::initial());
        tokio::spawn(crate::backend::run(cmd_rx, view_tx));
        tokio::spawn(async move {
            let (r, w) = server.into_split();
            let _ = handle_conn(r, w, cmd_tx, view_rx).await;
        });

        let (r, mut w) = client.into_split();
        let mut lines = BufReader::new(r).lines();

        // 第一帧必是握手。
        let hello = timeout(Duration::from_secs(2), lines.next_line())
            .await
            .expect("hello timed out")
            .unwrap()
            .unwrap();
        assert!(hello.contains("\"type\":\"hello\""), "first frame is hello: {hello}");

        // 读若干帧,应能见到共享状态与视口。
        let mut saw_shared = false;
        let mut saw_window = false;
        for _ in 0..10 {
            let Ok(Ok(Some(line))) = timeout(Duration::from_secs(2), lines.next_line()).await else {
                break;
            };
            if line.contains("\"type\":\"shared\"") {
                saw_shared = true;
            }
            if line.contains("\"type\":\"window\"") {
                saw_window = true;
            }
            if saw_shared && saw_window {
                break;
            }
        }
        assert!(saw_shared, "should receive a shared frame");
        assert!(saw_window, "should receive a window frame");

        // 发一条 help 命令,应在后续窗口里看到框架生成的 "commands"。
        w.write_all(b"{\"type\":\"input\",\"text\":\"help\"}\n").await.unwrap();
        let mut saw_help = false;
        for _ in 0..40 {
            let Ok(Ok(Some(line))) = timeout(Duration::from_secs(2), lines.next_line()).await else {
                break;
            };
            if line.contains("commands") {
                saw_help = true;
                break;
            }
        }
        assert!(saw_help, "help output should flow back over the socket");
    }
}
