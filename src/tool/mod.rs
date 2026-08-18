//! 通用 Tool 框架：只定义 trait，不关心具体 task 类型。
//!
//! 框架负责：消息日志、滚动计数、select 循环、watch 推送。
//! Tool 负责：命令定义、业务逻辑、自定义状态快照。
//!
//! 新增 tool 只需实现 [`Tool`] trait + 在 `registry` 注册，无需改动框架代码。

pub mod registry;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::log_buffer::LogBuffer;
use crate::message::{LogLevel, Message, TimedMessage};

// ── 基础命令 ──────────────────────────────────────────────────────────

/// 命令树节点。`subs` 为空即叶子命令;非空即分组命令,可任意嵌套多层。
#[derive(Debug, Clone)]
pub struct Cmd {
    pub name: &'static str,
    pub desc: &'static str,
    pub subs: &'static [Cmd],
}

/// 叶子命令(无子命令)。
pub const fn cmd(name: &'static str, desc: &'static str) -> Cmd {
    Cmd { name, desc, subs: &[] }
}

/// 分组命令(带子命令,可继续嵌套)。
pub const fn group(name: &'static str, desc: &'static str, subs: &'static [Cmd]) -> Cmd {
    Cmd { name, desc, subs }
}

pub fn base_cmds() -> Vec<Cmd> {
    vec![
        cmd("help", "show commands"),
        cmd("clear", "clear log"),
        cmd("exit", "quit"),
    ]
}

// ── ToolState ──────────────────────────────────────────────────────────

/// Tool 自定义状态快照，框架透传给 UI。
#[derive(Debug, Clone, Default)]
pub struct ToolState {
    /// state panel 中显示的键值对。
    pub fields: Vec<(String, String)>,
    /// 为 `true` 时 tab 栏显示绿色圆点。
    pub active: bool,
    /// 状态栏 badge。为 `Some` 时替换默认的 "idle" 文字。
    pub badge: Option<String>,
}

// ── Tool trait ─────────────────────────────────────────────────────────

/// Tool 只需实现业务逻辑，框架管理消息日志和事件循环。
pub trait Tool: Send + 'static {
    /// 命令列表（不含 help / clear / exit，框架自动追加）。
    fn commands(&self) -> Vec<Cmd>;

    /// 处理用户命令。`args` 不含命令名本身。
    /// 返回的消息由框架写入 LogBuffer。
    fn handle(&mut self, cmd: &str, args: &[&str]) -> Vec<Message>;

    /// 定时调用，用于轮询 I/O 或周期性任务。
    fn tick(&mut self) -> Vec<Message> { vec![] }

    /// 自定义状态快照。
    fn snapshot(&self) -> ToolState { ToolState::default() }

    /// tick 间隔（毫秒）。覆盖可改变轮询频率。
    fn tick_ms(&self) -> u64 { 500 }

    /// snapshot 推送间隔（毫秒）。
    fn push_ms(&self) -> u64 { 100 }

    /// 运行时是否可用。返回 `false` 时该 tab 仍会显示,但呈灰色禁用态,
    /// 无法通过 ←/→ 切换进入,输入也不会转发给它。
    ///
    /// 与编译期 `#[cfg(...)]` 门控(见 `register_tools!`)互补:
    /// - 编译期决定某个 tool 是否**编入**二进制(不同平台不同集合);
    /// - 运行期 `available()` 决定已编入的 tool **此刻能否使用**
    ///   (可根据 `std::env::consts::OS/ARCH`、环境变量、探测结果动态判断)。
    ///
    /// 例:仅在 Linux 上启用某 tool
    /// ```ignore
    /// fn available(&self) -> bool { cfg!(target_os = "linux") }
    /// ```
    fn available(&self) -> bool { true }
}

