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

/// Worker → Pusher 的内部事件。把「跑 Tool 逻辑」与「维护缓冲/推送 UI」解耦,
/// 让定时器(`tick`)所在的 worker 循环不被 UI 推送的 O(缓冲) 重活拖慢。
enum ToView {
    /// 追加一条消息(带**产生时刻**的时间戳:屏显 / 落盘 / 发生时刻三者一致)。
    Log(TimedMessage),
    /// 最新的 Tool 状态快照(状态面板 + badge)。
    State(ToolState),
    /// 清空缓冲(`clear` 命令)。
    Clear,
}

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

    let tick_ms = tool.tick_ms();
    let push_ms = tool.push_ms();

    // Worker → Pusher。无界通道:worker(定时器)永不因推送慢而阻塞。
    // Pusher 每条消息只做 O(1) 追加,O(缓冲) 的克隆只发生在周期性 push,故通道
    // 会被快速排空,积压有界。
    let (evt_tx, mut evt_rx) = mpsc::unbounded_channel::<ToView>();

    // ── Worker 任务:只跑 Tool 逻辑(tick + 命令)。────────────────────────
    // 循环里没有缓冲、没有 O(n) 拷贝 → tick 节拍尽量精确。
    let mut tool = tool;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(tick_ms));
        // 迟到不补发(不 burst),保持固定节拍 —— 对定时采集类 tool 更准。
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 即使无消息产生,也周期性刷新状态快照(某些 tool 状态变化不发消息)。
        let mut snap = tokio::time::interval(Duration::from_millis(push_ms));
        let tick_warn_threshold = Duration::from_millis(tick_ms).saturating_mul(10);
        let mut last_tick: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                    Some(text) => {
                        let parts: Vec<&str> = text.split_whitespace().collect();
                        if parts.is_empty() { continue; }
                        let cmd = parts[0];
                        let args: &[&str] = if parts.len() > 1 { &parts[1..] } else { &[] };
                        match cmd {
                            "help" => emit(&evt_tx, build_help(&cmds)),
                            "clear" => {
                                let _ = evt_tx.send(ToView::Clear);
                                emit(&evt_tx, vec![msg("conversation cleared", LogLevel::Notice)]);
                            }
                            _ => emit(&evt_tx, tool.handle(cmd, args)),
                        }
                        // 命令可能改了状态,刷新一次快照。
                        let _ = evt_tx.send(ToView::State(tool.snapshot()));
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    // 使用实际执行时刻,首个 tick 只建立基准。
                    let now = tokio::time::Instant::now();
                    let elapsed = last_tick.replace(now).map(|last| now.duration_since(last));
                    if let Some(elapsed) = elapsed {
                        if elapsed > tick_warn_threshold {
                            emit(&evt_tx, vec![msg(
                                &format!(
                                    "tick interval exceeded 10x period: period={} ms, actual={:.3} ms, threshold={:.3} ms",
                                    tick_ms,
                                    elapsed.as_secs_f64() * 1000.0,
                                    tick_warn_threshold.as_secs_f64() * 1000.0,
                                ),
                                LogLevel::Warn,
                            )]);
                        }
                    }
                    let msgs = tool.tick();
                    if !msgs.is_empty() { emit(&evt_tx, msgs); }
                }
                _ = snap.tick() => {
                    let _ = evt_tx.send(ToView::State(tool.snapshot()));
                }
            }
        }
    });

    // ── Pusher 任务:拥有 LogBuffer,做落盘 + 周期性快照推送(重活在这)。──
    tokio::spawn(async move {
        let mut log = LogBuffer::new(crate::log_buffer::default_max());
        let mut state = ToolState::default();
        let mut push = tokio::time::interval(Duration::from_millis(push_ms));

        loop {
            tokio::select! {
                evt = evt_rx.recv() => match evt {
                    Some(ToView::Log(tm)) => {
                        // 屏显与落盘共用同一时刻(在 worker 产生时已打好)。
                        crate::msg_log::record_at(tm.time, &name, &tm.msg);
                        log.push_at(tm.time, tm.msg);
                    }
                    Some(ToView::State(s)) => state = s,
                    Some(ToView::Clear) => log.clear(),
                    None => break, // worker 结束
                },
                _ = push.tick() => {
                    let _ = view_tx.send(ViewUpdate {
                        name: name.clone(),
                        messages: log.to_arc(),
                        evicted_lines: log.evicted_lines(),
                        buffer_total_lines: log.total_lines(),
                        state: state.clone(),
                    });
                }
            }
        }
    });

    ToolHandle { cmd_tx, view_rx }
}

