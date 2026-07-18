use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::frontend::InputState;
use crate::ui::render_state::RenderState;

pub fn render_ratatui(f: &mut Frame, area: Rect, state: &RenderState) {
    let menu_open = !state.menu_items.is_empty();

    let border_color = if state.streaming {
        Color::Yellow
    } else {
        match state.input_state {
            InputState::Ambiguous | InputState::Unknown => Color::Red,
            InputState::Resolvable => Color::Green,
            InputState::Empty => Color::DarkGray,
        }
    };

    let title_text = if state.streaming {
        " streaming… ".to_string()
    } else {
        match state.input_state {
            InputState::Ambiguous => " command  (ambiguous — Tab/Enter to pick) ".to_string(),
            InputState::Unknown => " command  (unknown — Tab for suggestions) ".to_string(),
            InputState::Resolvable | InputState::Empty => {
                " command  (Tab=complete  Enter=run  ↑↓=pick) ".to_string()
            }
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title_text, Style::default().fg(Color::Cyan)));

    f.render_widget(block, area);

    // Render input text as a paragraph (simplified from TextArea widget).
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let input_text = if state.input_text.is_empty() {
        "command (Tab to complete, Enter to run)".to_string()
    } else {
        state.input_text.clone()
    };
    let input_p = Paragraph::new(Line::from(Span::styled(input_text, Style::default())));
    f.render_widget(input_p, inner);

    // Autocomplete menu
    if menu_open {
        let menu = &state.menu_items;
        let menu_height = (menu.len() as u16 + 2).min(10);
        let menu_width = 56.min(area.width.saturating_sub(2));
        let menu_x = area.x + 1;
        let menu_y = area.y.saturating_sub(menu_height);
        let menu_area = Rect { x: menu_x, y: menu_y, width: menu_width, height: menu_height };
        f.render_widget(Clear, menu_area);

        let lines: Vec<Line> = menu
            .iter()
            .enumerate()
            .map(|(i, (name, desc))| {
                let style = if i == state.menu_idx {
                    Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(format!(" {name:<10} "), style),
                    Span::styled(format!("{desc} "), Style::default().fg(Color::Gray)),
                ])
            })
            .collect();

        let title_label = state.menu_title.clone().unwrap_or_else(|| format!("({})", menu.len()));
        let menu_widget = Paragraph::new(lines).block(
            Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)).title(title_label),
        );
        f.render_widget(menu_widget, menu_area);
    }
}
