#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    AwaitingPermission,
    Running,
    Done,
    Denied,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub args_preview: String,
    pub status: ToolStatus,
    pub output: String,
}

/// Log level for system messages. Controls display colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Notice,
    Info,
    #[cfg_attr(not(feature = "demo-task"), allow(dead_code))]
    Debug,
}

#[derive(Debug, Clone)]
pub enum Message {
    #[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
    Assistant { text: String, streaming: bool },
    #[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
    Tool(ToolCall),
    System {
        text: String,
        level: LogLevel,
    },
}

/// 本地时区时间戳(毫秒精度用于展示与落盘)。
pub type Timestamp = chrono::DateTime<chrono::Local>;

/// 带时间戳的消息。时间戳在消息进入 [`LogBuffer`](crate::log_buffer::LogBuffer)
/// 时赋予,既用于界面展示,也写入消息日志文件。
#[derive(Debug, Clone)]
pub struct TimedMessage {
    pub time: Timestamp,
    pub msg: Message,
}
