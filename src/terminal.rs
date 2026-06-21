use std::io::{Stdout, stdout};
use std::panic;

use anyhow::Result;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub struct TerminalGuard;

pub fn install() -> Result<TerminalGuard> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;

    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    Ok(TerminalGuard)
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

pub fn new_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    let backend = CrosstermBackend::new(stdout());
    Ok(Terminal::new(backend)?)
}
