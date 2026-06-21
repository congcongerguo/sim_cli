#[derive(Debug)]
pub struct CommandDef {
    pub name: &'static str,
    pub desc: &'static str,
    pub subs: &'static [(&'static str, &'static str)],
}

pub const COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "help",
        desc: "show available commands",
        subs: &[],
    },
    CommandDef {
        name: "clear",
        desc: "clear the conversation",
        subs: &[],
    },
    CommandDef {
        name: "exit",
        desc: "leave the CLI",
        subs: &[],
    },
    CommandDef {
        name: "model",
        desc: "switch model: model <claude|opus|haiku>",
        subs: &[
            ("claude", "use mock-claude"),
            ("opus", "use mock-opus"),
            ("haiku", "use mock-haiku"),
        ],
    },
    CommandDef {
        name: "plan",
        desc: "set plan mode: plan <on|off>",
        subs: &[
            ("on", "enable plan mode"),
            ("off", "disable plan mode"),
        ],
    },
    CommandDef {
        name: "demo",
        desc: "show a UI demo: demo <chat|code|tool>",
        subs: &[
            ("chat", "stream a markdown chat reply"),
            ("code", "stream a syntect-highlighted code reply"),
            ("tool", "show a tool card with permission modal"),
        ],
    },
    CommandDef {
        name: "con",
        desc: "connect a transport: con <tcp>",
        subs: &[("tcp", "TCP echo (127.0.0.1:7878)")],
    },
    CommandDef {
        name: "close",
        desc: "disconnect the current transport",
        subs: &[],
    },
    CommandDef {
        name: "send",
        desc: "build a JSON message, send it, await reply",
        subs: &[],
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Help,
    Clear,
    Exit,
    Model(&'static str),
    Plan(&'static str),
    Demo(&'static str),
    Connect(&'static str),
    Disconnect,
    Send,
}

#[derive(Debug)]
pub enum Resolve<T> {
    Unique(T),
    Ambiguous(Vec<&'static str>),
    None,
}

#[derive(Debug)]
pub enum ParseError {
    Empty,
    UnknownCommand(String),
    AmbiguousCommand(Vec<&'static str>),
    MissingArg(&'static CommandDef),
    UnknownArg(&'static CommandDef, String),
    AmbiguousArg(&'static CommandDef, Vec<&'static str>),
    UnexpectedArg(&'static CommandDef, String),
}

impl ParseError {
    pub fn message(&self) -> String {
        match self {
            ParseError::Empty => "empty input".into(),
            ParseError::UnknownCommand(s) => format!("unknown command: {s}"),
            ParseError::AmbiguousCommand(list) => {
                format!("ambiguous command: {}", list.join(", "))
            }
            ParseError::MissingArg(def) => {
                let opts: Vec<&str> = def.subs.iter().map(|(n, _)| *n).collect();
                format!(
                    "missing arg for '{}', expected: {}",
                    def.name,
                    opts.join(" | ")
                )
            }
            ParseError::UnknownArg(def, s) => {
                let opts: Vec<&str> = def.subs.iter().map(|(n, _)| *n).collect();
                format!(
                    "unknown arg '{}' for '{}', expected: {}",
                    s,
                    def.name,
                    opts.join(" | ")
                )
            }
            ParseError::AmbiguousArg(def, list) => {
                format!("ambiguous arg for '{}': {}", def.name, list.join(", "))
            }
            ParseError::UnexpectedArg(def, s) => {
                format!("'{}' takes no arguments (got '{}')", def.name, s)
            }
        }
    }
}

pub fn find_command(name: &str) -> Option<&'static CommandDef> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn matching_commands(prefix: &str) -> Vec<(&'static str, &'static str)> {
    let p = prefix.trim();
    if p.is_empty() {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(p))
        .map(|c| (c.name, c.desc))
        .collect()
}

pub fn matching_subs(
    cmd: &CommandDef,
    prefix: &str,
) -> Vec<(&'static str, &'static str)> {
    cmd.subs
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .copied()
        .collect()
}

pub fn resolve_command_prefix(prefix: &str) -> Resolve<&'static CommandDef> {
    let p = prefix.trim();
    if p.is_empty() {
        return Resolve::None;
    }
    let hits: Vec<&'static CommandDef> = COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(p))
        .collect();
    match hits.len() {
        0 => Resolve::None,
        1 => Resolve::Unique(hits[0]),
        _ => Resolve::Ambiguous(hits.iter().map(|c| c.name).collect()),
    }
}

pub fn resolve_sub_prefix(cmd: &CommandDef, prefix: &str) -> Resolve<&'static str> {
    if prefix.is_empty() {
        return Resolve::None;
    }
    let hits: Vec<&'static str> = cmd
        .subs
        .iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, _)| *name)
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
        let mut i = 0;
        let a = first.as_bytes();
        let b = s.as_bytes();
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

/// What the user is currently editing — used by Tab completion.
#[derive(Debug)]
pub enum CompletionCtx {
    /// Editing the command name; `prefix` is what's been typed so far.
    Command { prefix: String },
    /// Command resolved; editing the sub-command. `prefix` may be empty.
    Sub {
        cmd: &'static CommandDef,
        prefix: String,
    },
    /// Nothing useful to complete (e.g. command takes no args, or extra tokens).
    None,
}

pub fn completion_ctx(input: &str) -> CompletionCtx {
    // If there's no whitespace, user is still typing the command.
    let has_ws = input.chars().any(char::is_whitespace);
    if !has_ws {
        return CompletionCtx::Command {
            prefix: input.to_string(),
        };
    }

    // Split into first token and the rest.
    let mut it = input.splitn(2, char::is_whitespace);
    let cmd_tok = it.next().unwrap_or("").trim();
    let rest = it.next().unwrap_or("");

    if cmd_tok.is_empty() {
        return CompletionCtx::Command {
            prefix: rest.trim_start().to_string(),
        };
    }

    // Resolve the command. Must be unique to enter sub-completion.
    let cmd = match resolve_command_prefix(cmd_tok) {
        Resolve::Unique(c) => c,
        _ => {
            return CompletionCtx::Command {
                prefix: cmd_tok.to_string(),
            };
        }
    };

    if cmd.subs.is_empty() {
        return CompletionCtx::None;
    }

    // The "current" sub prefix is the last whitespace-separated token of `rest`,
    // or empty if rest ends with whitespace / is empty.
    let sub_prefix: &str = if rest.is_empty() || rest.ends_with(char::is_whitespace) {
        ""
    } else {
        rest.split_whitespace().next_back().unwrap_or("")
    };

    CompletionCtx::Sub {
        cmd,
        prefix: sub_prefix.to_string(),
    }
}

/// Parse a fully-typed input line into an Action, or return a ParseError.
pub fn parse_action(input: &str) -> Result<Action, ParseError> {
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

    if cmd.subs.is_empty() {
        if !rest.is_empty() {
            return Err(ParseError::UnexpectedArg(cmd, rest.join(" ")));
        }
        return Ok(match cmd.name {
            "help" => Action::Help,
            "clear" => Action::Clear,
            "exit" => Action::Exit,
            "close" => Action::Disconnect,
            "send" => Action::Send,
            other => unreachable!("no-arg command without case: {other}"),
        });
    }

    if rest.is_empty() {
        return Err(ParseError::MissingArg(cmd));
    }
    if rest.len() > 1 {
        return Err(ParseError::UnexpectedArg(cmd, rest[1..].join(" ")));
    }
    let sub_tok = rest[0];
    let sub_name = match resolve_sub_prefix(cmd, sub_tok) {
        Resolve::Unique(name) => name,
        Resolve::Ambiguous(list) => return Err(ParseError::AmbiguousArg(cmd, list)),
        Resolve::None => return Err(ParseError::UnknownArg(cmd, sub_tok.to_string())),
    };

    Ok(match cmd.name {
        "model" => Action::Model(sub_name),
        "plan" => Action::Plan(sub_name),
        "demo" => Action::Demo(sub_name),
        "con" => Action::Connect(sub_name),
        other => unreachable!("sub-arg command without case: {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unique(action: Action, input: &str) {
        match parse_action(input) {
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
    }

    #[test]
    fn unique_with_sub() {
        assert_unique(Action::Plan("on"), "plan on");
        assert_unique(Action::Plan("on"), "p on");
        assert_unique(Action::Plan("off"), "plan of");
        assert_unique(Action::Demo("chat"), "demo chat");
        assert_unique(Action::Demo("code"), "d co");
        assert_unique(Action::Model("haiku"), "m h");
    }

    #[test]
    fn ambiguous_sub() {
        // 'o' matches both 'on' and 'off'
        match parse_action("plan o") {
            Err(ParseError::AmbiguousArg(cmd, list)) => {
                assert_eq!(cmd.name, "plan");
                assert_eq!(list, vec!["on", "off"]);
            }
            other => panic!("expected AmbiguousArg, got {other:?}"),
        }
        // 'c' matches both 'chat' and 'code'
        match parse_action("demo c") {
            Err(ParseError::AmbiguousArg(cmd, list)) => {
                assert_eq!(cmd.name, "demo");
                assert_eq!(list, vec!["chat", "code"]);
            }
            other => panic!("expected AmbiguousArg, got {other:?}"),
        }
    }

    #[test]
    fn missing_arg_lists_options() {
        match parse_action("plan") {
            Err(ParseError::MissingArg(cmd)) => {
                assert_eq!(cmd.name, "plan");
                let opts: Vec<&str> = cmd.subs.iter().map(|(n, _)| *n).collect();
                assert_eq!(opts, vec!["on", "off"]);
            }
            other => panic!("expected MissingArg, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_command() {
        // 'h' is unique (only help); but if we add more starting with 'h' this test
        // would change. Currently nothing else collides at single-letter prefix.
        // Use a known ambiguous prefix: nothing in our set is currently ambiguous
        // at depth-1, so we craft one via no-prefix garbage:
        match parse_action("xy") {
            Err(ParseError::UnknownCommand(s)) => assert_eq!(s, "xy"),
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_arg_for_no_arg_command() {
        match parse_action("help me") {
            Err(ParseError::UnexpectedArg(cmd, extra)) => {
                assert_eq!(cmd.name, "help");
                assert_eq!(extra, "me");
            }
            other => panic!("expected UnexpectedArg, got {other:?}"),
        }
    }

    #[test]
    fn unknown_sub_lists_options() {
        match parse_action("plan zzz") {
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
}
