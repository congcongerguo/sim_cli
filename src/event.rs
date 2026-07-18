//! LLM streaming events. Only used with the `mock-llm` feature.

#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
#[derive(Debug, Clone)]
pub enum LlmEvent {
    Token(String),
    StartTool { tool_name: String, args_preview: String },
    ToolDone { output: String },
    Done,
}
