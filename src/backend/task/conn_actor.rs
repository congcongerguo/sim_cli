use crate::message::{LogLevel, Message};
use crate::transport::{Protocol, TransportEvent};

use super::super::chat::ChatState;
use super::super::conn::{self, ConnSubsystem};
use super::registry::TaskDef;
use super::{base_commands, cmd, CommandDef, SubDef, TaskActor, TaskInternalState, TaskSnapshot};

#[cfg(feature = "zmq")]
const CON_SUBS: &[SubDef] = &[
    SubDef { name: "tcp", desc: "TCP echo" },
    SubDef { name: "zmq", desc: "ZMQ pub/sub" },
];
#[cfg(not(feature = "zmq"))]
const CON_SUBS: &[SubDef] = &[
    SubDef { name: "tcp", desc: "TCP echo" },
];

pub struct ConnTask {
    chat: ChatState,
    conn: ConnSubsystem,
    def: &'static TaskDef,
}

impl ConnTask {
    pub fn new(def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(), conn: ConnSubsystem::new(), def }
    }

    fn do_connect(&mut self, sub: Option<&str>) -> Vec<Message> {
        let protocol = match sub {
            Some("tcp") => Protocol::Tcp,
            #[cfg(feature = "zmq")]
            Some("zmq") => Protocol::Zmq,
            _ => return vec![msg("usage: con <tcp|zmq>", LogLevel::Warn)],
        };
        let addr: &str = match protocol {
            Protocol::Tcp => self.def.tcp_addr(),
            #[cfg(feature = "zmq")]
            Protocol::Zmq => self.def.zmq_sub_addr(),
        };
        self.conn.connect(protocol, addr).into_iter().map(|o| fmt_outcome(&o)).collect()
    }

    fn do_disconnect(&mut self) -> Vec<Message> {
        self.conn.disconnect().into_iter().map(|o| fmt_outcome(&o)).collect()
    }

    fn do_send(&mut self) -> Vec<Message> {
        self.conn.send_ping().into_iter().map(|o| fmt_outcome(&o)).collect()
    }

    /// Convert [`ConnState`] into the framework's generic [`TaskInternalState`].
    fn to_internal(&self) -> TaskInternalState {
        let (label, active) = match &self.conn.conn {
            crate::backend::conn::ConnState::Disconnected => ("off".to_string(), false),
            crate::backend::conn::ConnState::Connecting { protocol, .. } => {
                return TaskInternalState {
                    active: true,
                    badge: Some(format!("{}: ⌛", protocol.as_str())),
                    ..Default::default()
                };
            }
            crate::backend::conn::ConnState::Connected { addr, .. } => {
                (addr.clone(), true)
            }
            crate::backend::conn::ConnState::Error(e) => {
                (e.chars().take(40).collect(), false)
            }
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        if let Some(ref v) = self.conn.latest_recv {
            crate::ui::state_panel::flatten_json("", v, &mut fields);
        }
        TaskInternalState {
            active,
            badge: Some(format!("net: {label}")),
            fields,
        }
    }
}

impl TaskActor for ConnTask {
    fn commands(&self) -> Vec<CommandDef> {
        let mut v = base_commands();
        v.push(CommandDef { name: "con", desc: "connect transport", subs: CON_SUBS });
        v.push(cmd("close", "disconnect"));
        v.push(cmd("send", "send message"));
        v
    }

    fn handle_own(&mut self, cmd: &str, sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "con" => self.do_connect(sub),
            "close" => self.do_disconnect(),
            "send" => self.do_send(),
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn take_transport_rx(&mut self) -> tokio::sync::mpsc::Receiver<crate::transport::TransportEvent> {
        self.conn.take_rx()
    }

    fn tick(&mut self) -> Vec<Message> {
        // Transport events now arrive via on_transport → select branch,
        // so tick() only handles non-I/O periodic work. Currently none.
        vec![]
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.to_arc(),
            evicted_lines: self.chat.messages.evicted_lines(),
            buffer_total_lines: self.chat.messages.total_lines(),
            internal: self.to_internal(),
        }
    }

    fn on_transport(&mut self, ev: TransportEvent) -> Vec<Message> {
        self.conn.handle_event(ev).into_iter().map(|o| fmt_outcome(&o)).collect()
    }

    fn chat(&self) -> &ChatState { &self.chat }
    fn chat_mut(&mut self) -> &mut ChatState { &mut self.chat }
}

fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}

fn fmt_outcome(o: &conn::ConnOutcome) -> Message {
    let (text, level) = conn::format(o);
    Message::System { text, level }
}

use super::TaskRuntime;

/// Callback: construct and spawn this actor. Called by the registry dispatch.
pub fn create(def: &'static TaskDef) -> TaskRuntime {
    let actor = ConnTask::new(def);
    let cmds = std::sync::Arc::new(actor.commands());
    let handle = super::spawn_actor(actor);
    TaskRuntime { handle, commands: cmds }
}
