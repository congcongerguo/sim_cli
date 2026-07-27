use crate::message::{LogLevel, Message};

use super::{cmd, group, msg, Cmd, Tool, ToolState};

// A multi-level command tree example:  log > level > debug|info|notice
//                                       log > reset
const LEVEL_SUBS: &[Cmd] = &[
    cmd("debug", "verbose"),
    cmd("info", "normal"),
    cmd("notice", "important"),
];
const LOG_SUBS: &[Cmd] = &[
    group("level", "set the periodic log level", LEVEL_SUBS),
    cmd("reset", "reset the counter to 0"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Running { counter: u64 },
}

pub struct DemoTool {
    state: State,
    /// Log level of the periodic counter messages (changed via `log level …`).
    level: LogLevel,
}

impl DemoTool {
    pub fn new(_def: &'static super::registry::ToolDef) -> Self {
        Self { state: State::Idle, level: LogLevel::Debug }
    }

    /// Handle the nested `log …` command tree. `args` is the path after `log`.
    fn handle_log(&mut self, args: &[&str]) -> Vec<Message> {
        match args {
            ["level", lvl] => {
                let level = match *lvl {
                    "debug" => LogLevel::Debug,
                    "info" => LogLevel::Info,
                    "notice" => LogLevel::Notice,
                    other => {
                        return vec![msg(&format!("unknown level: {other}"), LogLevel::Warn)];
                    }
                };
                self.level = level;
                vec![msg(&format!("log level set to {lvl}"), LogLevel::Notice)]
            }
            ["level"] => vec![msg("usage: log level <debug|info|notice>", LogLevel::Warn)],
            ["reset"] => {
                if let State::Running { counter } = &mut self.state {
                    *counter = 0;
                }
                vec![msg("counter reset", LogLevel::Notice)]
            }
            [] => vec![msg("usage: log <level|reset>", LogLevel::Warn)],
            _ => vec![msg("unknown log sub-command", LogLevel::Warn)],
        }
    }
}

impl Tool for DemoTool {
    fn commands(&self) -> Vec<Cmd> {
        vec![
            cmd("start", "begin periodic logging"),
            cmd("stop", "stop periodic logging"),
            group("log", "logging controls", LOG_SUBS),
        ]
    }

    fn handle(&mut self, cmd: &str, args: &[&str]) -> Vec<Message> {
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
            "log" => self.handle_log(args),
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        match &mut self.state {
            State::Running { counter } => {
                let n = *counter;
                *counter += 1;
                vec![Message::System { text: format!("{n}"), level: self.level }]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> DemoTool {
        DemoTool { state: State::Running { counter: 5 }, level: LogLevel::Debug }
    }

    #[test]
    fn log_level_sets_tick_level() {
        let mut d = demo();
        // Three-level command: log > level > info.
        let out = d.handle("log", &["level", "info"]);
        assert!(matches!(out[0], Message::System { level: LogLevel::Notice, .. }));
        // Subsequent periodic messages use the new level.
        let tick = d.tick();
        assert!(matches!(tick[0], Message::System { level: LogLevel::Info, .. }));
    }

    #[test]
    fn log_reset_zeroes_counter() {
        let mut d = demo();
        d.handle("log", &["reset"]);
        // Next tick emits "0".
        let tick = d.tick();
        assert!(matches!(&tick[0], Message::System { text, .. } if text == "0"));
    }

    #[test]
    fn log_unknown_level_warns() {
        let mut d = demo();
        let out = d.handle("log", &["level", "bogus"]);
        assert!(matches!(out[0], Message::System { level: LogLevel::Warn, .. }));
        // Level unchanged.
        assert_eq!(d.level, LogLevel::Debug);
    }
}
