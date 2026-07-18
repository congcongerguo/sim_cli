//! Task actor framework: trait, harness, and shared types.
//!
//! Each task type is behind its own feature flag. Disable a feature to
//! completely exclude that task's code from the binary.

#[cfg(feature = "conn-task")]
pub mod conn_actor;
#[cfg(feature = "demo-task")]
pub mod demo_actor;
pub mod registry;

use std::time::Duration;

use tokio::sync::{mpsc, watch};

use std::sync::Arc;

use crate::message::{LogLevel, Message};
use crate::transport::TransportEvent;

use super::chat::ChatState;
use super::conn::ConnState;
use registry::TaskDef;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub name: String,
    pub messages: Vec<Message>,
    pub model: String,
    pub conn: ConnState,
    #[allow(dead_code)]
    pub demo_running: bool,
    pub latest_recv: Option<serde_json::Value>,
    pub latest_recv_at: Option<chrono::DateTime<chrono::Local>>,
}

// ---------------------------------------------------------------------------
// TaskActor trait
// ---------------------------------------------------------------------------

pub trait TaskActor: Send + 'static {
    /// Commands supported by this task (autocomplete).
    fn commands(&self) -> Vec<CommandDef>;

    /// Handle a parsed command line.
    /// Default: matches "help"/"clear", delegates the rest to `handle_own`.
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

    /// Task-specific commands.
    fn handle_own(&mut self, cmd: &str, sub: Option<&str>, args: &[&str]) -> Vec<Message>;

    /// State snapshot for the Router.
    fn snapshot(&self) -> TaskSnapshot;

    /// Periodic tick (every 1s). Default: no-op.
    fn tick(&mut self) -> Vec<Message> { vec![] }

    /// Transport event callback. Default: no-op.
    #[allow(dead_code)]
    fn on_transport(&mut self, _ev: TransportEvent) -> Vec<Message> { vec![] }

    /// Chat log access.
    #[allow(dead_code)]
    fn chat(&self) -> &ChatState;
    fn chat_mut(&mut self) -> &mut ChatState;

    /// Build help text from self.commands().
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

/// Spawn a task actor into a background tokio task.
pub fn spawn_actor(actor: impl TaskActor) -> TaskHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let snapshot = actor.snapshot();
    let (state_tx, state_rx) = watch::channel(snapshot);

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut push = tokio::time::interval(Duration::from_millis(100));
        let mut actor = actor;

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
// Actor factory dispatch — maps task name to create callback.
// ---------------------------------------------------------------------------

/// Handle returned when an actor is spawned.
pub struct TaskRuntime {
    pub handle: TaskHandle,
    pub commands: Arc<Vec<CommandDef>>,
}

/// Look up the actor factory for `name` and call it.
/// Add a new branch here when adding a new task type.
pub fn create_actor(name: &str, model: String, def: &'static TaskDef) -> Option<TaskRuntime> {
    #[cfg(feature = "conn-task")]
    if name == "conn" { return Some(conn_actor::create(model, def)); }
    #[cfg(feature = "demo-task")]
    if name == "demo" { return Some(demo_actor::create(model, def)); }
    None
}
