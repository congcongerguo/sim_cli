use std::cell::Cell;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::backend::{Mode, ViewState};
use crate::message::Message;
use crate::ui::markdown::render_markdown;
use crate::ui::tool_card::tool_card_lines;

pub fn render(
    f: &mut Frame,
    area: Rect,
    view: &ViewState,
    scroll: &Cell<u16>,
    follow_tail: bool,
    prev_total_lines: &Cell<u16>,
) {
    let mut all: Vec<Line<'static>> = Vec::new();

    for msg in view.messages.iter() {
        match msg {
            Message::Assistant { text, streaming } => {
                if text.is_empty() && !*streaming {
                    continue;
                }
                let mut lines = render_markdown(text);
                if *streaming {
                    let last = lines.last_mut();
                    let cursor = Span::styled(
                        "▌",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::SLOW_BLINK),
                    );
                    if let Some(l) = last {
                        l.spans.push(cursor);
                    } else {
                        lines.push(Line::from(cursor));
                    }
                }
                for l in lines {
                    all.push(l);
                }
                all.push(Line::from(""));
            }
            Message::Tool(t) => {
                for l in tool_card_lines(t) {
                    all.push(l);
                }
                all.push(Line::from(""));
            }
            Message::System(text) => {
                for line in text.lines() {
                    all.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                all.push(Line::from(""));
            }
        }
    }

    while all.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
        all.pop();
    }

    let mode_label = match view.mode {
        Mode::Normal => "normal",
        Mode::Plan => "plan",
    };

    let title = format!(" sim_cli · {} · {} ", view.model, mode_label);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    let total_lines = all.len() as u16;
    let visible = inner.height;

    // Auto-adjust scroll when new content arrives while user is scrolled up.
    // `scroll` is "lines away from bottom", so when the bottom moves further
    // down (more content), scroll must increase by the same amount to keep
    // the view anchored at the same content.
    if !follow_tail {
        let prev = prev_total_lines.get();
        if total_lines > prev {
            let delta = total_lines - prev;
            scroll.set(scroll.get().saturating_add(delta));
        }
    }
    prev_total_lines.set(total_lines);

    let cur_scroll = scroll.get();
    let max_scroll = total_lines.saturating_sub(visible);
    let final_scroll = if follow_tail {
        max_scroll
    } else {
        max_scroll.saturating_sub(cur_scroll)
    };

    let para = Paragraph::new(all)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((final_scroll, 0));
    f.render_widget(para, area);

    if !follow_tail && total_lines > visible {
        let hint = " ▲ scrolled — PgDn to follow ";
        let hint_x = area.x + area.width.saturating_sub(hint.len() as u16 + 2);
        let hint_area = Rect {
            x: hint_x,
            y: area.y,
            width: hint.len() as u16,
            height: 1,
        };
        let p = Paragraph::new(Span::styled(
            hint,
            Style::default().fg(Color::Black).bg(Color::Yellow),
        ));
        f.render_widget(p, hint_area);
    }
}
