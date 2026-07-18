//! Renderer trait — swap implementations to change TUI backend.

use crate::ui::render_state::{RenderOutput, RenderState};

#[allow(dead_code)]
pub trait Renderer {
    fn render(&mut self, state: &RenderState) -> RenderOutput;
}
