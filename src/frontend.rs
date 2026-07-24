use std::collections::HashMap;
use std::sync::Arc;

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

/// Frontend-only commands (handled locally, never sent to a tool). Listed in
/// the completion menu next to the active tool's commands so they're
/// discoverable via Tab.
const FRONTEND_CMDS: &[(&str, &str)] = &[
    ("filter", "show only matching lines (regex; && || ! ; e.g. filter /error/)"),
    ("unfilter", "clear the include filter"),
    ("exclude", "hide matching lines (regex; && || ! ; e.g. exclude /debug/)"),
    ("unexclude", "clear the exclude filter"),
];

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
    /// All conversation scroll-back state (position, follow-tail, unseen count,
    /// cached viewport) in one place. See [`crate::scroll::Scrollback`].
    pub(crate) scrollback: crate::scroll::Scrollback,
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
    /// Display-only include filters, keyed by tool (tab) name — each tab keeps
    /// its own. A message shows only if it matches. The log buffer and the
    /// message-log file are untouched.
    filters: HashMap<String, crate::filter::Filter>,
    /// Display-only exclude filters, per tab: a message matching this is hidden.
    /// Applied on top of the include filter.
    excludes: HashMap<String, crate::filter::Filter>,
    /// Error from the last rejected `filter` expression, per tab.
    filter_errors: HashMap<String, String>,
    /// Error from the last rejected `exclude` expression, per tab.
    exclude_errors: HashMap<String, String>,
}

