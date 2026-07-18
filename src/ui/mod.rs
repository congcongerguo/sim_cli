pub mod conversation;
pub mod input;
pub mod markdown;
pub mod modal;
pub mod state_panel;
pub mod statusline;
pub mod tab_bar;
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
            Constraint::Length(1),              // tab bar
            Constraint::Min(3),                 // conversation
            Constraint::Length(input_height),   // input
            Constraint::Length(1),              // statusline
        ])
        .split(area);

    tab_bar::render(f, chunks[0], &fe.view.tasks, fe.view.active_task_index);

    let conv_area = chunks[1];
    let panel_on = fe.panel_visible
        && conv_area.width >= PANEL_MIN_TERM_WIDTH
        && conv_area
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
            .split(conv_area);
        // Border takes 2 lines; record visible lines for scroll step
        fe.viewport_height.set(top[0].height.saturating_sub(2));
        conversation::render(
            f, top[0], &fe.view, &fe.scroll, fe.follow_tail, &fe.prev_total_lines,
        );
        state_panel::render(f, top[1], &fe.view);
    } else {
        fe.viewport_height.set(conv_area.height.saturating_sub(2));
        conversation::render(
            f, conv_area, &fe.view, &fe.scroll, fe.follow_tail, &fe.prev_total_lines,
        );
    }

    input::render(f, chunks[2], fe);
    statusline::render(f, chunks[3], &fe.view);

    if let Some(req) = &fe.view.modal {
        modal::render(f, area, req, fe.modal_selected);
    }
}
