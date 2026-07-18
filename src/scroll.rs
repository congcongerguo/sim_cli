//! 翻滚状态机 —— PageUp/Down/Home/End 的纯函数实现。
//! 从 frontend.rs 的按键处理中抽离，方便单独测试。
//!
//! # 坐标系
//!
//! `offset` 是**绝对行号**，从第一条消息开始计数。被淘汰的消息仍然
//! 占据原来的绝对坐标 —— 可见窗口从 `evicted_lines` 开始。
//!
//! ```text
//!   绝对行 0 ─── [已淘汰] ─── [缓冲区内容] ─── [底部]
//!              ╰── 消失 ──╯  ╰─ total_lines ─╯
//! ```
//!
//! 渲染时将 `offset` 减去 `evicted_lines` 得到缓冲区内的相对位置。
//! 这样淘汰旧消息时视图自动锚定：offset 不变，减法自动对齐新窗口。
//!
//! # 数据流
//!
//! ```text
//!   LogBuffer (增量维护) → TaskSnapshot → ViewState → ScrollInput
//!        total_lines ──────────┤                │
//!        evicted_lines ────────┘                │
//!                                               ↓
//!                                  frontend::scroll_input()
//!                                               ↓
//!                              scroll::page_up/down/home/end
//!                                               ↓
//!                              ScrollState { offset, follow_tail }
//!                                               ↓
//!                              RenderState::scroll_offset ──→ conversation.rs
//!                                               │
//!                              adjusted = offset - evicted_lines
//!                                               │
//!                              ratatui Paragraph::scroll(adjusted)
//! ```

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    /// 绝对行号（从第一条消息开始算），u64 与 evicted_lines 同类型避免绕回。
    /// 在 conversation.rs 中通过 `offset - evicted_lines` 转为缓冲区内位置。
    pub offset: u64,
    /// true 时视图自动跟随缓冲区底部。
    pub follow_tail: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollInput {
    /// 对话区可见行数（viewport_height）。
    pub viewport: u16,
    /// 缓冲区当前总行数（LogBuffer::total_lines）。
    pub total_lines: u64,
    /// 累计淘汰行数（LogBuffer::evicted_lines）。
    pub evicted_lines: u64,
}

impl ScrollInput {
    fn viewport_u64(&self) -> u64 { self.viewport.max(1) as u64 }
    fn total_u64(&self) -> u64 { self.total_lines }
    fn evicted_u64(&self) -> u64 { self.evicted_lines }
    /// 最大可滚动行数 = 总行数 - 可见行数。
    fn max_scroll(&self) -> u64 {
        self.total_u64().saturating_sub(self.viewport_u64())
    }
    /// 每次翻页的步长 = 一屏的行数。
    fn step(&self) -> u64 { self.viewport_u64() }
    /// 底部绝对行号 = 已淘汰行数 + 最大滚动量。
    fn bottom_abs(&self) -> u64 {
        self.evicted_u64().saturating_add(self.max_scroll())
    }
}

/// PageUp / Ctrl+B：向上翻一页，自动脱离跟尾模式。
pub fn page_up(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    let cur = if state.follow_tail {
        // 从底部首次脱离：底部绝对位置 - 一页
        input.bottom_abs().saturating_sub(input.step())
    } else {
        // 已脱离：继续上翻
        state.offset.saturating_sub(input.step())
    };
    ScrollState { offset: cur, follow_tail: false }
}

/// PageDown / Ctrl+F：向下翻一页。到达底部时自动恢复跟尾模式。
pub fn page_down(state: &ScrollState, input: &ScrollInput) -> ScrollState {
    if state.follow_tail {
        return *state; // 已在底部，无需操作
    }
    let cur = state.offset.saturating_add(input.step());
    // 多留一页的余量：防止按键和渲染之间来了新 tick 导致差一行到不了底
    if cur + input.step() >= input.bottom_abs() {
        ScrollState { offset: 0, follow_tail: true }
    } else {
        ScrollState { offset: cur, follow_tail: false }
    }
}

/// Home：跳到当前缓冲区的最顶部（已淘汰行的下一行）。
pub fn home(input: &ScrollInput) -> ScrollState {
    ScrollState { offset: input.evicted_u64(), follow_tail: false }
}

/// End：跳到底部并恢复跟尾模式。
pub fn end() -> ScrollState {
    ScrollState { offset: 0, follow_tail: true }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn full_buf() -> ScrollInput {
        ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 500 }
    }
    fn at_bottom() -> ScrollState { ScrollState { offset: 0, follow_tail: true } }
    fn detached(o: u64) -> ScrollState { ScrollState { offset: o, follow_tail: false } }

    #[test]
    fn page_up_from_bottom_detaches() {
        let r = page_up(&at_bottom(), &full_buf());
        assert!(!r.follow_tail);
        assert_eq!(r.offset, 560);
    }
    #[test]
    fn page_up_detached_goes_further() {
        assert_eq!(page_up(&detached(560), &full_buf()).offset, 540);
    }
    #[test]
    fn page_up_saturates_at_zero() {
        assert_eq!(page_up(&detached(5), &full_buf()).offset, 0);
    }
    #[test]
    fn page_down_from_bottom_noop() {
        assert_eq!(page_down(&at_bottom(), &full_buf()), at_bottom());
    }
    #[test]
    fn page_down_from_middle() {
        let r = page_down(&detached(520), &full_buf());
        assert!(!r.follow_tail);
        assert_eq!(r.offset, 540);
    }
    #[test]
    fn page_down_reaches_bottom() {
        assert!(page_down(&detached(560), &full_buf()).follow_tail);
    }
    #[test]
    fn home_jumps_to_top() {
        assert_eq!(home(&full_buf()).offset, 500);
    }
    #[test]
    fn end_jumps_to_bottom() {
        assert_eq!(end(), ScrollState { offset: 0, follow_tail: true });
    }
    #[test]
    fn small_buffer_no_scroll() {
        assert_eq!(ScrollInput { viewport: 20, total_lines: 5, evicted_lines: 0 }.max_scroll(), 0);
    }
    #[test]
    fn eviction_still_scrollable() {
        let inp = ScrollInput { viewport: 20, total_lines: 100, evicted_lines: 1000 };
        assert_eq!(page_down(&home(&inp), &inp).offset, 1020);
    }
    #[test]
    fn viewport_zero_clamped() {
        let inp = ScrollInput { viewport: 0, total_lines: 100, evicted_lines: 0 };
        assert_eq!(inp.viewport_u64(), 1);
        assert_eq!(inp.max_scroll(), 99);
    }
}
