//! 前后端分离的线协议(wire protocol)。
//!
//! 传输格式:**NDJSON** —— 每行一个 JSON 对象。只依赖 `serde` / `serde_json`,
//! 跨语言、可读、易调试(`nc` / python 皆可直接对接)。
//!
//! 设计要点(见 `docs/service-design.md`):
//! - 内部类型([`crate::backend::ViewState`]、[`crate::tool::Cmd`] 等)带 `Arc`
//!   和 `&'static str`,不便直接序列化。这里定义**owned DTO**,并提供与内部
//!   类型的相互转换,序列化边界干净。
//! - [`ServerMsg`] 把状态拆成**共享状态**([`Shared`],广播,很小)与
//!   **视口内容**([`Window`],每连接,只发一屏),避免每帧把整个缓冲发过网络。
//!
//! 该模块仅在 `serve` feature 下编入。

use serde::{Deserialize, Serialize};

use crate::message::{LogLevel, Message, TimedMessage, ToolCall, ToolStatus};

/// 协议版本号。握手时随 [`ServerMsg::Hello`] 下发,便于版本追溯与兼容管理。
pub const PROTOCOL_VERSION: u32 = 1;

// ── 客户端 → 服务端 ──────────────────────────────────────────────────────

/// 客户端发给服务端的消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// 用户输入的一行文本(命令或对话)。对应 [`crate::backend::Command::Input`]。
    Input { text: String },
    /// 切换到指定名称的 tab。对应 [`crate::backend::Command::TagSwitch`]。
    TagSwitch { name: String },
    /// 权限弹窗选择。对应 [`crate::backend::Command::Permission`]。
    Permission { choice: Choice },
    /// 视口请求:声明本连接想看的窗口(每连接独立)。
    View(ViewReq),
    /// 在**服务端**(板子)落盘归档里 grep;结果作为该连接的视口内容返回。
    Grep { expr: String },
    /// 清除 grep,回到实时视图。
    Ungrep,
}

/// 权限弹窗选择。镜像 [`crate::backend::ModalChoice`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Yes,
    No,
    Always,
}

/// 视口请求。服务端据此为该连接计算并返回 [`Window`]。
///
/// 采用**按范围取**(start + count),一套协议同时服务 TUI 与 Web:
/// - TUI:`count` = 终端行数,`start` = 滚动偏移;
/// - Web:`count` = 容器高度 / 行高(+overscan),`start` = scrollTop / 行高。
///
/// `count == 0` 表示**全量模式**:返回当前 tab 过滤后的全部消息(远端 TUI 用它
/// 复用本地那套滚动/渲染;体积等价于本地进程内持有整份缓冲)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ViewReq {
    /// 绝对行偏移(从第一条消息算起,含已驱逐前缀)。`follow` 为真时忽略。
    #[serde(default)]
    pub start: u64,
    /// 想要的行数。`0` = 全量模式(见结构体说明)。
    #[serde(default)]
    pub count: u32,
    /// 跟随底部:忽略 `start`,总是取最新一屏。
    #[serde(default)]
    pub follow: bool,
    /// include 过滤表达式(只显示匹配);`None`/空 = 不过滤。
    #[serde(default)]
    pub filter: Option<String>,
    /// exclude 过滤表达式(隐藏匹配);叠加在 include 之上。
    #[serde(default)]
    pub exclude: Option<String>,
}

// ── 服务端 → 客户端 ──────────────────────────────────────────────────────

/// 服务端发给客户端的消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// 握手:连上后服务端先发,做版本协商。
    Hello { version: String, protocol: u32 },
    /// 共享状态(广播,很小):tab 列表、命令树、状态面板、mode、弹窗等。
    Shared(Shared),
    /// 视口内容(每连接):当前 tab 的一段(或全部)消息 + 滚动几何。
    Window(Window),
    /// 出错(非法命令 / 非法过滤表达式等);服务不会因此中断。
    Error { code: String, detail: String },
}

/// 共享状态:所有连接一致的部分。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shared {
    pub tools: Vec<ToolInfoDto>,
    pub active_index: usize,
    pub active_cmds: Vec<CmdDto>,
    pub state: ToolStateDto,
    /// `"normal"` 或 `"plan"`。
    pub mode: String,
    pub streaming: bool,
    pub modal: Option<ModalDto>,
    pub should_quit: bool,
}

