use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::message::{LogLevel, Message, Timestamp};
use crate::ui::render_state::RenderState;
use crate::ui::tool_card::tool_card_lines;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState, visible: u16) -> u16 {
    let mut all: Vec<Line<'static>> = Vec::new();

    for tm in state.messages.iter() {
        // Lines produced by this one message, before timestamp decoration.
        let mut lines: Vec<Line<'static>> = Vec::new();
        match &tm.msg {
            Message::Assistant { text, streaming } => {
                if text.is_empty() && !*streaming {
                    continue;
                }
                for line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::White),
                    )));
                }
                if *streaming {
                    lines.push(Line::from(Span::styled(
                        "▌",
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::SLOW_BLINK),
                    )));
                }
            }
            Message::Tool(t) => {
                for l in tool_card_lines(t) {
                    lines.push(l);
                }
            }
            Message::System { text, level } => {
                let color = log_level_color(*level);
                for line in text.lines() {
                    lines.push(Line::from(Span::styled(line.to_string(), Style::default().fg(color))));
                }
            }
        }
        prepend_timestamp(&mut lines, tm.time);
        all.extend(lines);
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

/// Prefix a message's first render line with a millisecond-precision
/// time-of-day (`HH:MM:SS.mmm`); align continuation lines under the text.
/// Adds no new lines, so `msg_line_count`/scroll math stay correct.
fn prepend_timestamp(lines: &mut [Line<'static>], time: Timestamp) {
    if lines.is_empty() {
        return;
    }
    let ts = time.format("%H:%M:%S%.3f").to_string(); // 12 chars, e.g. 14:23:01.123
    let indent = " ".repeat(ts.chars().count() + 1);
    for (i, line) in lines.iter_mut().enumerate() {
        if i == 0 {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(format!("{ts} "), Style::default().fg(Color::DarkGray)));
            spans.append(&mut line.spans);
            line.spans = spans;
        } else if !line.spans.is_empty() {
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::raw(indent.clone()));
            spans.append(&mut line.spans);
            line.spans = spans;
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::Arc;

    use crate::frontend::InputState;
    use crate::message::TimedMessage;
    use crate::tool::ToolState;

    fn fixed_time() -> Timestamp {
        Local.with_ymd_and_hms(2026, 7, 20, 14, 23, 1).unwrap()
            + chrono::Duration::milliseconds(123)
    }

    fn state_with(messages: Vec<TimedMessage>, total_lines: u64) -> RenderState {
        RenderState {
            messages: Arc::new(messages),
            streaming: false,
            state: ToolState::default(),
            tools: Arc::new(vec![]),
            active_index: 0,
            input_text: String::new(),
            input_cursor: (0, 0),
            input_state: InputState::Empty,
            menu_items: vec![],
            menu_idx: 0,
            menu_title: None,
            scroll_offset: 0,
            follow_tail: true,
            prev_total_lines: 0,
            unseen_lines: 0,
            evicted_lines: 0,
            buffer_total_lines: total_lines,
            panel_visible: true,
            modal_request: None,
            modal_selected: 0,
        }
    }

    fn buffer_text(state: &RenderState) -> String {
        let backend = TestBackend::new(60, 6);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| {
            let area = f.area();
            render_ratatui(f, area, state, area.height);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_millisecond_timestamp_before_message() {
        let msg = Message::System { text: "hello".into(), level: LogLevel::Info };
        let state = state_with(vec![TimedMessage { time: fixed_time(), msg }], 1);
        let text = buffer_text(&state);
        assert!(text.contains("14:23:01.123"), "timestamp missing:\n{text}");
        assert!(text.contains("14:23:01.123 hello"), "timestamp should sit before text:\n{text}");
    }

    #[test]
    fn continuation_lines_are_indented_not_prefixed() {
        let msg = Message::System { text: "one\ntwo".into(), level: LogLevel::Info };
        let state = state_with(vec![TimedMessage { time: fixed_time(), msg }], 2);
        let text = buffer_text(&state);
        let lines: Vec<&str> = text.lines().collect();
        // First line carries the timestamp, the second is indented under the text.
        assert!(lines[0].contains("14:23:01.123 one"));
        assert!(!lines[1].contains("14:23:01.123"), "only the first line gets a stamp");
        assert!(lines[1].trim_start().starts_with("two"));
        assert!(lines[1].starts_with(' '), "continuation line should be indented");
    }
}
