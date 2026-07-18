use chrono::Local;
use crate::message::{LogLevel, Message};

use super::super::chat::ChatState;
use super::registry::TaskDef;
use super::{cmd, CommandDef, TaskActor, TaskSnapshot};

pub struct DemoTask {
    chat: ChatState,
    def: &'static TaskDef,
    running: bool,
}

impl DemoTask {
    pub fn new(model: String, def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(model), def, running: false }
    }
}

impl TaskActor for DemoTask {
    fn commands(&self) -> Vec<CommandDef> {
        vec![
            cmd("help", "show commands"),
            cmd("clear", "clear log"),
            cmd("exit", "quit"),
            cmd("start", "begin periodic logging"),
            cmd("stop", "stop periodic logging"),
        ]
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
            let ts = Local::now().format("%H:%M:%S").to_string();
            vec![Message::System { text: format!("[demo tick {ts}]"), level: LogLevel::Debug }]
        } else {
            vec![]
        }
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.clone(),
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
