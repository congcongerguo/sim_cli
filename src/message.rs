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

#[derive(Debug, Clone)]
pub enum Message {
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool(ToolCall),
    System(String),
}
