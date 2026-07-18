//! 翻滚状态机 —— PageUp/Down/Home/End 的纯函数实现。
//!
//! # 绕回处理：Linux 内核 time_after 模式
//!
//! offset 和 evicted_lines 都是 u32，理论上会溢出绕回。
//! 采用内核的 time_after 模式：不比较绝对值，比较差值。
//! `time_after(a, b)` = `(a - b) as i32 > 0`
//! 只要两次比较的间隔不超过 u32::MAX/2（约 21 亿行），
//! 即使 a 或 b 绕回了，差值仍然正确。
//!
//! 每秒 2 条消息，21 亿行需要约 34 年。足够。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    /// 绝对行号（从第一条消息开始算），u32，溢出时绕回。
    pub offset: u32,
    /// true 时视图自动跟随缓冲区底部。
    pub follow_tail: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollInput {
    pub viewport: u16,
    pub total_lines: u64,
    pub evicted_lines: u64,
}

/// 内核 time_after：比较两个可能绕回的 u32 的先后顺序。
#[allow(dead_code)]
fn time_after(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) > 0 }
fn time_after_eq(a: u32, b: u32) -> bool { (a.wrapping_sub(b) as i32) >= 0 }

impl ScrollInput {
    fn vp(&self) -> u32 { self.viewport.max(1) as u32 }
    fn total(&self) -> u32 { self.total_lines.min(u32::MAX as u64) as u32 }
    fn evicted(&self) -> u32 { self.evicted_lines.min(u32::MAX as u64) as u32 }
    fn max_scroll(&self) -> u32 { self.total().saturating_sub(self.vp()) }
    fn step(&self) -> u32 { self.vp() }
    fn bottom_abs(&self) -> u32 { self.evicted().wrapping_add(self.max_scroll()) }
}

/// PageUp / Ctrl+B：向上翻一页，自动脱离跟尾模式。
pub fn page_up(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    let cur = if state.follow_tail {
        input.bottom_abs().wrapping_sub(input.step())
    } else {
        state.offset.wrapping_sub(input.step())
    };
    ScrollState { offset: cur, follow_tail: false }
}

/// PageDown / Ctrl+F：向下翻一页。到达底部时自动恢复跟尾。
pub fn page_down(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    if state.follow_tail {
        return *state;
    }
    let cur = state.offset.wrapping_add(input.step());
    // time_after_eq 处理绕回比较：即使值绕回了也能正确判断
    if time_after_eq(cur.wrapping_add(input.step()), input.bottom_abs()) {
        ScrollState { offset: 0, follow_tail: true }
    } else {
        ScrollState { offset: cur, follow_tail: false }
    }
}

/// Home：跳到当前缓冲区顶部（已淘汰行之后的第一行）。
pub fn home(input: &ScrollInput) -> ScrollState {
    ScrollState { offset: input.evicted(), follow_tail: false }
}

/// End：跳到底部并恢复跟尾模式。
pub fn end() -> ScrollState {
    ScrollState { offset: 0, follow_tail: true }
}

// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn buf() -> ScrollInput { ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 500 } }
    fn bottom() -> ScrollState { ScrollState { offset: 0, follow_tail: true } }
    fn detach(o: u32) -> ScrollState { ScrollState { offset: o, follow_tail: false } }

    #[test]
    fn time_after_basic() {
        assert!(time_after(100, 50));
        assert!(!time_after(50, 100));
    }

    #[test]
    fn time_after_wrap() {
        // a=100, b=u32::MAX-50 → diff=151 → a 在 b 之后
        assert!(time_after(100, u32::MAX - 50));
        // b=100, a=u32::MAX-50 → diff 很大且最高位为 1 → a 在 b 之前
        assert!(!time_after(u32::MAX - 50, 100));
    }

    #[test]
    fn pu_from_bottom() {
        let r = page_up(&bottom(), &buf());
        assert!(!r.follow_tail);
        assert_eq!(r.offset, 560); // bottom_abs=580, step=20
    }

    #[test]
    fn pu_detached() {
        assert_eq!(page_up(&detach(560), &buf()).offset, 540);
    }

    #[test]
    fn pd_from_middle() {
        let r = page_down(&detach(520), &buf());
        assert!(!r.follow_tail);
        assert_eq!(r.offset, 540);
    }

    #[test]
    fn pd_reaches_bottom() {
        assert!(page_down(&detach(560), &buf()).follow_tail);
    }

    #[test]
    fn home_test() { assert_eq!(home(&buf()).offset, 500); }

    #[test]
    fn end_test() { assert_eq!(end(), bottom()); }

    #[test]
    fn eviction_still_works() {
        let inp = ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 1000 };
        assert_eq!(page_down(&home(&inp), &inp).offset, 1020);
    }
}
