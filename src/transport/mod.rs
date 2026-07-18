pub mod tcp;
#[cfg(feature = "zmq")]
pub mod zmq;

pub use crate::json_framer::JsonFramer;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    #[cfg(feature = "zmq")]
    Zmq,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
            #[cfg(feature = "zmq")]
            Protocol::Zmq => "zmq",
        }
    }

    #[allow(dead_code)]
    pub fn from_name(name: &str) -> Option<Protocol> {
        match name {
            "tcp" => Some(Protocol::Tcp),
            #[cfg(feature = "zmq")]
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
        #[cfg(feature = "zmq")]
        Protocol::Zmq => zmq::spawn(addr, ev_tx),
    }
}
