//! 通用 Tool 框架：只定义 trait，不关心具体 task 类型。
//!
//! 框架负责：消息日志、滚动计数、select 循环、watch 推送。
//! Tool 负责：命令定义、业务逻辑、自定义状态快照。
//!
//! 新增 tool 只需实现 [`Tool`] trait + 在 `registry` 注册，无需改动框架代码。

#[cfg(feature = "conn-task")]
pub mod conn;
#[cfg(feature = "demo-task")]
pub mod demo;
pub mod registry;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::log_buffer::LogBuffer;
use crate::message::{LogLevel, Message, TimedMessage};
use crate::transport::TransportEvent;

// ── 基础命令 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Cmd {
    pub name: &'static str,
    pub desc: &'static str,
    pub subs: &'static [Sub],
}

#[derive(Debug, Clone)]
pub struct Sub {
    pub name: &'static str,
    pub desc: &'static str,
}

pub const fn cmd(name: &'static str, desc: &'static str) -> Cmd {
    Cmd { name, desc, subs: &[] }
}

pub fn base_cmds() -> Vec<Cmd> {
    vec![
        cmd("help", "show commands"),
        cmd("clear", "clear log"),
        cmd("exit", "quit"),
    ]
}

// ── ToolState ──────────────────────────────────────────────────────────

/// Tool 自定义状态快照，框架透传给 UI。
#[derive(Debug, Clone, Default)]
pub struct ToolState {
    /// state panel 中显示的键值对。
    pub fields: Vec<(String, String)>,
    /// 为 `true` 时 tab 栏显示绿色圆点。
    pub active: bool,
    /// 状态栏 badge。为 `Some` 时替换默认的 "idle" 文字。
    pub badge: Option<String>,
}

// ── Tool trait ─────────────────────────────────────────────────────────

/// Tool 只需实现业务逻辑，框架管理消息日志和事件循环。
pub trait Tool: Send + 'static {
    /// 命令列表（不含 help / clear / exit，框架自动追加）。
    fn commands(&self) -> Vec<Cmd>;

    /// 处理用户命令。`args` 不含命令名本身。
    /// 返回的消息由框架写入 LogBuffer。
    fn handle(&mut self, cmd: &str, args: &[&str]) -> Vec<Message>;

    /// 定时调用，用于轮询 I/O 或周期性任务。
    fn tick(&mut self) -> Vec<Message> { vec![] }

    /// 自定义状态快照。
    fn snapshot(&self) -> ToolState { ToolState::default() }

    /// tick 间隔（毫秒）。覆盖可改变轮询频率。
    fn tick_ms(&self) -> u64 { 500 }

    /// snapshot 推送间隔（毫秒）。
    fn push_ms(&self) -> u64 { 100 }

    /// 如果 tool 有 transport，返回事件接收端让框架 select 在其上。
    /// 默认返回空 channel（永不触发）。
    fn take_transport_rx(&mut self) -> mpsc::Receiver<TransportEvent> {
        let (tx, rx) = mpsc::channel(1);
        std::mem::forget(tx);
        rx
    }

    /// 收到 transport 事件时调用。
    fn on_transport(&mut self, _ev: TransportEvent) -> Vec<Message> { vec![] }
}

// ── 框架内部类型 ──────────────────────────────────────────────────────

/// 框架持有的 tool 运行时。Tool 不感知这些字段。
struct ToolCtx {
    name: String,
    log: LogBuffer,
    tool: Box<dyn Tool>,
}

/// 推送给 UI 的单帧快照，框架自动填充消息和滚动信息。
#[derive(Debug, Clone)]
pub struct ViewUpdate {
    pub name: String,
    pub messages: Arc<Vec<TimedMessage>>,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
    pub state: ToolState,
}

/// 外部持有的句柄。
pub struct ToolHandle {
    pub cmd_tx: mpsc::Sender<String>,
    pub view_rx: watch::Receiver<ViewUpdate>,
}

// ── 注册信息（供 tab 栏） ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub active: bool,
}

// ── spawn ──────────────────────────────────────────────────────────────

