use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::Value;

use crate::tool::ToolState;

pub fn render(f: &mut Frame, area: Rect, state: &ToolState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " state ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();

    if state.fields.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no data yet)",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        let key_w = state.fields.iter()
            .map(|(k, _)| k.chars().count())
            .max()
            .unwrap_or(0)
            .min(((inner.width as usize) / 2).max(1))
            .min(20);
        let value_w = (inner.width as usize).saturating_sub(key_w + 1);
        for (k, val) in &state.fields {
            let key = truncate(k, key_w);
            let value = truncate(val, value_w);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<width$}", key, width = key_w),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(value, Style::default().fg(Color::White)),
            ]));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// Recursively flatten a JSON value into dot-separated key-value pairs.
/// Public so tools can flatten received JSON into [`ToolState::fields`].
pub fn flatten_json(prefix: &str, v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            if map.is_empty() && !prefix.is_empty() {
                out.push((prefix.to_string(), "{}".into()));
                return;
            }
            for (k, child) in map {
                let next = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_json(&next, child, out);
            }
        }
        Value::Array(_) => {
            let key = if prefix.is_empty() { "_" } else { prefix };
            let compact = serde_json::to_string(v).unwrap_or_else(|_| v.to_string());
            out.push((key.to_string(), compact));
        }
        Value::String(s) => {
            let key = if prefix.is_empty() { "_" } else { prefix };
            out.push((key.to_string(), s.clone()));
        }
        Value::Null => {
            let key = if prefix.is_empty() { "_" } else { prefix };
            out.push((key.to_string(), "null".into()));
        }
        Value::Bool(b) => {
            let key = if prefix.is_empty() { "_" } else { prefix };
            out.push((key.to_string(), b.to_string()));
        }
        Value::Number(n) => {
            let key = if prefix.is_empty() { "_" } else { prefix };
            out.push((key.to_string(), n.to_string()));
        }
    }
}

fn truncate(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= width {
        return s.to_string();
    }
    if width == 1 {
        return s.chars().next().map(String::from).unwrap_or_default();
    }
    // Reserve two columns for an ASCII ellipsis ("..") — single-width in every
    // terminal, unlike "…" which is East-Asian-ambiguous and would overflow.
    let take = width - 2;
    let mut out: String = s.chars().take(take).collect();
    out.push_str("..");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flatten_nested_object() {
        let v = json!({
            "id": 1,
            "msg": "ping 1",
            "pos": {"x": 100, "y": 200, "c": 50},
        });
        let mut out = Vec::new();
        flatten_json("", &v, &mut out);
        let map: std::collections::HashMap<_, _> = out.into_iter().collect();
        assert_eq!(map.get("id").map(String::as_str), Some("1"));
        assert_eq!(map.get("msg").map(String::as_str), Some("ping 1"));
        assert_eq!(map.get("pos.x").map(String::as_str), Some("100"));
        assert_eq!(map.get("pos.y").map(String::as_str), Some("200"));
        assert_eq!(map.get("pos.c").map(String::as_str), Some("50"));
    }

    #[test]
    fn flatten_array_compact() {
        let v = json!({"ma": [1, 2, 3]});
        let mut out = Vec::new();
        flatten_json("", &v, &mut out);
        assert_eq!(out, vec![("ma".into(), "[1,2,3]".into())]);
    }

    #[test]
    fn flatten_scalars() {
        let v = json!({"b": true, "n": null, "s": "hi"});
        let mut out = Vec::new();
        flatten_json("", &v, &mut out);
        let map: std::collections::HashMap<_, _> = out.into_iter().collect();
        assert_eq!(map.get("b").map(String::as_str), Some("true"));
        assert_eq!(map.get("n").map(String::as_str), Some("null"));
        assert_eq!(map.get("s").map(String::as_str), Some("hi"));
    }

    #[test]
    fn truncate_keeps_ascii() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hel..");
        assert_eq!(truncate("hello", 1), "h");
        assert_eq!(truncate("hello", 0), "");
        // Result never exceeds the requested column budget.
        assert!(truncate("hello world", 5).chars().count() <= 5);
    }
}
