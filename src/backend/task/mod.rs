//! Task actor framework: trait, harness, and shared types.
//!
//! Each task type is behind its own feature flag. Disable a feature to
//! completely exclude that task's code from the binary.

#[cfg(feature = "conn-task")]
pub mod conn_actor;
#[cfg(feature = "demo-task")]
pub mod demo_actor;
pub mod registry;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::message::{LogLevel, Message};
use crate::transport::TransportEvent;

use super::chat::ChatState;
use registry::TaskDef;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// task 无关的通用内部状态，暴露给 UI 层。
///
/// 每个 actor 在 [`TaskActor::snapshot`] 中把自己的私有状态机转换为此结构体。
/// 新增 task 类型无需改动本结构体或任何下游框架类型。
#[derive(Debug, Clone, Default)]
pub struct TaskInternalState {
    /// 显示在 state panel 中的键值对。
    pub fields: Vec<(String, String)>,
    /// 为 `true` 时 tab 栏显示绿色圆点。
    pub active: bool,
    /// 状态栏中间的 badge。为 `Some` 时替换默认的 "idle" 文字。
    pub badge: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommandDef {
    pub name: &'static str,
    pub desc: &'static str,
    pub subs: &'static [SubDef],
}

#[derive(Debug, Clone)]
pub struct SubDef {
    pub name: &'static str,
    pub desc: &'static str,
}

pub const fn cmd(name: &'static str, desc: &'static str) -> CommandDef {
    CommandDef { name, desc, subs: &[] }
}

/// 某个时间点上的 task actor 状态快照。每 [`push_interval_ms`] ms 通过
/// `watch` channel 推送给 UI。
///
/// 除 [`Self::name`] 外，所有字段都来自 task 自身的状态——框架只负责透传，不做解读。
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    /// tab 标签名（来自 tasks.toml）。
    pub name: String,

    /// 当前消息日志。用 `Arc` 包裹，渲染器可以持有廉价引用，避免每帧克隆整个 buffer。
    pub messages: Arc<Vec<Message>>,

    /// 已从环形 buffer 中淘汰的消息的累计渲染行数。
    /// 滚动系统用这个值保持视口在淘汰前后不跳动。
    pub evicted_lines: u64,

    /// 当前 buffer 中所有消息的渲染总行数。
    pub buffer_total_lines: u64,

    /// 当前使用的 LLM 模型名（例如 "claude"、"opus"、"haiku"）。
    pub model: String,

    /// 由 task 自行定义的状态数据——驱动 tab 栏圆点、状态栏 badge
    /// 和 state panel 键值对。框架不做任何解读。
    pub internal: TaskInternalState,

    /// 从 transport 收到的最新 JSON 值（如有）。
    pub latest_recv: Option<serde_json::Value>,

    /// 最近一次 `latest_recv` 的本地时间。
    pub latest_recv_at: Option<chrono::DateTime<chrono::Local>>,
}

/// Shared commands every task provides — use `base_commands()` in `commands()`.
pub fn base_commands() -> Vec<CommandDef> {
    vec![
        cmd("help", "show commands"),
        cmd("clear", "clear log"),
        cmd("exit", "quit"),
    ]
}

// ---------------------------------------------------------------------------
// TaskActor trait
// ---------------------------------------------------------------------------

pub trait TaskActor: Send + 'static {
    fn commands(&self) -> Vec<CommandDef>;

    fn handle_command(&mut self, cmd: &str, sub: Option<&str>, args: &[&str]) -> Vec<Message> {
        match cmd {
            "help" => self.build_help(),
            "clear" => {
                self.chat_mut().clear();
                vec![]
            }
            _ => self.handle_own(cmd, sub, args),
        }
    }

    fn handle_own(&mut self, cmd: &str, sub: Option<&str>, args: &[&str]) -> Vec<Message>;
    fn snapshot(&self) -> TaskSnapshot;

    fn tick(&mut self) -> Vec<Message> { vec![] }

    /// Tick interval in milliseconds. Override to change polling frequency.
    fn tick_interval_ms(&self) -> u64 { 500 }

    /// Snapshot push interval in milliseconds.
    fn push_interval_ms(&self) -> u64 { 100 }

    /// If the actor has a transport, return its event receiver so the
    /// harness can select on it directly (no polling delay).
    /// Default: a dummy channel that never fires.
    fn take_transport_rx(&mut self) -> mpsc::Receiver<TransportEvent> {
        let (_tx, rx) = mpsc::channel(1);
        rx
    }

    #[allow(dead_code)]
    fn on_transport(&mut self, _ev: TransportEvent) -> Vec<Message> { vec![] }

    #[allow(dead_code)]
    fn chat(&self) -> &ChatState;
    fn chat_mut(&mut self) -> &mut ChatState;

    fn build_help(&self) -> Vec<Message> {
        let cmds = self.commands();
        let mut s = String::from("commands:\n");
        for c in &cmds {
            s.push_str(&format!("  {:<8} — {}\n", c.name, c.desc));
            for sub in c.subs {
                s.push_str(&format!("      {:<6} {}\n", sub.name, sub.desc));
            }
        }
        s.push_str("\n←/→ switch tab  ^C exit");
        vec![Message::System { text: s, level: LogLevel::Info }]
    }
}

// ---------------------------------------------------------------------------
// Harness: tokio event loop shared by all task actors.
// ---------------------------------------------------------------------------

pub struct TaskHandle {
    pub cmd_tx: mpsc::Sender<String>,
    pub state_rx: watch::Receiver<TaskSnapshot>,
}

pub fn spawn_actor(mut actor: impl TaskActor) -> TaskHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let snapshot = actor.snapshot();
    let (state_tx, state_rx) = watch::channel(snapshot);

    tokio::spawn(async move {
        let tick_ms = actor.tick_interval_ms();
        let push_ms = actor.push_interval_ms();
        let mut tick = tokio::time::interval(Duration::from_millis(tick_ms));
        let mut push = tokio::time::interval(Duration::from_millis(push_ms));
        let mut transport_rx = actor.take_transport_rx();

        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                    Some(text) => {
                        let parts: Vec<&str> = text.split_whitespace().collect();
                        if parts.is_empty() { continue; }
                        let cmd = parts[0];
                        let sub = parts.get(1).copied();
                        let args: &[&str] = if parts.len() > 2 { &parts[2..] } else { &[] };
                        for m in actor.handle_command(cmd, sub, args) {
                            actor.chat_mut().push_message(m);
                        }
                    }
                    None => break,
                },
                maybe_ev = transport_rx.recv() => match maybe_ev {
                    Some(ev) => {
                        for m in actor.on_transport(ev) {
                            actor.chat_mut().push_message(m);
                        }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    for m in actor.tick() {
                        actor.chat_mut().push_message(m);
                    }
                }
                _ = push.tick() => {
                    let _ = state_tx.send(actor.snapshot());
                }
            }
        }
    });

    TaskHandle { cmd_tx, state_rx }
}

// ---------------------------------------------------------------------------
// Actor factory dispatch
// ---------------------------------------------------------------------------

pub struct TaskRuntime {
    pub handle: TaskHandle,
    pub commands: Arc<Vec<CommandDef>>,
}

pub fn create_actor(name: &str, model: String, def: &'static TaskDef) -> Option<TaskRuntime> {
    #[cfg(feature = "conn-task")]
    if name == "conn" { return Some(conn_actor::create(model, def)); }
    #[cfg(feature = "demo-task")]
    if name == "demo" { return Some(demo_actor::create(model, def)); }
    None
}
