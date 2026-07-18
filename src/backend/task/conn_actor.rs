use crate::message::{LogLevel, Message};
use crate::transport::{Protocol, TransportEvent};

use super::super::chat::ChatState;
use super::super::conn::{self, ConnSubsystem};
use super::registry::TaskDef;
use super::{cmd, CommandDef, SubDef, TaskActor, TaskSnapshot};

pub struct ConnTask {
    chat: ChatState,
    conn: ConnSubsystem,
    def: &'static TaskDef,
}

impl ConnTask {
    pub fn new(model: String, def: &'static TaskDef) -> Self {
        Self { chat: ChatState::new(model), conn: ConnSubsystem::new(), def }
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

impl TaskActor for ConnTask {
    fn commands(&self) -> Vec<CommandDef> {
        let mut subs = vec![SubDef { name: "tcp", desc: "TCP echo" }];
        #[cfg(feature = "zmq")]
        subs.push(SubDef { name: "zmq", desc: "ZMQ pub/sub" });

        vec![
            cmd("help", "show commands"),
            cmd("clear", "clear log"),
            cmd("exit", "quit"),
            CommandDef { name: "con", desc: "connect transport", subs: subs.leak() },
            cmd("close", "disconnect"),
            cmd("send", "send message"),
        ]
    }

    fn handle_own(&mut self, cmd: &str, sub: Option<&str>, _args: &[&str]) -> Vec<Message> {
        match cmd {
            "con" => self.do_connect(sub),
            "close" => self.do_disconnect(),
            "send" => self.do_send(),
            _ => vec![msg("unknown command", LogLevel::Error)],
        }
    }

    fn tick(&mut self) -> Vec<Message> {
        let mut msgs = vec![];
        while let Ok(ev) = self.conn.ev_rx.try_recv() {
            msgs.extend(self.conn.handle_event(ev).into_iter().map(|o| fmt_outcome(&o)));
        }
        msgs
    }

    fn snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            name: self.def.name.into(),
            messages: self.chat.messages.clone(),
            model: self.chat.model.clone(),
            conn: self.conn.conn.clone(),
            demo_running: false,
            latest_recv: self.conn.latest_recv.clone(),
            latest_recv_at: self.conn.latest_recv_at,
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
