use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::backend::{ConnState, Mode, ViewState};

pub fn render(f: &mut Frame, area: Rect, view: &ViewState) {
    let mode_text = match view.mode {
        Mode::Normal => Span::styled(
            " normal ",
            Style::default().bg(Color::Blue).fg(Color::White),
        ),
        Mode::Plan => Span::styled(
            " plan ",
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let model = Span::styled(
        format!(" {} ", view.model),
        Style::default().bg(Color::DarkGray).fg(Color::White),
    );

    let (conn_text, conn_bg) = match &view.conn {
        ConnState::Disconnected => (" net: off ".to_string(), Color::DarkGray),
        ConnState::Connecting { protocol, addr } => (
            format!(" {}: ⌛ {addr} ", protocol.as_str()),
            Color::Yellow,
        ),
        ConnState::Connected { protocol, addr } => (
            format!(" {}: ● {addr} ", protocol.as_str()),
            Color::Green,
        ),
        ConnState::Error(e) => {
            let trimmed: String = e.chars().take(40).collect();
            (format!(" net: ✕ {trimmed} "), Color::Red)
        }
    };
    let conn = Span::styled(
        conn_text,
        Style::default().bg(conn_bg).fg(Color::Black),
    );

    let middle_text = if view.streaming {
        " ● streaming "
    } else if view.modal.is_some() {
        " ⚑ awaiting permission "
    } else {
        " idle "
    };
    let middle = Span::styled(
        middle_text,
        Style::default().fg(if view.streaming || view.modal.is_some() {
            Color::Yellow
        } else {
            Color::DarkGray
        }),
    );

    let hint = Span::styled(
        " ⏎ send  /  cmd  ^C exit ",
        Style::default().fg(Color::DarkGray),
    );

    let left_line = Line::from(vec![mode_text, model, conn, middle]);
    let right_line = Line::from(hint);

    let left = Paragraph::new(left_line);
    let right = Paragraph::new(right_line).alignment(ratatui::layout::Alignment::Right);
    f.render_widget(left, area);
    f.render_widget(right, area);
}
