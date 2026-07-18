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
    // Generate task tab descriptions from TASK_DEFS.
    s.push_str("Task tabs (←/→ to switch):\n");
    for d in crate::backend::TASK_DEFS {
        let _ = writeln!(s, "  {:<8} — {}", d.name, d.hint);
    }
    s.push_str("\n");
    s.push_str(HOTKEYS);
    s
}
