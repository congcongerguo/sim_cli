use std::fmt::Write;

use crate::commands::COMMANDS;

pub const WELCOME: &str =
    "Welcome — type 'help' (or just 'h') and Enter to see commands.";

const HOTKEYS: &str = "\
Hotkeys:
  Ctrl+G  help          Ctrl+L  clear         Ctrl+Q  exit
  Ctrl+P  toggle plan   Ctrl+O  cycle model   Ctrl+E  cycle demo
  Ctrl+S  toggle state panel
  ←/→     previous / next task tab (when input is empty)
  Ctrl+B / PgUp  scroll up    Ctrl+F / PgDn  scroll down";

const PREFIX_NOTE: &str =
    "Prefixes work at both levels: 'p o' runs 'plan on', 'd c' is ambiguous.";

const TASK_NOTE: &str = "\
3 fixed task tabs: main, conn, demo. Use ←/→ to switch.
  main — all commands (model, plan, demo, con, start, stop)
  conn — transport only (con, close, send)
  demo — logger only (start, stop)";

pub fn full_help(task_name: &str) -> String {
    let filter = crate::commands::task_filter(task_name);
    let mut s = format!("Available commands ({task_name}):\n");
    for c in COMMANDS {
        if filter.map_or(true, |f| f.contains(&c.name)) {
            let _ = writeln!(s, "  {:<8} — {}", c.name, c.desc);
            for sub in c.subs {
                let _ = writeln!(s, "      {:<6} {}", sub.name, sub.desc);
            }
        }
    }
    s.push('\n');
    s.push_str(PREFIX_NOTE);
    s.push_str("\n\n");
    s.push_str(TASK_NOTE);
    s.push_str("\n\n");
    s.push_str(HOTKEYS);
    s
}
