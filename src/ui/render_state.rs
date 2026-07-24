//! Pure-data snapshot passed from frontend to renderer.
//! No rendering library types — just data.

use std::sync::Arc;

use crate::backend::ModalRequest;
use crate::frontend::InputState;
use crate::message::TimedMessage;
use crate::tool::{ToolInfo, ToolState};

/// Everything the renderer needs to draw one frame.
pub struct RenderState {
    // ── From ViewState ──
    pub messages: Arc<Vec<TimedMessage>>,
    pub streaming: bool,
    pub state: ToolState,
    pub tools: Arc<Vec<ToolInfo>>,
    pub active_index: usize,

    // ── Frontend interaction state ──
    pub input_text: String,
    #[allow(dead_code)]
    pub input_cursor: (u16, u16),
    pub input_state: InputState,
    pub menu_items: Vec<(String, String)>,
    pub menu_idx: usize,
    pub menu_title: Option<String>,
    pub scroll_offset: u64,
    pub follow_tail: bool,
    pub unseen_lines: u32,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
    pub panel_visible: bool,
    pub modal_request: Option<ModalRequest>,
    pub modal_selected: usize,
    /// Active include-filter expression (show only matches), if any.
    pub filter: Option<String>,
    /// Message from the last rejected `filter` expression, if any.
    pub filter_error: Option<String>,
    /// Active exclude-filter expression (hide matches), if any.
    pub exclude: Option<String>,
    /// Message from the last rejected `exclude` expression, if any.
    pub exclude_error: Option<String>,
    /// (shown, total) message counts when any view filter is active — lets the
    /// status line show how many messages are currently visible.
    pub filter_counts: Option<(usize, usize)>,
}

/// Values the renderer computes and returns to the frontend.
pub struct RenderOutput {
    pub viewport_height: u16,
    pub total_lines: u16,
}
