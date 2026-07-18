//! Command registry: a single table drives parsing, completion, and dispatch.
//!
//! Adding a new command: append one row to [`COMMANDS`], add one variant to
//! [`Action`], and add one match arm in `backend::dispatch`. The compiler
//! enforces all three exist; there is no runtime string matching here that
//! can fall out of sync.

use std::fmt;

use crate::transport::Protocol;

// -----------------------------------------------------------------------------
// Typed action arguments
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelChoice {
    Claude,
    Opus,
    Haiku,
}

impl ModelChoice {
    pub fn slug(self) -> &'static str {
        match self {
            ModelChoice::Claude => "claude",
            ModelChoice::Opus => "opus",
            ModelChoice::Haiku => "haiku",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanToggle {
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoScenario {
    Chat,
    Code,
    Tool,
}

// -----------------------------------------------------------------------------
// Action enum: the typed command the backend executes
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Help,
    Clear,
    Exit,
    Model(ModelChoice),
    Plan(PlanToggle),
    Demo(DemoScenario),
    Connect(Protocol),
    Disconnect,
    Send,
    /// Switch to a task by name (used by Left/Right key navigation).
    TaskSwitch(String),
    /// Demo logger: start periodic ticks on the active task.
    Start,
    /// Demo logger: stop periodic ticks on the active task.
    Stop,
}

// -----------------------------------------------------------------------------
// Command table
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct SubSpec {
    pub name: &'static str,
    pub desc: &'static str,
}

/// Description of a CLI command. `build` is called by the parser after args
/// have been validated against `subs`; it returns the typed [`Action`].
#[derive(Debug)]
pub struct CommandSpec {
    pub name: &'static str,
    pub desc: &'static str,
    pub subs: &'static [SubSpec],
    /// Build the action from the (already-validated) sub-name.
    pub build: fn(Option<&'static str>) -> Result<Action, ParseError>,
}

const MODEL_SUBS: &[SubSpec] = &[
    SubSpec { name: "claude", desc: "use mock-claude" },
    SubSpec { name: "opus", desc: "use mock-opus" },
    SubSpec { name: "haiku", desc: "use mock-haiku" },
];

const PLAN_SUBS: &[SubSpec] = &[
    SubSpec { name: "on", desc: "enable plan mode" },
    SubSpec { name: "off", desc: "disable plan mode" },
];

const DEMO_SUBS: &[SubSpec] = &[
    SubSpec { name: "chat", desc: "stream a markdown chat reply" },
    SubSpec { name: "code", desc: "stream a syntect-highlighted code reply" },
    SubSpec { name: "tool", desc: "show a tool card with permission modal" },
];

const CON_SUBS: &[SubSpec] = &[
    SubSpec { name: "tcp", desc: "TCP echo (127.0.0.1:7878)" },
    SubSpec { name: "zmq", desc: "ZMQ pub/sub (sub tcp://127.0.0.1:5555 / pub tcp://127.0.0.1:5556)" },
];

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "help",
        desc: "show available commands",
        subs: &[],
        build: |_| Ok(Action::Help),
    },
    CommandSpec {
        name: "clear",
        desc: "clear the conversation",
        subs: &[],
        build: |_| Ok(Action::Clear),
    },
    CommandSpec {
        name: "exit",
        desc: "leave the CLI",
        subs: &[],
        build: |_| Ok(Action::Exit),
    },
    CommandSpec {
        name: "model",
        desc: "switch model: model <claude|opus|haiku>",
        subs: MODEL_SUBS,
        build: |s| Ok(Action::Model(parse_model(s.ok_or_else(|| ParseError::InternalDrift("missing sub".into()))?)?)),
    },
    CommandSpec {
        name: "plan",
        desc: "set plan mode: plan <on|off>",
        subs: PLAN_SUBS,
        build: |s| Ok(Action::Plan(parse_plan(s.ok_or_else(|| ParseError::InternalDrift("missing sub".into()))?)?)),
    },
    CommandSpec {
        name: "demo",
        desc: "show a UI demo: demo <chat|code|tool>",
        subs: DEMO_SUBS,
        build: |s| Ok(Action::Demo(parse_demo(s.ok_or_else(|| ParseError::InternalDrift("missing sub".into()))?)?)),
    },
    CommandSpec {
        name: "con",
        desc: "connect a transport: con <tcp|zmq>",
        subs: CON_SUBS,
        build: |s| Ok(Action::Connect(parse_proto(s.ok_or_else(|| ParseError::InternalDrift("missing sub".into()))?)?)),
    },
    CommandSpec {
        name: "close",
        desc: "disconnect the current transport",
        subs: &[],
        build: |_| Ok(Action::Disconnect),
    },
    CommandSpec {
        name: "send",
        desc: "build a JSON message, send it, await reply",
        subs: &[],
        build: |_| Ok(Action::Send),
    },
    CommandSpec {
        name: "start",
        desc: "start demo periodic logging on current task",
        subs: &[],
        build: |_| Ok(Action::Start),
    },
    CommandSpec {
        name: "stop",
        desc: "stop demo periodic logging on current task",
        subs: &[],
        build: |_| Ok(Action::Stop),
    },
];

fn parse_model(s: &str) -> Result<ModelChoice, ParseError> {
    match s {
        "claude" => Ok(ModelChoice::Claude),
        "opus" => Ok(ModelChoice::Opus),
        "haiku" => Ok(ModelChoice::Haiku),
        other => Err(ParseError::InternalDrift(format!("MODEL_SUBS drift: {other}"))),
    }
}

fn parse_plan(s: &str) -> Result<PlanToggle, ParseError> {
    match s {
        "on" => Ok(PlanToggle::On),
        "off" => Ok(PlanToggle::Off),
        other => Err(ParseError::InternalDrift(format!("PLAN_SUBS drift: {other}"))),
    }
}

fn parse_demo(s: &str) -> Result<DemoScenario, ParseError> {
    match s {
        "chat" => Ok(DemoScenario::Chat),
        "code" => Ok(DemoScenario::Code),
        "tool" => Ok(DemoScenario::Tool),
        other => Err(ParseError::InternalDrift(format!("DEMO_SUBS drift: {other}"))),
    }
}

fn parse_proto(s: &str) -> Result<Protocol, ParseError> {
    Protocol::from_name(s).ok_or_else(|| {
        ParseError::InternalDrift(format!("CON_SUBS drift: {s}"))
    })
}

// -----------------------------------------------------------------------------
// Error type
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum ParseError {
    Empty,
    UnknownCommand(String),
    AmbiguousCommand(Vec<&'static str>),
    MissingArg(&'static CommandSpec),
    UnknownArg(&'static CommandSpec, String),
    AmbiguousArg(&'static CommandSpec, Vec<&'static str>),
    UnexpectedArg(&'static CommandSpec, String),
    CommandNotAllowed(String, String),
    /// Internal consistency error (COMMANDS table out of sync with parse_*).
    InternalDrift(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty input"),
            ParseError::UnknownCommand(s) => write!(f, "unknown command: {s}"),
            ParseError::AmbiguousCommand(list) => {
                write!(f, "ambiguous command: {}", list.join(", "))
            }
            ParseError::MissingArg(def) => {
                let opts = sub_names(def).join(" | ");
                write!(f, "missing arg for '{}', expected: {}", def.name, opts)
            }
            ParseError::UnknownArg(def, s) => {
                let opts = sub_names(def).join(" | ");
                write!(
                    f,
                    "unknown arg '{s}' for '{}', expected: {opts}",
                    def.name
                )
            }
            ParseError::AmbiguousArg(def, list) => {
                write!(f, "ambiguous arg for '{}': {}", def.name, list.join(", "))
            }
            ParseError::UnexpectedArg(def, s) => {
                write!(f, "'{}' takes no arguments (got '{s}')", def.name)
            }
            ParseError::CommandNotAllowed(cmd, task) => {
                write!(f, "'{cmd}' is not available in the '{task}' tab")
            }
            ParseError::InternalDrift(msg) => {
                write!(f, "internal error: {msg}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

fn sub_names(def: &CommandSpec) -> Vec<&'static str> {
    def.subs.iter().map(|s| s.name).collect()
}

// -----------------------------------------------------------------------------
// Task-scoped command filter (delegates to TaskDef)
// -----------------------------------------------------------------------------

/// Returns the set of allowed command names for a task. `None` = all allowed.
pub fn task_filter(task_name: &str) -> Option<&'static [&'static str]> {
    let def = crate::backend::TaskDef::find(task_name)?;
    if def.commands.is_empty() {
        None
    } else {
        Some(def.commands)
    }
}

pub fn is_command_allowed(cmd_name: &str, task_name: &str) -> bool {
    crate::backend::TaskDef::find(task_name)
        .map_or(true, |d| d.is_allowed(cmd_name))
}

// -----------------------------------------------------------------------------
// Lookup helpers
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum Resolve<T> {
    Unique(T),
    Ambiguous(Vec<&'static str>),
    None,
}

pub fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn matching_commands(prefix: &str, task: Option<&str>) -> Vec<(&'static str, &'static str)> {
    let p = prefix.trim();
    if p.is_empty() {
        return Vec::new();
    }
    let filter = task.and_then(task_filter);
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(p))
        .filter(|c| filter.map_or(true, |f| f.contains(&c.name)))
        .map(|c| (c.name, c.desc))
        .collect()
}

pub fn matching_subs(cmd: &CommandSpec, prefix: &str) -> Vec<(&'static str, &'static str)> {
    cmd.subs
        .iter()
        .filter(|s| s.name.starts_with(prefix))
        .map(|s| (s.name, s.desc))
        .collect()
}

pub fn resolve_command_prefix(prefix: &str) -> Resolve<&'static CommandSpec> {
    let p = prefix.trim();
    if p.is_empty() {
        return Resolve::None;
    }
    let hits: Vec<&'static CommandSpec> =
        COMMANDS.iter().filter(|c| c.name.starts_with(p)).collect();
    match hits.len() {
        0 => Resolve::None,
        1 => Resolve::Unique(hits[0]),
        _ => Resolve::Ambiguous(hits.iter().map(|c| c.name).collect()),
    }
}

pub fn resolve_sub_prefix(cmd: &CommandSpec, prefix: &str) -> Resolve<&'static str> {
    if prefix.is_empty() {
        return Resolve::None;
    }
    let hits: Vec<&'static str> = cmd
        .subs
        .iter()
        .filter(|s| s.name.starts_with(prefix))
        .map(|s| s.name)
        .collect();
    match hits.len() {
        0 => Resolve::None,
        1 => Resolve::Unique(hits[0]),
        _ => Resolve::Ambiguous(hits),
    }
}

pub fn longest_common_prefix(names: &[&str]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let first = names[0];
    let mut end = first.len();
    for s in &names[1..] {
        end = end.min(s.len());
        let a = first.as_bytes();
        let b = s.as_bytes();
        let mut i = 0;
        while i < end && a[i] == b[i] {
            i += 1;
        }
        end = i;
        if end == 0 {
            break;
        }
    }
    first[..end].to_string()
}

// -----------------------------------------------------------------------------
// Completion context
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum CompletionCtx {
    Command { prefix: String },
    Sub { cmd: &'static CommandSpec, prefix: String },
    None,
}

pub fn completion_ctx(input: &str) -> CompletionCtx {
    let has_ws = input.chars().any(char::is_whitespace);
    if !has_ws {
        return CompletionCtx::Command { prefix: input.to_string() };
    }

    let mut it = input.splitn(2, char::is_whitespace);
    let cmd_tok = it.next().unwrap_or("").trim();
    let rest = it.next().unwrap_or("");

    if cmd_tok.is_empty() {
        return CompletionCtx::Command { prefix: rest.trim_start().to_string() };
    }

    let cmd = match resolve_command_prefix(cmd_tok) {
        Resolve::Unique(c) => c,
        _ => return CompletionCtx::Command { prefix: cmd_tok.to_string() },
    };

    if cmd.subs.is_empty() {
        return CompletionCtx::None;
    }

    let sub_prefix: &str = if rest.is_empty() || rest.ends_with(char::is_whitespace) {
        ""
    } else {
        rest.split_whitespace().next_back().unwrap_or("")
    };

    CompletionCtx::Sub { cmd, prefix: sub_prefix.to_string() }
}

// -----------------------------------------------------------------------------
// Parse a fully-typed input line into an Action
// -----------------------------------------------------------------------------

pub fn parse_action(input: &str, task: Option<&str>) -> Result<Action, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut tokens = trimmed.split_whitespace();
    let cmd_tok = tokens.next().unwrap();
    let rest: Vec<&str> = tokens.collect();

    let cmd = match resolve_command_prefix(cmd_tok) {
        Resolve::Unique(c) => c,
        Resolve::Ambiguous(list) => return Err(ParseError::AmbiguousCommand(list)),
        Resolve::None => return Err(ParseError::UnknownCommand(cmd_tok.to_string())),
    };

    // Check task-scoped command filter.
    if let Some(task_name) = task {
        if !is_command_allowed(cmd.name, task_name) {
            return Err(ParseError::CommandNotAllowed(
                cmd.name.to_string(),
                task_name.to_string(),
            ));
        }
    }

    if cmd.subs.is_empty() {
        if !rest.is_empty() {
            return Err(ParseError::UnexpectedArg(cmd, rest.join(" ")));
        }
        return (cmd.build)(None);
    }

    if rest.is_empty() {
        return Err(ParseError::MissingArg(cmd));
    }
    if rest.len() > 1 {
        return Err(ParseError::UnexpectedArg(cmd, rest[1..].join(" ")));
    }

    let sub_name = match resolve_sub_prefix(cmd, rest[0]) {
        Resolve::Unique(name) => name,
        Resolve::Ambiguous(list) => return Err(ParseError::AmbiguousArg(cmd, list)),
        Resolve::None => return Err(ParseError::UnknownArg(cmd, rest[0].to_string())),
    };

    (cmd.build)(Some(sub_name))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> Result<Action, ParseError> {
        parse_action(input, None)
    }

    fn assert_unique(action: Action, input: &str) {
        match parse(input) {
            Ok(a) => assert_eq!(a, action, "input={input}"),
            Err(e) => panic!("expected Ok({action:?}) for '{input}', got {e:?}"),
        }
    }

    #[test]
    fn unique_no_arg_command() {
        assert_unique(Action::Help, "h");
        assert_unique(Action::Help, "help");
        assert_unique(Action::Clear, "cle");
        assert_unique(Action::Exit, "e");
        assert_unique(Action::Start, "start");
        assert_unique(Action::Stop, "stop");
    }

    #[test]
    fn unique_with_sub() {
        assert_unique(Action::Plan(PlanToggle::On), "plan on");
        assert_unique(Action::Plan(PlanToggle::On), "p on");
        assert_unique(Action::Plan(PlanToggle::Off), "plan of");
        assert_unique(Action::Demo(DemoScenario::Chat), "demo chat");
        assert_unique(Action::Demo(DemoScenario::Code), "d co");
        assert_unique(Action::Model(ModelChoice::Haiku), "m h");
        assert_unique(Action::Connect(Protocol::Tcp), "con tcp");
        assert_unique(Action::Connect(Protocol::Zmq), "con zmq");
    }

    #[test]
    fn ambiguous_sub() {
        match parse("plan o") {
            Err(ParseError::AmbiguousArg(cmd, list)) => {
                assert_eq!(cmd.name, "plan");
                assert_eq!(list, vec!["on", "off"]);
            }
            other => panic!("expected AmbiguousArg, got {other:?}"),
        }
        match parse("demo c") {
            Err(ParseError::AmbiguousArg(cmd, list)) => {
                assert_eq!(cmd.name, "demo");
                assert_eq!(list, vec!["chat", "code"]);
            }
            other => panic!("expected AmbiguousArg, got {other:?}"),
        }
    }

    #[test]
    fn missing_arg_lists_options() {
        match parse("plan") {
            Err(ParseError::MissingArg(cmd)) => {
                assert_eq!(cmd.name, "plan");
                let opts: Vec<&str> = cmd.subs.iter().map(|s| s.name).collect();
                assert_eq!(opts, vec!["on", "off"]);
            }
            other => panic!("expected MissingArg, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_command() {
        match parse("xy") {
            Err(ParseError::UnknownCommand(s)) => assert_eq!(s, "xy"),
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_arg_for_no_arg_command() {
        match parse("help me") {
            Err(ParseError::UnexpectedArg(cmd, extra)) => {
                assert_eq!(cmd.name, "help");
                assert_eq!(extra, "me");
            }
            other => panic!("expected UnexpectedArg, got {other:?}"),
        }
    }

    #[test]
    fn unknown_sub_lists_options() {
        match parse("plan zzz") {
            Err(ParseError::UnknownArg(cmd, s)) => {
                assert_eq!(cmd.name, "plan");
                assert_eq!(s, "zzz");
            }
            other => panic!("expected UnknownArg, got {other:?}"),
        }
    }

    #[test]
    fn completion_command_level() {
        match completion_ctx("pl") {
            CompletionCtx::Command { prefix } => assert_eq!(prefix, "pl"),
            other => panic!("expected Command, got {other:?}"),
        }
        match completion_ctx("") {
            CompletionCtx::Command { prefix } => assert_eq!(prefix, ""),
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn completion_sub_level_after_space() {
        match completion_ctx("plan ") {
            CompletionCtx::Sub { cmd, prefix } => {
                assert_eq!(cmd.name, "plan");
                assert_eq!(prefix, "");
            }
            other => panic!("expected Sub, got {other:?}"),
        }
        match completion_ctx("p o") {
            CompletionCtx::Sub { cmd, prefix } => {
                assert_eq!(cmd.name, "plan");
                assert_eq!(prefix, "o");
            }
            other => panic!("expected Sub, got {other:?}"),
        }
    }

    #[test]
    fn completion_none_for_no_arg_command() {
        match completion_ctx("help ") {
            CompletionCtx::None => {}
            other => panic!("expected None, got {other:?}"),
        }
    }

    #[test]
    fn lcp_basic() {
        assert_eq!(longest_common_prefix(&["chat", "code"]), "c");
        assert_eq!(longest_common_prefix(&["on", "off"]), "o");
        assert_eq!(longest_common_prefix(&[]), "");
        assert_eq!(longest_common_prefix(&["abc"]), "abc");
    }

    #[test]
    fn task_filter_blocks_disallowed_commands() {
        let err = parse_action("model claude", Some("conn")).unwrap_err();
        assert!(matches!(err, ParseError::CommandNotAllowed(_, _)));
        assert!(parse_action("con zmq", Some("conn")).is_ok());
        assert!(parse_action("start", Some("demo")).is_ok());
        assert!(parse_action("stop", Some("demo")).is_ok());
        let err = parse_action("con tcp", Some("demo")).unwrap_err();
        assert!(matches!(err, ParseError::CommandNotAllowed(_, _)));
        assert!(parse_action("model claude", Some("main")).is_ok());
        assert!(parse_action("demo chat", Some("main")).is_ok());
    }

    /// Drift-prevention: every sub declared in the table must be accepted by
    /// `parse_action` and produce a matching Action.
    #[test]
    fn every_spec_builds_action_for_every_sub() {
        for spec in COMMANDS {
            if spec.subs.is_empty() {
                let input = spec.name;
                let action = parse_action(input, None)
                    .unwrap_or_else(|e| panic!("'{input}' should parse: {e}"));
                let expected = (spec.build)(None).unwrap();
                assert_eq!(
                    action, expected,
                    "build({}, None) must match parse_action result",
                    spec.name
                );
            } else {
                for sub in spec.subs {
                    let input = format!("{} {}", spec.name, sub.name);
                    let action = parse_action(&input, None)
                        .unwrap_or_else(|e| panic!("'{input}' should parse: {e}"));
                    let expected = (spec.build)(Some(sub.name)).unwrap();
                    assert_eq!(
                        action, expected,
                        "build({}, Some({})) must match parse_action result",
                        spec.name, sub.name
                    );
                }
            }
        }
    }
}
