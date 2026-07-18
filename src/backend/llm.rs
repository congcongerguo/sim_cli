use tokio::sync::mpsc;

#[cfg(feature = "mock-llm")]
use crate::commands::DemoScenario;
use crate::event::LlmEvent;
use crate::message::Message;
#[cfg(feature = "mock-llm")]
use crate::mock_llm::{self, Scenario};

use super::chat::ChatState;
use super::modal::ModalSubsystem;

const CHANNEL_BUFFER: usize = 64;

/// LLM streaming + scripted demos. Mutates `ChatState` directly because the
/// token stream is inherently message-shaped — there is no useful intermediate
/// representation.
pub struct LlmSubsystem {
    pub streaming: bool,
    #[allow(dead_code)]
    tx: mpsc::Sender<LlmEvent>,
    pub rx: mpsc::Receiver<LlmEvent>,
}

impl LlmSubsystem {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(CHANNEL_BUFFER);
        Self { streaming: false, tx, rx }
    }

    #[cfg(feature = "mock-llm")]
    pub fn start_demo(&mut self, scenario: DemoScenario, chat: &mut ChatState) {
        chat.messages.push(Message::Assistant {
            text: String::new(),
            streaming: true,
        });
        self.streaming = true;
        mock_llm::spawn(map_scenario(scenario), self.tx.clone());
    }

    pub fn handle_event(
        &mut self,
        ev: LlmEvent,
        chat: &mut ChatState,
        modal: &mut ModalSubsystem,
    ) {
        match ev {
            LlmEvent::Token(t) => {
                if let Some(Message::Assistant { text, .. }) = chat.messages.last_mut() {
                    text.push_str(&t);
                }
            }
            LlmEvent::StartTool { tool_name, args_preview } => {
                modal.start_tool(tool_name, args_preview, chat);
            }
            LlmEvent::ToolDone { output } => {
                modal.finish_tool(output, chat);
            }
            LlmEvent::Done => {
                if let Some(Message::Assistant { streaming, .. }) = chat.messages.last_mut() {
                    *streaming = false;
                }
                self.streaming = false;
            }
        }
    }
}

#[cfg(feature = "mock-llm")]
fn map_scenario(d: DemoScenario) -> Scenario {
    match d {
        DemoScenario::Chat => Scenario::Chat,
        DemoScenario::Code => Scenario::Code,
        DemoScenario::Tool => Scenario::Tool,
    }
}