impl Frontend {
    pub fn new(cmd_tx: mpsc::Sender<Command>, view_rx: watch::Receiver<ViewState>) -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(ratatui::style::Style::default());
        input.set_placeholder_text(PLACEHOLDER);
        let view = view_rx.borrow().clone();
        Self {
            input,
            scrollback: crate::scroll::Scrollback::default(),
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
            filters: HashMap::new(),
            excludes: HashMap::new(),
            filter_errors: HashMap::new(),
            exclude_errors: HashMap::new(),
        }
    }

    /// Name of the currently active tool (tab), used to key per-tab state.
    fn current_tool(&self) -> Option<String> {
        self.view.tools.get(self.view.active_index).map(|t| t.name.clone())
    }

    /// The messages to show plus the scroll geometry for them, after applying
    /// the display filter. Without a filter this is the raw view; with one, the
    /// matching messages form a self-contained view (no evicted prefix), so the
    /// existing scroll machinery works unchanged on the filtered set.
    fn effective_view(&self) -> (Arc<Vec<crate::message::TimedMessage>>, u64, u64) {
        let tool = self.current_tool();
        let include = tool.as_ref().and_then(|n| self.filters.get(n));
        let exclude = tool.as_ref().and_then(|n| self.excludes.get(n));
        if include.is_none() && exclude.is_none() {
            return (
                self.view.messages.clone(),
                self.view.buffer_total_lines,
                self.view.evicted_lines,
            );
        }
        let filtered: Vec<crate::message::TimedMessage> = self
            .view
            .messages
            .iter()
            .filter(|tm| {
                // Show if it matches the include filter (or there is none) and
                // does NOT match the exclude filter.
                include.is_none_or(|f| f.matches_msg(&tm.msg))
                    && !exclude.is_some_and(|f| f.matches_msg(&tm.msg))
            })
            .cloned()
            .collect();
        let total: u64 = filtered
            .iter()
            .map(|tm| crate::log_buffer::msg_line_count(&tm.msg))
            .sum();
        (Arc::new(filtered), total, 0)
    }

    /// Handle the display-only view commands (`filter`/`unfilter` — show only
    /// matches; `exclude`/`unexclude` — hide matches) locally, never sent to the
    /// tool. Applies to the active tab only. Returns `true` if `text` was one.
    fn try_filter_command(&mut self, text: &str) -> bool {
        let trimmed = text.trim();
        let (cmd, rest) = match trimmed.split_once(char::is_whitespace) {
            Some((c, r)) => (c, r.trim()),
            None => (trimmed, ""),
        };
        if !matches!(cmd, "filter" | "unfilter" | "exclude" | "unexclude") {
            return false;
        }
        // View filters are per-tab; without an active tab there's nothing to key on.
        let tool = match self.current_tool() {
            Some(t) => t,
            None => return true,
        };
        let (map, errs) = match cmd {
            "filter" | "unfilter" => (&mut self.filters, &mut self.filter_errors),
            _ => (&mut self.excludes, &mut self.exclude_errors),
        };
        let clear = matches!(cmd, "unfilter" | "unexclude") || rest.is_empty();
        if clear {
            map.remove(&tool);
            errs.remove(&tool);
        } else {
            match crate::filter::Filter::parse(rest) {
                Ok(f) => {
                    map.insert(tool.clone(), f);
                    errs.remove(&tool);
                }
                Err(e) => {
                    // Keep the previous expression; just report the error.
                    errs.insert(tool, e);
                    return true;
                }
            }
        }
        // A view-filter change re-bases the scroll coordinate; jump to the tail.
        self.scrollback = crate::scroll::Scrollback::default();
        true
    }

    pub fn build_render_state(&self) -> crate::ui::render_state::RenderState {
        use crate::ui::render_state::RenderState;
        let menu = self.menu_items();
        let (messages, buffer_total_lines, evicted_lines) = self.effective_view();
        let filter_active = self.current_tool().is_some_and(|t| {
            self.filters.contains_key(&t) || self.excludes.contains_key(&t)
        });
        let filter_counts =
            filter_active.then(|| (messages.len(), self.view.messages.len()));
        RenderState {
            messages,
            streaming: self.view.streaming,
            state: self.view.state.clone(),
            tools: self.view.tools.clone(),
            active_index: self.view.active_index,
            input_text: self.current_text(),
            input_cursor: (0, 0),
            input_state: self.input_state(),
            menu_items: menu.into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            menu_idx: self.menu_idx,
            menu_title: self.menu_title(),
            scroll_offset: self.scrollback.offset(),
            follow_tail: self.scrollback.follow_tail(),
            unseen_lines: self.scrollback.unseen(),
            evicted_lines,
            buffer_total_lines,
            panel_visible: self.panel_visible,
            modal_request: self.view.modal.clone(),
            modal_selected: self.modal_selected,
            filter: self.current_tool()
                .and_then(|t| self.filters.get(&t))
                .map(|f| f.src().to_string()),
            filter_error: self.current_tool().and_then(|t| self.filter_errors.get(&t).cloned()),
            exclude: self.current_tool()
                .and_then(|t| self.excludes.get(&t))
                .map(|f| f.src().to_string()),
            exclude_error: self.current_tool().and_then(|t| self.exclude_errors.get(&t).cloned()),
            filter_counts,
        }
    }

    /// 每帧渲染后：记录视口尺寸并刷新未读行数(驱动 "▼ N new" 提示)。
    fn apply_output(&mut self, out: &crate::ui::render_state::RenderOutput) {
        self.scrollback.on_frame(out.viewport_height, out.total_lines as u64);
    }

    pub async fn run<B: Backend>(&mut self, term: &mut Terminal<B>) -> Result<()> {
        let mut events = EventStream::new();
        // Windows ConPTY can drop the first frame drawn right after entering the
        // alternate screen. ratatui only repaints cells that changed since the
        // previous frame, so a static region like the tab bar would then stay
        // blank until something forced a repaint (switching tabs with ←/→, or a
        // resize). Reset the diff baseline for the first couple of frames so
        // they fully repaint once the screen is initialized. `clear()` is
        // immediately followed by `draw()`, so there is no blank gap.
        let mut warmup_repaints = 2u8;
        loop {
            let state = self.build_render_state();
            if warmup_repaints > 0 {
                term.clear()?;
                warmup_repaints -= 1;
            }
            // Seed values are overwritten by the draw closure below; only the
            // viewport matters if the closure somehow doesn't run.
            let mut render_out = crate::ui::render_state::RenderOutput {
                viewport_height: self.scrollback.viewport(),
                total_lines: 0,
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
        let exact_match = self.view.active_cmds.iter().any(|c| c.name == first && !c.subs.is_empty());
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
                let mut items: Vec<(String, String)> = self.view.active_cmds.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .map(|c| (c.name.to_string(), c.desc.to_string()))
                    .collect();
                items.extend(
                    FRONTEND_CMDS.iter()
                        .filter(|(name, _)| name.starts_with(&prefix))
                        .map(|(name, desc)| (name.to_string(), desc.to_string())),
                );
                items
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_cmds.iter() {
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

    /// Full input text for the menu item currently highlighted by `menu_idx`,
    /// including the `"cmd "` head for sub-command completions. Returns `None`
    /// when no completion menu is open.
    fn highlighted_menu_text(&self) -> Option<String> {
        let menu = self.menu_items();
        if menu.is_empty() {
            return None;
        }
        let idx = self.menu_idx.min(menu.len() - 1);
        let head = match self.completion_ctx() {
            CompletionCtx::Sub { cmd_name, .. } => format!("{cmd_name} "),
            _ => String::new(),
        };
        Some(format!("{head}{}", menu[idx].0))
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
                let count = self.view.active_cmds.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .count()
                    + FRONTEND_CMDS.iter()
                        .filter(|(name, _)| name.starts_with(&prefix))
                        .count();
                match count {
                    0 => InputState::Unknown,
                    1 => InputState::Resolvable,
                    _ => InputState::Ambiguous,
                }
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_cmds.iter() {
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

    fn send(&self, cmd: Command) {
        let _ = self.cmd_tx.try_send(cmd);
    }

    fn tab_next(&mut self) {
        if self.view.tools.len() <= 1 { return; }
        let next = (self.view.active_index + 1) % self.view.tools.len();
        let name = self.view.tools[next].name.clone();
        self.send(Command::TagSwitch(name));
    }

    fn tab_prev(&mut self) {
        if self.view.tools.len() <= 1 { return; }
        let prev = if self.view.active_index == 0 {
            self.view.tools.len() - 1
        } else {
            self.view.active_index - 1
        };
        let name = self.view.tools[prev].name.clone();
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
                let (_, total, evicted) = self.effective_view();
                self.scrollback.page_up(total, evicted);
                return;
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                let (_, total, evicted) = self.effective_view();
                self.scrollback.page_down(total, evicted);
                return;
            }
            (KeyCode::Home, _) => {
                let (_, total, evicted) = self.effective_view();
                self.scrollback.home(total, evicted);
                return;
            }
            (KeyCode::End, _) => {
                self.scrollback.end();
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
                        // Drop any Tab cycle so the next Tab resumes from here.
                        self.tab_cycle = None;
                        true
                    } else {
                        false
                    }
                }
                KeyCode::Down => {
                    if self.menu_idx + 1 < menu.len() {
                        self.menu_idx += 1;
                        self.tab_cycle = None;
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
            if self.view.streaming {
                return;
            }
            // If the completion menu is open, Enter commits the item currently
            // highlighted via ↑↓, not just whatever prefix was typed. Without this
            // an ambiguous input would fall through to handle_tab(), which resets
            // the selection to the first item and ignores menu_idx.
            if let Some(sel) = self.highlighted_menu_text() {
                self.replace_input(&sel);
                self.tab_cycle = None;
            }
            let text = self.current_text().trim().to_string();
            if text.is_empty() {
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
                // Begin cycling from the item currently highlighted by ↑↓ rather
                // than always from the first, so Tab agrees with arrow selection.
                let start = self.menu_idx.min(menu.len() - 1);
                self.tab_cycle = Some(TabCycle { idx: start });
                start
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
                let matches: Vec<_> = self.view.active_cmds.iter()
                    .filter(|c| c.name.starts_with(&prefix))
                    .collect();
                if matches.len() == 1 && matches[0].name != prefix {
                    return matches[0].name.to_string();
                }
            }
            CompletionCtx::Sub { cmd_name, prefix } => {
                for c in self.view.active_cmds.iter() {
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

        // Display-only filter commands are handled locally, never sent to the
        // tool. Still recorded in history for convenience.
        if self.try_filter_command(&text) {
            self.push_history(text);
            self.replace_input("");
            return;
        }

        // Ambiguous → auto-complete instead of submitting
        if matches!(self.input_state(), InputState::Ambiguous) {
            self.handle_tab();
            return;
        }

        // Expand partial sub-command prefix before sending
        let text = self.expand_text(text);

        self.push_history(text.clone());
        self.replace_input("");
        self.send(Command::Input(text));
    }

    fn push_history(&mut self, text: String) {
        if self.history.last().map_or(true, |last| last != &text) {
            self.history.push(text);
            if self.history.len() > 1000 {
                self.history.remove(0);
            }
        }
    }

    fn run_hotkey(&mut self, text: &str) {
        self.replace_input("");
        self.tab_cycle = None;
        self.menu_idx = 0;
        self.send(Command::Input(text.to_string()));
    }
}

// (removed unused build_sub_head)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{Cmd, Sub};

    const SUBS: &[Sub] = &[
        Sub { name: "alpha", desc: "" },
        Sub { name: "beta", desc: "" },
        Sub { name: "gamma", desc: "" },
    ];
    const CMDS: &[Cmd] = &[Cmd { name: "con", desc: "", subs: SUBS }];

    fn frontend_with_cmds() -> (Frontend, mpsc::Receiver<Command>) {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (_view_tx, view_rx) = watch::channel(ViewState::initial());
        let mut fe = Frontend::new(cmd_tx, view_rx);
        fe.view.active_cmds = Arc::new(CMDS.to_vec());
        (fe, cmd_rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn recv_input(rx: &mut mpsc::Receiver<Command>) -> String {
        match rx.try_recv() {
            Ok(Command::Input(text)) => text,
            other => panic!("expected Command::Input, got {other:?}"),
        }
    }

    fn sys(text: &str) -> crate::message::TimedMessage {
        crate::message::TimedMessage {
            time: chrono::Local::now(),
            msg: crate::message::Message::System {
                text: text.into(),
                level: crate::message::LogLevel::Info,
            },
        }
    }

    #[test]
    fn filter_is_discoverable_in_completion_menu() {
        let (mut fe, _rx) = frontend_with_cmds();
        fe.replace_input("fil");
        let menu = fe.menu_items();
        assert!(
            menu.iter().any(|(name, _)| name == "filter"),
            "filter should appear in the completion menu, got {menu:?}",
        );
        // A unique prefix resolves (green), so Tab completes it.
        assert_eq!(fe.input_state(), InputState::Resolvable);

        fe.replace_input("unf");
        assert!(fe.menu_items().iter().any(|(name, _)| name == "unfilter"));
    }

    #[test]
    fn filter_command_hides_non_matching_messages() {
        let (mut fe, mut cmd_rx) = frontend_with_cmds();
        fe.view.messages = Arc::new(vec![
            sys("error: disk full"),
            sys("info: all good"),
            sys("error: timeout"),
        ]);
        fe.view.buffer_total_lines = 3;
        fe.view.evicted_lines = 0;

        // No filter → all three, raw geometry preserved.
        assert_eq!(fe.effective_view().0.len(), 3);

        // Regex filter → display-only, nothing sent to the tool.
        assert!(fe.try_filter_command("filter /error/"));
        let (msgs, total, evicted) = fe.effective_view();
        assert_eq!(msgs.len(), 2);
        assert_eq!(total, 2);
        assert_eq!(evicted, 0);

        // Boolean AND narrows further.
        assert!(fe.try_filter_command("filter /error/ && /timeout/"));
        assert_eq!(fe.effective_view().0.len(), 1);

        // Clearing restores everything.
        assert!(fe.try_filter_command("filter"));
        assert_eq!(fe.effective_view().0.len(), 3);

        // A bad expression is still "handled" but records an error and keeps
        // the (now cleared) filter unchanged.
        assert!(fe.try_filter_command("filter /("));
        assert!(!fe.filter_errors.is_empty());
        assert_eq!(fe.effective_view().0.len(), 3);

        // Non-filter input is not consumed here.
        assert!(!fe.try_filter_command("start"));

        // The filter path never emits a tool command.
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn exclude_hides_matches_and_composes_with_filter() {
        let (mut fe, _rx) = frontend_with_cmds();
        fe.view.messages = Arc::new(vec![
            sys("request /api ok"),
            sys("request /health ok"),
            sys("debug noise"),
            sys("request /api error"),
        ]);
        fe.view.buffer_total_lines = 4;

        // exclude hides matching lines.
        assert!(fe.try_filter_command("exclude /debug/"));
        assert_eq!(fe.effective_view().0.len(), 3);

        // exclude supports AND/OR like filter.
        assert!(fe.try_filter_command("exclude /health/ || /debug/"));
        assert_eq!(fe.effective_view().0.len(), 2, "health and debug hidden");

        // include + exclude compose: show requests, but not health.
        assert!(fe.try_filter_command("filter /request/"));
        assert!(fe.try_filter_command("exclude /health/"));
        let (msgs, _, _) = fe.effective_view();
        assert_eq!(msgs.len(), 2, "two /api request lines remain");

        // Status line reflects both, plus the shown/total count.
        let rs = fe.build_render_state();
        assert_eq!(rs.filter.as_deref(), Some("/request/"));
        assert_eq!(rs.exclude.as_deref(), Some("/health/"));
        assert_eq!(rs.filter_counts, Some((2, 4)));

        // Clearing exclude leaves the include filter in place.
        assert!(fe.try_filter_command("unexclude"));
        assert_eq!(fe.effective_view().0.len(), 3, "3 request lines");
        assert!(fe.build_render_state().exclude.is_none());
    }

    #[test]
    fn filters_are_per_tab() {
        let (mut fe, _rx) = frontend_with_cmds();
        // Initial view has at least two tabs (conn, demo); active is index 0.
        assert!(fe.view.tools.len() >= 2, "need two tabs for this test");

        // Filter the first tab.
        fe.view.messages = Arc::new(vec![sys("error x"), sys("ok y")]);
        fe.view.buffer_total_lines = 2;
        assert!(fe.try_filter_command("filter /error/"));
        assert_eq!(fe.effective_view().0.len(), 1, "tab 0 is filtered");

        // Switch to the second tab: it has no filter of its own.
        fe.view.active_index = 1;
        fe.view.messages = Arc::new(vec![sys("error a"), sys("ok b"), sys("ok c")]);
        fe.view.buffer_total_lines = 3;
        assert_eq!(fe.effective_view().0.len(), 3, "tab 1 is unfiltered");

        // The status line reflects the active tab's filter (none here).
        assert!(fe.build_render_state().filter.is_none());

        // Back to tab 0: its filter is still in effect.
        fe.view.active_index = 0;
        fe.view.messages = Arc::new(vec![sys("error x"), sys("ok y")]);
        assert_eq!(fe.effective_view().0.len(), 1, "tab 0 filter persisted");
        let rs = fe.build_render_state();
        assert_eq!(rs.filter.as_deref(), Some("/error/"));
        // Status line shows shown/total for the active tab.
        assert_eq!(rs.filter_counts, Some((1, 2)));
    }

    /// ↓ to the second sub-command then Enter must run that second item,
    /// not fall back to the first one.
    #[test]
    fn enter_runs_arrow_selected_sub() {
        let (mut fe, mut cmd_rx) = frontend_with_cmds();
        fe.replace_input("con ");

        fe.on_key(key(KeyCode::Down)); // highlight "beta"
        assert_eq!(fe.menu_idx, 1);

        fe.on_key(key(KeyCode::Enter));
        assert_eq!(recv_input(&mut cmd_rx), "con beta");
    }

    /// Two ↓ presses land on the third item; Enter runs it.
    #[test]
    fn enter_runs_third_sub() {
        let (mut fe, mut cmd_rx) = frontend_with_cmds();
        fe.replace_input("con ");

        fe.on_key(key(KeyCode::Down));
        fe.on_key(key(KeyCode::Down));
        assert_eq!(fe.menu_idx, 2);

        fe.on_key(key(KeyCode::Enter));
        assert_eq!(recv_input(&mut cmd_rx), "con gamma");
    }

    /// Without touching the arrows, Enter still picks the first item.
    #[test]
    fn enter_default_runs_first_sub() {
        let (mut fe, mut cmd_rx) = frontend_with_cmds();
        fe.replace_input("con ");

        fe.on_key(key(KeyCode::Enter));
        assert_eq!(recv_input(&mut cmd_rx), "con alpha");
    }

    /// Tab resumes cycling from the arrow-highlighted item.
    #[test]
    fn tab_starts_from_arrow_selection() {
        let (mut fe, _cmd_rx) = frontend_with_cmds();
        fe.replace_input("con ");

        fe.on_key(key(KeyCode::Down)); // highlight "beta" (idx 1)
        fe.on_key(key(KeyCode::Tab)); // should fill "con beta"
        assert_eq!(fe.current_text(), "con beta");
        assert_eq!(fe.menu_idx, 1);
    }
}
