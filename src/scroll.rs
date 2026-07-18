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
}
