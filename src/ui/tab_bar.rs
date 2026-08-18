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
        let (fg, bg) = if !t.available {
            // 运行期不可用:整块灰显,无法切入。
            (Color::DarkGray, Color::Black)
        } else if i == active {
            (Color::White, Color::Blue)
        } else {
            (Color::Gray, Color::DarkGray)
        };

        // 标记位:不可用显示 'x',运行中显示 '*',其余留空。
        let (dot, dot_color) = if !t.available {
            ("x", Color::DarkGray)
        } else if t.active {
            ("*", Color::Green)
        } else {
            (" ", Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" {dot} "),
            Style::default().fg(dot_color).bg(bg),
        ));
        let mut name_style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
        if !t.available {
            name_style = name_style.add_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(
            format!(" {} ", t.name),
            name_style,
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
