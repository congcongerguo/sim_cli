//! 只影响显示的日志过滤器。
//!
//! 支持对每条消息做**正则**匹配,并用布尔运算组合多个条件:
//!
//! ```text
//!   /error/                 只显示匹配 error 的行
//!   /error/ && /timeout/    同时匹配两者(与)
//!   /warn/ || /error/       任一匹配(或)
//!   !/debug/                取反(非)
//!   (/a/ || /b/) && !/c/    括号分组
//! ```
//!
//! 正则用 `/.../ ` 包起来最稳妥(里面的 `|`、`&`、`(`、`)`、`!` 都当正则字符);
//! 不含这些运算符字符的简单模式也可以直接写裸词,例如 `error && timeout`。
//!
//! 运算符:`&&`/`and`、`||`/`or`、`!`/`not`,以及括号 `()`。
//! 优先级:`!` > `&&` > `||`(标准布尔优先级)。
//!
//! 这个过滤器只在渲染时筛选 [`crate::message::Message`],不改动
//! [`crate::log_buffer::LogBuffer`],也不影响落盘的消息日志文件。

use regex_lite::Regex;

use crate::message::Message;

/// 编译好的过滤表达式 + 原始文本(用于状态栏回显)。
pub struct Filter {
    expr: Expr,
    src: String,
}

impl Filter {
    /// 解析并编译一条过滤表达式。出错时返回可读的错误信息。
    pub fn parse(src: &str) -> Result<Filter, String> {
        let toks = tokenize(src)?;
        if toks.is_empty() {
            return Err("empty filter".into());
        }
        let mut p = Parser { toks, pos: 0 };
        let expr = p.parse_expr()?;
        if p.pos != p.toks.len() {
            return Err("trailing input after filter expression".into());
        }
        Ok(Filter { expr, src: src.trim().to_string() })
    }

    /// 该消息是否应当显示。
    pub fn matches_msg(&self, msg: &Message) -> bool {
        self.expr.matches(&message_text(msg))
    }

    /// 原始表达式文本(状态栏展示用)。
    pub fn src(&self) -> &str {
        &self.src
    }
}

/// 一条消息用于匹配的可搜索文本。
pub fn message_text(msg: &Message) -> String {
    match msg {
        Message::System { text, .. } => text.clone(),
        Message::Assistant { text, .. } => text.clone(),
        Message::Tool(t) => format!("{} {} {}", t.name, t.args_preview, t.output),
    }
}

// ── AST ────────────────────────────────────────────────────────────────

enum Expr {
    Regex(Regex),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    fn matches(&self, text: &str) -> bool {
        match self {
            Expr::Regex(re) => re.is_match(text),
            Expr::Not(e) => !e.matches(text),
            Expr::And(a, b) => a.matches(text) && b.matches(text),
            Expr::Or(a, b) => a.matches(text) || b.matches(text),
        }
    }
}

// ── 词法 ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Tok {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Regex(String),
}

fn tokenize(input: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            '!' => {
                toks.push(Tok::Not);
                i += 1;
            }
            '&' if i + 1 < chars.len() && chars[i + 1] == '&' => {
                toks.push(Tok::And);
                i += 2;
            }
            '|' if i + 1 < chars.len() && chars[i + 1] == '|' => {
                toks.push(Tok::Or);
                i += 2;
            }
            '/' => {
                // `/.../ ` slash-delimited regex; `\/` yields a literal slash.
                i += 1;
                let mut pat = String::new();
                let mut closed = false;
                while i < chars.len() {
                    let ch = chars[i];
                    if ch == '\\' && i + 1 < chars.len() {
                        let next = chars[i + 1];
                        if next == '/' {
                            pat.push('/');
                        } else {
                            pat.push('\\');
                            pat.push(next);
                        }
                        i += 2;
                        continue;
                    }
                    if ch == '/' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    pat.push(ch);
                    i += 1;
                }
                if !closed {
                    return Err("unterminated /regex/".into());
                }
                toks.push(Tok::Regex(pat));
            }
            _ => {
                // Bareword regex: up to whitespace, ( ) ! or a `&&`/`||` operator.
                let mut word = String::new();
                while i < chars.len() {
                    let ch = chars[i];
                    if ch.is_whitespace() || ch == '(' || ch == ')' || ch == '!' {
                        break;
                    }
                    if ch == '&' && i + 1 < chars.len() && chars[i + 1] == '&' {
                        break;
                    }
                    if ch == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
                        break;
                    }
                    word.push(ch);
                    i += 1;
                }
                match word.to_ascii_lowercase().as_str() {
                    "and" => toks.push(Tok::And),
                    "or" => toks.push(Tok::Or),
                    "not" => toks.push(Tok::Not),
                    _ => toks.push(Tok::Regex(word)),
                }
            }
        }
    }
    Ok(toks)
}

