use std::collections::HashSet;

use crate::message::{Message, ToolCall, ToolStatus};

use super::chat::ChatState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalChoice {
    Yes,
    No,
    Always,
}

#[derive(Debug, Clone)]
pub struct ModalRequest {
    #[allow(dead_code)]
    pub tool_index: usize,
    pub tool_name: String,
    pub args_preview: String,
}

/// Permission-modal lifecycle. Unused without mock-llm.
#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
pub struct ModalSubsystem {
    pub request: Option<ModalRequest>,
    #[allow(dead_code)]
    allow_always: HashSet<String>,
}

#[cfg_attr(not(feature = "mock-llm"), allow(dead_code))]
impl ModalSubsystem {
    pub fn new() -> Self {
        Self {
            request: None,
            allow_always: HashSet::new(),
        }
    }

    /// LLM is asking permission to run a tool. Pushes a tool card into chat
    /// and, unless this tool is on the always-allow list, opens the modal.
    pub fn start_tool(
        &mut self,
        tool_name: String,
        args_preview: String,
        chat: &mut ChatState,
    ) {
        let auto = self.allow_always.contains(&tool_name);
        let status = if auto {
            ToolStatus::Running
        } else {
            ToolStatus::AwaitingPermission
        };
        chat.messages.push(Message::Tool(ToolCall {
            name: tool_name.clone(),
            args_preview: args_preview.clone(),
            status,
            output: String::new(),
        }));
        if !auto {
            let idx = chat.messages.len() - 1;
            self.request = Some(ModalRequest {
                tool_index: idx,
                tool_name,
                args_preview,
            });
        }
    }

    pub fn finish_tool(&mut self, output: String, chat: &mut ChatState) {
        if let Some(Message::Tool(t)) = chat.messages.last_mut()
            && t.status != ToolStatus::Denied
        {
            t.status = ToolStatus::Done;
            t.output = output;
        }
        chat.messages.push(Message::Assistant {
            text: String::new(),
            streaming: true,
        });
    }

    pub fn resolve(&mut self, choice: ModalChoice, chat: &mut ChatState) {
        let req = match self.request.take() {
            Some(r) => r,
            None => return,
        };
        if let Some(Message::Tool(t)) = chat.messages.get_mut(req.tool_index) {
            match choice {
                ModalChoice::Yes => t.status = ToolStatus::Running,
                ModalChoice::Always => {
                    t.status = ToolStatus::Running;
                    self.allow_always.insert(t.name.clone());
                }
                ModalChoice::No => {
                    t.status = ToolStatus::Denied;
                    t.output = "(permission denied)".into();
                }
            }
        }
    }
}
