pub mod conversation;
pub mod input;
pub mod markdown;
pub mod modal;
pub mod state_panel;
pub mod statusline;
pub mod tool_card;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::frontend::Frontend;

const STATE_PANEL_WIDTH: u16 = 36;
const MIN_CONVERSATION_WIDTH: u16 = 40;
const PANEL_MIN_TERM_WIDTH: u16 = 80;

pub fn render(f: &mut Frame, fe: &Frontend) {
    let area = f.area();

    let input_height = (fe.input.lines().len() as u16 + 2).clamp(3, 10);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

    let panel_on = fe.panel_visible
        && chunks[0].width >= PANEL_MIN_TERM_WIDTH
        && chunks[0]
            .width
            .saturating_sub(STATE_PANEL_WIDTH)
            >= MIN_CONVERSATION_WIDTH;

    if panel_on {
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(MIN_CONVERSATION_WIDTH),
                Constraint::Length(STATE_PANEL_WIDTH),
            ])
            .split(chunks[0]);
        // Border takes 2 lines; record visible lines for scroll step
        fe.viewport_height.set(top[0].height.saturating_sub(2));
        conversation::render(f, top[0], &fe.view, &fe.scroll, fe.follow_tail, &fe.prev_total_lines);
        state_panel::render(f, top[1], &fe.view);
    } else {
        fe.viewport_height.set(chunks[0].height.saturating_sub(2));
        conversation::render(f, chunks[0], &fe.view, &fe.scroll, fe.follow_tail, &fe.prev_total_lines);
    }

    input::render(f, chunks[1], fe);
    statusline::render(f, chunks[2], &fe.view);

    if let Some(req) = &fe.view.modal {
        modal::render(f, area, req, fe.modal_selected);
    }
}
