use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub fn render_markdown(src: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(src, opts);

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut in_code_block: Option<String> = None;
    let mut code_buf = String::new();
    let mut list_depth: usize = 0;
    let mut ordered_index: Vec<u64> = Vec::new();

    let push_line = |current: &mut Vec<Span<'static>>, out: &mut Vec<Line<'static>>| {
        let line = std::mem::take(current);
        out.push(Line::from(line));
    };

    for ev in parser {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    if !current.is_empty() {
                        push_line(&mut current, &mut out);
                    }
                    let mut s = *style_stack.last().unwrap();
                    s = s
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD);
                    style_stack.push(s);
                    let prefix = "#".repeat(level as usize);
                    current.push(Span::styled(format!("{prefix} "), s));
                }
                Tag::Paragraph => {}
                Tag::BlockQuote(_) => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC);
                    style_stack.push(s);
                    current.push(Span::styled("│ ", s));
                }
                Tag::CodeBlock(kind) => {
                    if !current.is_empty() {
                        push_line(&mut current, &mut out);
                    }
                    let lang = match kind {
                        CodeBlockKind::Fenced(s) => s.to_string(),
                        CodeBlockKind::Indented => String::new(),
                    };
                    in_code_block = Some(lang);
                    code_buf.clear();
                }
                Tag::List(start) => {
                    list_depth += 1;
                    ordered_index.push(start.unwrap_or(0));
                }
                Tag::Item => {
                    if !current.is_empty() {
                        push_line(&mut current, &mut out);
                    }
                    let indent = "  ".repeat(list_depth.saturating_sub(1));
                    let bullet = if let Some(n) = ordered_index.last_mut().filter(|n| **n > 0) {
                        let s = format!("{}{}. ", indent, n);
                        *n += 1;
                        s
                    } else {
                        format!("{}• ", indent)
                    };
                    current.push(Span::raw(bullet));
                }
                Tag::Emphasis => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::ITALIC);
                    style_stack.push(s);
                }
                Tag::Strong => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::BOLD);
                    style_stack.push(s);
                }
                Tag::Strikethrough => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .add_modifier(Modifier::CROSSED_OUT);
                    style_stack.push(s);
                }
                Tag::Link { .. } => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or_default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::UNDERLINED);
                    style_stack.push(s);
                }
                _ => {
                    style_stack.push(*style_stack.last().unwrap());
                }
            },
            Event::End(end) => match end {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    push_line(&mut current, &mut out);
                }
                TagEnd::Paragraph => {
                    push_line(&mut current, &mut out);
                }
                TagEnd::BlockQuote(_) => {
                    style_stack.pop();
                    push_line(&mut current, &mut out);
                }
                TagEnd::CodeBlock => {
                    if let Some(lang) = in_code_block.take() {
                        let highlighted =
                            highlight_code(&code_buf, &lang).unwrap_or_else(|| {
                                code_buf
                                    .lines()
                                    .map(|l| {
                                        Line::from(Span::styled(
                                            l.to_string(),
                                            Style::default().fg(Color::Yellow),
                                        ))
                                    })
                                    .collect()
                            });
                        let label = if lang.is_empty() {
                            "code".to_string()
                        } else {
                            lang
                        };
                        out.push(Line::from(Span::styled(
                            format!("┌─ {label} ─"),
                            Style::default().fg(Color::DarkGray),
                        )));
                        for line in highlighted {
                            let mut spans: Vec<Span<'static>> = vec![Span::styled(
                                "│ ",
                                Style::default().fg(Color::DarkGray),
                            )];
                            spans.extend(line.spans);
                            out.push(Line::from(spans));
                        }
                        out.push(Line::from(Span::styled(
                            "└─",
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                TagEnd::List(_) => {
                    list_depth = list_depth.saturating_sub(1);
                    ordered_index.pop();
                    if !current.is_empty() {
                        push_line(&mut current, &mut out);
                    }
                }
                TagEnd::Item => {
                    if !current.is_empty() {
                        push_line(&mut current, &mut out);
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                    style_stack.pop();
                }
                _ => {
                    if style_stack.len() > 1 {
                        style_stack.pop();
                    }
                }
            },
            Event::Text(t) => {
                if in_code_block.is_some() {
                    code_buf.push_str(&t);
                } else {
                    let s = *style_stack.last().unwrap();
                    current.push(Span::styled(t.to_string(), s));
                }
            }
            Event::Code(c) => {
                let s = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD);
                current.push(Span::styled(format!("`{c}`"), s));
            }
            Event::SoftBreak => {
                current.push(Span::raw(" "));
            }
            Event::HardBreak => {
                push_line(&mut current, &mut out);
            }
            Event::Rule => {
                if !current.is_empty() {
                    push_line(&mut current, &mut out);
                }
                out.push(Line::from(Span::styled(
                    "──────",
                    Style::default().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }
    if !current.is_empty() {
        push_line(&mut current, &mut out);
    }
    out
}

fn highlight_code(code: &str, lang: &str) -> Option<Vec<Line<'static>>> {
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = ts.themes.get("base16-ocean.dark")?;
    let syntax = ps
        .find_syntax_by_token(lang)
        .or_else(|| ps.find_syntax_by_extension(lang))
        .unwrap_or_else(|| ps.find_syntax_plain_text());
    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ranges = h.highlight_line(line, &ps).ok()?;
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (style, text) in ranges {
            spans.push(Span::styled(
                text.trim_end_matches('\n').to_string(),
                syn_to_rat(style),
            ));
        }
        out.push(Line::from(spans));
    }
    Some(out)
}

fn syn_to_rat(s: SynStyle) -> Style {
    Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b))
}
