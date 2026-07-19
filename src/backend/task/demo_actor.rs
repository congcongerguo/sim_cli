use crate::message::{LogLevel, Message};

use super::super::chat::ChatState;
use super::registry::TaskDef;
use super::{base_commands, cmd, CommandDef, TaskActor, TaskInternalState, TaskSnapshot};

// ── Private state machine (not visible to the framework) ──────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum DemoState {
    Idle,
    Running { counter: u64 },
}

impl DemoState {
    fn is_running(&self) -> bool {
        matches!(self, DemoState::Running { .. })
    }
}

// ── Actor ─────────────────────────────────────────────────────────────

pub struct DemoTask {
    chat: ChatState,
    def: &'static TaskDef,
    state: DemoState,
}

impl DemoTask {
    pub fn new(def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(), def, state: DemoState::Idle }
    }

    /// Convert the private state machine into the framework's generic
    /// [`TaskInternalState`] so the UI can render it without knowing about
    /// [`DemoState`].
    fn to_internal(&self) -> TaskInternalState {
        match &self.state {
            DemoState::Idle => TaskInternalState::default(),
            DemoState::Running { counter } => TaskInternalState {
                active: true,
                badge: Some(format!("demo: ● #{counter}")),
                fields: vec![("status".into(), format!("running (#{counter})"))],
            },
        }
    }
}

impl TaskActor for DemoTask {
    fn tick_interval_ms(&self) -> u64 { 1000 }

    fn commands(&self) -> Vec<CommandDef> {
        let mut v = base_commands();
        v.push(cmd("start", "begin periodic logging"));
        v.push(cmd("stop", "stop periodic logging"));
        v
    }

    fn handle_own(&mut self, cmd: &str, _sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "start" => {
                if self.state.is_running() {
                    vec![msg("already running", LogLevel::Warn)]
                } else {
                    self.state = DemoState::Running { counter: 0 };
                    vec![msg("demo started — logging every 1s", LogLevel::Notice)]
                }
            }
            "stop" => {
                if !self.state.is_running() {
                    vec![msg("not running", LogLevel::Warn)]
                } else {
                    self.state = DemoState::Idle;
                    vec![msg("demo stopped", LogLevel::Notice)]
                }
            }
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        match &mut self.state {
            DemoState::Running { counter } => {
                let n = *counter;
                *counter += 1;
                vec![Message::System { text: format!("{n}"), level: LogLevel::Debug }]
            }
            DemoState::Idle => vec![],
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.to_arc(),
            evicted_lines: self.chat.messages.evicted_lines(),
            buffer_total_lines: self.chat.messages.total_lines(),
            internal: self.to_internal(),
            latest_recv: None,
            latest_recv_at: None,
        }
    }

    fn chat(&self) -> &ChatState { &self.chat }
    fn chat_mut(&mut self) -> &mut ChatState { &mut self.chat }
}

fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}

use super::TaskRuntime;

/// Callback: construct and spawn this actor.
pub fn create(def: &'static TaskDef) -> TaskRuntime {
    let actor = DemoTask::new(def);
    let cmds = std::sync::Arc::new(actor.commands());
    let handle = super::spawn_actor(actor);
    TaskRuntime { handle, commands: cmds }
}
