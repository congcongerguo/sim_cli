//! Scroll state machine — pure functions for PageUp/Down/Home/End.
//!
//! # Coordinate system
//!
//! `offset` is an **absolute** line number counting from the very first
//! message ever pushed.  Messages that have been evicted from the buffer
//! still occupy their original absolute coordinates — the visible window
//! starts at `evicted_lines`.
//!
//! ```text
//!   absolute line 0 ─── [evicted] ─── [buffer content] ─── [bottom]
//!                      ╰── gone ──╯  ╰── total_lines ──╯
//! ```
//!
//! The renderer converts `offset` to a buffer-relative position by
//! subtracting `evicted_lines`.  This keeps the view anchored when old
//! messages are evicted: the offset stays the same, the subtraction
//! automatically shifts to the new buffer window.
//!
//! # Data flow
//!
//! ```text
//!   LogBuffer (incremental) → TaskSnapshot → ViewState → ScrollInput
//!          total_lines ──────────┤                │
//!          evicted_lines ────────┘                │
//!                                                 ↓
//!                                     frontend::scroll_input()
//!                                                 ↓
//!                               scroll::page_up / page_down / home / end
//!                                                 ↓
//!                               ScrollState { offset, follow_tail }
//!                                                 ↓
//!                               RenderState::scroll_offset ──→ conversation.rs
//!                                                 │
//!                               adjusted = offset - evicted_lines
//!                                                 │
//!                               ratatui Paragraph::scroll(adjusted)
//! ```

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    /// Absolute line number (from the first message ever).
    /// Converted to buffer-relative in conversation.rs via `offset - evicted_lines`.
    pub offset: u32,
    /// When true, the view tracks the bottom of the buffer automatically.
    pub follow_tail: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollInput {
    /// Number of visible lines in the conversation area (viewport_height).
    pub viewport: u16,
    /// Total render lines in the buffer (LogBuffer::total_lines).
    pub total_lines: u64,
    /// Cumulative evicted render lines (LogBuffer::evicted_lines).
    pub evicted_lines: u64,
}

impl ScrollInput {
    fn viewport_u32(&self) -> u32 { self.viewport.max(1) as u32 }
    fn total_u32(&self) -> u32 { self.total_lines as u32 }
    fn evicted_u32(&self) -> u32 { self.evicted_lines as u32 }
    fn max_scroll(&self) -> u32 {
        self.total_u32().saturating_sub(self.viewport_u32())
    }
    fn step(&self) -> u32 { self.viewport_u32() }
    fn bottom_abs(&self) -> u32 {
        self.evicted_u32().saturating_add(self.max_scroll())
    }
}

/// PageUp / Ctrl+B: scroll one page up.
pub fn page_up(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    let cur = if state.follow_tail {
        input.bottom_abs().saturating_sub(input.step())
    } else {
        state.offset.saturating_sub(input.step())
    };
    ScrollState { offset: cur, follow_tail: false }
}

/// PageDown / Ctrl+F: scroll one page down. Returns to follow mode at bottom.
pub fn page_down(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    if state.follow_tail {
        return *state;
    }
    let cur = state.offset.saturating_add(input.step());
    // Add 1-step slack so a tick arriving between key press and render
    // doesn't keep us one line short of the bottom.
    if cur + input.step() >= input.bottom_abs() {
        ScrollState { offset: 0, follow_tail: true }
    } else {
        ScrollState { offset: cur, follow_tail: false }
    }
}

/// Home: jump to the top of the current buffer window.
pub fn home(input: &ScrollInput) -> ScrollState {
    ScrollState { offset: input.evicted_u32(), follow_tail: false }
}

