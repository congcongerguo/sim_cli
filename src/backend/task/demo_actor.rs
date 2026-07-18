use crate::message::{LogLevel, Message};

use super::super::chat::ChatState;
use super::registry::TaskDef;
use super::{base_commands, cmd, CommandDef, TaskActor, TaskSnapshot};

pub struct DemoTask {
    chat: ChatState,
    def: &'static TaskDef,
    running: bool,
    counter: u64,
}

impl DemoTask {
    pub fn new(model: String, def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(model), def, running: false, counter: 0 }
    }
}

impl TaskActor for DemoTask {
    fn commands(&self) -> Vec<CommandDef> {
        let mut v = base_commands();
        v.push(cmd("start", "begin periodic logging"));
        v.push(cmd("stop", "stop periodic logging"));
        v
    }

    fn handle_own(&mut self, cmd: &str, _sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "start" => {
                if self.running {
                    vec![msg("already running", LogLevel::Warn)]
                } else {
                    self.running = true;
                    vec![msg("demo started — logging every 1s", LogLevel::Notice)]
                }
            }
            "stop" => {
                if !self.running {
                    vec![msg("not running", LogLevel::Warn)]
                } else {
                    self.running = false;
                    vec![msg("demo stopped", LogLevel::Notice)]
                }
            }
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        if self.running {
            let n = self.counter;
            self.counter += 1;
            vec![Message::System { text: format!("{n}"), level: LogLevel::Debug }]
        } else {
            vec![]
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.to_arc(),
            evicted_lines: self.chat.messages.evicted_lines(),
            buffer_total_lines: self.chat.messages.total_lines(),
            model: self.chat.model.clone(),
            conn: crate::backend::ConnState::Disconnected,
            demo_running: self.running,
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
pub fn create(model: String, def: &'static TaskDef) -> TaskRuntime {
    let actor = DemoTask::new(model, def);
    let cmds = std::sync::Arc::new(actor.commands());
    let handle = super::spawn_actor(actor);
    TaskRuntime { handle, commands: cmds }
}
