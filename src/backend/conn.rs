use chrono::{DateTime, Local};
use tokio::sync::mpsc;

use crate::transport::{self, Protocol, TransportEvent, TransportHandle};

const CHANNEL_BUFFER: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Disconnected,
    Connecting { protocol: Protocol, addr: String },
    Connected { protocol: Protocol, addr: String },
    Error(String),
}

/// Things the conn subsystem can emit. The orchestrator turns these into
/// user-visible system messages — never the subsystem itself.
#[derive(Debug)]
pub enum ConnOutcome {
    Connecting { protocol: Protocol, addr: String },
    AlreadyActive { state: ConnState },
    Connected { protocol: Protocol, addr: String },
    Disconnected,
    PeerClosed,
    NotConnected,
    Sent { line: String },
    SendFailed(String),
    RecvJson {
        n: u64,
        bytes: usize,
    },
    RecvInvalid { raw: String, error: String },
    Error(String),
}

pub struct ConnSubsystem {
    pub conn: ConnState,
    handle: Option<TransportHandle>,
    send_counter: u64,
    recv_counter: u64,
    pub latest_recv: Option<serde_json::Value>,
    pub latest_recv_at: Option<DateTime<Local>>,
    ev_tx: mpsc::Sender<TransportEvent>,
    pub ev_rx: mpsc::Receiver<TransportEvent>,
}

impl ConnSubsystem {
    pub fn new() -> Self {
        let (ev_tx, ev_rx) = mpsc::channel(CHANNEL_BUFFER);
        Self {
            conn: ConnState::Disconnected,
            handle: None,
            send_counter: 0,
            recv_counter: 0,
            latest_recv: None,
            latest_recv_at: None,
            ev_tx,
            ev_rx,
        }
    }

    pub fn connect(&mut self, protocol: Protocol) -> Vec<ConnOutcome> {
        if matches!(
            self.conn,
            ConnState::Connecting { .. } | ConnState::Connected { .. }
        ) {
            return vec![ConnOutcome::AlreadyActive {
                state: self.conn.clone(),
            }];
        }
        let addr = protocol.default_addr().to_string();
        self.conn = ConnState::Connecting {
            protocol,
            addr: addr.clone(),
        };
        self.handle = Some(transport::spawn(protocol, addr.clone(), self.ev_tx.clone()));
        vec![ConnOutcome::Connecting { protocol, addr }]
    }

    pub fn disconnect(&mut self) -> Vec<ConnOutcome> {
        if self.handle.take().is_none() {
            return vec![ConnOutcome::NotConnected];
        }
        self.conn = ConnState::Disconnected;
        vec![ConnOutcome::Disconnected]
    }

    pub fn send_ping(&mut self) -> Vec<ConnOutcome> {
        let handle = match self.handle.as_ref() {
            Some(h) => h,
            None => return vec![ConnOutcome::NotConnected],
        };
        self.send_counter += 1;
        let id = self.send_counter;
        let payload = serde_json::json!({
            "id": id,
            "msg": format!("ping {id}"),
        });
        let line = payload.to_string();
        let mut outs = vec![ConnOutcome::Sent { line: line.clone() }];
        if let Err(e) = handle.out_tx.try_send(line) {
            outs.push(ConnOutcome::SendFailed(e.to_string()));
        }
        outs
    }

    pub fn handle_event(&mut self, ev: TransportEvent) -> Vec<ConnOutcome> {
        match ev {
            TransportEvent::Connecting { protocol, addr } => {
                self.conn = ConnState::Connecting {
                    protocol,
                    addr: addr.clone(),
                };
                Vec::new()
            }
            TransportEvent::Connected { protocol, addr } => {
                self.conn = ConnState::Connected {
                    protocol,
                    addr: addr.clone(),
                };
                vec![ConnOutcome::Connected { protocol, addr }]
            }
            TransportEvent::Recv(line) => {
                let bytes = line.len();
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => {
                        self.recv_counter += 1;
                        let n = self.recv_counter;
                        self.latest_recv = Some(v);
                        self.latest_recv_at = Some(Local::now());
                        vec![ConnOutcome::RecvJson { n, bytes }]
                    }
                    Err(e) => vec![ConnOutcome::RecvInvalid {
                        raw: line,
                        error: e.to_string(),
                    }],
                }
            }
            TransportEvent::Disconnected => {
                self.conn = ConnState::Disconnected;
                self.handle = None;
                vec![ConnOutcome::PeerClosed]
            }
            TransportEvent::Error(e) => {
                self.conn = ConnState::Error(e.clone());
                self.handle = None;
                vec![ConnOutcome::Error(e)]
            }
        }
    }
}

/// Format a [`ConnOutcome`] into the user-visible system-message string. All
/// transport-related copy lives in this one function.
pub fn format(outcome: &ConnOutcome) -> String {
    match outcome {
        ConnOutcome::Connecting { protocol, addr } => {
            format!("connecting [{}] to {addr}...", protocol.as_str())
        }
        ConnOutcome::AlreadyActive { state } => {
            format!("already connected/connecting (state: {state:?})")
        }
        ConnOutcome::Connected { protocol, addr } => {
            format!("[{}] connected: {addr}", protocol.as_str())
        }
        ConnOutcome::Disconnected => "disconnected".into(),
        ConnOutcome::PeerClosed => "peer closed connection".into(),
        ConnOutcome::NotConnected => "not connected — run 'con <protocol>' first".into(),
        ConnOutcome::Sent { line } => format!("→ send: {line}"),
        ConnOutcome::SendFailed(e) => format!("send failed: {e}"),
        ConnOutcome::RecvJson { n, bytes } => format!("← recv #{n} ({bytes} bytes)"),
        ConnOutcome::RecvInvalid { raw, error } => {
            format!("← recv (invalid JSON: {error})\nraw: {raw}")
        }
        ConnOutcome::Error(e) => format!("transport error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_when_idle_reports_not_connected() {
        let mut c = ConnSubsystem::new();
        let outs = c.disconnect();
        assert!(matches!(outs.as_slice(), [ConnOutcome::NotConnected]));
    }

    #[test]
    fn send_without_connection_reports_not_connected() {
        let mut c = ConnSubsystem::new();
        let outs = c.send_ping();
        assert!(matches!(outs.as_slice(), [ConnOutcome::NotConnected]));
    }

    #[test]
    fn handle_recv_with_valid_json_stores_value() {
        let mut c = ConnSubsystem::new();
        let outs = c.handle_event(TransportEvent::Recv("{\"a\":1}".into()));
        assert!(matches!(outs.as_slice(), [ConnOutcome::RecvJson { n: 1, .. }]));
        assert!(c.latest_recv.is_some());
        assert!(c.latest_recv_at.is_some());
    }

    #[test]
    fn handle_recv_with_garbage_keeps_previous_value() {
        let mut c = ConnSubsystem::new();
        c.handle_event(TransportEvent::Recv("{\"a\":1}".into()));
        let snapshot = c.latest_recv.clone();
        let outs = c.handle_event(TransportEvent::Recv("not json".into()));
        assert!(matches!(outs.as_slice(), [ConnOutcome::RecvInvalid { .. }]));
        assert_eq!(c.latest_recv, snapshot, "garbage frame must not clobber");
    }
}