/// 运行时统一禁用开关:环境变量 `SIM_CLI_DISABLED_TOOLS` 里(逗号分隔)
/// 列出的 tool 名会被标记为不可用,方便在不重新编译的情况下按平台/部署
/// 关掉某些 tab。与 [`Tool::available`] 取逻辑与。
fn runtime_disabled(name: &str) -> bool {
    std::env::var("SIM_CLI_DISABLED_TOOLS")
        .ok()
        .is_some_and(|list| name_in_list(&list, name))
}

/// `name` 是否出现在逗号分隔的禁用清单里(去空白、忽略大小写、跳过空项)。
fn name_in_list(list: &str, name: &str) -> bool {
    list.split(',')
        .map(str::trim)
        .any(|t| !t.is_empty() && t.eq_ignore_ascii_case(name))
}

// ── 框架内部类型 ──────────────────────────────────────────────────────

/// 框架持有的 tool 运行时。Tool 不感知这些字段。
struct ToolCtx {
    name: String,
    log: LogBuffer,
    tool: Box<dyn Tool>,
}

/// 推送给 UI 的单帧快照，框架自动填充消息和滚动信息。
#[derive(Debug, Clone)]
pub struct ViewUpdate {
    pub name: String,
    pub messages: Arc<Vec<TimedMessage>>,
    pub evicted_lines: u64,
    pub buffer_total_lines: u64,
    pub state: ToolState,
}

/// 外部持有的句柄。
pub struct ToolHandle {
    pub cmd_tx: mpsc::Sender<String>,
    pub view_rx: watch::Receiver<ViewUpdate>,
}

// ── 注册信息（供 tab 栏） ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub active: bool,
    /// 运行期是否可用。`false` 时 tab 灰显且无法切入。
    pub available: bool,
}

// ── spawn ──────────────────────────────────────────────────────────────

pub fn spawn(name: String, tool: impl Tool, cmds: Arc<Vec<Cmd>>) -> ToolHandle {
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let initial = ViewUpdate {
        name: name.clone(),
        messages: Arc::new(vec![]),
        evicted_lines: 0,
        buffer_total_lines: 0,
        state: tool.snapshot(),
    };
    let (view_tx, view_rx) = watch::channel(initial);

    let mut ctx = ToolCtx {
        name,
        log: LogBuffer::new(crate::log_buffer::default_max()),
        tool: Box::new(tool),
    };

    tokio::spawn(async move {
        let tick_ms = ctx.tool.tick_ms();
        let push_ms = ctx.tool.push_ms();
        let mut tick = tokio::time::interval(Duration::from_millis(tick_ms));
        let mut push = tokio::time::interval(Duration::from_millis(push_ms));

        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                    Some(text) => {
                        let parts: Vec<&str> = text.split_whitespace().collect();
                        if parts.is_empty() { continue; }
                        let cmd = parts[0];
                        let args: &[&str] = if parts.len() > 1 { &parts[1..] } else { &[] };

                        let msgs = match cmd {
                            "help" => build_help(&cmds),
                            "clear" => {
                                ctx.log.clear();
                                log_msg(&mut ctx.log, &ctx.name, msg("conversation cleared", LogLevel::Notice));
                                continue;
                            }
                            _ => ctx.tool.handle(cmd, args),
                        };
                        for m in msgs { log_msg(&mut ctx.log, &ctx.name, m); }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    for m in ctx.tool.tick() { log_msg(&mut ctx.log, &ctx.name, m); }
                }
                _ = push.tick() => {
                    let _ = view_tx.send(ViewUpdate {
                        name: ctx.name.clone(),
                        messages: ctx.log.to_arc(),
                        evicted_lines: ctx.log.evicted_lines(),
                        buffer_total_lines: ctx.log.total_lines(),
                        state: ctx.tool.snapshot(),
                    });
                }
            }
        }
    });

    ToolHandle { cmd_tx, view_rx }
}

fn build_cmds(mut own: Vec<Cmd>) -> Vec<Cmd> {
    let mut all = base_cmds();
    all.append(&mut own);
    all
}

fn build_help(cmds: &[Cmd]) -> Vec<Message> {
    let mut s = String::from("commands:\n");
    write_cmd_tree(&mut s, cmds, 0);
    s.push_str("\n<-/-> switch tab  ^C exit");
    vec![Message::System { text: s, level: LogLevel::Info }]
}

