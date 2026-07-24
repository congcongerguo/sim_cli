use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::message::{LogLevel, Message, Timestamp};
use crate::ui::render_state::RenderState;
use crate::ui::tool_card::tool_card_lines;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState, visible: u16) -> u16 {
    // Each render line is tagged with the timestamp of the message it came
    // from, so every line can be prefixed with its own `[HH:MM:SS.mmm]`.
    let mut all: Vec<(Timestamp, Line<'static>)> = Vec::new();

    for tm in state.messages.iter() {
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
                        "|",
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
        all.extend(lines.into_iter().map(|l| (tm.time, l)));
    }

    // Trim trailing blank lines based on their original (pre-timestamp) content.
    while all.last().map(|(_, l)| l.spans.is_empty()).unwrap_or(false) {
        all.pop();
    }

    // Prefix every line with its own `[HH:MM:SS.mmm] ` (millisecond precision).
    let all: Vec<Line<'static>> = all
        .into_iter()
        .map(|(time, mut line)| {
            let ts = time.format("[%H:%M:%S%.3f] ").to_string();
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            spans.push(Span::styled(ts, Style::default().fg(Color::DarkGray)));
            spans.append(&mut line.spans);
            line.spans = spans;
            line
        })
        .collect();

    // total_lines 来自 LogBuffer（消息渲染行数），不是 all.len()
    let total_lines = state.buffer_total_lines as u16;
    let block = Block::default();

    // 绝对行号 → 缓冲区内 u16 偏移的换算，统一走 scroll::viewport_offset,
    // 全项目只有这一处做这件事。
    let scroll = crate::scroll::viewport_offset(
        &crate::scroll::ScrollState {
            offset: state.scroll_offset,
            follow_tail: state.follow_tail,
        },
        &crate::scroll::ScrollInput {
            viewport: visible,
            total_lines: state.buffer_total_lines,
            evicted_lines: state.evicted_lines,
        },
    );

    let para = Paragraph::new(all).block(block).wrap(Wrap { trim: false }).scroll((scroll, 0));
    f.render_widget(para, area);

    if !state.follow_tail && total_lines > visible {
        let hint = if state.unseen_lines > 0 {
            format!(" v {} new - PgDn to follow ", state.unseen_lines as u16)
        } else {
            " ^ scrolled - PgDn to follow ".to_string()
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
            unseen_lines: 0,
            evicted_lines: 0,
            buffer_total_lines: total_lines,
            panel_visible: true,
            modal_request: None,
            modal_selected: 0,
            filter: None,
            filter_error: None,
            filter_counts: None,
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
        assert!(text.contains("[14:23:01.123]"), "timestamp missing:\n{text}");
        assert!(text.contains("[14:23:01.123] hello"), "timestamp should sit before text:\n{text}");
    }

    #[test]
    fn every_line_gets_its_own_timestamp() {
        let msg = Message::System { text: "one\ntwo".into(), level: LogLevel::Info };
        let state = state_with(vec![TimedMessage { time: fixed_time(), msg }], 2);
        let text = buffer_text(&state);
        let lines: Vec<&str> = text.lines().collect();
        // Both lines carry the timestamp — not just the first.
        assert!(lines[0].contains("[14:23:01.123] one"));
        assert!(lines[1].contains("[14:23:01.123] two"), "continuation line must be stamped too");
    }
}
