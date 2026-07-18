use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::backend::{ConnState, Mode};
use crate::ui::render_state::RenderState;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState) {
    let mode_text = match state.mode {
        Mode::Normal => Span::styled(" normal ", Style::default().bg(Color::Blue).fg(Color::White)),
        Mode::Plan => Span::styled(
            " plan ",
            Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD),
        ),
    };

    let model = Span::styled(format!(" {} ", state.model), Style::default().bg(Color::DarkGray).fg(Color::White));

    let (conn_text, conn_bg) = match &state.conn {
        ConnState::Disconnected => (" net: off ".to_string(), Color::DarkGray),
        ConnState::Connecting { protocol, addr } => (format!(" {}: ⌛ {addr} ", protocol.as_str()), Color::Yellow),
        ConnState::Connected { protocol, addr } => (format!(" {}: ● {addr} ", protocol.as_str()), Color::Green),
        ConnState::Error(e) => {
            let trimmed: String = e.chars().take(40).collect();
            (format!(" net: ✕ {trimmed} "), Color::Red)
        }
    };
    let conn = Span::styled(conn_text, Style::default().bg(conn_bg).fg(Color::Black));

    let task_label = Span::styled(
        format!(" {} ({}/{}) ", state.active_task, state.active_task_index + 1, state.tasks.len()),
        Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD),
    );

    let middle_text = if state.streaming {
        " ● streaming "
    } else if state.modal_request.is_some() {
        " ⚑ awaiting permission "
    } else {
        " idle "
    };
    let middle = Span::styled(middle_text, Style::default().fg(if state.streaming || state.modal_request.is_some() {
        Color::Yellow
    } else {
        Color::DarkGray
    }));

    let hint = Span::styled(" ⏎ send  /  cmd  ^C exit  ←→ switch tab ", Style::default().fg(Color::DarkGray));

    let left_line = Line::from(vec![mode_text, model, task_label, conn, middle]);
    let right_line = Line::from(hint);

    f.render_widget(Paragraph::new(left_line), area);
    f.render_widget(Paragraph::new(right_line).alignment(ratatui::layout::Alignment::Right), area);
}