/// tab 信息。镜像 [`crate::tool::ToolInfo`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInfoDto {
    pub name: String,
    pub active: bool,
    pub available: bool,
}

/// 命令树节点(owned)。镜像 [`crate::tool::Cmd`],把 `&'static` 转成 owned。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CmdDto {
    pub name: String,
    pub desc: String,
    #[serde(default)]
    pub subs: Vec<CmdDto>,
}

impl CmdDto {
    /// 从内部 [`crate::tool::Cmd`] 树递归构造 owned DTO。
    pub fn from_cmd(c: &crate::tool::Cmd) -> Self {
        Self {
            name: c.name.to_string(),
            desc: c.desc.to_string(),
            subs: c.subs.iter().map(CmdDto::from_cmd).collect(),
        }
    }
}

/// Tool 自定义状态。镜像 [`crate::tool::ToolState`]。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolStateDto {
    pub fields: Vec<(String, String)>,
    pub active: bool,
    pub badge: Option<String>,
}

/// 权限弹窗内容。镜像 [`crate::backend::ModalRequest`](仅展示所需字段)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModalDto {
    pub tool_name: String,
    pub args_preview: String,
}

/// 视口内容:一段消息 + 让客户端正确渲染/滚动所需的几何信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    /// 该视口对应的 tab 名。
    pub tab: String,
    /// `messages` 中第一条消息的**绝对行偏移**(含已驱逐前缀)。
    pub start: u64,
    /// 这一段消息(全量模式即全部过滤后消息)。
    pub messages: Vec<MsgDto>,
    /// 过滤后可显示的总行数(用于滚动条 / 翻页)。
    pub total: u64,
    /// 已驱逐行前缀(绝对坐标基准);过滤视图下为 0。
    pub evicted: u64,
    /// 是否跟随底部。
    pub follow: bool,
    /// 当前生效的 include 过滤(回显)。
    pub filter: Option<String>,
    /// 当前生效的 exclude 过滤(回显)。
    pub exclude: Option<String>,
    /// 上次 include 过滤解析错误(若有)。
    pub filter_error: Option<String>,
    /// 上次 exclude 过滤解析错误(若有)。
    pub exclude_error: Option<String>,
    /// 过滤生效时的 (显示条数, 总条数),供状态栏显示。
    pub matched: Option<(usize, usize)>,
    /// grep 生效时的 (表达式, 命中条数)。
    pub grep: Option<(String, usize)>,
    /// 上次 grep 解析错误(若有)。
    pub grep_error: Option<String>,
}

/// 带时间戳的消息(owned DTO)。镜像 [`crate::message::TimedMessage`]。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MsgDto {
    /// 本地时间的 Unix 毫秒(可跨语言/无损往返)。
    pub time_ms: i64,
    pub body: MsgBody,
}

/// 消息体。镜像 [`crate::message::Message`] 的三个变体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MsgBody {
    System { text: String, level: String },
    Assistant { text: String, streaming: bool },
    Tool { name: String, args_preview: String, status: String, output: String },
}

// ── 转换:内部类型 ↔ DTO ─────────────────────────────────────────────────

fn level_to_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Notice => "notice",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
    }
}

fn level_from_str(s: &str) -> LogLevel {
    match s {
        "error" => LogLevel::Error,
        "warn" => LogLevel::Warn,
        "notice" => LogLevel::Notice,
        "debug" => LogLevel::Debug,
        _ => LogLevel::Info,
    }
}

fn status_to_str(s: &ToolStatus) -> &'static str {
    match s {
        ToolStatus::AwaitingPermission => "awaiting",
        ToolStatus::Running => "running",
        ToolStatus::Done => "done",
        ToolStatus::Denied => "denied",
    }
}

fn status_from_str(s: &str) -> ToolStatus {
    match s {
        "awaiting" => ToolStatus::AwaitingPermission,
        "done" => ToolStatus::Done,
        "denied" => ToolStatus::Denied,
        _ => ToolStatus::Running,
    }
}

