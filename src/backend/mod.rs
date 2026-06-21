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

use std::sync::Arc;

use chrono::{DateTime, Local};
use tokio::sync::{mpsc, watch};

use crate::commands::Action;
use crate::message::Message;

pub use chat::{ChatState, Mode};
pub use conn::ConnState;
pub use modal::{ModalChoice, ModalRequest};

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
}

impl ViewState {
    pub fn initial(model: String) -> Self {
        Self {
            messages: Arc::new(vec![Message::System(crate::help::WELCOME.into())]),
            model,
            mode: Mode::Normal,
            streaming: false,
            modal: None,
            should_quit: false,
            conn: ConnState::Disconnected,
            latest_recv: None,
            latest_recv_at: None,
        }
    }
}

pub struct Backend {
    pub chat: ChatState,
    pub conn: conn::ConnSubsystem,
    pub llm: llm::LlmSubsystem,
    pub modal: modal::ModalSubsystem,
    pub should_quit: bool,
}

impl Backend {
    pub fn new(model: String) -> Self {
        Self {
            chat: ChatState::new(model),
            conn: conn::ConnSubsystem::new(),
            llm: llm::LlmSubsystem::new(),
            modal: modal::ModalSubsystem::new(),
            should_quit: false,
        }
    }

    pub fn snapshot(&self) -> ViewState {
        ViewState {
            messages: Arc::new(self.chat.messages.clone()),
            model: self.chat.model.clone(),
            mode: self.chat.mode,
            streaming: self.llm.streaming,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            conn: self.conn.conn.clone(),
            latest_recv: self.conn.latest_recv.clone(),
            latest_recv_at: self.conn.latest_recv_at,
        }
    }

    pub fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Run(action) => dispatch::run_action(self, action),
            Command::Permission(choice) => self.modal.resolve(choice, &mut self.chat),
            Command::ShowSystem(text) => self.chat.push_system(text),
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

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(cmd) => backend.handle_command(cmd),
                None => break,
            },
            Some(ev) = backend.llm.rx.recv() => {
                backend.llm.handle_event(ev, &mut backend.chat, &mut backend.modal);
            }
            Some(ev) = backend.conn.ev_rx.recv() => {
                dispatch::apply_transport_event(&mut backend, ev);
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
        b.llm.handle_event(LlmEvent::Done, &mut b.chat, &mut b.modal);
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
            &mut b.chat,
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
}
