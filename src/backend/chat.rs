use crate::message::{LogLevel, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Plan,
}

/// Conversation log.
pub struct ChatState {
    pub(crate) messages: Vec<Message>,
    pub(crate) model: String,
    #[allow(dead_code)]
    pub(crate) mode: Mode,
}

impl ChatState {
    pub fn new(model: String) -> Self {
        Self { messages: Vec::new(), model, mode: Mode::Normal }
    }

    pub fn push_message(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn push_system(&mut self, text: impl Into<String>, level: LogLevel) {
        self.messages.push(Message::System { text: text.into(), level });
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.push_system("conversation cleared", LogLevel::Notice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_leaves_only_cleared_notice() {
        let mut c = ChatState::new("claude".into());
        c.push_system("noise", LogLevel::Info);
        c.clear();
        assert_eq!(c.messages.len(), 1);
        match &c.messages[0] {
            Message::System { text, level } => {
                assert_eq!(text, "conversation cleared");
                assert_eq!(*level, LogLevel::Notice);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
