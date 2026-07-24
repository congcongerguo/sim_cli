use crate::message::{LogLevel, Message};
use crate::transport::{Protocol, TransportEvent};

use crate::backend::conn::{self, ConnSubsystem};
use super::{cmd, msg, Cmd, Sub, Tool, ToolState};

#[cfg(feature = "zmq")]
const CON_SUBS: &[Sub] = &[
    Sub { name: "tcp", desc: "TCP echo" },
    Sub { name: "zmq", desc: "ZMQ pub/sub" },
];
#[cfg(not(feature = "zmq"))]
const CON_SUBS: &[Sub] = &[
    Sub { name: "tcp", desc: "TCP echo" },
];

pub struct ConnTool {
    conn: ConnSubsystem,
    def: &'static super::registry::ToolDef,
}

impl ConnTool {
    pub fn new(def: &'static super::registry::ToolDef) -> Self {
        Self { conn: ConnSubsystem::new(), def }
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
}

impl Tool for ConnTool {
    fn commands(&self) -> Vec<Cmd> {
        vec![
            Cmd { name: "con", desc: "connect transport", subs: CON_SUBS },
            cmd("close", "disconnect"),
            cmd("send", "send message"),
        ]
    }

    fn handle(&mut self, cmd: &str, args: &[&str]) -> Vec<Message> {
        match cmd {
            "con" => self.do_connect(args.first().copied()),
            "close" => self.do_disconnect(),
            "send" => self.do_send(),
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn snapshot(&self) -> ToolState {
        let (label, active) = match &self.conn.conn {
            conn::ConnState::Disconnected => ("off".to_string(), false),
            conn::ConnState::Connecting { protocol, .. } => {
                return ToolState {
                    active: true,
                    badge: Some(format!("{}: ...", protocol.as_str())),
                    ..Default::default()
                };
            }
            conn::ConnState::Connected { addr, .. } => (addr.clone(), true),
            conn::ConnState::Error(e) => (e.chars().take(40).collect(), false),
        };
        let mut fields: Vec<(String, String)> = Vec::new();
        if let Some(ref v) = self.conn.latest_recv {
            crate::ui::state_panel::flatten_json("", v, &mut fields);
        }
        ToolState {
            active,
            badge: Some(format!("net: {label}")),
            fields,
        }
    }

    fn take_transport_rx(&mut self) -> tokio::sync::mpsc::Receiver<TransportEvent> {
        self.conn.take_rx()
    }

    fn tick(&mut self) -> Vec<Message> {
        vec![]
    }

    fn on_transport(&mut self, ev: TransportEvent) -> Vec<Message> {
        self.conn.handle_event(ev).into_iter().map(|o| fmt_outcome(&o)).collect()
    }
}

fn fmt_outcome(o: &conn::ConnOutcome) -> Message {
    let (text, level) = conn::format(o);
    Message::System { text, level }
}
