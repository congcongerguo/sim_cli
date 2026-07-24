mod chat;
pub mod conn;
mod llm;
mod modal;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::message::{LogLevel, Message, TimedMessage};
use crate::tool;
use crate::tool::{ToolHandle, ToolInfo, ToolState};

pub use chat::Mode;
pub use modal::{ModalChoice, ModalRequest};
pub use tool::registry::TOOL_DEFS;

#[derive(Debug)]
pub enum Command {
    Input(String),
    TagSwitch(String),
    Permission(#[allow(dead_code)] ModalChoice),
}

#[derive(Debug, Clone)]
pub struct ViewState {
    pub messages: Arc<Vec<TimedMessage>>,
    pub mode: Mode,
    pub streaming: bool,
    pub modal: Option<ModalRequest>,
    pub should_quit: bool,
    pub state: ToolState,
    pub tools: Arc<Vec<ToolInfo>>,
    pub active_index: usize,
    pub active_cmds: Arc<Vec<tool::Cmd>>,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
}

impl ViewState {
    pub fn initial() -> Self {
        let tools: Vec<ToolInfo> = TOOL_DEFS.iter()
            .map(|d| ToolInfo { name: d.name.into(), active: false })
            .collect();
        let first = TOOL_DEFS.first();
        let msg = match first {
            Some(d) => format!("[{}] {} - type 'help' for commands", d.name, d.hint),
            None => "no tools configured - check tasks.toml and features".to_string(),
        };
        Self {
            messages: Arc::new(vec![
                TimedMessage {
                    time: chrono::Local::now(),
                    msg: Message::System { text: msg, level: LogLevel::Notice },
                },
            ]),
            mode: Mode::Normal,
            streaming: false,
            modal: None,
            should_quit: false,
            state: ToolState::default(),
            tools: Arc::new(tools),
            active_index: 0,
            active_cmds: Arc::new(vec![]),
            evicted_lines: 0,
            buffer_total_lines: 0,
        }
    }
}

struct ToolEntry {
    name: String,
    handle: ToolHandle,
    cmds: Arc<Vec<tool::Cmd>>,
}

pub struct Router {
    tools: Vec<ToolEntry>,
    active: usize,
    should_quit: bool,
    modal: modal::ModalSubsystem,
}

impl Router {
    pub fn new() -> Self {
        let tools: Vec<ToolEntry> = TOOL_DEFS.iter().filter_map(|def| {
            tool::create(def).map(|(handle, cmds)| ToolEntry { name: def.name.to_string(), handle, cmds })
        }).collect();
        assert!(!tools.is_empty(), "no tools created — check features and tasks.toml");
        Self { tools, active: 0, should_quit: false, modal: modal::ModalSubsystem::new() }
    }

    fn build_view(&self) -> ViewState {
        let tool_infos: Vec<ToolInfo> = self.tools.iter().map(|t| {
            let snap = t.handle.view_rx.borrow();
            ToolInfo { name: snap.name.clone(), active: snap.state.active }
        }).collect();

        let active = &self.tools[self.active];
        let snap = active.handle.view_rx.borrow().clone();

        ViewState {
            messages: snap.messages,
            mode: Mode::Normal,
            streaming: false,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            state: snap.state,
            tools: Arc::new(tool_infos),
            active_index: self.active,
            active_cmds: active.cmds.clone(),
            evicted_lines: snap.evicted_lines,
            buffer_total_lines: snap.buffer_total_lines,
        }
    }
}

pub async fn run(
    mut cmd_rx: mpsc::Receiver<Command>,
    view_tx: watch::Sender<ViewState>,
) {
    let mut router = Router::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(Command::Input(text)) => {
                    if text.trim() == "exit" {
                        router.should_quit = true;
                    } else {
                        let _ = router.tools[router.active].handle.cmd_tx.try_send(text);
                    }
                }
                Some(Command::TagSwitch(name)) => {
                    if let Some(pos) = router.tools.iter().position(|t| t.name == name) {
                        router.active = pos;
                    }
                }
                Some(Command::Permission(_)) => {}
                None => break,
            },
            _ = tick.tick() => {}
        }

        let _ = view_tx.send(router.build_view());
        if router.should_quit { break; }
    }
}
