use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ui::render_state::RenderState;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState) {
    let left_text = if let Some(ref badge) = state.state.badge {
        format!(" {badge} ")
    } else {
        " net: off ".to_string()
    };
    let left_bg = if state.state.active {
        Color::Green
    } else if state.state.badge.is_some() {
        Color::DarkGray
    } else {
        Color::DarkGray
    };
    let left = Span::styled(left_text, Style::default().bg(left_bg).fg(Color::Black));

    let middle_text = if state.streaming {
        " * streaming "
    } else if state.modal_request.is_some() {
        " ! awaiting permission "
    } else {
        " idle "
    };
    let middle = Span::styled(middle_text, Style::default().fg(if state.streaming || state.modal_request.is_some() {
        Color::Yellow
    } else {
        Color::DarkGray
    }));

    // Active include filter (or the error from a rejected expression). The
    // shown/total count sits on whichever filter chip is present.
    let counts = state
        .filter_counts
        .map(|(shown, total)| format!(" ({shown}/{total})"))
        .unwrap_or_default();
    let include_span = if let Some(ref err) = state.filter_error {
        Some(Span::styled(
            format!(" filter error: {err} "),
            Style::default().bg(Color::Red).fg(Color::Black),
        ))
    } else {
        state.filter.as_ref().map(|f| {
            Span::styled(
                format!(" filter: {f} "),
                Style::default().bg(Color::Cyan).fg(Color::Black),
            )
        })
    };
    let exclude_span = if let Some(ref err) = state.exclude_error {
        Some(Span::styled(
            format!(" exclude error: {err} "),
            Style::default().bg(Color::Red).fg(Color::Black),
        ))
    } else {
        state.exclude.as_ref().map(|f| {
            Span::styled(
                format!(" exclude: {f} "),
                Style::default().bg(Color::Magenta).fg(Color::Black),
            )
        })
    };

    let hint = Span::styled(" Enter=send  cmd  ^C=exit  <-/->=tab ", Style::default().fg(Color::DarkGray));

    let mut left_spans = vec![left, middle];
    if let Some(fs) = include_span {
        left_spans.push(fs);
    }
    if let Some(es) = exclude_span {
        left_spans.push(es);
    }
    // Show the shown/total count once, after the filter chips, if any filter is on.
    if !counts.is_empty() && (state.filter.is_some() || state.exclude.is_some()) {
        left_spans.push(Span::styled(
            format!("{counts} "),
            Style::default().fg(Color::Gray),
        ));
    }
    let left_line = Line::from(left_spans);
    let right_line = Line::from(hint);

    f.render_widget(Paragraph::new(left_line), area);
    f.render_widget(Paragraph::new(right_line).alignment(ratatui::layout::Alignment::Right), area);
}
