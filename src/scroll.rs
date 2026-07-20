//! 翻滚状态机 —— PageUp/Down/Home/End 的纯函数实现。
//!
//! offset 和 evicted_lines 都是 u64，不存在绕回问题。
//! 仅在传给 ratatui 时截断到 u16（clamp 到 u16::MAX）。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    /// 绝对行号（从第一条消息开始算），u64，与 evicted_lines 同类型。
    pub offset: u64,
    pub follow_tail: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollInput {
    pub viewport: u16,
    pub total_lines: u64,
    pub evicted_lines: u64,
}

impl ScrollInput {
    fn vp(&self) -> u64 { self.viewport.max(1) as u64 }
    fn max_scroll(&self) -> u64 { self.total_lines.saturating_sub(self.vp()) }
    fn step(&self) -> u64 { self.vp() }
    fn bottom_abs(&self) -> u64 { self.evicted_lines.saturating_add(self.max_scroll()) }
}

pub fn page_up(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    let cur = if state.follow_tail {
        input.bottom_abs().saturating_sub(input.step())
    } else {
        state.offset.saturating_sub(input.step())
    };
    ScrollState { offset: cur, follow_tail: false }
}

pub fn page_down(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    if state.follow_tail { return *state; }
    let cur = state.offset.saturating_add(input.step());
    if cur + input.step() >= input.bottom_abs() {
        ScrollState { offset: 0, follow_tail: true }
    } else {
        ScrollState { offset: cur, follow_tail: false }
    }
}

pub fn home(input: &ScrollInput) -> ScrollState {
    ScrollState { offset: input.evicted_lines, follow_tail: false }
}

pub fn end() -> ScrollState {
    ScrollState { offset: 0, follow_tail: true }
}

/// Reconcile the absolute scroll coordinate into the `u16` offset that
/// ratatui's `Paragraph::scroll` expects: subtract the evicted-line prefix and
/// clamp into `[0, max_scroll]`. Follow-tail always pins to the bottom.
///
/// This is the *single* place the absolute↔viewport conversion lives; both the
/// renderer and the state machine go through it.
pub fn viewport_offset(state: &ScrollState, input: &ScrollInput) -> u16 {
    let off = if state.follow_tail {
        input.max_scroll()
    } else {
        state.offset.saturating_sub(input.evicted_lines).min(input.max_scroll())
    };
    off.min(u16::MAX as u64) as u16
}

/// All persistent scroll-back state for the conversation view: the pure
/// [`ScrollState`] machine plus the "N new lines" tracking, with the last
/// frame's viewport height cached so a key press knows the page size.
///
/// Owning it in one type keeps the frontend free of scattered
/// interior-mutability cells and keeps every invariant in one place.
#[derive(Debug, Clone)]
pub struct Scrollback {
    state: ScrollState,
    /// Conversation viewport height at the last render (page step size).
    viewport: u16,
    /// `total_lines` the last time we were pinned to the tail.
    total_at_follow: u64,
    /// Lines appended since the user scrolled up (drives "▼ N new").
    unseen: u64,
}

impl Default for Scrollback {
    fn default() -> Self {
        Self {
            state: ScrollState { offset: 0, follow_tail: true },
            viewport: 20,
            total_at_follow: 0,
            unseen: 0,
        }
    }
}

impl Scrollback {
    fn input(&self, total_lines: u64, evicted_lines: u64) -> ScrollInput {
        ScrollInput { viewport: self.viewport, total_lines, evicted_lines }
    }

    pub fn page_up(&mut self, total_lines: u64, evicted_lines: u64) {
        self.state = page_up(&self.state, &self.input(total_lines, evicted_lines));
    }

    pub fn page_down(&mut self, total_lines: u64, evicted_lines: u64) {
        self.state = page_down(&self.state, &self.input(total_lines, evicted_lines));
    }

    pub fn home(&mut self, total_lines: u64, evicted_lines: u64) {
        self.state = home(&self.input(total_lines, evicted_lines));
    }

    pub fn end(&mut self) {
        self.state = end();
    }

