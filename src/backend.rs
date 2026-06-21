use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{mpsc, watch};

use crate::commands::Action;
use crate::event::LlmEvent;
use crate::message::{Message, ToolCall, ToolStatus};
use crate::mock_llm::{self, Scenario};
use crate::transport::{self, Protocol, TransportEvent, TransportHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Plan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalChoice {
    Yes,
    No,
    Always,
}

#[derive(Debug, Clone)]
pub struct ModalRequest {
    pub tool_index: usize,
    pub tool_name: String,
    pub args_preview: String,
}

#[derive(Debug)]
pub enum Command {
    Run(Action),
    Permission(ModalChoice),
    ShowSystem(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting { protocol: Protocol, addr: String },
    Connected { protocol: Protocol, addr: String },
    Error(String),
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
}

impl ViewState {
    pub fn initial(model: String) -> Self {
        Self {
            messages: Arc::new(vec![Message::System(
                "Welcome — type 'help' (or just 'h') and Enter to see commands.".into(),
            )]),
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

pub struct BackendState {
    messages: Vec<Message>,
    model: String,
    mode: Mode,
    streaming: bool,
    allow_always: HashSet<String>,
    modal: Option<ModalRequest>,
    should_quit: bool,
    llm_tx: mpsc::Sender<LlmEvent>,
    llm_rx: mpsc::Receiver<LlmEvent>,
    transport_tx: mpsc::Sender<TransportEvent>,
    pub transport_rx: mpsc::Receiver<TransportEvent>,
    transport_handle: Option<TransportHandle>,
    conn: ConnState,
    send_counter: u64,
    recv_counter: u64,
    latest_recv: Option<serde_json::Value>,
    latest_recv_at: Option<chrono::DateTime<chrono::Local>>,
}

impl BackendState {
    pub fn new(model: String) -> Self {
        let (llm_tx, llm_rx) = mpsc::channel(64);
        let (transport_tx, transport_rx) = mpsc::channel(64);
        Self {
            messages: vec![Message::System(
                "Welcome — type 'help' (or just 'h') and Enter to see commands.".into(),
            )],
            model,
            mode: Mode::Normal,
            streaming: false,
            allow_always: HashSet::new(),
            modal: None,
            should_quit: false,
            llm_tx,
            llm_rx,
            transport_tx,
            transport_rx,
            transport_handle: None,
            conn: ConnState::Disconnected,
            send_counter: 0,
            recv_counter: 0,
            latest_recv: None,
            latest_recv_at: None,
        }
    }

    pub fn snapshot(&self) -> ViewState {
        ViewState {
            messages: Arc::new(self.messages.clone()),
            model: self.model.clone(),
            mode: self.mode,
            streaming: self.streaming,
            modal: self.modal.clone(),
            should_quit: self.should_quit,
            conn: self.conn.clone(),
            latest_recv: self.latest_recv.clone(),
            latest_recv_at: self.latest_recv_at,
        }
    }

    fn publish(&self, view_tx: &watch::Sender<ViewState>) {
        let _ = view_tx.send(self.snapshot());
    }

    pub fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::Run(action) => self.run_action(action),
            Command::Permission(choice) => self.resolve_modal(choice),
            Command::ShowSystem(text) => self.messages.push(Message::System(text)),
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Help => {
                let mut s = String::from("Available commands:\n");
                for c in crate::commands::COMMANDS {
                    s.push_str(&format!("  {:<8} — {}\n", c.name, c.desc));
                    for (sn, sd) in c.subs {
                        s.push_str(&format!("      {:<6} {}\n", sn, sd));
                    }
                }
                s.push_str(
                    "\nPrefixes work at both levels: 'p o' runs 'plan on', 'd c' is ambiguous.",
                );
                s.push_str(
                    "\n\nHotkeys:\n  Ctrl+G  help          Ctrl+L  clear         Ctrl+Q  exit\n  Ctrl+P  toggle plan   Ctrl+O  cycle model   Ctrl+E  cycle demo\n  Ctrl+S  toggle state panel\n  Ctrl+B / PgUp  scroll up    Ctrl+F / PgDn  scroll down",
                );
                self.messages.push(Message::System(s));
            }
            Action::Clear => {
                self.messages.clear();
                self.messages
                    .push(Message::System("conversation cleared".into()));
            }
            Action::Exit => self.should_quit = true,
            Action::Model(name) => {
                self.model = format!("mock-{name}");
                self.messages
                    .push(Message::System(format!("model -> {}", self.model)));
            }
            Action::Plan(state) => {
                self.mode = match state {
                    "on" => Mode::Plan,
                    _ => Mode::Normal,
                };
                self.messages
                    .push(Message::System(format!("mode -> {:?}", self.mode)));
            }
            Action::Demo(kind) => {
                let scenario = match kind {
                    "chat" => Scenario::Chat,
                    "code" => Scenario::Code,
                    _ => Scenario::Tool,
                };
                self.start_demo(scenario);
            }
            Action::Connect(proto) => match Protocol::from_name(proto) {
                Some(p) => {
                    let addr = p.default_addr().to_string();
                    self.connect(p, addr);
                }
                None => self.messages.push(Message::System(format!(
                    "unknown protocol: {proto}"
                ))),
            },
            Action::Disconnect => self.disconnect(),
            Action::Send => self.send_json(),
        }
    }

    fn connect(&mut self, protocol: Protocol, addr: String) {
        if matches!(
            self.conn,
            ConnState::Connecting { .. } | ConnState::Connected { .. }
        ) {
            self.messages.push(Message::System(format!(
                "already connected/connecting (state: {:?})",
                self.conn
            )));
            return;
        }
        self.conn = ConnState::Connecting {
            protocol,
            addr: addr.clone(),
        };
        self.messages.push(Message::System(format!(
            "connecting [{}] to {addr}...",
            protocol.as_str()
        )));
        self.transport_handle =
            Some(transport::spawn(protocol, addr, self.transport_tx.clone()));
    }

    fn disconnect(&mut self) {
        if self.transport_handle.take().is_none() {
            self.messages
                .push(Message::System("not connected".into()));
            return;
        }
        self.conn = ConnState::Disconnected;
        self.messages
            .push(Message::System("disconnected".into()));
    }

    fn send_json(&mut self) {
        let handle = match self.transport_handle.as_ref() {
            Some(h) => h,
            None => {
                self.messages.push(Message::System(
                    "not connected — run 'con <protocol>' first".into(),
                ));
                return;
            }
        };
        self.send_counter += 1;
        let id = self.send_counter;
        let payload = serde_json::json!({
            "id": id,
            "msg": format!("ping {id}"),
        });
        let line = payload.to_string();
        self.messages
            .push(Message::System(format!("→ send: {line}")));
        if let Err(e) = handle.out_tx.try_send(line) {
            self.messages
                .push(Message::System(format!("send failed: {e}")));
        }
    }

    pub fn handle_transport(&mut self, ev: TransportEvent) {
        match ev {
            TransportEvent::Connecting { protocol, addr } => {
                self.conn = ConnState::Connecting { protocol, addr };
            }
            TransportEvent::Connected { protocol, addr } => {
                self.messages.push(Message::System(format!(
                    "[{}] connected: {addr}",
                    protocol.as_str()
                )));
                self.conn = ConnState::Connected { protocol, addr };
            }
            TransportEvent::Recv(line) => {
                let bytes = line.len();
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => {
                        self.recv_counter += 1;
                        let n = self.recv_counter;
                        self.latest_recv = Some(v);
                        self.latest_recv_at = Some(chrono::Local::now());
                        self.messages.push(Message::System(format!(
                            "← recv #{n} ({bytes} bytes)"
                        )));
                    }
                    Err(e) => {
                        self.messages.push(Message::System(format!(
                            "← recv (invalid JSON: {e})\nraw: {line}"
                        )));
                    }
                }
            }
            TransportEvent::Disconnected => {
                self.conn = ConnState::Disconnected;
                self.transport_handle = None;
                self.messages
                    .push(Message::System("peer closed connection".into()));
            }
            TransportEvent::Error(e) => {
                self.conn = ConnState::Error(e.clone());
                self.transport_handle = None;
                self.messages
                    .push(Message::System(format!("transport error: {e}")));
            }
        }
    }

    fn start_demo(&mut self, scenario: Scenario) {
        self.messages.push(Message::Assistant {
            text: String::new(),
            streaming: true,
        });
        self.streaming = true;
        mock_llm::spawn(scenario, self.llm_tx.clone());
    }

    pub fn handle_llm(&mut self, ev: LlmEvent) {
        match ev {
            LlmEvent::Token(t) => {
                if let Some(Message::Assistant { text, .. }) = self.messages.last_mut() {
                    text.push_str(&t);
                }
            }
            LlmEvent::StartTool {
                tool_name,
                args_preview,
            } => {
                let auto = self.allow_always.contains(&tool_name);
                let status = if auto {
                    ToolStatus::Running
                } else {
                    ToolStatus::AwaitingPermission
                };
                self.messages.push(Message::Tool(ToolCall {
                    name: tool_name.clone(),
                    args_preview: args_preview.clone(),
                    status,
                    output: String::new(),
                }));
                if !auto {
                    let idx = self.messages.len() - 1;
                    self.modal = Some(ModalRequest {
                        tool_index: idx,
                        tool_name,
                        args_preview,
                    });
                }
            }
            LlmEvent::ToolDone { output } => {
                if let Some(Message::Tool(t)) = self.messages.last_mut() {
                    if t.status != ToolStatus::Denied {
                        t.status = ToolStatus::Done;
                        t.output = output;
                    }
                }
                self.messages.push(Message::Assistant {
                    text: String::new(),
                    streaming: true,
                });
            }
            LlmEvent::Done => {
                if let Some(Message::Assistant { streaming, .. }) = self.messages.last_mut() {
                    *streaming = false;
                }
                self.streaming = false;
            }
        }
    }

    fn resolve_modal(&mut self, choice: ModalChoice) {
        let req = match self.modal.take() {
            Some(r) => r,
            None => return,
        };
        if let Some(Message::Tool(t)) = self.messages.get_mut(req.tool_index) {
            match choice {
                ModalChoice::Yes => t.status = ToolStatus::Running,
                ModalChoice::Always => {
                    t.status = ToolStatus::Running;
                    self.allow_always.insert(t.name.clone());
                }
                ModalChoice::No => {
                    t.status = ToolStatus::Denied;
                    t.output = "(permission denied)".into();
                }
            }
        }
    }
}

pub async fn run(
    mut cmd_rx: mpsc::Receiver<Command>,
    view_tx: watch::Sender<ViewState>,
    initial_model: String,
) {
    let mut state = BackendState::new(initial_model);
    state.publish(&view_tx);

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(cmd) => state.handle_command(cmd),
                None => break,
            },
            Some(ev) = state.llm_rx.recv() => state.handle_llm(ev),
            Some(ev) = state.transport_rx.recv() => state.handle_transport(ev),
        }
        state.publish(&view_tx);
        if state.should_quit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_action_sets_mode() {
        let mut s = BackendState::new("mock-claude".into());
        s.handle_command(Command::Run(Action::Plan("on")));
        assert_eq!(s.snapshot().mode, Mode::Plan);
        s.handle_command(Command::Run(Action::Plan("off")));
        assert_eq!(s.snapshot().mode, Mode::Normal);
    }

    #[tokio::test]
    async fn demo_action_marks_streaming_and_appends_assistant() {
        let mut s = BackendState::new("mock-claude".into());
        s.handle_command(Command::Run(Action::Demo("tool")));
        let view = s.snapshot();
        assert!(view.streaming, "streaming should be true after demo start");
        assert!(matches!(
            view.messages.last(),
            Some(Message::Assistant { streaming: true, .. })
        ));
    }

    #[tokio::test]
    async fn done_event_clears_streaming() {
        let mut s = BackendState::new("mock-claude".into());
        s.handle_command(Command::Run(Action::Demo("chat")));
        assert!(s.snapshot().streaming);
        s.handle_llm(LlmEvent::Done);
        assert!(!s.snapshot().streaming);
    }

    #[test]
    fn modal_yes_runs_tool() {
        let mut s = BackendState::new("mock-claude".into());
        s.handle_llm(LlmEvent::StartTool {
            tool_name: "Bash".into(),
            args_preview: "ls".into(),
        });
        assert!(s.snapshot().modal.is_some());
        s.handle_command(Command::Permission(ModalChoice::Yes));
        let view = s.snapshot();
        assert!(view.modal.is_none());
        if let Some(Message::Tool(t)) = view.messages.last() {
            assert_eq!(t.status, ToolStatus::Running);
        } else {
            panic!("expected last message to be Tool");
        }
    }
}
