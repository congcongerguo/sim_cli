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
    Debug,
}

#[derive(Debug, Clone)]
pub enum Message {
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool(ToolCall),
    System {
        text: String,
        level: LogLevel,
    },
}
