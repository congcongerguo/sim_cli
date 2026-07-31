//! 消息持久化:把流经框架的每一条消息追加写入日志文件,每条记录带毫秒
//! 精度的时间戳。这是"尽量多保存历史"的**存储层**。
//!
//! # 设计
//!
//! - **后台写线程**:所有消息经一个 `std::sync::mpsc` 通道交给一个专用线程
//!   写盘,把文件 I/O 移出 tokio 运行时(不再在 async 任务里做阻塞写)。
//! - **缓冲写**:线程用 `BufWriter`,并每 ~250ms 或空闲时 flush 一次 —— 既
//!   合并了 syscall,又把崩溃丢失窗口限制在几百毫秒内。
//! - **按大小轮转**:单文件超过 `max_bytes` 就轮转
//!   (`base` → `base.1` → `base.2` … 最旧的删掉),总占用 ≈
//!   `max_bytes × (max_files + 1)`。这样"存尽量多"变成一个可控的磁盘预算。
//!
//! # 配置(环境变量)
//!
//! | 变量 | 含义 | 默认 |
//! |---|---|---|
//! | `SIM_CLI_MSG_LOG`       | 基础文件路径      | `sim_cli-messages.log` |
//! | `SIM_CLI_LOG_MAX_BYTES` | 单文件字节上限    | 32 MiB |
//! | `SIM_CLI_LOG_MAX_FILES` | 轮转保留的旧文件数 | 8(默认总预算 ≈ 288 MiB) |
//!
//! 打开失败时静默降级为不记录,绝不影响 UI。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::message::{LogLevel, Message, Timestamp};

const DEFAULT_MAX_BYTES: u64 = 32 * 1024 * 1024;
const DEFAULT_MAX_FILES: usize = 8;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

/// 发给写线程的命令。
enum Cmd {
    Line(String),
    /// 请求 flush 并在完成后 ack(用于测试与优雅退出)。
    Flush(Sender<()>),
}

/// 进程内唯一的写线程句柄。`None` = 初始化失败(降级为不记录)。
static SENDER: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();

fn env_path() -> PathBuf {
    std::env::var_os("SIM_CLI_MSG_LOG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("sim_cli-messages.log"))
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(default)
}

fn sender() -> Option<&'static Sender<Cmd>> {
    SENDER
        .get_or_init(|| {
            let base = env_path();
            let max_bytes = env_u64("SIM_CLI_LOG_MAX_BYTES", DEFAULT_MAX_BYTES);
            let max_files = env_u64("SIM_CLI_LOG_MAX_FILES", DEFAULT_MAX_FILES as u64) as usize;
            let mut writer = RotatingWriter::new(base, max_bytes, max_files);
            // Fail fast: if we can't open the file at all, degrade to no-op.
            writer.ensure_open()?;
            let (tx, rx) = mpsc::channel::<Cmd>();
            std::thread::Builder::new()
                .name("msg-log".into())
                .spawn(move || writer_loop(rx, writer))
                .ok()?;
            Some(tx)
        })
        .as_ref()
}

/// 后台写线程主循环:批量写、周期性 flush。
fn writer_loop(rx: mpsc::Receiver<Cmd>, mut writer: RotatingWriter) {
    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(cmd) => {
                handle(&mut writer, cmd);
                // Drain whatever else is queued without blocking, then flush once.
                while let Ok(cmd) = rx.try_recv() {
                    handle(&mut writer, cmd);
                }
                writer.flush();
            }
            Err(RecvTimeoutError::Timeout) => writer.flush(),
            Err(RecvTimeoutError::Disconnected) => {
                writer.flush();
                break;
            }
        }
    }
}

fn handle(writer: &mut RotatingWriter, cmd: Cmd) {
    match cmd {
        Cmd::Line(line) => writer.write_line(&line),
        Cmd::Flush(ack) => {
            writer.flush();
            let _ = ack.send(());
        }
    }
}

// ── 轮转写入器(纯逻辑,可单测)──────────────────────────────────────────

/// 缓冲 + 按大小轮转的追加写入器。不涉及线程,便于单测。
struct RotatingWriter {
    base: PathBuf,
    file: Option<BufWriter<File>>,
    written: u64,
    max_bytes: u64,
    max_files: usize,
    dirty: bool,
}

impl RotatingWriter {
    fn new(base: PathBuf, max_bytes: u64, max_files: usize) -> Self {
        Self { base, file: None, written: 0, max_bytes, max_files: max_files.max(1), dirty: false }
    }