/// Render a command tree as an indented list, recursing into sub-commands.
fn write_cmd_tree(s: &mut String, cmds: &[Cmd], depth: usize) {
    for c in cmds {
        let indent = "  ".repeat(depth + 1);
        // Widen the name column less as we indent, so descriptions stay aligned.
        let width = 10usize.saturating_sub(depth * 2).max(1);
        s.push_str(&format!("{indent}{:<width$} - {}\n", c.name, c.desc));
        write_cmd_tree(s, c.subs, depth + 1);
    }
}

/// 创建一条系统消息。
pub fn msg(text: &str, level: LogLevel) -> Message {
    Message::System { text: text.into(), level }
}

/// 用同一时间戳把消息写入界面缓冲区并落盘到消息日志文件,
/// 保证屏幕上显示的时间与日志文件中的时间完全一致。
fn log_msg(log: &mut LogBuffer, tool: &str, m: Message) {
    let time = chrono::Local::now();
    crate::msg_log::record_at(time, tool, &m);
    log.push_at(time, m);
}

// ── 工厂函数 ──────────────────────────────────────────────────────────

use registry::ToolDef;

/// 声明 tool 模块并生成工厂函数。每个 tool 一行:"module::Type,"。
///
/// 每一行前面可加任意 `#[cfg(...)]` 属性做**编译期平台门控**——属性会同时
/// 作用于 `pub mod` 声明和工厂里对应的分支,因此被门控掉的 tool 在该平台上
/// 根本不会编入二进制,其 tab 也不会出现。示例:
///
/// ```ignore
/// register_tools! {
///     conn::ConnTool,
///     #[cfg(target_os = "linux")]           // 仅 Linux 编入
///     demo::DemoTool,
///     #[cfg(any(target_os = "linux", target_os = "windows"))]
///     ser::SerTool,
/// }
/// ```
///
/// `create` 返回 `(handle, cmds, available)`,其中 `available` 为**运行期**
/// 门控结果(见 [`Tool::available`] 与 [`runtime_disabled`])。
macro_rules! register_tools {
    ($( $(#[$attr:meta])* $mod:ident :: $ty:ident ),* $(,)?) => {
        $( $(#[$attr])* pub mod $mod; )*

        /// 根据 tool 名创建实例。由 Router 调用。
        /// 返回 `(句柄, 命令树, 运行期是否可用)`。
        pub fn create(def: &'static ToolDef) -> Option<(ToolHandle, Arc<Vec<Cmd>>, bool)> {
            $(
                $(#[$attr])*
                {
                    if def.name == stringify!($mod) {
                        let tool = $mod::$ty::new(def);
                        let available = tool.available() && !runtime_disabled(def.name);
                        let cmds = Arc::new(build_cmds(tool.commands()));
                        return Some((spawn(def.name.to_string(), tool, cmds.clone()), cmds, available));
                    }
                }
            )*
            None
        }
    };
}

register_tools! {
    conn::ConnTool,
    demo::DemoTool,
    // 编译期平台门控示例:echo server 仅在 Linux / Windows 上编入,
    // 其它平台既不编译也不显示该 tab。linux-arm 与 win 两个目标都命中此 cfg。
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    ser::SerTool,
}

#[cfg(test)]
mod gating_tests {
    use super::name_in_list;

    #[test]
    fn disabled_list_matches_by_name() {
        assert!(name_in_list("conn,ser", "conn"));
        assert!(name_in_list("conn,ser", "ser"));
        assert!(!name_in_list("conn,ser", "demo"));
    }

    #[test]
    fn disabled_list_trims_and_ignores_case_and_blanks() {
        assert!(name_in_list("  Conn , , SER ", "conn"));
        assert!(name_in_list("  Conn , , SER ", "ser"));
        assert!(!name_in_list("", "conn"));
        assert!(!name_in_list(" , ,", "conn"));
    }
}
