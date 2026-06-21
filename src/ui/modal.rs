use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::backend::ModalRequest;

pub fn render(f: &mut Frame, area: Rect, req: &ModalRequest, selected: usize) {
    let width = 60.min(area.width.saturating_sub(4));
    let height = 9u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let modal_area = Rect {
        x,
        y,
        width,
        height,
    };

    f.render_widget(Clear, modal_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " permission required ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Tool: ", Style::default().fg(Color::Gray)),
        Span::styled(
            req.tool_name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Args: ", Style::default().fg(Color::Gray)),
        Span::styled(req.args_preview.clone(), Style::default().fg(Color::Yellow)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Allow this call?",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let opts = ["[ Yes ]", "[ No ]", "[ Always ]"];
    let mut spans: Vec<Span> = Vec::new();
    for (i, label) in opts.iter().enumerate() {
        let style = if i == selected {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    lines.push(Line::from(spans));

    let p = Paragraph::new(lines);
    f.render_widget(p, inner);
}