    /// 确保当前文件已打开;返回 `None` 表示打开失败。
    fn ensure_open(&mut self) -> Option<()> {
        if self.file.is_some() {
            return Some(());
        }
        let file = OpenOptions::new().create(true).append(true).open(&self.base).ok()?;
        self.written = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(BufWriter::new(file));
        Some(())
    }

    fn write_line(&mut self, line: &str) {
        if self.ensure_open().is_none() {
            return;
        }
        if let Some(f) = self.file.as_mut() {
            if f.write_all(line.as_bytes()).is_ok() {
                self.written += line.len() as u64;
                self.dirty = true;
            }
        }
        if self.written >= self.max_bytes {
            self.rotate();
        }
    }

    fn flush(&mut self) {
        if self.dirty {
            if let Some(f) = self.file.as_mut() {
                let _ = f.flush();
            }
            self.dirty = false;
        }
    }

    /// `base` → `base.1` → … → `base.max_files`(最旧的删掉),然后开一个新的 `base`。
    fn rotate(&mut self) {
        // Flush and close the current file first.
        if let Some(mut f) = self.file.take() {
            let _ = f.flush();
        }
        self.dirty = false;
        let _ = std::fs::remove_file(rotated(&self.base, self.max_files));
        for i in (1..self.max_files).rev() {
            let _ = std::fs::rename(rotated(&self.base, i), rotated(&self.base, i + 1));
        }
        let _ = std::fs::rename(&self.base, rotated(&self.base, 1));
        self.written = 0;
        let _ = self.ensure_open();
    }
}

fn rotated(base: &PathBuf, i: usize) -> PathBuf {
    let mut s = base.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

// ── 记录 API ────────────────────────────────────────────────────────────

/// 用指定时间戳追加记录一条消息(界面展示与落盘共用同一时刻)。
/// 非阻塞:仅把格式化好的行投递给后台写线程。
pub fn record_at(time: Timestamp, tool: &str, msg: &Message) {
    if let Some(tx) = sender() {
        let _ = tx.send(Cmd::Line(format!("{}\n", format_line(time, tool, msg))));
    }
}

/// 追加记录一条消息,时间戳取当前本地时间(毫秒精度)。
#[allow(dead_code)]
pub fn record(tool: &str, msg: &Message) {
    record_at(chrono::Local::now(), tool, msg);
}

/// 单次 grep 最多扫描的行数(封顶,避免在超大归档上卡太久)。
const SCAN_CAP_LINES: usize = 2_000_000;

/// 在磁盘归档(当前文件 + 所有轮转文件)里搜索匹配 `pred` 的行。
///
/// 从**最新**往旧扫,收集到 `limit` 条或扫过 `SCAN_CAP_LINES` 行即停 ——
/// 保证在大归档上也能快速返回"最近的匹配"。返回结果按**时间顺序**
/// (旧→新)排列。逐行读取(`BufReader`),内存占用有界。
pub fn scan<F: Fn(&str) -> bool>(pred: F, limit: usize) -> Vec<String> {
    flush(); // make buffered-but-not-yet-written lines visible to the search
    let base = env_path();
    let max_files = env_u64("SIM_CLI_LOG_MAX_FILES", DEFAULT_MAX_FILES as u64) as usize;
    scan_files(&base, max_files, pred, limit)
}

/// Search a specific archive (base + its rotated files). Split out from `scan`
/// so it's testable without the process-global writer/env.
pub(crate) fn scan_files<F: Fn(&str) -> bool>(
    base: &PathBuf,
    max_files: usize,
    pred: F,
    limit: usize,
) -> Vec<String> {
    use std::io::{BufRead, BufReader};

    // Newest → oldest: base is the current file, then .1, .2, …
    let mut files = vec![base.clone()];
    for i in 1..=max_files {
        files.push(rotated(base, i));
    }

    let mut per_file: Vec<Vec<String>> = Vec::with_capacity(files.len());
    let mut found = 0usize;
    let mut scanned = 0usize;
    'outer: for path in &files {
        let mut matches = Vec::new();
        if let Ok(file) = File::open(path) {
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                scanned += 1;
                if pred(&line) {
                    matches.push(line);
                    found += 1;
                    if found >= limit {
                        per_file.push(matches);
                        break 'outer;
                    }
                }
                if scanned >= SCAN_CAP_LINES {
                    per_file.push(matches);
                    break 'outer;
                }
            }
        }
        per_file.push(matches);
    }

    // We scanned newest file first; emit oldest file first for chronological order
    // (lines within each file are already chronological).
    let mut out = Vec::new();
    for matches in per_file.iter().rev() {
        out.extend(matches.iter().cloned());
    }
    out
}

