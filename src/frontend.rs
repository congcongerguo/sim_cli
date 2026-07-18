use std::cell::Cell;

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
    pub(crate) scroll: Cell<u16>,
    pub(crate) follow_tail: bool,
    pub(crate) menu_idx: usize,
    pub(crate) tab_cycle: Option<TabCycle>,
    #[allow(dead_code)]
    pub(crate) demo_idx: usize,
    pub(crate) modal_selected: usize,
    pub view: ViewState,
    pub(crate) panel_visible: bool,
    cmd_tx: mpsc::Sender<Command>,
    view_rx: watch::Receiver<ViewState>,
    history: Vec<String>,
    history_cursor: Option<usize>,
    pub(crate) viewport_height: Cell<u16>,
    pub(crate) prev_total_lines: Cell<u16>,
}

impl Frontend {
    pub fn new(cmd_tx: mpsc::Sender<Command>, view_rx: watch::Receiver<ViewState>) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(ratatui::style::Style::default());
        input.set_placeholder_text(PLACEHOLDER);
        let view = view_rx.borrow().clone();
        Self {
            input,
            scroll: Cell::new(0),
            follow_tail: true,
            menu_idx: 0,
            tab_cycle: None,
            demo_idx: 0,
            modal_selected: 0,
            view,
            panel_visible: true,
            cmd_tx,
            view_rx,
            history: Vec::new(),
            history_cursor: None,
            viewport_height: Cell::new(20),
            prev_total_lines: Cell::new(0),
        }
    }

    pub fn build_render_state(&self) -> crate::ui::render_state::RenderState {
        use crate::ui::render_state::RenderState;
        let menu = self.menu_items();
        RenderState {
            messages: self.view.messages.clone(),
            model: self.view.model.clone(),
            mode: self.view.mode,
            streaming: self.view.streaming,
            conn: self.view.conn.clone(),
            tasks: self.view.tasks.clone(),
            active_task_index: self.view.active_task_index,
            active_task: self.view.active_task.clone(),
            latest_recv: self.view.latest_recv.clone(),
            latest_recv_at: self.view.latest_recv_at,
            input_text: self.current_text(),
            input_cursor: (0, 0),
            input_state: self.input_state(),
            menu_items: menu.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            menu_idx: self.menu_idx,
            menu_title: self.menu_title(),
            scroll_offset: self.scroll.get(),
            follow_tail: self.follow_tail,
            prev_total_lines: self.prev_total_lines.get(),
            panel_visible: self.panel_visible,
            modal_request: self.view.modal.clone(),
            modal_selected: self.modal_selected,
        }
    }

    fn apply_output(&self, out: &crate::ui::render_state::RenderOutput) {
        self.viewport_height.set(out.viewport_height);
        self.prev_total_lines.set(out.total_lines);
    }

    pub async fn run<B: Backend>(&mut self, term: &mut Terminal<B>) -> Result<()> {
        let mut events = EventStream::new();
        loop {
            let state = self.build_render_state();
            let mut render_out = crate::ui::render_state::RenderOutput {
                viewport_height: self.viewport_height.get(),
                total_lines: self.prev_total_lines.get(),
            };
            term.draw(|f| {
                render_out = crate::ui::ratatui_renderer::RatatuiRenderer::draw(f, &state);
            })?;
            self.apply_output(&render_out);

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

    pub fn menu_items(&self) -> Vec<(String, String)> {
        let text = self.current_text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return vec![];
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let prefix = parts[0];
        // Filter active task's commands by prefix
        self.view.active_commands.iter()
            .filter(|c| c.name.starts_with(prefix))
            .map(|c| (c.name.to_string(), c.desc.to_string()))
            .collect()
    }

    pub fn menu_title(&self) -> Option<String> {
        let menu = self.menu_items();
        if menu.is_empty() { None } else { Some("commands".into()) }
    }

    pub fn input_state(&self) -> InputState {
        let current = self.current_text();
        if current.trim().is_empty() {
            return InputState::Empty;
        }
        let parts: Vec<&str> = current.trim().split_whitespace().collect();
        let matches: Vec<_> = self.view.active_commands.iter()
            .filter(|c| c.name.starts_with(parts[0]))
            .collect();
        match matches.len() {
            0 => InputState::Unknown,
            1 => InputState::Resolvable,
            _ => InputState::Ambiguous,
        }
    }

    fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    fn tab_next(&mut self) {
        if self.view.tasks.len() <= 1 { return; }
        let next = (self.view.active_task_index + 1) % self.view.tasks.len();
        let name = self.view.tasks[next].name.clone();
        self.send(Command::TagSwitch(name));
    }

    fn tab_prev(&mut self) {
        if self.view.tasks.len() <= 1 { return; }
        let prev = if self.view.active_task_index == 0 {
            self.view.tasks.len() - 1
        } else {
            self.view.active_task_index - 1
        };
        let name = self.view.tasks[prev].name.clone();
        self.send(Command::TagSwitch(name));
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.view.modal.is_some() {
            self.on_key_modal(key);
            return;
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.input.lines().iter().all(|l| l.is_empty()) {
                    self.send(Command::Input("exit".into()));
                } else {
                    self.replace_input("");
                    self.tab_cycle = None;
                    self.menu_idx = 0;
                }
                return;
            }
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                let step = self.viewport_height.get().max(1);
                self.scroll.set(self.scroll.get().saturating_add(step));
                self.follow_tail = false;
                return;
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                let step = self.viewport_height.get().max(1);
                self.scroll.set(self.scroll.get().saturating_sub(step));
                if self.scroll.get() == 0 {
                    self.follow_tail = true;
                }
                return;
            }
            (KeyCode::Home, _) => {
                self.scroll.set(u16::MAX);
                self.follow_tail = false;
                return;
            }
            (KeyCode::End, _) => {
                self.scroll.set(0);
                self.follow_tail = true;
                return;
            }
            (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                self.run_hotkey("help");
                return;
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.run_hotkey("clear");
                return;
            }
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                self.send(Command::Input("exit".into()));
                return;
            }
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                let next = if self.view.mode == Mode::Plan { "off" } else { "on" };
                self.run_hotkey(&format!("plan {next}"));
                return;
            }
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                let order = ["claude", "opus", "haiku"];
                let cur = &self.view.model;
                let i = order.iter().position(|m| m == &cur.as_str()).unwrap_or(0);
                let next = order[(i + 1) % order.len()];
                self.run_hotkey(&format!("model {next}"));
                return;
            }
            (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
                self.panel_visible = !self.panel_visible;
                return;
            }
            (KeyCode::Left, _) => {
                if self.input.lines().iter().all(|l| l.is_empty()) {
                    self.tab_prev();
                    return;
                }
            }
            (KeyCode::Right, _) => {
                if self.input.lines().iter().all(|l| l.is_empty()) {
                    self.tab_next();
                    return;
                }
            }
            (KeyCode::Tab, KeyModifiers::CONTROL) => {
                self.tab_next();
                return;
            }
            (KeyCode::BackTab, _) => {
                self.tab_prev();
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
            let menu_consumed = match key.code {
                KeyCode::Up => {
                    if self.menu_idx > 0 {
                        self.menu_idx -= 1;
                        true
                    } else {
                        false
                    }
                }
                KeyCode::Down => {
                    if self.menu_idx + 1 < menu.len() {
                        self.menu_idx += 1;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if menu_consumed {
                return;
            }
        }

        // History navigation (Up/Down not consumed by menu, or no menu)
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            match key.code {
                KeyCode::Up => {
                    if !self.history.is_empty() {
                        let pos = match self.history_cursor {
                            None => self.history.len().saturating_sub(1),
                            Some(0) => 0,
                            Some(n) => n.saturating_sub(1),
                        };
                        let text = self.history[pos].clone();
                        self.history_cursor = Some(pos);
                        self.tab_cycle = None;
                        self.replace_input(&text);
                    }
                    return;
                }
                KeyCode::Down => {
                    if let Some(pos) = self.history_cursor {
                        if pos + 1 < self.history.len() {
                            let text = self.history[pos + 1].clone();
                            self.history_cursor = Some(pos + 1);
                            self.tab_cycle = None;
                            self.replace_input(&text);
                        } else {
                            self.history_cursor = None;
                            self.tab_cycle = None;
                            self.replace_input("");
                        }
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
        self.history_cursor = None;
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
        let menu = self.menu_items();
        if menu.len() == 1 {
            let text = menu[0].0.clone();
            self.replace_input(&text);
            self.menu_idx = 0;
            self.tab_cycle = None;
        } else if menu.len() > 1 {
            if let Some(cycle) = self.tab_cycle.as_mut() {
                cycle.idx = (cycle.idx + 1) % menu.len();
                self.menu_idx = cycle.idx;
                let text = menu[cycle.idx].0.clone();
                drop(cycle);
                self.replace_input(&text);
            } else {
                self.tab_cycle = Some(TabCycle {
                    names: menu.iter().map(|(n, _)| n.clone()).collect(),
                    head: String::new(),
                    idx: 0,
                });
                let text = menu[0].0.clone();
                self.replace_input(&text);
            }
        }
    }

    fn submit(&mut self, text: String) {
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.follow_tail = true;
        self.scroll.set(0);
        self.history_cursor = None;

        if self.history.last().map_or(true, |last| last != &text) {
            self.history.push(text.clone());
            if self.history.len() > 1000 {
                self.history.remove(0);
            }
        }

        self.replace_input("");
        self.send(Command::Input(text));
    }

    fn run_hotkey(&mut self, text: &str) {
        self.replace_input("");
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.follow_tail = true;
        self.scroll.set(0);
        self.send(Command::Input(text.to_string()));
    }
}

// (removed unused build_sub_head)
