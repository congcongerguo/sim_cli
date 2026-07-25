use chrono::{DateTime, Local};
use tokio::sync::mpsc;

use crate::message::LogLevel;
use crate::transport::{self, Protocol, TransportEvent, TransportHandle};

/// 事件通道缓冲区大小。
const CHANNEL_BUFFER: usize = 64;

/// 连接状态机。
///
/// 连接的生命周期：Disconnected → Connecting → Connected → Disconnected → ...
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// 未连接
    Disconnected,
    /// 正在连接中（携带协议和地址信息）
    Connecting { protocol: Protocol, addr: String },
    /// 已连接（携带协议和地址信息）
    Connected { protocol: Protocol, addr: String },
    /// 连接出错（携带错误描述）
    Error(String),
}

/// 连接子系统对外产出的事件。
///
/// 注意：这里只产出**原始结果**，不直接写入用户界面。
/// 上层编排器负责将 [`ConnOutcome`] 转换为用户可见的系统消息。
#[derive(Debug)]
pub enum ConnOutcome {
    /// 正在发起连接
    Connecting { protocol: Protocol, addr: String },
    /// 已有活跃连接，重复连接被拒绝
    AlreadyActive { state: ConnState },
    /// 连接成功建立
    Connected { protocol: Protocol, addr: String },
    /// 主动断开连接
    Disconnected,
    /// 对端关闭连接
    PeerClosed,
    /// 未连接时尝试发送/断开
    NotConnected,
    /// 消息已发送
    Sent { line: String },
    /// 发送失败
    SendFailed(String),
    /// 收到合法的 JSON 消息
    RecvJson {
        /// 接收序号（从 1 开始递增）
        n: u64,
        /// 原始字节数
        bytes: usize,
        /// 编码方式（如 "json"）
        encoding: String,
    },
    /// 收到非法 JSON（保留原始文本和错误信息）
    RecvInvalid { raw: String, error: String },
    /// 传输层错误
    Error(String),
}

/// 连接子系统：管理传输层连接、收发消息、维护连接状态。
///
/// ## 职责
/// - 连接生命周期管理（connect / disconnect）
/// - 消息收发（send_ping / 接收 JSON）
/// - 维护收发计数器和最近接收的数据
/// - 生成传输事件通道供外部 select 使用
pub struct ConnSubsystem {
    /// 当前连接状态
    pub(crate) conn: ConnState,
    /// 当前传输层句柄（None 表示未连接）
    handle: Option<TransportHandle>,
    /// 发送消息计数器
    send_counter: u64,
    /// 接收消息计数器
    recv_counter: u64,
    /// 最近收到的 JSON 值
    pub(crate) latest_recv: Option<serde_json::Value>,
    /// 最近收到消息的时间
    pub(crate) latest_recv_at: Option<DateTime<Local>>,
    /// 传输事件发送端（用于向 channel 写入事件）
    ev_tx: mpsc::Sender<TransportEvent>,
    /// 传输事件接收端（供外部 select 循环使用）
    pub(crate) ev_rx: mpsc::Receiver<TransportEvent>,
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

    /// 取出传输事件接收端，供外部 select 循环使用。
    ///
    /// 取出后用 dummy channel 替换内部字段，使后续 `tick()` 轮询无害
    ///（但更推荐的做法是让框架直接 select 此 channel）。
    pub fn take_rx(&mut self) -> mpsc::Receiver<TransportEvent> {
        let (_, dummy) = mpsc::channel(1);
        std::mem::replace(&mut self.ev_rx, dummy)
    }

    /// 返回发送端的克隆，用于在新建传输任务时传递。
    #[allow(dead_code)]
    pub fn sender(&self) -> mpsc::Sender<TransportEvent> {
        self.ev_tx.clone()
    }

    /// 发起连接。
    ///
    /// 如果当前已经在连接中或已连接，返回 [`ConnOutcome::AlreadyActive`]，
    /// 不会重复连接。
    pub fn connect(&mut self, protocol: Protocol, addr: &str) -> Vec<ConnOutcome> {
        if matches!(
            self.conn,
            ConnState::Connecting { .. } | ConnState::Connected { .. }
        ) {
            return vec![ConnOutcome::AlreadyActive {
                state: self.conn.clone(),
            }];
        }
        let addr = addr.to_string();
        self.conn = ConnState::Connecting {
            protocol,
            addr: addr.clone(),
        };
        // 启动传输层任务，事件通过 ev_tx 发回
        self.handle = Some(transport::spawn(protocol, addr.clone(), self.ev_tx.clone()));
        vec![ConnOutcome::Connecting { protocol, addr }]
    }

