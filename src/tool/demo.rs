use crate::message::{LogLevel, Message};

use super::{cmd, msg, Cmd, Tool, ToolState};

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Running { counter: u64 },
}

pub struct DemoTool {
    state: State,
}

impl DemoTool {
    pub fn new() -> Self {
        Self { state: State::Idle }
    }
}

impl Tool for DemoTool {
    fn commands(&self) -> Vec<Cmd> {
        vec![
            cmd("start", "begin periodic logging"),
            cmd("stop", "stop periodic logging"),
        ]
    }

    fn handle(&mut self, cmd: &str, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "start" => {
                if matches!(self.state, State::Running { .. }) {
                    vec![msg("already running", LogLevel::Warn)]
                } else {
                    self.state = State::Running { counter: 0 };
                    vec![msg("demo started", LogLevel::Notice)]
                }
            }
            "stop" => {
                if matches!(self.state, State::Idle) {
                    vec![msg("not running", LogLevel::Warn)]
                } else {
                    self.state = State::Idle;
                    vec![msg("demo stopped", LogLevel::Notice)]
                }
            }
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        match &mut self.state {
            State::Running { counter } => {
                let n = *counter;
                *counter += 1;
                vec![Message::System { text: format!("{n}"), level: LogLevel::Debug }]
            }
            State::Idle => vec![],
        }
    }

    fn snapshot(&self) -> ToolState {
        match &self.state {
            State::Idle => ToolState::default(),
            State::Running { counter } => ToolState {
                active: true,
                badge: Some(format!("demo: #{counter}", counter = counter)),
                fields: vec![("counter".into(), counter.to_string())],
            },
        }
    }

    fn tick_ms(&self) -> u64 { 1000 }
}
