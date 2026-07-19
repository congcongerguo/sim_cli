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

/// Task-agnostic internal state exposed to the UI.
///
/// Each actor converts its own private state machine into this struct in
/// [`TaskActor::snapshot`]. Adding a new task type never requires changes
/// to this struct or any downstream framework types.
#[derive(Debug, Clone, Default)]
pub struct TaskInternalState {
    /// Key-value rows shown in the state panel.
    pub fields: Vec<(String, String)>,
    /// Green dot in the tab bar when `true`.
    pub active: bool,
    /// Status line middle badge. When `Some`, replaces the "idle" label.
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

/// Point-in-time snapshot of a task actor's state, pushed to the UI every
/// [`push_interval_ms`](TaskActor::push_interval_ms) via a `watch` channel.
///
/// Every field except [`Self::name`] comes from the task's own state — the
/// framework just passes this struct through without interpretation.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    /// Tab label (from tasks.toml).
    pub name: String,

    /// Current message log. Wrapped in `Arc` so the renderer can hold a
    /// cheap reference without cloning the whole buffer every frame.
    pub messages: Arc<Vec<Message>>,

    /// Cumulative render-line count of messages evicted from the ring buffer.
    /// Used by the scroll system to keep the viewport stable across evictions.
    pub evicted_lines: u64,

    /// Total render-line count of all messages currently in the buffer.
    pub buffer_total_lines: u64,

    /// Active LLM model name (e.g. "claude", "opus", "haiku").
    pub model: String,

    /// Task-defined state blob — drives the tab bar dot, status line badge,
    /// and state panel key-value rows. The framework never inspects it.
    pub internal: TaskInternalState,

    /// Most recent JSON value received from the transport, if any.
    pub latest_recv: Option<serde_json::Value>,

    /// Wall-clock time of the most recent `latest_recv`.
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
