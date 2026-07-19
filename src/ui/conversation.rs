use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::message::{LogLevel, Message};
use crate::ui::render_state::RenderState;
use crate::ui::tool_card::tool_card_lines;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState, visible: u16) -> u16 {
    let mut all: Vec<Line<'static>> = Vec::new();

    for msg in state.messages.iter() {
        match msg {
            Message::Assistant { text, streaming } => {
                if text.is_empty() && !*streaming {
                    continue;
                }
                for line in text.lines() {
                    all.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::White),
                    )));
                }
                if *streaming {
                    all.push(Line::from(Span::styled(
                        "▌",
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::SLOW_BLINK),
                    )));
                }
            }
            Message::Tool(t) => {
                for l in tool_card_lines(t) {
                    all.push(l);
                }
            }
            Message::System { text, level } => {
                let color = log_level_color(*level);
                for line in text.lines() {
                    all.push(Line::from(Span::styled(line.to_string(), Style::default().fg(color))));
                }
            }
        }
    }

    while all.last().map(|l| l.spans.is_empty()).unwrap_or(false) {
        all.pop();
    }

    // total_lines 来自 LogBuffer 增量维护（O(1)），不是 all.len()
    let total_lines = state.buffer_total_lines as u16;
    let block = Block::default();

    // 绝对行号 → 缓冲区内相对位置
    //   offset  = 绝对行号（从第一条消息算起）
    //   evicted = 累计淘汰行数
    //   adjusted = offset - evicted = 当前缓冲区内位置
    // 饱和运算保证不会出现负数，限制在 [0, max_scroll] 内
    let max_scroll = total_lines.saturating_sub(visible);
    let scroll = if state.follow_tail {
        max_scroll
    } else {
        // offset 和 evicted_lines 都是 u64，直接减。
        let adjusted = (state.scroll_offset.saturating_sub(state.evicted_lines))
            .min(u16::MAX as u64) as u16;
        adjusted.min(max_scroll)
    };

    let para = Paragraph::new(all).block(block).wrap(Wrap { trim: false }).scroll((scroll, 0));
    f.render_widget(para, area);

    if !state.follow_tail && total_lines > visible {
        let hint = if state.unseen_lines > 0 {
            format!(" ▼ {} new — PgDn to follow ", state.unseen_lines as u16)
        } else {
            " ▲ scrolled — PgDn to follow ".to_string()
        };
        let hint_y = area.y + area.height.saturating_sub(1);
        let hint_area = Rect { x: area.x, y: hint_y, width: hint.len() as u16, height: 1 };
        let p = Paragraph::new(Span::styled(hint, Style::default().bg(Color::Yellow).fg(Color::Black)));
        f.render_widget(p, hint_area);
    }

    total_lines
}

fn log_level_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Error => Color::Red,
        LogLevel::Warn => Color::Yellow,
        LogLevel::Notice => Color::Cyan,
        LogLevel::Info => Color::White,
        LogLevel::Debug => Color::DarkGray,
    }
}
