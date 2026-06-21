use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::event::LlmEvent;

#[derive(Debug, Clone, Copy)]
pub enum Scenario {
    Chat,
    Code,
    Tool,
}

pub fn spawn(scenario: Scenario, tx: mpsc::Sender<LlmEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let chunks = scripted_response(scenario);
        for ev in chunks {
            match &ev {
                LlmEvent::Token(_) => {
                    tokio::time::sleep(Duration::from_millis(18)).await;
                }
                LlmEvent::StartTool { .. } => {
                    tokio::time::sleep(Duration::from_millis(120)).await;
                }
                LlmEvent::ToolDone { .. } => {
                    tokio::time::sleep(Duration::from_millis(60)).await;
                }
                LlmEvent::Done => {}
            }
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    })
}

fn scripted_response(scenario: Scenario) -> Vec<LlmEvent> {
    let body = match scenario {
        Scenario::Chat => CHAT_RESPONSE,
        Scenario::Code => CODE_RESPONSE,
        Scenario::Tool => TOOL_RESPONSE,
    };

    let mut events = Vec::new();
    let mut buf = String::new();
    let mut chars = body.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\u{0001}' {
            if !buf.is_empty() {
                events.push(LlmEvent::Token(std::mem::take(&mut buf)));
            }
            let mut name = String::new();
            while let Some(&c2) = chars.peek() {
                chars.next();
                if c2 == '\u{0002}' {
                    break;
                }
                name.push(c2);
            }
            let mut args = String::new();
            while let Some(&c2) = chars.peek() {
                chars.next();
                if c2 == '\u{0003}' {
                    break;
                }
                args.push(c2);
            }
            let mut output = String::new();
            while let Some(&c2) = chars.peek() {
                chars.next();
                if c2 == '\u{0004}' {
                    break;
                }
                output.push(c2);
            }
            events.push(LlmEvent::StartTool {
                tool_name: name,
                args_preview: args,
            });
            events.push(LlmEvent::ToolDone { output });
            continue;
        }
        buf.push(c);
        if buf.chars().count() >= 4 {
            events.push(LlmEvent::Token(std::mem::take(&mut buf)));
        }
    }
    if !buf.is_empty() {
        events.push(LlmEvent::Token(buf));
    }
    events.push(LlmEvent::Done);
    events
}

const CHAT_RESPONSE: &str = "Hi! I'm a **mock** assistant pretending to be Claude Code.\n\n\
Here is a quick *plan* for what we could do:\n\
1. Restate the problem\n\
2. List candidate approaches\n\
3. Pick the smallest one that works\n\n\
Run `demo-code` to see syntax highlighting, or `demo-tool` for a tool card.\n";

const CODE_RESPONSE: &str = "Here is a tiny Rust snippet that loops:\n\n\
```rust\n\
fn main() {\n\
    for i in 0..5 {\n\
        println!(\"hello {i}\");\n\
    }\n\
}\n\
```\n\n\
And the equivalent in shell:\n\n\
```bash\n\
for i in 0 1 2 3 4; do echo \"hello $i\"; done\n\
```\n";

// \u{0001} starts a tool block: name \u{0002} args \u{0003} output \u{0004}
const TOOL_RESPONSE: &str = "Sure, let me list the directory for you.\n\n\
\u{0001}Bash\u{0002}ls -la\u{0003}total 24\ndrwxr-xr-x  src/\n-rw-r--r--  Cargo.toml\n-rw-r--r--  Cargo.lock\n\u{0004}\n\
That looks like a normal Rust project.\n";