    /// 断开连接。
    ///
    /// 丢弃传输句柄（触发 Drop → 关闭连接），状态回到 Disconnected。
    pub fn disconnect(&mut self) -> Vec<ConnOutcome> {
        if self.handle.take().is_none() {
            return vec![ConnOutcome::NotConnected];
        }
        self.conn = ConnState::Disconnected;
        vec![ConnOutcome::Disconnected]
    }

    /// 发送一条 ping 消息（JSON 格式）。
    ///
    /// 消息格式：`{"id": N, "msg": "ping N"}`
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
        if let Err(e) = handle.out_tx.try_send(line.clone()) {
            vec![ConnOutcome::SendFailed(e.to_string())]
        } else {
            vec![ConnOutcome::Sent { line }]
        }
    }

    /// 处理传输层事件。
    ///
    /// 根据事件类型更新连接状态、计数器、最近接收值。
    /// 返回对应的 [`ConnOutcome`] 列表供上层消费。
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
            TransportEvent::Recv { encoding, text } => {
                let bytes = text.len();
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) => {
                        self.recv_counter += 1;
                        let n = self.recv_counter;
                        self.latest_recv = Some(v);
                        self.latest_recv_at = Some(Local::now());
                        vec![ConnOutcome::RecvJson { n, bytes, encoding }]
                    }
                    Err(e) => vec![ConnOutcome::RecvInvalid {
                        raw: text,
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

/// 将 [`ConnOutcome`] 格式化为面向用户的 `(消息文本, 日志级别)` 元组。
///
/// 不同的事件对应不同的级别：
/// - Info: 连接中、发送成功、收到消息
/// - Notice: 连接成功、主动断开
/// - Warn: 重复连接、对端关闭、非法 JSON、未连接时的操作
/// - Error: 发送失败、传输错误
pub fn format(outcome: &ConnOutcome) -> (String, LogLevel) {
    use LogLevel::*;
    match outcome {
        ConnOutcome::Connecting { protocol, addr } => {
            (format!("connecting [{}] to {addr}...", protocol.as_str()), Info)
        }
        ConnOutcome::AlreadyActive { state } => {
            (format!("already connected/connecting (state: {state:?})"), Warn)
        }
        ConnOutcome::Connected { protocol, addr } => {
            (format!("[{}] connected: {addr}", protocol.as_str()), Notice)
        }
        ConnOutcome::Disconnected => ("disconnected".into(), Notice),
        ConnOutcome::PeerClosed => ("peer closed connection".into(), Warn),
        ConnOutcome::NotConnected => {
            ("not connected - run 'con <protocol>' first".into(), Warn)
        }
        ConnOutcome::Sent { line } => (format!("-> send: {line}"), Info),
        ConnOutcome::SendFailed(e) => (format!("send failed: {e}"), Error),
        ConnOutcome::RecvJson { n, bytes, encoding } => {
            (format!("<- recv #{n} ({encoding}, {bytes} bytes)"), Info)
        }
        ConnOutcome::RecvInvalid { raw, error } => {
            (format!("<- recv (invalid JSON: {error})\nraw: {raw}"), Warn)
        }
        ConnOutcome::Error(e) => (format!("transport error: {e}"), Error),
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
        let outs = c.handle_event(TransportEvent::Recv { encoding: "json".into(), text: "{\"a\":1}".into() });
        assert!(matches!(outs.as_slice(), [ConnOutcome::RecvJson { n: 1, .. }]));
        assert!(c.latest_recv.is_some());
        assert!(c.latest_recv_at.is_some());
    }

    #[test]
    fn handle_recv_with_garbage_keeps_previous_value() {
        let mut c = ConnSubsystem::new();
        c.handle_event(TransportEvent::Recv { encoding: "json".into(), text: "{\"a\":1}".into() });
        let snapshot = c.latest_recv.clone();
        let outs = c.handle_event(TransportEvent::Recv {
            encoding: "json".into(),
            text: "not json".into(),
        });
        assert!(matches!(outs.as_slice(), [ConnOutcome::RecvInvalid { .. }]));
        assert_eq!(c.latest_recv, snapshot, "garbage frame must not clobber");
    }
}
