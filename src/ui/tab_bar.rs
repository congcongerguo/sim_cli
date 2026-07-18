use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::backend::{ConnState, TaskInfo};

/// Render a 1-line tab bar. Active tab is highlighted; other tabs are dimmed.
/// Connection status is shown as a coloured dot prefix.
pub fn render(f: &mut Frame, area: Rect, tasks: &[TaskInfo], active: usize) {
    let mut spans: Vec<Span> = Vec::new();

    for (i, t) in tasks.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }

        let (base_fg, base_bg, bold) = if i == active {
            (Color::Black, Color::Cyan, true)
        } else {
            (Color::Gray, Color::DarkGray, false)
        };

        let dot = conn_dot(&t.conn);
        let demo_icon = if t.demo_running { " ⏳" } else { "" };
        let label = format!("{dot} {}{demo_icon} ", t.name);

        let mut style = Style::default().fg(base_fg).bg(base_bg);
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }

        spans.push(Span::styled(label, style));
    }

    // Fill remaining space with the active tab's background colour.
    let fill = Span::styled(
        " ".repeat(area.width as usize),
        Style::default().bg(Color::Cyan),
    );
    spans.push(fill);

    let line = Line::from(spans);
    let p = Paragraph::new(line);
    f.render_widget(p, area);
}

fn conn_dot(state: &ConnState) -> &'static str {
    match state {
        ConnState::Connected { .. } => "●",
        ConnState::Connecting { .. } => "○",
        ConnState::Error(_) => "✕",
        ConnState::Disconnected => "·",
    }
}
