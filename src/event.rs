#[derive(Debug, Clone)]
pub enum LlmEvent {
    Token(String),
    StartTool {
        tool_name: String,
        args_preview: String,
    },
    ToolDone {
        output: String,
    },
    Done,
}