/// 阻塞直到后台写线程把已投递的记录 flush 到磁盘。用于优雅退出与测试。
#[allow(dead_code)]
pub fn flush() {
    if let Some(tx) = sender() {
        let (ack_tx, ack_rx) = mpsc::channel();
        if tx.send(Cmd::Flush(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}

// ── 行格式化 ────────────────────────────────────────────────────────────

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
        Message::System { text, level } => format!("{} {}", level_str(*level), one_line(text)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ToolCall, ToolStatus};

    fn ts_prefix_len() -> usize {
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

    // ── RotatingWriter 单测(同步,不经过后台线程)────────────────────

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sim_cli_rot_{}_{name}", std::process::id()))
    }

    fn cleanup(base: &PathBuf, max_files: usize) {
        let _ = std::fs::remove_file(base);
        for i in 1..=max_files + 2 {
            let _ = std::fs::remove_file(rotated(base, i));
        }
    }

    #[test]
    fn writer_appends_and_flushes() {
        let base = tmp("append.log");
        cleanup(&base, 3);
        let mut w = RotatingWriter::new(base.clone(), 1024, 3);
        w.write_line("2026-01-01 00:00:00.000 [conn] NOTICE connected\n");
        w.write_line("2026-01-01 00:00:00.001 [demo] DEBUG 42\n");
        w.flush();
        let contents = std::fs::read_to_string(&base).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("[conn] NOTICE connected"));
        assert!(lines[1].contains("[demo] DEBUG 42"));
        cleanup(&base, 3);
    }

    #[test]
    fn writer_rotates_when_over_max_bytes() {
        let base = tmp("rotate.log");
        cleanup(&base, 2);
        // 20-byte cap, keep 2 rotated files. Each "line-N\n" is 7 bytes, so the
        // 3rd line of a file crosses the cap and triggers rotation.
        let mut w = RotatingWriter::new(base.clone(), 20, 2);
        for i in 0..7 {
            w.write_line(&format!("line-{i}\n"));
        }
        w.flush();
        // Layout: base=[6], base.1=[3,4,5], base.2=[0,1,2]; base.3 never exists.
        assert!(rotated(&base, 1).exists(), ".1 exists");
        assert!(rotated(&base, 2).exists(), ".2 exists");
        assert!(!rotated(&base, 3).exists(), "oldest beyond max_files is dropped");
        assert!(std::fs::read_to_string(&base).unwrap().contains("line-6"), "newest in current");
        assert!(std::fs::read_to_string(rotated(&base, 2)).unwrap().contains("line-0"), "oldest kept in .2");
        cleanup(&base, 2);
    }

    #[test]
    fn scan_searches_across_rotated_files_chronologically() {
        let base = tmp("scan.log");
        cleanup(&base, 2);
        // Oldest → newest lives across .2, .1, base.
        std::fs::write(rotated(&base, 2), "a error 0\nb ok 1\n").unwrap();
        std::fs::write(rotated(&base, 1), "c error 2\nd ok 3\n").unwrap();
        std::fs::write(&base, "e error 4\nf ok 5\n").unwrap();

        let hits = scan_files(&base, 2, |l| l.contains("error"), 100);
        // Chronological: oldest file first, oldest line first.
        assert_eq!(hits, vec!["a error 0", "c error 2", "e error 4"]);
        cleanup(&base, 2);
    }

    #[test]
    fn scan_limit_keeps_most_recent_matches() {
        let base = tmp("scan_limit.log");
        cleanup(&base, 1);
        std::fs::write(rotated(&base, 1), "old error\n").unwrap();
        std::fs::write(&base, "new error\n").unwrap();
        // limit=1: newest-first scan stops after one match → the newest.
        let hits = scan_files(&base, 1, |l| l.contains("error"), 1);
        assert_eq!(hits, vec!["new error"]);
        cleanup(&base, 1);
    }

    #[test]
    fn record_via_background_thread_then_flush() {
        // Only this test drives the global writer; set a deterministic path first.
        let path = tmp("global.log");
        cleanup(&path, DEFAULT_MAX_FILES);
        // SAFETY: single-threaded setup before the writer thread is spawned.
        unsafe { std::env::set_var("SIM_CLI_MSG_LOG", &path); }

        record("conn", &Message::System { text: "connected".into(), level: LogLevel::Notice });
        record("demo", &Message::System { text: "42".into(), level: LogLevel::Debug });
        flush(); // block until the background thread has written to disk

        let contents = std::fs::read_to_string(&path).expect("log file written");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "one line per message: {contents:?}");
        assert!(lines[0].contains("[conn] NOTICE connected"));
        assert!(lines[1].contains("[demo] DEBUG 42"));
        cleanup(&path, DEFAULT_MAX_FILES);
    }
}