    /// Record the geometry of the frame just rendered and refresh the unseen
    /// counter. Called once per frame, after drawing.
    pub fn on_frame(&mut self, viewport: u16, total_lines: u64) {
        self.viewport = viewport;
        if self.state.follow_tail {
            self.total_at_follow = total_lines;
            self.unseen = 0;
        } else {
            self.unseen = total_lines.saturating_sub(self.total_at_follow);
        }
    }

    pub fn offset(&self) -> u64 { self.state.offset }
    pub fn follow_tail(&self) -> bool { self.state.follow_tail }
    pub fn viewport(&self) -> u16 { self.viewport }
    pub fn unseen(&self) -> u32 { self.unseen.min(u32::MAX as u64) as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn buf() -> ScrollInput { ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 500 } }
    fn bottom() -> ScrollState { ScrollState { offset: 0, follow_tail: true } }
    fn detach(o: u64) -> ScrollState { ScrollState { offset: o, follow_tail: false } }

    #[test]
    fn pu_from_bottom() { let r = page_up(&bottom(), &buf()); assert!(!r.follow_tail); assert_eq!(r.offset, 560); }
    #[test]
    fn pu_detached() { assert_eq!(page_up(&detach(560), &buf()).offset, 540); }
    #[test]
    fn pd_from_middle() { let r = page_down(&detach(520), &buf()); assert!(!r.follow_tail); assert_eq!(r.offset, 540); }
    #[test]
    fn pd_reaches_bottom() { assert!(page_down(&detach(560), &buf()).follow_tail); }
    #[test]
    fn eviction_large() {
        let inp = ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 5_000_000_000 };
        let r = home(&inp);
        assert_eq!(r.offset, 5_000_000_000);
        assert!(!r.follow_tail);
    }

    // The previous inline renderer math, kept here to pin viewport_offset to it.
    fn legacy_offset(state: &ScrollState, input: &ScrollInput) -> u16 {
        let total = input.total_lines as u16;
        let max_scroll = total.saturating_sub(input.viewport);
        if state.follow_tail {
            max_scroll
        } else {
            let adjusted =
                (state.offset.saturating_sub(input.evicted_lines)).min(u16::MAX as u64) as u16;
            adjusted.min(max_scroll)
        }
    }

    #[test]
    fn viewport_offset_matches_legacy() {
        let cases = [
            (bottom(), buf()),
            (detach(560), buf()),
            (detach(520), buf()),
            (detach(0), buf()),
            (detach(1_000_000), buf()),
            (bottom(), ScrollInput { viewport: 10, total_lines: 5, evicted_lines: 0 }),
            (detach(3), ScrollInput { viewport: 10, total_lines: 40, evicted_lines: 2 }),
        ];
        for (st, inp) in cases {
            assert_eq!(
                viewport_offset(&st, &inp),
                legacy_offset(&st, &inp),
                "mismatch for state={st:?} input={inp:?}",
            );
        }
    }

    #[test]
    fn scrollback_defaults_to_following() {
        let sb = Scrollback::default();
        assert!(sb.follow_tail());
        assert_eq!(sb.offset(), 0);
        assert_eq!(sb.unseen(), 0);
    }

    #[test]
    fn scrollback_page_up_detaches_like_pure_fn() {
        let mut sb = Scrollback::default();
        sb.on_frame(20, 100);            // viewport 20, total 100
        sb.page_up(100, 500);            // total 100, evicted 500
        assert!(!sb.follow_tail());
        // Mirrors pu_from_bottom: bottom_abs - step = (500 + 80) - 20 = 560.
        assert_eq!(sb.offset(), 560);
    }

    #[test]
    fn scrollback_tracks_unseen_only_when_detached() {
        let mut sb = Scrollback::default();
        sb.on_frame(20, 100);            // following → unseen stays 0
        assert_eq!(sb.unseen(), 0);
        sb.page_up(100, 0);              // detach
        sb.on_frame(20, 130);            // 30 lines appended while detached
        assert_eq!(sb.unseen(), 30);
        sb.end();                        // back to tail
        sb.on_frame(20, 130);
        assert_eq!(sb.unseen(), 0);
    }
}