/// 给一批消息打上「产生时刻」的时间戳,发给 pusher。
fn emit(tx: &mpsc::UnboundedSender<ToView>, msgs: Vec<Message>) {
    for m in msgs {
        let _ = tx.send(ToView::Log(TimedMessage { time: chrono::Local::now(), msg: m }));
    }
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

#[cfg(test)]
mod spawn_tests {
    use super::*;
    use std::time::Duration;

    /// 一个极简 tool:tick 产出消息、命令产出消息、状态带 badge。
    /// 用来验证 worker/pusher 双任务管线把三条路径都正确送达 UI。
    #[derive(Default)]
    struct TestTool;
    impl Tool for TestTool {
        fn commands(&self) -> Vec<Cmd> {
            vec![cmd("ping", "reply pong")]
        }
        fn handle(&mut self, c: &str, _a: &[&str]) -> Vec<Message> {
            if c == "ping" { vec![msg("pong", LogLevel::Info)] } else { vec![] }
        }
        fn tick(&mut self) -> Vec<Message> {
            vec![msg("t", LogLevel::Debug)]
        }
        fn snapshot(&self) -> ToolState {
            ToolState { badge: Some("live".into()), ..Default::default() }
        }
        fn tick_ms(&self) -> u64 { 5 }
        fn push_ms(&self) -> u64 { 10 }
    }

    fn system_texts(v: &ViewUpdate) -> Vec<String> {
        v.messages
            .iter()
            .filter_map(|tm| match &tm.msg {
                Message::System { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// tick(定时器)、snapshot(状态)、handle(命令)三条路径都经由
    /// 「worker → 无界通道 → pusher → watch」正确送达。
    #[tokio::test]
    async fn worker_pusher_pipeline_delivers_ticks_state_and_commands() {
        let cmds = Arc::new(build_cmds(TestTool.commands()));
        let mut handle = spawn("test".to_string(), TestTool, cmds);

        // 定时器输出 + 状态快照流过管线。
        tokio::time::sleep(Duration::from_millis(120)).await;
        {
            let v = handle.view_rx.borrow_and_update().clone();
            assert!(v.buffer_total_lines > 0, "tick 产出的消息应累积到缓冲");
            assert_eq!(v.state.badge.as_deref(), Some("live"), "状态快照应流过管线");
        }

        // 命令路径:输出应出现在缓冲里。
        handle.cmd_tx.send("ping".to_string()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let v = handle.view_rx.borrow().clone();
        assert!(
            system_texts(&v).iter().any(|t| t == "pong"),
            "命令输出应流过管线",
        );
    }

    /// clear 命令清空缓冲后,仍能继续接收后续 tick(管线未中断)。
    #[tokio::test]
    async fn clear_command_flows_and_pipeline_continues() {
        let cmds = Arc::new(build_cmds(TestTool.commands()));
        let mut handle = spawn("test".to_string(), TestTool, cmds);

        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.cmd_tx.send("clear".to_string()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // clear 后仍有 tick 继续进来,且"conversation cleared"通知出现过。
        let v = handle.view_rx.borrow_and_update().clone();
        assert!(v.buffer_total_lines > 0, "clear 后管线继续送 tick");
    }
}
