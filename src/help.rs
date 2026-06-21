use std::fmt::Write;

use crate::commands::COMMANDS;

pub const WELCOME: &str =
    "Welcome — type 'help' (or just 'h') and Enter to see commands.";

const HOTKEYS: &str = "\
Hotkeys:
  Ctrl+G  help          Ctrl+L  clear         Ctrl+Q  exit
  Ctrl+P  toggle plan   Ctrl+O  cycle model   Ctrl+E  cycle demo
  Ctrl+S  toggle state panel
  Ctrl+B / PgUp  scroll up    Ctrl+F / PgDn  scroll down";

const PREFIX_NOTE: &str =
    "Prefixes work at both levels: 'p o' runs 'plan on', 'd c' is ambiguous.";

pub fn full_help() -> String {
    let mut s = String::from("Available commands:\n");
    for c in COMMANDS {
        let _ = writeln!(s, "  {:<8} — {}", c.name, c.desc);
        for sub in c.subs {
            let _ = writeln!(s, "      {:<6} {}", sub.name, sub.desc);
        }
    }
    s.push('\n');
    s.push_str(PREFIX_NOTE);
    s.push_str("\n\n");
    s.push_str(HOTKEYS);
    s
}
