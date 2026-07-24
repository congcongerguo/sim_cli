use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tool::ToolInfo;

pub fn render(f: &mut Frame, area: Rect, tools: &[ToolInfo], active: usize) {
    let tab_row = Rect { height: 1, ..area };
    let mut spans: Vec<Span> = Vec::new();

    for (i, t) in tools.iter().enumerate() {
        let (fg, bg) = if i == active {
            (Color::White, Color::Blue)
        } else {
            (Color::Gray, Color::DarkGray)
        };

        let (dot, dot_color) = if t.active {
            ("*", Color::Green)
        } else {
            (" ", Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" {dot} "),
            Style::default().fg(dot_color).bg(bg),
        ));
        spans.push(Span::styled(
            format!(" {} ", t.name),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ));
    }

    spans.push(Span::raw(" ".repeat(area.width as usize)));
    f.render_widget(Paragraph::new(Line::from(spans)), tab_row);

    let sep_row = Rect { y: area.y + 1, height: 1, ..area };
    let sep = Span::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(Color::DarkGray),
    );
    f.render_widget(Paragraph::new(Line::from(sep)), sep_row);
}
