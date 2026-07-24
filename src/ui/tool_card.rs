use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::message::{ToolCall, ToolStatus};

pub fn tool_card_lines(t: &ToolCall) -> Vec<Line<'static>> {
    let (badge, badge_style) = match t.status {
        ToolStatus::AwaitingPermission => (
            "[?] awaiting permission",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        ToolStatus::Running => (
            "[*] running",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        ToolStatus::Done => (
            "[+] done",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ),
        ToolStatus::Denied => (
            "[x] denied",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
    };

    let border = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::new();

    let title_left = format!("┌─ tool: {} ", t.name);
    lines.push(Line::from(vec![
        Span::styled(title_left, border),
        Span::styled(badge.to_string(), badge_style),
    ]));

    for arg_line in t.args_preview.lines() {
        lines.push(Line::from(vec![
            Span::styled("│ ", border),
            Span::styled(
                format!("$ {arg_line}"),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    if !t.output.is_empty() {
        lines.push(Line::from(Span::styled("│", border)));
        for o in t.output.lines() {
            lines.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::raw(o.to_string()),
            ]));
        }
    }

    lines.push(Line::from(Span::styled("└─", border)));
    lines
}
