//! Ratatui implementation of [`Renderer`].

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::ui::conversation;
use crate::ui::input;
use crate::ui::modal;
use crate::ui::render_state::{RenderOutput, RenderState};
use crate::ui::renderer::Renderer;
use crate::ui::state_panel;
use crate::ui::statusline;
use crate::ui::tab_bar;

const STATE_PANEL_WIDTH: u16 = 36;
const MIN_CONVERSATION_WIDTH: u16 = 40;
const PANEL_MIN_TERM_WIDTH: u16 = 80;

pub struct RatatuiRenderer;

impl Renderer for RatatuiRenderer {
    fn render(&mut self, _state: &RenderState) -> RenderOutput {
        // We need a terminal to render into. This renderer is called from
        // frontend's run() which holds the Terminal<CrosstermBackend>.
        // The actual terminal draw happens in frontend::run() — see below.
        unreachable!("use RatatuiRenderer::draw() directly from the frontend loop")
    }
}

impl RatatuiRenderer {
    /// Called from the frontend's term.draw() closure.
    pub fn draw(f: &mut Frame, state: &RenderState) -> RenderOutput {
        let area = f.area();
        let input_line_count = state.input_text.lines().count().max(1) as u16;
        let input_height = (input_line_count + 2).clamp(3, 10);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ])
            .split(area);

        // Tab bar
        tab_bar::render(f, chunks[0], &state.tasks, state.active_task_index);

        // Conversation + optional state panel
        let conv_area = chunks[1];
        let panel_on = state.panel_visible
            && conv_area.width >= PANEL_MIN_TERM_WIDTH
            && conv_area.width.saturating_sub(STATE_PANEL_WIDTH) >= MIN_CONVERSATION_WIDTH;

        let (viewport_height, total_lines) = if panel_on {
            let top = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(MIN_CONVERSATION_WIDTH),
                    Constraint::Length(STATE_PANEL_WIDTH),
                ])
                .split(conv_area);
            let vh = top[0].height;
            let tl = conversation::render_ratatui(
                f, top[0], state, vh,
            );
            state_panel::render(f, top[1], &state.internal, &state.latest_recv, &state.latest_recv_at);
            (vh, tl)
        } else {
            let vh = conv_area.height;
            let tl = conversation::render_ratatui(
                f, conv_area, state, vh,
            );
            (vh, tl)
        };

        // Input bar
        input::render_ratatui(f, chunks[2], state);

        // Statusline
        statusline::render_ratatui(f, chunks[3], state);

        // Modal overlay
        if let Some(req) = &state.modal_request {
            modal::render(f, area, req, state.modal_selected);
        }

        RenderOutput { viewport_height, total_lines }
    }
}
