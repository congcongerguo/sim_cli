//! 后端模块：管理所有 Tool 的路由、命令分发和视图状态聚合。
//!
//! ## 架构
//! ```text
//! Frontend ──Command──▶ Router ──命令文本──▶ Tool (spawn 的异步任务)
//! Frontend ◀─ViewState─ Router ◀─ViewUpdate─ Tool (通过 watch channel)
//! ```
//!
//! Router 是后端的核心：
//! - 接收前端发来的 [`Command`]（用户输入、tab 切换、权限选择）
//! - 将命令转发给当前活跃的 Tool
//! - 从每个 Tool 的 watch channel 拉取 [`ViewUpdate`]，聚合为 [`ViewState`] 推送给前端

mod chat;
pub mod conn;
mod llm;
mod modal;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::message::{LogLevel, Message, TimedMessage};
use crate::tool;
use crate::tool::{ToolHandle, ToolInfo, ToolState};

pub use chat::Mode;
pub use modal::{ModalChoice, ModalRequest};
pub use tool::registry::TOOL_DEFS;

/// 前端发给后端的命令。
///
/// 当前支持三种命令：用户文本输入、Tab 切换、权限弹窗选择。
#[derive(Debug)]
pub enum Command {
    /// 用户在输入框中按下回车的文本（可能包含命令或对话内容）
    Input(String),
    /// 用户按 ←/→ 切换到指定名称的 tab
    TagSwitch(String),
    /// 用户在权限弹窗中的选择（Yes / No / Always）
    Permission(#[allow(dead_code)] ModalChoice),
}

/// 后端推送给前端的完整视图状态。
///
/// 前端每一帧都会收到一份 [`ViewState`] 快照，直接渲染即可，
/// 无需自行维护任何状态。
#[derive(Debug, Clone)]
pub struct ViewState {
    /// 当前活跃 tab 的消息列表（只读共享，避免拷贝）
    pub messages: Arc<Vec<TimedMessage>>,
    /// 当前模式：Normal 或 Plan
    pub mode: Mode,
    /// 是否正在流式输出（用于显示打字动画）
    pub streaming: bool,
    /// 当前权限弹窗的内容，`None` 表示不显示弹窗
    pub modal: Option<ModalRequest>,
    /// 是否应该退出程序
    pub should_quit: bool,
    /// 当前 Tool 的自定义状态（状态面板 + badge）
    pub state: ToolState,
    /// 所有 tab 的信息列表（名称 + 活跃指示）
    pub tools: Arc<Vec<ToolInfo>>,
    /// 当前活跃 tab 的索引
    pub active_index: usize,
    /// 当前活跃 tab 的命令列表（用于 help 显示）
    pub active_cmds: Arc<Vec<tool::Cmd>>,
    /// 因容量限制被驱逐的行数（用于滚动偏移计算）
    pub evicted_lines: u64,
    /// buffer 历史总行数（含已驱逐的）
    pub buffer_total_lines: u64,
}

impl ViewState {
    /// 构造初始视图状态。
    ///
    /// 根据 [`TOOL_DEFS`] 注册表生成 tab 列表，并将第一个 tool 的提示信息
    /// 作为初始欢迎消息。
    pub fn initial() -> Self {
        let tools: Vec<ToolInfo> = TOOL_DEFS.iter()
            .map(|d| ToolInfo { name: d.name.into(), active: false })
            .collect();
        let first = TOOL_DEFS.first();
        let msg = match first {
            Some(d) => format!("[{}] {} - type 'help' for commands", d.name, d.hint),
            None => "no tools configured - check tasks.toml and features".to_string(),
        };
        Self {
            messages: Arc::new(vec![
                TimedMessage {
                    time: chrono::Local::now(),
                    msg: Message::System { text: msg, level: LogLevel::Notice },
                },
            ]),
            mode: Mode::Normal,
            streaming: false,
            modal: None,
            should_quit: false,
            state: ToolState::default(),
            tools: Arc::new(tools),
            active_index: 0,
            active_cmds: Arc::new(vec![]),
            evicted_lines: 0,
            buffer_total_lines: 0,
        }
    }
}

/// 路由器中单个 Tool 的条目：名称 + 运行时句柄 + 命令列表。
struct ToolEntry {
    name: String,
    handle: ToolHandle,
    cmds: Arc<Vec<tool::Cmd>>,
}

/// 后端路由器。
///
/// 持有所有 Tool 实例，负责：
/// - 命令路由（用户输入 → 当前活跃 Tool）
/// - Tab 切换
/// - 视图聚合（从 Tool watch channel 读取并构建 ViewState）
/// - 退出信号
/// - 权限弹窗子系统的生命周期
pub struct Router {
    /// 所有已注册的 Tool 条目
    tools: Vec<ToolEntry>,
    /// 当前活跃 Tool 的索引
    active: usize,
    /// 退出标志
    should_quit: bool,
    /// 权限弹窗子系统
    modal: modal::ModalSubsystem,
}

impl Router {
    /// 从 [`TOOL_DEFS`] 注册表创建所有 Tool 实例并构建路由器。
    ///
    /// ## Panics
    /// 当没有任何 Tool 被成功创建时 panic（说明配置有问题）。
    pub fn new() -> Self {
        let tools: Vec<ToolEntry> = TOOL_DEFS.iter().filter_map(|def| {
            tool::create(def).map(|(handle, cmds)| ToolEntry { name: def.name.to_string(), handle, cmds })
        }).collect();
        assert!(!tools.is_empty(), "no tools created — check features and tasks.toml");
        Self { tools, active: 0, should_quit: false, modal: modal::ModalSubsystem::new() }
    }