/// End: jump to the bottom and resume follow mode.
pub fn end() -> ScrollState {
    ScrollState { offset: 0, follow_tail: true }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: a full buffer of 100 lines, 20-line viewport, 500 lines evicted.
    fn full_buf() -> ScrollInput {
        ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 500 }
    }

    fn at_bottom() -> ScrollState {
        ScrollState { offset: 0, follow_tail: true }
    }

    fn detached(offset: u32) -> ScrollState {
        ScrollState { offset, follow_tail: false }
    }

    #[test]
    fn page_up_from_bottom_detaches_one_page_up() {
        let input = full_buf();
        let result = page_up(&at_bottom(), &input);
        assert!(!result.follow_tail);
        // bottom_abs = 500+80=580, step=20, cur=560. Render: 560-500=60.
        assert_eq!(result.offset, 560);
    }

    #[test]
    fn page_up_when_detached_goes_further_up() {
        let input = full_buf();
        let s = page_up(&detached(560), &input);
        assert_eq!(s.offset, 540);
    }

    #[test]
    fn page_up_cannot_go_below_zero() {
        let input = full_buf();
        let s = page_up(&detached(5), &input);
        assert_eq!(s.offset, 0); // saturating_sub
    }

    #[test]
    fn page_down_from_bottom_does_nothing() {
        let input = full_buf();
        let result = page_down(&at_bottom(), &input);
        assert_eq!(result, at_bottom());
    }

    #[test]
    fn page_down_from_detached_moves_down() {
        let input = full_buf();
        // offset=520, step=20 → cur=540, 540+20=560 < 580 → still detached
        let s = page_down(&detached(520), &input);
        assert!(!s.follow_tail);
        assert_eq!(s.offset, 540);
    }

    #[test]
    fn page_down_reaches_bottom_resumes_follow() {
        let input = full_buf();
        // bottom_abs=580, slack: cur+20>=580 → cur>=560
        let s = page_down(&detached(560), &input);
        assert!(s.follow_tail);
        assert_eq!(s.offset, 0);
    }

    #[test]
    fn page_down_with_slack_still_reaches_bottom() {
        let input = full_buf();
        // cur=540, cur+20=560, 560+20=580 >= 580 → follow
        let s = page_down(&detached(540), &input);
        assert!(s.follow_tail);
    }

    #[test]
    fn home_jumps_to_top_of_buffer() {
        let input = full_buf();
        let s = home(&input);
        assert!(!s.follow_tail);
        assert_eq!(s.offset, 500); // evicted_lines
    }

    #[test]
    fn end_jumps_to_bottom() {
        assert_eq!(end(), ScrollState { offset: 0, follow_tail: true });
    }

    #[test]
    fn small_buffer_fits_on_screen() {
        let input = ScrollInput { viewport: 20, total_lines: 5, evicted_lines: 0 };
        // max_scroll = 0 (5 < 20)
        assert_eq!(input.max_scroll(), 0);
        assert_eq!(input.bottom_abs(), 0);
        // PageUp from bottom: bottom_abs(0)-20=0
        let s = page_up(&at_bottom(), &input);
        assert_eq!(s.offset, 0);
    }

    #[test]
    fn empty_buffer() {
        let input = ScrollInput { viewport: 20, total_lines: 0, evicted_lines: 0 };
        assert_eq!(input.max_scroll(), 0);
        let s = home(&input);
        assert_eq!(s.offset, 0);
        assert!(!s.follow_tail);
    }

    #[test]
    fn eviction_in_progress_still_scrolls() {
        // Buffer full, 1000 lines evicted, user at top
        let input = ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 1000 };
        let s = home(&input);
        assert_eq!(s.offset, 1000);
        // PageDown from top
        let s = page_down(&s, &input);
        assert_eq!(s.offset, 1020);
        assert!(!s.follow_tail);
    }

    #[test]
    fn viewport_zero_clamps_to_one() {
        let input = ScrollInput { viewport: 0, total_lines: 100, evicted_lines: 0 };
        assert_eq!(input.viewport_u32(), 1);
        assert_eq!(input.max_scroll(), 99);
    }
}
