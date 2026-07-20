//! 消息持久化：把流经框架的每一条消息追加写入一个日志文件,
//! 每条记录带毫秒精度的时间戳。
//!
//! 所有消息都在 `tool::spawn` 的 `ctx.log.push(...)` 处进入缓冲区,
//! 我们在同一处调用 [`record`],因此不会遗漏任何一条。
//!
//! 文件路径默认 `sim_cli-messages.log`(当前工作目录),可用环境变量
//! `SIM_CLI_MSG_LOG` 覆盖。多个 tool 任务并发写同一个文件,用 `Mutex`
//! 串行化。首次打开失败时静默降级为不记录,绝不影响 UI。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use crate::message::{LogLevel, Message, Timestamp};

/// 进程内唯一的日志文件句柄。`None` 表示打开失败(降级为不记录)。
static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();

fn log_path() -> String {
    std::env::var("SIM_CLI_MSG_LOG").unwrap_or_else(|_| "sim_cli-messages.log".to_string())
}

fn file_cell() -> &'static Mutex<Option<File>> {
    LOG_FILE.get_or_init(|| {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .ok();
        Mutex::new(file)
    })
}

fn level_str(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN",
        LogLevel::Notice => "NOTICE",
        LogLevel::Info => "INFO",
        LogLevel::Debug => "DEBUG",
    }
}

/// 把消息渲染成单行记录体(内部换行折叠为空格,保证一条消息一行)。
fn body(msg: &Message) -> String {
    match msg {
        Message::System { text, level } => {
            format!("{} {}", level_str(*level), one_line(text))
        }
        Message::Assistant { text, streaming } => {
            let tag = if *streaming { "ASSISTANT*" } else { "ASSISTANT" };
            format!("{tag} {}", one_line(text))
        }
        Message::Tool(t) => format!(
            "TOOL {} [{:?}] args={:?} output={:?}",
            t.name,
            t.status,
            one_line(&t.args_preview),
            one_line(&t.output),
        ),
    }
}

fn one_line(s: &str) -> String {
    s.replace('\n', " ⏎ ")
}

/// 构造一条完整记录行(含毫秒时间戳,不含结尾换行)。
/// 时间戳采用本地时区,格式 `YYYY-MM-DD HH:MM:SS.mmm`。
fn format_line(time: Timestamp, tool: &str, msg: &Message) -> String {
    let ts = time.format("%Y-%m-%d %H:%M:%S%.3f");
    format!("{ts} [{tool}] {}", body(msg))
}

/// 用指定时间戳追加记录一条消息,让界面展示与落盘共用同一时刻。
/// 写入失败或文件未打开时静默返回。
pub fn record_at(time: Timestamp, tool: &str, msg: &Message) {
    let line = format!("{}\n", format_line(time, tool, msg));
    if let Ok(mut guard) = file_cell().lock() {
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// 追加记录一条消息,时间戳取当前本地时间(毫秒精度)。
#[allow(dead_code)]
pub fn record(tool: &str, msg: &Message) {
    record_at(chrono::Local::now(), tool, msg);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ToolCall, ToolStatus};

    fn ts_prefix_len() -> usize {
        // "YYYY-MM-DD HH:MM:SS.mmm" == 23 chars
        "2026-07-20 14:23:01.123".len()
    }

    fn now() -> Timestamp {
        chrono::Local::now()
    }

    #[test]
    fn line_starts_with_millisecond_timestamp() {
        let m = Message::System { text: "hello".into(), level: LogLevel::Info };
        let line = format_line(now(), "conn", &m);
        let ts = &line[..ts_prefix_len()];
        // Exactly three fractional-second digits.
        let (_, frac) = ts.split_once('.').expect("timestamp has fractional part");
        assert_eq!(frac.len(), 3, "millisecond precision = 3 digits, got {frac:?}");
        assert!(line.starts_with(ts));
        assert!(line.contains("[conn] INFO hello"));
    }

    #[test]
    fn system_levels_render() {
        for (lvl, name) in [
            (LogLevel::Error, "ERROR"),
            (LogLevel::Warn, "WARN"),
            (LogLevel::Notice, "NOTICE"),
            (LogLevel::Info, "INFO"),
            (LogLevel::Debug, "DEBUG"),
        ] {
            let m = Message::System { text: "x".into(), level: lvl };
            assert!(format_line(now(), "t", &m).contains(&format!("[t] {name} x")));
        }
    }

    #[test]
    fn multiline_text_collapsed_to_one_line() {
        let m = Message::System { text: "a\nb\nc".into(), level: LogLevel::Info };
        let line = format_line(now(), "t", &m);
        assert!(!line.contains('\n'), "record must stay on a single line");
        assert!(line.contains("a ⏎ b ⏎ c"));
    }

    #[test]
    fn record_appends_to_file() {
        // This is the only test that touches the file-backed global, so the
        // env-var path is deterministic regardless of test ordering.
        let path = std::env::temp_dir().join(format!("sim_cli_msglog_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // SAFETY: single-threaded setup before any record() call in this test.
        unsafe { std::env::set_var("SIM_CLI_MSG_LOG", &path); }

        record("conn", &Message::System { text: "connected".into(), level: LogLevel::Notice });
        record("demo", &Message::System { text: "42".into(), level: LogLevel::Debug });

        let contents = std::fs::read_to_string(&path).expect("log file written");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "one line per message: {contents:?}");
        assert!(lines[0].contains("[conn] NOTICE connected"));
        assert!(lines[1].contains("[demo] DEBUG 42"));
        // Each line carries a millisecond timestamp.
        for l in &lines {
            let ts = &l[..ts_prefix_len()];
            assert_eq!(ts.split_once('.').unwrap().1.len(), 3);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tool_and_assistant_render() {
        let tool = Message::Tool(ToolCall {
            name: "ls".into(),
            args_preview: "-la".into(),
            status: ToolStatus::Done,
            output: "file1\nfile2".into(),
        });
        let line = format_line(now(), "demo", &tool);
        assert!(line.contains("TOOL ls"));
        assert!(line.contains("Done"));
        assert!(line.contains("file1 ⏎ file2"));

        let asst = Message::Assistant { text: "hi".into(), streaming: true };
        assert!(format_line(now(), "demo", &asst).contains("ASSISTANT* hi"));
    }
}