pub fn spawn(name: String, tool: impl Tool, cmds: Arc<Vec<Cmd>>) -> ToolHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let initial = ViewUpdate {
        name: name.clone(),
        messages: Arc::new(vec![]),
        evicted_lines: 0,
        buffer_total_lines: 0,
        state: tool.snapshot(),
    };
    let (view_tx, view_rx) = watch::channel(initial);

    let mut ctx = ToolCtx {
        name,
        log: LogBuffer::new(crate::log_buffer::DEFAULT_MAX),
        tool: Box::new(tool),
    };

    tokio::spawn(async move {
        let tick_ms = ctx.tool.tick_ms();
        let push_ms = ctx.tool.push_ms();
        let mut tick = tokio::time::interval(Duration::from_millis(tick_ms));
        let mut push = tokio::time::interval(Duration::from_millis(push_ms));
        let mut transport_rx = ctx.tool.take_transport_rx();

        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                    Some(text) => {
                        let parts: Vec<&str> = text.split_whitespace().collect();
                        if parts.is_empty() { continue; }
                        let cmd = parts[0];
                        let args: &[&str] = if parts.len() > 1 { &parts[1..] } else { &[] };

                        let msgs = match cmd {
                            "help" => build_help(&cmds),
                            "clear" => {
                                ctx.log.clear();
                                log_msg(&mut ctx.log, &ctx.name, msg("conversation cleared", LogLevel::Notice));
                                continue;
                            }
                            _ => ctx.tool.handle(cmd, args),
                        };
                        for m in msgs { log_msg(&mut ctx.log, &ctx.name, m); }
                    }
                    None => break,
                },
                maybe_ev = transport_rx.recv() => {
                    if let Some(ev) = maybe_ev {
                        for m in ctx.tool.on_transport(ev) { log_msg(&mut ctx.log, &ctx.name, m); }
                    }
                }
                _ = tick.tick() => {
                    for m in ctx.tool.tick() { log_msg(&mut ctx.log, &ctx.name, m); }
                }
                _ = push.tick() => {
                    let _ = view_tx.send(ViewUpdate {
                        name: ctx.name.clone(),
                        messages: ctx.log.to_arc(),
                        evicted_lines: ctx.log.evicted_lines(),
                        buffer_total_lines: ctx.log.total_lines(),
                        state: ctx.tool.snapshot(),
                    });
                }
            }
        }
    });

    ToolHandle { cmd_tx, view_rx }
}

fn build_cmds(mut own: Vec<Cmd>) -> Vec<Cmd> {
    let mut all = base_cmds();
    all.append(&mut own);
    all
}

fn build_help(cmds: &[Cmd]) -> Vec<Message> {
    let mut s = String::from("commands:\n");
    for c in cmds {
        s.push_str(&format!("  {:<8} - {}\n", c.name, c.desc));
        for sub in c.subs {
            s.push_str(&format!("      {:<6} {}\n", sub.name, sub.desc));
        }
    }
    s.push_str("\n<-/-> switch tab  ^C exit");
    vec![Message::System { text: s, level: LogLevel::Info }]
}

/// 创建一条系统消息。
pub fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}

/// 用同一时间戳把消息写入界面缓冲区并落盘到消息日志文件,
/// 保证屏幕上显示的时间与日志文件中的时间完全一致。
fn log_msg(log: &mut LogBuffer, tool: &str, m: Message) {
    let time = chrono::Local::now();
    crate::msg_log::record_at(time, tool, &m);
    log.push_at(time, m);
}

// ── 工厂函数 ──────────────────────────────────────────────────────────

use registry::ToolDef;

/// 根据 tool 名创建实例。由 Router 调用。
pub fn create(def: &'static ToolDef) -> Option<(ToolHandle, Arc<Vec<Cmd>>)> {
    #[cfg(feature = "conn-task")]
    if def.name == "conn" {
        let tool = conn::ConnTool::new(def);
        let cmds = Arc::new(build_cmds(tool.commands()));
        return Some((spawn(def.name.to_string(), tool, cmds.clone()), cmds));
    }
    #[cfg(feature = "demo-task")]
    if def.name == "demo" {
        let tool = demo::DemoTool::new();
        let cmds = Arc::new(build_cmds(tool.commands()));
        return Some((spawn(def.name.to_string(), tool, cmds.clone()), cmds));
    }
    None
}
