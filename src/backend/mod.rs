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
pub use conn::ConnState;
pub use modal::{ModalChoice, ModalRequest};
pub use task::registry::{TaskDef, TaskInfo, TASK_DEFS};

#[derive(Debug)]
pub enum Command {
    Input(String),
    TagSwitch(String),
    Permission(ModalChoice),
}

#[derive(Debug, Clone)]
pub struct ViewState {
    pub messages: Arc<Vec<Message>>,
    pub model: String,
    pub mode: Mode,
    pub streaming: bool,
    pub modal: Option<ModalRequest>,
    pub should_quit: bool,
    pub conn: ConnState,
    pub latest_recv: Option<serde_json::Value>,
    pub latest_recv_at: Option<chrono::DateTime<chrono::Local>>,
    pub tasks: Arc<Vec<TaskInfo>>,
    pub active_task: String,
    pub active_task_index: usize,
    pub active_commands: Arc<Vec<task::CommandDef>>,
}

impl ViewState {
    pub fn initial(model: String) -> Self {
        let (tasks, first_name) = if TASK_DEFS.is_empty() {
            (vec![], String::new())
        } else {
            let d = &TASK_DEFS[0];
            let tasks: Vec<TaskInfo> = TASK_DEFS.iter()
                .map(|d| TaskInfo { name: d.name.into(), demo_running: false, conn: ConnState::Disconnected })
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
            model,
            mode: Mode::Normal, streaming: false, modal: None, should_quit: false,
            conn: ConnState::Disconnected, latest_recv: None, latest_recv_at: None,
            tasks: Arc::new(tasks),
            active_task: first_name, active_task_index: 0,
            active_commands: Arc::new(vec![]),
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
    pub fn new(model: String) -> Self {
        let tasks: Vec<TaskRuntime> = TASK_DEFS.iter().filter_map(|def| {
            task::create_actor(def.name, model.clone(), def).map(|rt| TaskRuntime {
                name: def.name.to_string(),
                handle: rt.handle,
                commands: rt.commands,
            })
        }).collect();
        assert!(!tasks.is_empty(), "no task actors created — check features and tasks.toml");
        Self { tasks, active: 0, should_quit: false, modal: modal::ModalSubsystem::new() }
    }

    /// Build ViewState from live actor snapshots — reads every actor's
    /// state_rx so the tab bar shows real connection status and demo_running.
    fn build_view(&self) -> ViewState {
        // Collect live TaskInfo from all actors (not from TASK_DEFS).
        let task_infos: Vec<TaskInfo> = self.tasks.iter().map(|rt| {
            let snap = rt.handle.state_rx.borrow();
            TaskInfo {
                name: snap.name.clone(),
                demo_running: snap.demo_running,
                conn: snap.conn.clone(),
            }
        }).collect();

        let active_rt = &self.tasks[self.active];
        let snap = active_rt.handle.state_rx.borrow().clone();

        ViewState {
            messages: Arc::new(snap.messages),
            model: snap.model,
            mode: Mode::Normal,
            streaming: false,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            conn: snap.conn,
            latest_recv: snap.latest_recv,
            latest_recv_at: snap.latest_recv_at,
            tasks: Arc::new(task_infos),
            active_task: snap.name.clone(),
            active_task_index: self.active,
            active_commands: active_rt.commands.clone(),
        }
    }
}

pub async fn run(
    mut cmd_rx: mpsc::Receiver<Command>,
    view_tx: watch::Sender<ViewState>,
    initial_model: String,
) {
    let mut router = Router::new(initial_model);
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(Command::Input(text)) => {
                    if text.trim() == "exit" {
                        router.should_quit = true;
                    } else {
                        // Use send().await to backpressure instead of silently dropping.
                        // The select! polls this branch only when the channel has capacity.
                        let tx = router.tasks[router.active].handle.cmd_tx.clone();
                        if tx.send(text).await.is_err() {
                            // Actor died — shouldn't happen in normal operation.
                            break;
                        }
                    }
                }
                Some(Command::TagSwitch(name)) => {
                    // Search self.tasks (runtime array, not TASK_DEFS) so
                    // feature-filtered tasks are excluded from the index.
                    if let Some(pos) = router.tasks.iter().position(|rt| rt.name == name) {
                        router.active = pos;
                    }
                }
                Some(Command::Permission(_choice)) => {
                    // Permission modal handling requires mock-llm wiring.
                    // Currently a no-op; the feature-gated code path is unused.
                }
                None => break,
            },
            _ = tick.tick() => {}
        }

        let _ = view_tx.send(router.build_view());
        if router.should_quit { break; }
    }
}
