use crate::commands::{ModelChoice, PlanToggle};
use crate::help::WELCOME;
use crate::message::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Plan,
}

/// Conversation log + model/mode selection. The orchestrator funnels all
/// user-visible text into this struct via `push_system`.
pub struct ChatState {
    pub messages: Vec<Message>,
    pub model: String,
    pub mode: Mode,
}

impl ChatState {
    pub fn new(model: String) -> Self {
        Self {
            messages: vec![Message::System(WELCOME.into())],
            model,
            mode: Mode::Normal,
        }
    }

    pub fn push_system(&mut self, text: impl Into<String>) {
        self.messages.push(Message::System(text.into()));
    }

    pub fn clear(&mut self) {
        self.messages.clear();
        self.push_system("conversation cleared");
    }

    pub fn set_model(&mut self, choice: ModelChoice) {
        self.model = format!("mock-{}", choice.slug());
    }

    pub fn set_plan(&mut self, toggle: PlanToggle) {
        self.mode = match toggle {
            PlanToggle::On => Mode::Plan,
            PlanToggle::Off => Mode::Normal,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_model_updates_model_string() {
        let mut c = ChatState::new("mock-claude".into());
        c.set_model(ModelChoice::Haiku);
        assert_eq!(c.model, "mock-haiku");
    }

    #[test]
    fn clear_leaves_only_cleared_notice() {
        let mut c = ChatState::new("mock-claude".into());
        c.push_system("noise");
        c.clear();
        assert_eq!(c.messages.len(), 1);
        match &c.messages[0] {
            Message::System(s) => assert_eq!(s, "conversation cleared"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