    /// 从所有 Tool 的 watch channel 读取最新快照，构建完整视图状态。
    fn build_view(&self) -> ViewState {
        let tool_infos: Vec<ToolInfo> = self.tools.iter().map(|t| {
            let snap = t.handle.view_rx.borrow();
            ToolInfo { name: snap.name.clone(), active: snap.state.active }
        }).collect();

        let active = &self.tools[self.active];
        let snap = active.handle.view_rx.borrow().clone();

        ViewState {
            messages: snap.messages,
            mode: Mode::Normal,
            streaming: false,
            modal: self.modal.request.clone(),
            should_quit: self.should_quit,
            state: snap.state,
            tools: Arc::new(tool_infos),
            active_index: self.active,
            active_cmds: active.cmds.clone(),
            evicted_lines: snap.evicted_lines,
            buffer_total_lines: snap.buffer_total_lines,
        }
    }
}

/// 后端主循环。
///
/// 启动路由器，进入 select 循环：
/// - 接收前端命令 → 转发给当前 Tool 或处理 exit / tab 切换
/// - 定时 tick → 刷新视图推送给前端
/// - 收到 exit 或 channel 关闭 → 退出循环
pub async fn run(
    mut cmd_rx: mpsc::Receiver<Command>,
    view_tx: watch::Sender<ViewState>,
) {
    let mut router = Router::new();
    // 每 100ms 刷新一次视图推送给前端
    let mut tick = tokio::time::interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => match maybe_cmd {
                Some(Command::Input(text)) => {
                    if text.trim() == "exit" {
                        router.should_quit = true;
                    } else {
                        // 将用户输入转发给当前活跃 Tool
                        let _ = router.tools[router.active].handle.cmd_tx.try_send(text);
                    }
                }
                Some(Command::TagSwitch(name)) => {
                    // 按名称查找 tab 并切换
                    if let Some(pos) = router.tools.iter().position(|t| t.name == name) {
                        router.active = pos;
                    }
                }
                Some(Command::Permission(_)) => {}
                None => break, // 前端已关闭 channel，退出
            },
            _ = tick.tick() => {}
        }

        // 每轮循环都推送最新视图
        let _ = view_tx.send(router.build_view());
        if router.should_quit { break; }
    }
}
