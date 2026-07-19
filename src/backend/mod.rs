mod chat;
mod conn;
mod llm;
mod modal;
pub mod task;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::message::{LogLevel, Message};

pub use chat::Mode;
pub use modal::{ModalChoice, ModalRequest};
pub use task::TaskInternalState;
pub use task::registry::{TaskInfo, TASK_DEFS};

#[derive(Debug)]
pub enum Command {
    Input(String),
    TagSwitch(String),
    Permission(ModalChoice),
}

#[derive(Debug, Clone)]
pub struct ViewState {
    pub messages: Arc<Vec<Message>>,
    pub mode: Mode,
    pub streaming: bool,
    pub modal: Option<ModalRequest>,
    pub should_quit: bool,
    pub internal: TaskInternalState,
    pub tasks: Arc<Vec<TaskInfo>>,
    pub active_task_index: usize,
    pub active_commands: Arc<Vec<task::CommandDef>>,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
}

impl ViewState {
    pub fn initial() -> Self {
        let (tasks, first_name) = if TASK_DEFS.is_empty() {
            (vec![], String::new())
        } else {
            let d = &TASK_DEFS[0];
            let tasks: Vec<TaskInfo> = TASK_DEFS.iter()
                .map(|d| TaskInfo { name: d.name.into(), active: false })
                .collect();
            (tasks, d.name.to_string())
        };
        let msg = if first_name.is_empty() {
            "no tasks configured — check tasks.toml and features".to_string()
        } else {
            format!("[{}] {} — type 'help' for commands", first_name, TASK_DEFS[0].hint)
        };
        Self {
            messages: Arc::new(vec![
                Message::System { text: msg, level: LogLevel::Notice },
            ]),
            mode: Mode::Normal, streaming: false, modal: None, should_quit: false,
            internal: TaskInternalState::default(),
            tasks: Arc::new(tasks),
            active_task_index: 0,
            active_commands: Arc::new(vec![]),
            evicted_lines: 0,
            buffer_total_lines: 0,
        }
    }
}

struct TaskRuntime {
    name: String,
    handle: task::TaskHandle,
    commands: Arc<Vec<task::CommandDef>>,
}

pub struct Router {
    tasks: Vec<TaskRuntime>,
    active: usize,
    should_quit: bool,
    modal: modal::ModalSubsystem,
}

impl Router {
    pub fn new() -> Self {
        let tasks: Vec<TaskRuntime> = TASK_DEFS.iter().filter_map(|def| {
            task::create_actor(def.name, def).map(|rt| TaskRuntime {
                name: def.name.to_string(),
                handle: rt.handle,
                commands: rt.commands,
            })
        }).collect();
        assert!(!tasks.is_empty(), "no task actors created — check features and tasks.toml");
        Self { tasks, active: 0, should_quit: false, modal: modal::ModalSubsystem::new() }
    }

    fn build_view(&self) -> ViewState {
        let task_infos: Vec<TaskInfo> = self.tasks.iter().map(|rt| {
            let snap = rt.handle.state_rx.borrow();
            TaskInfo {
                name: snap.name.clone(),
                active: snap.internal.active,
            }
        }).collect();

        let active_rt = &self.tasks[self.active];
        let snap = active_rt.handle.state_rx.borrow().clone();

        ViewState {
            messages: snap.messages.clone(),
            mode: Mode::Normal,
            streaming: false,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            internal: snap.internal,
            tasks: Arc::new(task_infos),
            active_task_index: self.active,
            active_commands: active_rt.commands.clone(),
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
                        let _ = router.tasks[router.active].handle.cmd_tx.try_send(text);
                    }
                }
                Some(Command::TagSwitch(name)) => {
                    if let Some(pos) = router.tasks.iter().position(|rt| rt.name == name) {
                        router.active = pos;
                    }
                }
                Some(Command::Permission(_choice)) => {}
                None => break,
            },
            _ = tick.tick() => {}
        }

        let _ = view_tx.send(router.build_view());
        if router.should_quit { break; }
    }
}
