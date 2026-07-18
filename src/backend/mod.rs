//! Backend orchestration: owns the subsystems, runs the tokio loop, and
//! publishes the immutable `ViewState` snapshot the frontend reads from.
//!
//! Each side of the program touches exactly one thing here:
//! - frontend pushes [`Command`]s; we apply them via `dispatch::run_action`.
//! - subsystems generate events on their own channels; we drain them in the
//!   main `select!` and route through dispatch helpers.
//! - after every step we publish a `ViewState` snapshot.

mod chat;
mod conn;
mod dispatch;
mod llm;
mod modal;
mod task;

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local};
use tokio::sync::{mpsc, watch};

use crate::commands::Action;
use crate::message::Message;

pub use chat::Mode;
pub use conn::ConnState;
pub use modal::{ModalChoice, ModalRequest};
pub use task::{TaskInfo, TaskManager};

#[derive(Debug)]
pub enum Command {
    Run(Action),
    Permission(ModalChoice),
    ShowSystem(String),
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
    pub latest_recv_at: Option<DateTime<Local>>,
    /// Task list for the tab bar.
    pub tasks: Arc<Vec<TaskInfo>>,
    /// Name of the currently active task.
    pub active_task: String,
    /// Index of the currently active task in `tasks`.
    pub active_task_index: usize,
}

impl ViewState {
    pub fn initial(model: String) -> Self {
        Self {
            messages: Arc::new(vec![
                Message::System("[main] general chat  —  model / plan / demo".into()),
                Message::System("type 'help' for commands, ←/→ to switch tabs".into()),
            ]),
            model,
            mode: Mode::Normal,
            streaming: false,
            modal: None,
            should_quit: false,
            conn: ConnState::Disconnected,
            latest_recv: None,
            latest_recv_at: None,
            tasks: Arc::new(vec![
                TaskInfo { name: "main".into(), demo_running: false, conn: ConnState::Disconnected },
                TaskInfo { name: "conn".into(), demo_running: false, conn: ConnState::Disconnected },
                TaskInfo { name: "demo".into(), demo_running: false, conn: ConnState::Disconnected },
            ]),
            active_task: "main".into(),
            active_task_index: 0,
        }
    }
}

pub struct Backend {
    pub tasks: TaskManager,
    pub llm: llm::LlmSubsystem,
    pub modal: modal::ModalSubsystem,
    pub should_quit: bool,
}

impl Backend {
    pub fn new(model: String) -> Self {
        Self {
            tasks: TaskManager::new(model),
            llm: llm::LlmSubsystem::new(),
            modal: modal::ModalSubsystem::new(),
            should_quit: false,
        }
    }

    pub fn snapshot(&self) -> ViewState {
        let active = self.tasks.active();
        ViewState {
            messages: Arc::new(active.chat.messages.clone()),
            model: active.chat.model.clone(),
            mode: active.chat.mode,
            streaming: self.llm.streaming,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            conn: active.conn.conn.clone(),
            latest_recv: active.conn.latest_recv.clone(),
            latest_recv_at: active.conn.latest_recv_at,
            tasks: Arc::new(self.tasks.list()),
            active_task: self.tasks.active_name().to_string(),
            active_task_index: self.tasks.active,
        }
    }

    pub fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Run(action) => dispatch::run_action(self, action),
            Command::Permission(choice) => {
                self.modal.resolve(choice, self.tasks.active_chat_mut());
            }
            Command::ShowSystem(text) => {
                self.tasks.active_mut().chat.push_system(text);
            }
        }
    }
}