impl MsgDto {
    /// 从内部 [`TimedMessage`] 构造 DTO。
    pub fn from_timed(tm: &TimedMessage) -> Self {
        let time_ms = tm.time.timestamp_millis();
        let body = match &tm.msg {
            Message::System { text, level } => MsgBody::System {
                text: text.clone(),
                level: level_to_str(*level).to_string(),
            },
            Message::Assistant { text, streaming } => MsgBody::Assistant {
                text: text.clone(),
                streaming: *streaming,
            },
            Message::Tool(t) => MsgBody::Tool {
                name: t.name.clone(),
                args_preview: t.args_preview.clone(),
                status: status_to_str(&t.status).to_string(),
                output: t.output.clone(),
            },
        };
        Self { time_ms, body }
    }

    /// 还原为内部 [`TimedMessage`](客户端渲染用)。
    pub fn to_timed(&self) -> TimedMessage {
        use chrono::TimeZone;
        let time = chrono::Local
            .timestamp_millis_opt(self.time_ms)
            .single()
            .unwrap_or_else(chrono::Local::now);
        let msg = match &self.body {
            MsgBody::System { text, level } => Message::System {
                text: text.clone(),
                level: level_from_str(level),
            },
            MsgBody::Assistant { text, streaming } => Message::Assistant {
                text: text.clone(),
                streaming: *streaming,
            },
            MsgBody::Tool { name, args_preview, status, output } => Message::Tool(ToolCall {
                name: name.clone(),
                args_preview: args_preview.clone(),
                status: status_from_str(status),
                output: output.clone(),
            }),
        };
        TimedMessage { time, msg }
    }
}

impl From<Choice> for crate::backend::ModalChoice {
    fn from(c: Choice) -> Self {
        match c {
            Choice::Yes => crate::backend::ModalChoice::Yes,
            Choice::No => crate::backend::ModalChoice::No,
            Choice::Always => crate::backend::ModalChoice::Always,
        }
    }
}

impl From<crate::backend::ModalChoice> for Choice {
    fn from(c: crate::backend::ModalChoice) -> Self {
        match c {
            crate::backend::ModalChoice::Yes => Choice::Yes,
            crate::backend::ModalChoice::No => Choice::No,
            crate::backend::ModalChoice::Always => Choice::Always,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{LogLevel, Message, TimedMessage};

    fn sys(text: &str, level: LogLevel) -> TimedMessage {
        TimedMessage { time: chrono::Local::now(), msg: Message::System { text: text.into(), level } }
    }

    #[test]
    fn msg_roundtrips_through_dto() {
        for tm in [
            sys("hello", LogLevel::Error),
            sys("multi\nline", LogLevel::Notice),
            TimedMessage {
                time: chrono::Local::now(),
                msg: Message::Assistant { text: "hi".into(), streaming: true },
            },
            TimedMessage {
                time: chrono::Local::now(),
                msg: Message::Tool(ToolCall {
                    name: "ls".into(),
                    args_preview: "-la".into(),
                    status: ToolStatus::Done,
                    output: "a\nb".into(),
                }),
            },
        ] {
            let dto = MsgDto::from_timed(&tm);
            let back = dto.to_timed();
            // Millisecond-truncated timestamp round-trips.
            assert_eq!(back.time.timestamp_millis(), tm.time.timestamp_millis());
            // Body content preserved.
            assert_eq!(MsgDto::from_timed(&back), dto);
        }
    }

    #[test]
    fn client_msg_json_shape_is_stable() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"input","text":"con zmq"}"#).unwrap();
        assert!(matches!(m, ClientMsg::Input { text } if text == "con zmq"));
        let m: ClientMsg =
            serde_json::from_str(r#"{"type":"view","count":40,"follow":true}"#).unwrap();
        assert!(matches!(m, ClientMsg::View(v) if v.count == 40 && v.follow));
        let m: ClientMsg = serde_json::from_str(r#"{"type":"permission","choice":"always"}"#).unwrap();
        assert!(matches!(m, ClientMsg::Permission { choice: Choice::Always }));
    }

    #[test]
    fn cmd_tree_converts() {
        use crate::tool::Cmd;
        const SUBS: &[Cmd] = &[
            Cmd { name: "zmq", desc: "z", subs: &[] },
            Cmd { name: "tcp", desc: "t", subs: &[] },
        ];
        const TREE: Cmd = Cmd { name: "con", desc: "transport", subs: SUBS };
        let dto = CmdDto::from_cmd(&TREE);
        assert_eq!(dto.name, "con");
        assert_eq!(dto.subs.len(), 2);
        assert_eq!(dto.subs[0].name, "zmq");
    }
}
