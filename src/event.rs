#[derive(Debug, Clone)]
pub enum LlmEvent {
    #[allow(dead_code)]
    Token(String),
    #[allow(dead_code)]
    StartTool {
        tool_name: String,
        args_preview: String,
    },
    #[allow(dead_code)]
    ToolDone {
        output: String,
    },
    #[allow(dead_code)]
    Done,
}
