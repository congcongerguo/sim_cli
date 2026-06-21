use anyhow::Result;
use crossterm::event::{
    Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::Backend;
use tokio::sync::{mpsc, watch};
use tui_textarea::{Input, Key, TextArea};

use crate::backend::{Command, ModalChoice, Mode, ViewState};
use crate::commands::{
    self, Action, CommandDef, CompletionCtx, ParseError, COMMANDS,
};
use crate::ui;

const PLACEHOLDER: &str = "command (Tab to complete, Enter to run)";

#[derive(Debug, Default)]
pub struct TabCycle {
    pub names: Vec<String>,
    pub head: String,
    pub idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputState {
    Empty,
    Resolvable,
    Ambiguous,
    MissingArg,
    Unknown,
}

pub struct Frontend {
    pub input: TextArea<'static>,
    pub scroll: u16,
    pub follow_tail: bool,
    pub menu_idx: usize,
    pub tab_cycle: Option<TabCycle>,
    pub demo_idx: usize,
    pub modal_selected: usize,
    pub view: ViewState,
    pub panel_visible: bool,
    cmd_tx: mpsc::Sender<Command>,
    view_rx: watch::Receiver<ViewState>,
}

impl Frontend {
    pub fn new(cmd_tx: mpsc::Sender<Command>, view_rx: watch::Receiver<ViewState>) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(ratatui::style::Style::default());
        input.set_placeholder_text(PLACEHOLDER);
        let view = view_rx.borrow().clone();
        Self {
            input,
            scroll: 0,
            follow_tail: true,
            menu_idx: 0,
            tab_cycle: None,
            demo_idx: 0,
            modal_selected: 0,
            view,
            panel_visible: true,
            cmd_tx,
            view_rx,
        }
    }

    pub async fn run<B: Backend>(&mut self, term: &mut Terminal<B>) -> Result<()> {
        let mut events = EventStream::new();
        loop {
            term.draw(|f| ui::render(f, self))?;
            tokio::select! {
                maybe = events.next() => match maybe {
                    Some(Ok(CtEvent::Key(k))) if k.kind == KeyEventKind::Press => {
                        self.on_key(k);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                    None => break,
                },
                changed = self.view_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let prev_modal = self.view.modal.is_some();
                    self.view = self.view_rx.borrow().clone();
                    if prev_modal && self.view.modal.is_none() {
                        self.modal_selected = 0;
                    }
                }
            }
            if self.view.should_quit {
                break;
            }
        }
        Ok(())
    }

    pub fn current_text(&self) -> String {
        self.input.lines().join("\n")
    }

    fn replace_input(&mut self, text: &str) {
        self.input = TextArea::default();
        self.input.set_placeholder_text(PLACEHOLDER);
        for ch in text.chars() {
            self.input.input(Input {
                key: Key::Char(ch),
                ctrl: false,
                alt: false,
                shift: false,
            });
        }
    }

    pub fn menu_items(&self) -> Vec<(&'static str, &'static str)> {
        match commands::completion_ctx(&self.current_text()) {
            CompletionCtx::Command { prefix } => commands::matching_commands(&prefix),
            CompletionCtx::Sub { cmd, prefix } => commands::matching_subs(cmd, &prefix),
            CompletionCtx::None => Vec::new(),
        }
    }

    pub fn menu_title(&self) -> Option<String> {
        match commands::completion_ctx(&self.current_text()) {
            CompletionCtx::Command { .. } => Some("commands".into()),
            CompletionCtx::Sub { cmd, .. } => Some(format!("{} <arg>", cmd.name)),
            CompletionCtx::None => None,
        }
    }

    pub fn input_state(&self) -> InputState {
        let current = self.current_text();
        let trimmed = current.trim();
        if trimmed.is_empty() {
            return InputState::Empty;
        }
        match commands::parse_action(trimmed) {
            Ok(_) => InputState::Resolvable,
            Err(ParseError::MissingArg(_)) => InputState::MissingArg,
            Err(ParseError::AmbiguousCommand(_)) | Err(ParseError::AmbiguousArg(_, _)) => {
                InputState::Ambiguous
            }
            Err(_) => InputState::Unknown,
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.view.modal.is_some() {
            self.on_key_modal(key);
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.input.lines().iter().all(|l| l.is_empty()) {
                    self.send(Command::Run(Action::Exit));
                } else {
                    self.replace_input("");
                    self.tab_cycle = None;
                    self.menu_idx = 0;
                }
                return;
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_add(5);
                self.follow_tail = false;
                return;
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_sub(5);
                if self.scroll == 0 {
                    self.follow_tail = true;
                }
                return;
            }
            (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                self.run_hotkey(Action::Help);
                return;
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.run_hotkey(Action::Clear);
                return;
            }
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                self.send(Command::Run(Action::Exit));
                return;
            }
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                let next = if self.view.mode == Mode::Plan { "off" } else { "on" };
                self.run_hotkey(Action::Plan(next));
                return;
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                let order = ["claude", "opus", "haiku"];
                let cur = self.view.model.strip_prefix("mock-").unwrap_or(&self.view.model);
                let i = order.iter().position(|m| *m == cur).unwrap_or(0);
                let next = order[(i + 1) % order.len()];
                self.run_hotkey(Action::Model(next));
                return;
            }
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                if self.view.streaming {
                    return;
                }
                let order = ["chat", "code", "tool"];
                let pick = order[self.demo_idx % order.len()];
                self.demo_idx = (self.demo_idx + 1) % order.len();
                self.run_hotkey(Action::Demo(pick));
                return;
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                self.panel_visible = !self.panel_visible;
                return;
            }
            _ => {}
        }

        if matches!(key.code, KeyCode::Tab) {
            self.handle_tab();
            return;
        }

        let menu = self.menu_items();
        if !menu.is_empty() {
            match key.code {
                KeyCode::Up => {
                    if self.menu_idx > 0 {
                        self.menu_idx -= 1;
                    }
                    return;
                }
                KeyCode::Down => {
                    if self.menu_idx + 1 < menu.len() {
                        self.menu_idx += 1;
                    }
                    return;
                }
                _ => {}
            }
        }

        if matches!(key.code, KeyCode::Enter)
            && !key.modifiers.contains(KeyModifiers::SHIFT)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            let text = self.current_text().trim().to_string();
            if text.is_empty() || self.view.streaming {
                return;
            }
            match self.input_state() {
                InputState::Resolvable => self.submit(text),
                InputState::MissingArg | InputState::Ambiguous => self.handle_tab(),
                InputState::Unknown => self.submit(text),
                InputState::Empty => {}
            }
            return;
        }

        self.tab_cycle = None;
        let input: Input = key.into();
        self.input.input(input);
        let menu = self.menu_items();
        if self.menu_idx >= menu.len() {
            self.menu_idx = 0;
        }
    }

    fn on_key_modal(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left | KeyCode::Up => {
                if self.modal_selected > 0 {
                    self.modal_selected -= 1;
                }
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                if self.modal_selected < 2 {
                    self.modal_selected += 1;
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => self.send_modal(ModalChoice::Yes),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.send_modal(ModalChoice::No)
            }
            KeyCode::Char('a') | KeyCode::Char('A') => self.send_modal(ModalChoice::Always),
            KeyCode::Enter => {
                let choice = match self.modal_selected {
                    0 => ModalChoice::Yes,
                    1 => ModalChoice::No,
                    _ => ModalChoice::Always,
                };
                self.send_modal(choice);
            }
            _ => {}
        }
    }

    fn send_modal(&self, choice: ModalChoice) {
        self.send(Command::Permission(choice));
    }

    fn handle_tab(&mut self) {
        if let Some(cycle) = self.tab_cycle.as_mut() {
            if !cycle.names.is_empty() {
                cycle.idx = (cycle.idx + 1) % cycle.names.len();
                let new_text = format!("{}{}", cycle.head, cycle.names[cycle.idx]);
                self.menu_idx = cycle.idx;
                self.replace_input(&new_text);
                return;
            }
        }

        let current = self.current_text();
        let ctx = commands::completion_ctx(&current);

        match ctx {
            CompletionCtx::None => {}
            CompletionCtx::Command { prefix } => {
                let names: Vec<&'static str> = COMMANDS
                    .iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .map(|c| c.name)
                    .collect();
                self.tab_complete(&"".to_string(), &prefix, &names, |def_idx| {
                    let name = names.get(def_idx).copied();
                    name.and_then(commands::find_command)
                        .map(|c| !c.subs.is_empty())
                        .unwrap_or(false)
                });
            }
            CompletionCtx::Sub { cmd, prefix } => {
                let names: Vec<&'static str> = cmd
                    .subs
                    .iter()
                    .filter(|(n, _)| n.starts_with(&prefix))
                    .map(|(n, _)| *n)
                    .collect();
                let head = build_sub_head(&current, cmd);
                self.tab_complete(&head, &prefix, &names, |_| false);
            }
        }
    }

    fn tab_complete<F: Fn(usize) -> bool>(
        &mut self,
        head: &str,
        prefix: &str,
        names: &[&'static str],
        should_append_space: F,
    ) {
        if names.is_empty() {
            return;
        }
        if names.len() == 1 {
            let mut text = format!("{}{}", head, names[0]);
            if should_append_space(0) {
                text.push(' ');
            }
            self.replace_input(&text);
            self.menu_idx = 0;
            self.tab_cycle = None;
            return;
        }
        let lcp = commands::longest_common_prefix(names);
        if lcp.len() > prefix.len() {
            let text = format!("{head}{lcp}");
            self.replace_input(&text);
            self.menu_idx = 0;
            return;
        }
        let pick = names[0].to_string();
        let text = format!("{head}{pick}");
        self.replace_input(&text);
        self.menu_idx = 0;
        self.tab_cycle = Some(TabCycle {
            names: names.iter().map(|s| s.to_string()).collect(),
            head: head.to_string(),
            idx: 0,
        });
    }

    fn submit(&mut self, text: String) {
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.follow_tail = true;
        self.scroll = 0;

        match commands::parse_action(&text) {
            Ok(action) => {
                self.replace_input("");
                self.send(Command::Run(action));
            }
            Err(err) => {
                self.replace_input("");
                self.send(Command::ShowSystem(err.message()));
            }
        }
    }

    fn run_hotkey(&mut self, action: Action) {
        self.replace_input("");
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.follow_tail = true;
        self.scroll = 0;
        self.send(Command::Run(action));
    }
}

fn build_sub_head(_input: &str, cmd: &CommandDef) -> String {
    format!("{} ", cmd.name)
}
