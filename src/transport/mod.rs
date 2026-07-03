pub mod tcp;
pub mod zmq;

pub use crate::json_framer::JsonFramer;
use tokio::sync::mpsc;

pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:7878";
pub const DEFAULT_ZMQ_ADDR: &str = "tcp://127.0.0.1:5555";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Zmq,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Zmq => "zmq",
        }
    }

    pub fn default_addr(&self) -> &'static str {
        match self {
            Protocol::Tcp => DEFAULT_TCP_ADDR,
            Protocol::Zmq => DEFAULT_ZMQ_ADDR,
        }
    }

    pub fn from_name(name: &str) -> Option<Protocol> {
        match name {
            "tcp" => Some(Protocol::Tcp),
            "zmq" => Some(Protocol::Zmq),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TransportEvent {
    Connecting { protocol: Protocol, addr: String },
    Connected { protocol: Protocol, addr: String },
    /// `encoding` is the wire encoding label — for ZMQ it is the pub/sub topic
    /// ("json" or "pb"); for TCP it is always "json". `text` is the payload
    /// normalized to a JSON string at the transport boundary (proto messages
    /// are decoded to JSON here so the rest of the app stays encoding-agnostic).
    Recv { encoding: String, text: String },
    Disconnected,
    Error(String),
}

pub struct TransportHandle {
    #[allow(dead_code)]
    pub protocol: Protocol,
    pub out_tx: mpsc::Sender<String>,
}

pub fn spawn(
    protocol: Protocol,
    addr: String,
    ev_tx: mpsc::Sender<TransportEvent>,
) -> TransportHandle {
    match protocol {
        Protocol::Tcp => tcp::spawn(addr, ev_tx),
        Protocol::Zmq => zmq::spawn(addr, ev_tx),
    }
}