// ── 语法(递归下降)─────────────────────────────────────────────────────
//
//   expr  := or
//   or    := and ( ("||"|"or") and )*
//   and   := unary ( ("&&"|"and") unary )*
//   unary := ("!"|"not") unary | atom
//   atom  := "(" expr ")" | regex

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Tok::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        while matches!(self.peek(), Some(Tok::And)) {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Tok::Not)) {
            self.pos += 1;
            let inner = self.parse_unary()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Some(Tok::LParen) => {
                let e = self.parse_or()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("expected ')'".into()),
                }
            }
            Some(Tok::Regex(p)) => {
                let re = Regex::new(&p).map_err(|e| format!("bad regex /{p}/: {e}"))?;
                Ok(Expr::Regex(re))
            }
            Some(Tok::Not) => Err("unexpected '!'".into()),
            Some(Tok::And) | Some(Tok::Or) => Err("missing left-hand side of operator".into()),
            Some(Tok::RParen) => Err("unexpected ')'".into()),
            None => Err("unexpected end of filter".into()),
        }
    }
}

// ── 测试 ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse `expr` and test it against `text`.
    fn hit(text: &str, expr: &str) -> bool {
        Filter::parse(expr).expect("valid filter").expr.matches(text)
    }

    #[test]
    fn single_regex() {
        assert!(hit("connection error", "/error/"));
        assert!(!hit("all good", "/error/"));
    }

    #[test]
    fn bareword_is_regex() {
        assert!(hit("timeout after 5s", "timeout"));
        assert!(hit("code 42", r"\d+"));
    }

    #[test]
    fn and_or_not() {
        assert!(hit("error: timeout", "/error/ && /timeout/"));
        assert!(!hit("error: refused", "/error/ && /timeout/"));
        assert!(hit("just a warning", "/warn/ || /error/"));
        assert!(hit("info only", "!/error/"));
        assert!(!hit("an error", "!/error/"));
    }

    #[test]
    fn keyword_operators() {
        assert!(hit("error timeout", "error and timeout"));
        assert!(hit("warn", "warn or error"));
        assert!(hit("clean", "not error"));
    }

    #[test]
    fn precedence_and_binds_tighter_than_or() {
        // alpha || beta && gamma  ==  alpha || (beta && gamma)
        let expr = "/alpha/ || /beta/ && /gamma/";
        assert!(hit("alpha", expr)); // alpha alone
        assert!(!hit("beta", expr)); // beta without gamma → whole false
        assert!(hit("beta gamma", expr)); // beta && gamma
    }

    #[test]
    fn parentheses_override_precedence() {
        // (alpha || beta) && gamma
        let expr = "(/alpha/ || /beta/) && /gamma/";
        assert!(hit("alpha gamma", expr));
        assert!(!hit("alpha", expr)); // no gamma → false
    }

    #[test]
    fn regex_alternation_inside_slashes() {
        assert!(hit("GET /x", "/GET|POST/"));
        assert!(hit("POST /y", "/GET|POST/"));
        assert!(!hit("HEAD /z", "/GET|POST/"));
    }

    #[test]
    fn errors_are_reported() {
        assert!(Filter::parse("/unterminated").is_err());
        assert!(Filter::parse("/a/ &&").is_err());
        assert!(Filter::parse("( /a/").is_err());
        assert!(Filter::parse("/(/").is_err()); // bad regex
        assert!(Filter::parse("").is_err());
    }
}
