pub mod tcp;

pub use crate::json_framer::JsonFramer;
use tokio::sync::mpsc;

pub const DEFAULT_TCP_ADDR: &str = "127.0.0.1:7878";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Tcp => "tcp",
        }
    }

    pub fn default_addr(&self) -> &'static str {
        match self {
            Protocol::Tcp => DEFAULT_TCP_ADDR,
        }
    }

    pub fn from_name(name: &str) -> Option<Protocol> {
        match name {
            "tcp" => Some(Protocol::Tcp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TransportEvent {
    Connecting { protocol: Protocol, addr: String },
    Connected { protocol: Protocol, addr: String },
    Recv(String),
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
    }
}
