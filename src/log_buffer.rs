//! Ring buffer for chat messages. Evicts oldest entries when full.
//! Tracks `total_evicted` so the scroll position can be corrected.

use std::collections::VecDeque;

use crate::message::Message;

pub const DEFAULT_MAX: usize = 200;

/// How many render lines a message produces.
pub fn msg_line_count(msg: &Message) -> u64 {
    match msg {
        Message::Assistant { text, streaming } => {
            let n = text.lines().count() as u64;
            if *streaming { n + 1 } else { n }  // +1 for cursor
        }
        Message::Tool(_t) => 4, // title + status + args + output
        Message::System { text, .. } => {
            text.lines().count() as u64 + 1  // +1 for blank separator
        }
    }
}

#[derive(Debug, Clone)]
pub struct LogBuffer {
    messages: VecDeque<Message>,
    max_entries: usize,
    total_evicted: u64,
    total_lines: u64,
}

impl LogBuffer {
    pub fn new(max_entries: usize) -> Self {
        Self { messages: VecDeque::new(), max_entries, total_evicted: 0, total_lines: 0 }
    }

    /// Add a message. If over capacity, evicts oldest.
    pub fn push(&mut self, msg: Message) {
        let added = msg_line_count(&msg);
        self.messages.push_back(msg);
        self.total_lines += added;
        while self.messages.len() > self.max_entries {
            if let Some(old) = self.messages.pop_front() {
                self.total_lines -= msg_line_count(&old);
                self.total_evicted += 1;
            }
        }
    }

    /// Number of messages currently held.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Clear all messages. Keeps eviction counter.
    pub fn clear(&mut self) {
        self.total_lines = 0;
        self.messages.clear();
    }

    /// Cumulative count of entries evicted since creation.
    pub fn evicted(&self) -> u64 {
        self.total_evicted
    }

    /// Total render lines (maintained incrementally — O(1)).
    pub fn total_lines(&self) -> u64 {
        self.total_lines
    }

    /// Iterate messages from oldest to newest.
    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &Message> {
        self.messages.iter()
    }

    /// Mutable reference to the last message (for streaming updates).
    pub fn last_mut(&mut self) -> Option<&mut Message> {
        self.messages.back_mut()
    }

    /// Mutable reference by index (for tool call lookups).
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Message> {
        self.messages.get_mut(index)
    }

    /// Clone all messages into a Vec (for tests).
    #[allow(dead_code)]
    pub fn to_vec(&self) -> Vec<Message> {
        self.messages.iter().cloned().collect()
    }

    /// Return a shared snapshot of the current buffer. Use this for ViewState
    /// to avoid re-cloning the entire buffer every frame.
    pub fn to_arc(&self) -> std::sync::Arc<Vec<Message>> {
        std::sync::Arc::new(self.messages.iter().cloned().collect())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::LogLevel;

    fn msg(text: &str) -> Message {
        Message::System { text: text.into(), level: LogLevel::Info }
    }

    #[test]
    fn push_within_limit() {
        let mut buf = LogBuffer::new(10);
        for i in 0..5 {
            buf.push(msg(&format!("line {i}")));
        }
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.evicted(), 0);
        let all: Vec<String> = buf.iter().map(|m| match m {
            Message::System { text, .. } => text.clone(),
            _ => String::new(),
        }).collect();
        assert_eq!(all, vec!["line 0", "line 1", "line 2", "line 3", "line 4"]);
    }

    #[test]
    fn push_beyond_limit_evicts() {
        let mut buf = LogBuffer::new(3);
        for i in 0..5 {
            buf.push(msg(&format!("line {i}")));
        }
        assert_eq!(buf.len(), 3);
        let all: Vec<String> = buf.iter().map(|m| match m {
            Message::System { text, .. } => text.clone(),
            _ => String::new(),
        }).collect();
        assert_eq!(all, vec!["line 2", "line 3", "line 4"]);
    }

    #[test]
    fn eviction_updates_counter() {
        let mut buf = LogBuffer::new(2);
        buf.push(msg("a"));
        buf.push(msg("b"));
        assert_eq!(buf.evicted(), 0);
        buf.push(msg("c")); // evicts "a"
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.evicted(), 1);
        buf.push(msg("d")); // evicts "b"
        assert_eq!(buf.evicted(), 2);
    }

    #[test]
    fn clear_keeps_eviction_count() {
        let mut buf = LogBuffer::new(2);
        buf.push(msg("a"));
        buf.push(msg("b"));
        buf.push(msg("c"));
        assert_eq!(buf.evicted(), 1);
        buf.clear();
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.evicted(), 1, "clear must not reset eviction counter");
    }

    #[test]
    fn empty_buffer() {
        let buf = LogBuffer::new(10);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.evicted(), 0);
        assert_eq!(buf.iter().count(), 0);
    }

    #[test]
    fn iter_respects_fifo() {
        let mut buf = LogBuffer::new(10);
        buf.push(msg("first"));
        buf.push(msg("second"));
        let texts: Vec<String> = buf.iter().map(|m| match m {
            Message::System { text, .. } => text.clone(),
            _ => String::new(),
        }).collect();
        assert_eq!(texts, vec!["first", "second"]);
    }

    #[test]
    fn single_entry() {
        let mut buf = LogBuffer::new(1);
        buf.push(msg("only"));
        assert_eq!(buf.len(), 1);
        buf.push(msg("replaced"));
        assert_eq!(buf.len(), 1);
        assert_eq!(buf.evicted(), 1);
    }

    #[test]
    fn max_zero() {
        let mut buf = LogBuffer::new(0);
        buf.push(msg("a"));
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.evicted(), 1);
    }

    #[test]
    fn to_vec_snapshot() {
        let mut buf = LogBuffer::new(10);
        buf.push(msg("hello"));
        buf.push(msg("world"));
        let snap = buf.to_vec();
        assert_eq!(snap.len(), 2);
    }

    #[test]
    fn total_lines_increments_and_decrements() {
        let mut buf = LogBuffer::new(10);
        // Single-line system messages: each = 2 lines (text + blank separator)
        buf.push(msg("a")); // 2 lines
        buf.push(msg("b")); // 2 lines
        assert_eq!(buf.total_lines(), 4);
        buf.clear();
        assert_eq!(buf.total_lines(), 0);
    }

    #[test]
    fn total_lines_drops_on_eviction() {
        let mut buf = LogBuffer::new(2);
        buf.push(msg("a")); // +2
        buf.push(msg("b")); // +2 -> total 4
        assert_eq!(buf.total_lines(), 4);
        buf.push(msg("c")); // +2, evicts "a" (-2) -> total 4
        assert_eq!(buf.total_lines(), 4);
        assert_eq!(buf.evicted(), 1);
    }

    #[test]
    fn push_many_evicts_correctly() {
        let mut buf = LogBuffer::new(5);
        for i in 0..20 {
            buf.push(msg(&format!("line {i}")));
        }
        assert_eq!(buf.len(), 5);
        assert_eq!(buf.evicted(), 15);
        let first = match buf.iter().next().unwrap() {
            Message::System { text, .. } => text.clone(),
            _ => String::new(),
        };
        assert_eq!(first, "line 15");
    }
}
