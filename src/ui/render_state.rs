//! Pure-data snapshot passed from frontend to renderer.
//! No rendering library types — just data.

use std::sync::Arc;

use crate::backend::{ConnState, ModalRequest, Mode, TaskInfo};
use crate::frontend::InputState;
use crate::message::Message;

/// Everything the renderer needs to draw one frame.
pub struct RenderState {
    // ── From ViewState ──
    pub messages: Arc<Vec<Message>>,
    pub model: String,
    pub mode: Mode,
    pub streaming: bool,
    pub conn: ConnState,
    pub tasks: Arc<Vec<TaskInfo>>,
    pub active_task_index: usize,
    pub active_task: String,
    pub latest_recv: Option<serde_json::Value>,
    pub latest_recv_at: Option<chrono::DateTime<chrono::Local>>,

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
    #[allow(dead_code)]
    pub prev_total_lines: u32,
    pub unseen_lines: u32,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
    pub panel_visible: bool,
    pub modal_request: Option<ModalRequest>,
    pub modal_selected: usize,
}

/// Values the renderer computes and returns to the frontend.
pub struct RenderOutput {
    pub viewport_height: u16,
    pub total_lines: u16,
}
