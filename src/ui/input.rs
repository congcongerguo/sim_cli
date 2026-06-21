use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::frontend::{Frontend, InputState};

pub fn render(f: &mut Frame, area: Rect, fe: &Frontend) {
    let menu = fe.menu_items();
    let menu_open = !menu.is_empty();
    let state = fe.input_state();

    let border_color = if fe.view.streaming {
        Color::Yellow
    } else {
        match state {
            InputState::Ambiguous | InputState::Unknown => Color::Red,
            InputState::MissingArg => Color::Yellow,
            InputState::Resolvable => Color::Green,
            InputState::Empty => Color::DarkGray,
        }
    };

    let title_text = if fe.view.streaming {
        " streaming… ".to_string()
    } else {
        match state {
            InputState::Ambiguous => " command  (ambiguous — Tab/Enter to disambiguate) ".to_string(),
            InputState::Unknown => " command  (unknown — Tab for suggestions) ".to_string(),
            InputState::MissingArg => " command  (needs an arg — Tab/Enter to pick) ".to_string(),
            InputState::Resolvable | InputState::Empty => {
                " command  (Tab=complete  Enter=run  ↑↓=pick) ".to_string()
            }
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title_text, Style::default().fg(Color::Cyan)));

    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(&fe.input, inner);

    if menu_open {
        let menu_height = (menu.len() as u16 + 2).min(10);
        let menu_width = 56.min(area.width.saturating_sub(2));
        let menu_x = area.x + 1;
        let menu_y = area.y.saturating_sub(menu_height);
        let menu_area = Rect {
            x: menu_x,
            y: menu_y,
            width: menu_width,
            height: menu_height,
        };
        f.render_widget(Clear, menu_area);

        let lines: Vec<Line> = menu
            .iter()
            .enumerate()
            .map(|(i, (name, desc))| {
                let style = if i == fe.menu_idx {
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(format!(" {name:<10} "), style),
                    Span::styled(format!("{desc} "), Style::default().fg(Color::Gray)),
                ])
            })
            .collect();

        let title_label = match fe.menu_title() {
            Some(t) => format!(" {t}  ({}) ", menu.len()),
            None => format!(" ({}) ", menu.len()),
        };
        let menu_widget = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title_label),
        );
        f.render_widget(menu_widget, menu_area);
    }
}
