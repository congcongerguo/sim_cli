use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use crate::message::{LogLevel, Message, Timestamp};
use crate::ui::render_state::RenderState;
use crate::ui::tool_card::tool_card_lines;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState, visible: u16) -> u64 {
    let total_lines = state.buffer_total_lines;

    // Absolute→buffer-relative scroll (logical-line offset, u64 so buffers
    // larger than 65 535 lines still scroll correctly). One place only.
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
    let window_end = scroll + visible.max(1) as u64;

    // Virtualized build: only materialize lines for the messages whose
    // logical-line range intersects the visible window [scroll, window_end).
    // Cost is O(viewport), independent of how many messages are buffered.
    // `intra` = leading lines of the first visible message to skip.
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cum: u64 = 0;
    let mut intra: u16 = 0;
    let mut started = false;
    for tm in state.messages.iter() {
        let lc = crate::log_buffer::msg_line_count(&tm.msg);
        let (start, end) = (cum, cum + lc);
        cum = end;
        if lc == 0 || end <= scroll {
            continue; // entirely above the viewport
        }
        if start >= window_end {
            break; // entirely below the viewport
        }
        if !started {
            intra = scroll.saturating_sub(start) as u16;
            started = true;
        }
        stamp_message(&mut lines, &tm.msg, tm.time);
    }

    let para = Paragraph::new(lines)
        .block(Block::default())
        .wrap(Wrap { trim: false })
        .scroll((intra, 0));
    f.render_widget(para, area);

    if !state.follow_tail && total_lines > visible as u64 {
        let hint = if state.unseen_lines > 0 {
            format!(" v {} new - PgDn to follow ", state.unseen_lines)
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

/// Append a message's render lines to `out`, each prefixed with the message's
/// `[HH:MM:SS.mmm]` timestamp. The number of lines produced MUST equal
/// `log_buffer::msg_line_count(msg)` (the scroll math depends on it).
fn stamp_message(out: &mut Vec<Line<'static>>, msg: &Message, time: Timestamp) {
    let ts = time.format("[%H:%M:%S%.3f] ").to_string();
    let mut push = |content: Vec<Span<'static>>| {
        let mut spans = Vec::with_capacity(content.len() + 1);
        spans.push(Span::styled(ts.clone(), Style::default().fg(Color::DarkGray)));
        spans.extend(content);
        out.push(Line::from(spans));
    };
    match msg {
        Message::Assistant { text, streaming } => {
            for line in text.lines() {
                push(vec![Span::styled(line.to_string(), Style::default().fg(Color::White))]);
            }
            if *streaming {
                push(vec![Span::styled(
                    "|",
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::SLOW_BLINK),
                )]);
            }
        }
        Message::Tool(t) => {
            for l in tool_card_lines(t) {
                push(l.spans);
            }
        }
        Message::System { text, level } => {
            let color = log_level_color(*level);
            for line in text.lines() {
                push(vec![Span::styled(line.to_string(), Style::default().fg(color))]);
            }
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
            unseen_lines: 0,
            evicted_lines: 0,
            buffer_total_lines: total_lines,
            panel_visible: true,
            modal_request: None,
            modal_selected: 0,
            filter: None,
            filter_error: None,
            exclude: None,
            exclude_error: None,
            filter_counts: None,
            grep: None,
            grep_error: None,
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

    fn many(n: usize) -> Vec<TimedMessage> {
        (0..n)
            .map(|i| TimedMessage {
                time: fixed_time(),
                msg: Message::System { text: format!("msg-{i}"), level: LogLevel::Info },
            })
            .collect()
    }

    #[test]
    fn large_buffer_follow_tail_shows_the_tail() {
        // 1000 messages, 6-row viewport, following the tail.
        let state = state_with(many(1000), 1000);
        let text = buffer_text(&state);
        assert!(text.contains("msg-999"), "newest must be visible:\n{text}");
        assert!(text.contains("msg-994"), "top of the 6-row window visible");
        assert!(!text.contains("msg-993"), "just above the window is not rendered");
        assert!(!text.contains("msg-0 "), "the head is not rendered");
    }

    #[test]
    fn buffer_over_u16_follows_the_real_tail() {
        // Regression for the "stuck at ~65535" bug: with more than u16::MAX
        // lines, following the tail must reach the true end, not clamp.
        let state = state_with(many(70_000), 70_000);
        let text = buffer_text(&state);
        assert!(text.contains("msg-69999"), "must reach the real tail:\n{text}");
        assert!(!text.contains("msg-65000"), "not stuck near u16::MAX");
    }

    #[test]
    fn large_buffer_mid_scroll_shows_the_right_window() {
        let mut state = state_with(many(1000), 1000);
        state.follow_tail = false;
        state.scroll_offset = 500; // logical line 500
        let text = buffer_text(&state);
        // 6-row viewport, but the bottom row shows the "scrolled" hint overlay,
        // so five message rows are visible: msg-500..504.
        assert!(text.contains("msg-500"), "window starts at the scroll offset:\n{text}");
        assert!(text.contains("msg-504"), "window spans the viewport:\n{text}");
        assert!(!text.contains("msg-499"), "above the window is not shown");
        assert!(!text.contains("msg-510"), "well below the window is not shown");
    }
}
