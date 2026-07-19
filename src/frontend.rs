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
    pub idx: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputState {
    Empty,
    Resolvable,
    Ambiguous,
    Unknown,
}

/// Completion context: are we matching a command prefix or a sub-command?
enum CompletionCtx {
    Command { prefix: String },
    Sub { cmd_name: String, prefix: String },
    None,
}

pub struct Frontend {
    pub input: TextArea<'static>,
    pub(crate) scroll: Cell<u64>,
    pub(crate) follow_tail: Cell<bool>,
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
    pub(crate) prev_total_lines: Cell<u32>,
    /// Lines added since user scrolled up (0 = following or content fits).
    unseen_lines: Cell<u32>,
    /// Total lines the last time we were in follow mode.
    total_at_follow: Cell<u32>,
}

impl Frontend {
    pub fn new(cmd_tx: mpsc::Sender<Command>, view_rx: watch::Receiver<ViewState>) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(ratatui::style::Style::default());
        input.set_placeholder_text(PLACEHOLDER);
        let view = view_rx.borrow().clone();
        Self {
            input,
            scroll: Cell::new(0u64),
            follow_tail: Cell::new(true),
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
            prev_total_lines: Cell::new(0u32),
            unseen_lines: Cell::new(0),
            total_at_follow: Cell::new(0),
        }
    }

    pub fn build_render_state(&self) -> crate::ui::render_state::RenderState {
        use crate::ui::render_state::RenderState;
        let menu = self.menu_items();
        RenderState {
            messages: self.view.messages.clone(),
            streaming: self.view.streaming,
            internal: self.view.internal.clone(),
            tasks: self.view.tasks.clone(),
            active_task_index: self.view.active_task_index,
            latest_recv: self.view.latest_recv.clone(),
            latest_recv_at: self.view.latest_recv_at,
            input_text: self.current_text(),
            input_cursor: (0, 0),
            input_state: self.input_state(),
            menu_items: menu.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            menu_idx: self.menu_idx,
            menu_title: self.menu_title(),
            scroll_offset: self.scroll.get(),
            follow_tail: self.follow_tail.get(),
            prev_total_lines: self.prev_total_lines.get(),
            unseen_lines: self.unseen_lines.get(),
            evicted_lines: self.view.evicted_lines,
            buffer_total_lines: self.view.buffer_total_lines,
            panel_visible: self.panel_visible,
            modal_request: self.view.modal.clone(),
            modal_selected: self.modal_selected,
        }
    }

    /// 每帧渲染后：更新视口大小、计算未读行数。
    ///
    /// 当 follow_tail 为 false（用户翻上去了），unseen_lines 统计
    /// 从离开跟尾模式以来新增的行数，驱动 "▼ N new" 提示。
    fn apply_output(&self, out: &crate::ui::render_state::RenderOutput) {
        let tl = out.total_lines as u32;
        if self.follow_tail.get() {
            self.unseen_lines.set(0);
            self.total_at_follow.set(tl);
        } else {
            let unseen = tl.saturating_sub(self.total_at_follow.get());
            self.unseen_lines.set(unseen);
        }
        self.viewport_height.set(out.viewport_height);
        self.prev_total_lines.set(out.total_lines as u32);
    }

    pub async fn run<B: Backend>(&mut self, term: &mut Terminal<B>) -> Result<()> {
        let mut events = EventStream::new();
        loop {
            let state = self.build_render_state();
            let mut render_out = crate::ui::render_state::RenderOutput {
                viewport_height: self.viewport_height.get(),
                total_lines: self.prev_total_lines.get() as u16,
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

    fn completion_ctx(&self) -> CompletionCtx {
        let text = self.current_text();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return CompletionCtx::None;
        }
        // Check if there's a trailing space → sub-command context
        let has_trailing_space = text.ends_with(char::is_whitespace);
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let first = parts[0].to_string();

        if parts.len() == 1 && !has_trailing_space {
            // "co" → command prefix
            return CompletionCtx::Command { prefix: first };
        }

        // "con " or "con t" → may be sub-context
        let exact_match = self.view.active_commands.iter().any(|c| c.name == first && !c.subs.is_empty());
        if exact_match {
            let sub_prefix = if parts.len() >= 2 { parts[1].to_string() } else { String::new() };
            return CompletionCtx::Sub { cmd_name: first, prefix: sub_prefix };
        }

        // Still matching command prefix (e.g., "c " or "co " after a partial)
        if parts.len() == 1 && has_trailing_space {
            return CompletionCtx::Command { prefix: first };
        }

        CompletionCtx::None
    }

    pub fn menu_items(&self) -> Vec<(String, String)> {
        match self.completion_ctx() {
            CompletionCtx::Command { prefix } => {
                self.view.active_commands.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .map(|c| (c.name.to_string(), c.desc.to_string()))
                    .collect()
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_commands.iter() {
                    if c.name == cmd_name {
                        return c.subs.iter()
                            .filter(|s| s.name.starts_with(&prefix))
                            .map(|s| (s.name.to_string(), s.desc.to_string()))
                            .collect();
                    }
                }
                vec![]
            }
            CompletionCtx::None => vec![],
        }
    }

    pub fn menu_title(&self) -> Option<String> {
        match self.completion_ctx() {
            CompletionCtx::Sub { cmd_name, .. } => Some(format!("{cmd_name} <arg>")),
            CompletionCtx::Command { .. } => {
                if self.menu_items().is_empty() { None } else { Some("commands".into()) }
            }
            CompletionCtx::None => None,
        }
    }

    pub fn input_state(&self) -> InputState {
        let current = self.current_text();
        if current.trim().is_empty() {
            return InputState::Empty;
        }
        match self.completion_ctx() {
            CompletionCtx::Command { prefix } => {
                let matches: Vec<_> = self.view.active_commands.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .collect();
                match matches.len() {
                    0 => InputState::Unknown,
                    1 => InputState::Resolvable,
                    _ => InputState::Ambiguous,
                }
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_commands.iter() {
                    if c.name == cmd_name {
                        let matches: Vec<_> = c.subs.iter()
                            .filter(|s| s.name.starts_with(&prefix))
                            .collect();
                        return match matches.len() {
                            0 => InputState::Unknown,
                            1 => InputState::Resolvable,
                            _ => InputState::Ambiguous,
                        };
                    }
                }
                InputState::Unknown
            }
            CompletionCtx::None => InputState::Unknown,
        }
    }

    /// 从当前 ViewState 快照构建 ScrollInput。
    /// 每次 PageUp/Down/Home/End 按键时调用。
    fn scroll_input(&self) -> crate::scroll::ScrollInput {
        crate::scroll::ScrollInput {
            viewport: self.viewport_height.get(),
            total_lines: self.view.buffer_total_lines,
            evicted_lines: self.view.evicted_lines,
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
                let input = self.scroll_input();
                let state = crate::scroll::ScrollState {
                    offset: self.scroll.get(),
                    follow_tail: self.follow_tail.get(),
                };
                let result = crate::scroll::page_up(&state, &input);
                self.scroll.set(result.offset);
                self.follow_tail.set(result.follow_tail);
                return;
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                let input = self.scroll_input();
                let state = crate::scroll::ScrollState {
                    offset: self.scroll.get(),
                    follow_tail: self.follow_tail.get(),
                };
                let result = crate::scroll::page_down(&state, &input);
                self.scroll.set(result.offset);
                self.follow_tail.set(result.follow_tail);
                return;
            }
            (KeyCode::Home, _) => {
                let input = self.scroll_input();
                let result = crate::scroll::home(&input);
                self.scroll.set(result.offset);
                self.follow_tail.set(result.follow_tail);
                return;
            }
            (KeyCode::End, _) => {
                let result = crate::scroll::end();
                self.scroll.set(result.offset);
                self.follow_tail.set(result.follow_tail);
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
                InputState::Ambiguous => self.handle_tab(),
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
        if menu.is_empty() {
            return;
        }

        // For sub-completions, preserve the command prefix ("con ") in the input.
        let head = match self.completion_ctx() {
            CompletionCtx::Sub { cmd_name, .. } => format!("{cmd_name} "),
            _ => String::new(),
        };

        if menu.len() == 1 {
            let text = format!("{head}{}", menu[0].0);
            self.replace_input(&text);
            self.menu_idx = 0;
            self.tab_cycle = None;
        } else {
            let new_idx = if let Some(cycle) = &mut self.tab_cycle {
                cycle.idx = (cycle.idx + 1) % menu.len();
                cycle.idx
            } else {
                self.tab_cycle = Some(TabCycle { idx: 0 });
                0
            };
            self.menu_idx = new_idx;
            let text = format!("{head}{}", menu[new_idx].0);
            self.replace_input(&text);
        }
    }

    /// Expand partial command/sub prefix to full name before sending.
    fn expand_text(&self, text: String) -> String {
        match self.completion_ctx() {
            CompletionCtx::Command { prefix } => {
                let matches: Vec<_> = self.view.active_commands.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .collect();
                if matches.len() == 1 && matches[0].name != prefix {
                    return matches[0].name.to_string();
                }
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_commands.iter() {
                    if c.name == cmd_name {
                        let matches: Vec<_> = c.subs.iter()
                            .filter(|s| s.name.starts_with(&prefix))
                            .collect();
                        if matches.len() == 1 && matches[0].name != prefix {
                            return format!("{cmd_name} {}", matches[0].name);
                        }
                    }
                }
            }
            CompletionCtx::None => {}
        }
        text
    }

    fn submit(&mut self, text: String) {
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.history_cursor = None;

        // Ambiguous → auto-complete instead of submitting
        if matches!(self.input_state(), InputState::Ambiguous) {
            self.handle_tab();
            return;
        }

        // Expand partial sub-command prefix before sending
        let text = self.expand_text(text);

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
        self.send(Command::Input(text.to_string()));
    }
}

// (removed unused build_sub_head)