pub async fn run(
    mut cmd_rx: mpsc::Receiver<Command>,
    view_tx: watch::Sender<ViewState>,
    initial_model: String,
) {
    let mut backend = Backend::new(initial_model);
    let _ = view_tx.send(backend.snapshot());

    // Periodic tick to drain transport event channels from all tasks.
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(cmd) => backend.handle_command(cmd),
                None => break,
            },
            Some(ev) = backend.llm.rx.recv() => {
                backend.llm.handle_event(
                    ev,
                    backend.tasks.active_chat_mut(),
                    &mut backend.modal,
                );
            }
            Some(task_idx) = backend.tasks.demo_tick_rx.recv() => {
                if task_idx < backend.tasks.tasks.len() {
                    let ts = Local::now().format("%H:%M:%S").to_string();
                    backend.tasks.tasks[task_idx]
                        .chat
                        .push_system(format!("[demo tick {ts}]"));
                }
            }
            _ = tick.tick() => {}  // periodic drain for transport events
        }

        // Drain transport events from every task (non-blocking).
        for task in backend.tasks.tasks.iter_mut() {
            while let Ok(ev) = task.conn.ev_rx.try_recv() {
                let outs = task.conn.handle_event(ev);
                for o in outs {
                    task.chat.push_system(conn::format(&o));
                }
            }
        }

        let _ = view_tx.send(backend.snapshot());
        if backend.should_quit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{DemoScenario, PlanToggle};
    use crate::event::LlmEvent;
    use crate::message::ToolStatus;

    #[test]
    fn plan_action_sets_mode() {
        let mut b = Backend::new("mock-claude".into());
        b.handle_command(Command::Run(Action::Plan(PlanToggle::On)));
        assert_eq!(b.snapshot().mode, Mode::Plan);
        b.handle_command(Command::Run(Action::Plan(PlanToggle::Off)));
        assert_eq!(b.snapshot().mode, Mode::Normal);
    }

    #[tokio::test]
    async fn demo_action_marks_streaming_and_appends_assistant() {
        let mut b = Backend::new("mock-claude".into());
        b.handle_command(Command::Run(Action::Demo(DemoScenario::Tool)));
        let view = b.snapshot();
        assert!(view.streaming);
        assert!(matches!(
            view.messages.last(),
            Some(Message::Assistant { streaming: true, .. })
        ));
    }

    #[tokio::test]
    async fn done_event_clears_streaming() {
        let mut b = Backend::new("mock-claude".into());
        b.handle_command(Command::Run(Action::Demo(DemoScenario::Chat)));
        assert!(b.snapshot().streaming);
        b.llm.handle_event(
            LlmEvent::Done,
            b.tasks.active_chat_mut(),
            &mut b.modal,
        );
        assert!(!b.snapshot().streaming);
    }

    #[test]
    fn modal_yes_runs_tool() {
        let mut b = Backend::new("mock-claude".into());
        b.llm.handle_event(
            LlmEvent::StartTool {
                tool_name: "Bash".into(),
                args_preview: "ls".into(),
            },
            b.tasks.active_chat_mut(),
            &mut b.modal,
        );
        assert!(b.snapshot().modal.is_some());
        b.handle_command(Command::Permission(ModalChoice::Yes));
        let view = b.snapshot();
        assert!(view.modal.is_none());
        if let Some(Message::Tool(t)) = view.messages.last() {
            assert_eq!(t.status, ToolStatus::Running);
        } else {
            panic!("expected last message to be Tool");
        }
    }

    #[test]
    fn starts_with_three_fixed_tasks() {
        let b = Backend::new("mock-claude".into());
        let view = b.snapshot();
        assert_eq!(view.tasks.len(), 3);
        assert_eq!(view.active_task, "main");
        let names: Vec<&str> = view.tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["main", "conn", "demo"]);
    }

    #[test]
    fn task_switch_changes_active() {
        let mut b = Backend::new("mock-claude".into());
        b.handle_command(Command::Run(Action::TaskSwitch("conn".into())));
        let view = b.snapshot();
        assert_eq!(view.active_task, "conn");
        b.handle_command(Command::Run(Action::TaskSwitch("demo".into())));
        let view = b.snapshot();
        assert_eq!(view.active_task, "demo");
    }

    #[test]
    fn messages_are_isolated_between_tabs() {
        let mut b = Backend::new("mock-claude".into());
        // Push to main tab
        b.handle_command(Command::ShowSystem("hello from main".into()));
        // Switch to conn — should NOT see main's message
        b.handle_command(Command::Run(Action::TaskSwitch("conn".into())));
        let view = b.snapshot();
        assert_eq!(view.active_task, "conn");
        // conn tab should NOT contain "hello from main"
        let has_main_msg = view.messages.iter().any(|m| {
            matches!(m, Message::System(s) if s.contains("hello from main"))
        });
        assert!(!has_main_msg, "conn tab should not see main's messages");
        // conn tab should have its own welcome
        let has_conn_welcome = view.messages.iter().any(|m| {
            matches!(m, Message::System(s) if s.contains("[conn]"))
        });
        assert!(has_conn_welcome, "conn tab should have its own welcome");
        // Switch back to main — should see main's message
        b.handle_command(Command::Run(Action::TaskSwitch("main".into())));
        let view = b.snapshot();
        let has_main_msg = view.messages.iter().any(|m| {
            matches!(m, Message::System(s) if s.contains("hello from main"))
        });
        assert!(has_main_msg, "main tab should still have its message");
    }

    #[tokio::test]
    async fn start_stop_demo_toggles_flag() {
        let mut b = Backend::new("mock-claude".into());
        b.handle_command(Command::Run(Action::Start));
        let view = b.snapshot();
        assert!(view.tasks[0].demo_running);
        b.handle_command(Command::Run(Action::Stop));
        let view = b.snapshot();
        assert!(!view.tasks[0].demo_running);
    }
}
