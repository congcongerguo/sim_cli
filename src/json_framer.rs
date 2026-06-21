/// Brace-balanced JSON framer for a TCP byte stream.
///
/// Bytes are appended via `feed`; each call to `next_message` returns the next
/// fully-balanced top-level JSON value (`{...}` or `[...]`) found in the buffer,
/// or `None` if no complete value is available yet. Bytes inside JSON strings
/// (with `\"` escapes honoured) are not counted toward the depth.
pub struct JsonFramer {
    buf: Vec<u8>,
    /// Position in `buf` examined so far (so re-feeds don't re-scan).
    cursor: usize,
    /// Where the current outer value starts. None when between messages.
    start: Option<usize>,
    /// Nesting depth of `{`/`[` seen since `start`.
    depth: u32,
    in_string: bool,
    /// Next byte inside a string is escaped.
    escape: bool,
}

impl Default for JsonFramer {
    fn default() -> Self {
        Self::new()
    }
}

impl JsonFramer {
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            cursor: 0,
            start: None,
            depth: 0,
            in_string: false,
            escape: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }

    /// Pull the next complete top-level JSON value out of the buffer.
    pub fn next_message(&mut self) -> Option<Vec<u8>> {
        while self.cursor < self.buf.len() {
            let b = self.buf[self.cursor];

            if self.start.is_none() {
                if matches!(b, b' ' | b'\t' | b'\r' | b'\n') {
                    self.cursor += 1;
                    continue;
                }
                if b == b'{' || b == b'[' {
                    if self.cursor > 0 {
                        self.buf.drain(..self.cursor);
                        self.cursor = 0;
                    }
                    self.start = Some(0);
                    self.depth = 1;
                    self.cursor = 1;
                    continue;
                }
                self.cursor += 1;
                continue;
            }

            if self.escape {
                self.escape = false;
                self.cursor += 1;
                continue;
            }

            if self.in_string {
                match b {
                    b'\\' => self.escape = true,
                    b'"' => self.in_string = false,
                    _ => {}
                }
                self.cursor += 1;
                continue;
            }

            match b {
                b'"' => self.in_string = true,
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        let start = self.start.expect("start set when depth > 0");
                        let end = self.cursor + 1;
                        let msg = self.buf[start..end].to_vec();
                        self.buf.drain(..end);
                        self.cursor = 0;
                        self.start = None;
                        return Some(msg);
                    }
                }
                _ => {}
            }
            self.cursor += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(bytes: &[u8]) -> Option<String> {
        let mut f = JsonFramer::new();
        f.feed(bytes);
        f.next_message().map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn single_object() {
        assert_eq!(one(b"{\"a\":1}").as_deref(), Some("{\"a\":1}"));
    }

    #[test]
    fn ignores_braces_inside_strings() {
        let s = r#"{"msg":"this } looks } closed but isn't","n":1}"#;
        assert_eq!(one(s.as_bytes()).as_deref(), Some(s));
    }

    #[test]
    fn honours_escape_quotes() {
        let s = r#"{"msg":"with \"quotes\" and a } brace"}"#;
        assert_eq!(one(s.as_bytes()).as_deref(), Some(s));
    }

    #[test]
    fn drip_feed_assembles_message() {
        let mut f = JsonFramer::new();
        for chunk in [&b"{\"a\":"[..], &b"[1,2"[..], &b",3]}"[..]] {
            f.feed(chunk);
        }
        assert_eq!(
            f.next_message().map(|b| String::from_utf8(b).unwrap()).as_deref(),
            Some("{\"a\":[1,2,3]}")
        );
        assert!(f.next_message().is_none());
    }

    #[test]
    fn back_to_back_messages() {
        let mut f = JsonFramer::new();
        f.feed(b"  {\"id\":1}{\"id\":2}\n{\"id\":3}");
        let m1 = String::from_utf8(f.next_message().unwrap()).unwrap();
        let m2 = String::from_utf8(f.next_message().unwrap()).unwrap();
        let m3 = String::from_utf8(f.next_message().unwrap()).unwrap();
        assert_eq!(m1, "{\"id\":1}");
        assert_eq!(m2, "{\"id\":2}");
        assert_eq!(m3, "{\"id\":3}");
        assert!(f.next_message().is_none());
    }

    #[test]
    fn array_top_level() {
        let s = r#"[1,2,{"k":"v}"}]"#;
        assert_eq!(one(s.as_bytes()).as_deref(), Some(s));
    }
}
