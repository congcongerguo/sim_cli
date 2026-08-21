//! 远端 TUI 客户端。`sim_cli --connect <sock>` 连上无头后端([`crate::serve`]),
//! 复用现有 [`crate::frontend::Frontend`] 渲染 —— 前端代码零改动。
//!
//! ## 桥接
//! ```text
//!   socket ──ServerMsg──▶ reader ──ViewState(watch)──▶ Frontend
//!   socket ◀─ClientMsg── writer ◀──Command(mpsc)────── Frontend
//! ```
//! 前端消费 [`ViewState`] / 产出 [`Command`],与本地模式完全一致;这里把这两条
//! 进程内 channel 桥接到 socket。远端 TUI 用**全量模式**(`count == 0`)订阅当前
//! tab 的全部消息,滚动 / include-exclude 过滤仍由前端本地处理(复用久经测试的
//! 那套逻辑)。
//!
//! 已知边界(阶段三范围):前端的 `grep` 命令仍走本地 [`crate::msg_log::scan`],
//! 在远端客户端上搜的是本机归档而非板子归档;把它接到 [`ClientMsg::Grep`] 需要
//! 前端一个小钩子,留待后续。服务端 grep 已实现,脚本/Web 客户端可直接用。

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, watch};

use crate::backend::{Command, Mode, ModalRequest, ViewState};
use crate::protocol::{ClientMsg, CmdDto, ServerMsg, Shared, ViewReq, Window};
use crate::tool::{Cmd, ToolInfo, ToolState};

/// 连接远端后端并运行本地 TUI。
pub async fn run(sock_path: String) -> Result<()> {
    let stream = UnixStream::connect(&sock_path)
        .await
        .with_context(|| format!("connect {sock_path}"))?;
    let (read_half, mut write_half) = stream.into_split();

    // 前端消费的 ViewState(watch)与产出的 Command(mpsc)。
    let (view_tx, view_rx) = watch::channel(ViewState::initial());
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(64);

    // 写端桥:Command → ClientMsg → socket。先发一个全量视口请求。
    let writer = tokio::spawn(async move {
        let hello = serde_json::to_string(&ClientMsg::View(ViewReq {
            count: 0,
            follow: true,
            ..Default::default()
        }))
        .unwrap();
        if write_line(&mut write_half, &hello).await.is_err() {
            return;
        }
        while let Some(cmd) = cmd_rx.recv().await {
            let msg = match cmd {
                Command::Input(text) => ClientMsg::Input { text },
                Command::TagSwitch(name) => ClientMsg::TagSwitch { name },
                Command::Permission(choice) => ClientMsg::Permission { choice: choice.into() },
            };
            let Ok(line) = serde_json::to_string(&msg) else { continue };
            if write_line(&mut write_half, &line).await.is_err() {
                break;
            }
        }
    });

    // 读端桥:ServerMsg → 合成 ViewState → watch。
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(read_half).lines();
        let mut shared: Option<Shared> = None;
        let mut window: Option<Window> = None;
        // 命令树很小且极少变化,只在 Shared 变化时重建(避免每帧 Box::leak)。
        let mut cmds: Arc<Vec<Cmd>> = Arc::new(vec![]);
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<ServerMsg>(&line) {
                Ok(ServerMsg::Hello { .. }) => {}
                Ok(ServerMsg::Shared(s)) => {
                    cmds = Arc::new(s.active_cmds.iter().map(leak_cmd).collect());
                    shared = Some(s);
                }
                Ok(ServerMsg::Window(w)) => window = Some(w),
                Ok(ServerMsg::Error { .. }) => {}
                Err(_) => continue, // 协议错误:忽略这一行,不拖垮连接
            }
            if let (Some(s), Some(w)) = (&shared, &window) {
                if view_tx.send(to_view_state(s, w, cmds.clone())).is_err() {
                    break; // 前端已退出
                }
            }
        }
        // socket 关闭:让前端退出。
        let mut last = view_tx.borrow().clone();
        last.should_quit = true;
        let _ = view_tx.send(last);
    });

    // 运行前端(拥有终端)。
    let _guard = crate::terminal::install()?;
    let mut term = crate::terminal::new_terminal()?;
    let mut fe = crate::frontend::Frontend::new(cmd_tx, view_rx);
    let res = fe.run(&mut term).await;
    drop(_guard);

    reader.abort();
    writer.abort();
    res
}

async fn write_line(w: &mut (impl AsyncWriteExt + Unpin), line: &str) -> Result<()> {
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

/// 由共享状态 + 视口内容合成前端消费的 [`ViewState`]。`cmds` 为预构建的命令树
/// (仅在 [`Shared`] 变化时重建,见调用处),避免每帧泄漏。
fn to_view_state(shared: &Shared, window: &Window, cmds: Arc<Vec<Cmd>>) -> ViewState {
    let messages: Vec<_> = window.messages.iter().map(|m| m.to_timed()).collect();
    ViewState {
        messages: Arc::new(messages),
        mode: if shared.mode == "plan" { Mode::Plan } else { Mode::Normal },
        streaming: shared.streaming,
        modal: shared.modal.as_ref().map(|m| ModalRequest {
            tool_index: 0,
            tool_name: m.tool_name.clone(),
            args_preview: m.args_preview.clone(),
        }),
        should_quit: shared.should_quit,
        state: ToolState {
            fields: shared.state.fields.clone(),
            active: shared.state.active,
            badge: shared.state.badge.clone(),
        },
        tools: Arc::new(
            shared
                .tools
                .iter()
                .map(|t| ToolInfo { name: t.name.clone(), active: t.active, available: t.available })
                .collect(),
        ),
        active_index: shared.active_index,
        active_cmds: cmds,
        evicted_lines: window.evicted,
        buffer_total_lines: window.total,
    }
}

/// 把 owned [`CmdDto`] 树转成前端要求的 `&'static` [`Cmd`] 树。
///
/// [`Cmd`] 用 `&'static str`(源于各 Tool 的编译期常量命令树),远端收到的是
/// owned String,只能靠 `Box::leak` 提升为 `'static`。命令树很小且**极少变化**
/// (仅切 tab 时),泄漏量可忽略;上层仅在 [`Shared`] 变化时才重建,避免每帧泄漏。
fn leak_cmd(d: &CmdDto) -> Cmd {
    let subs: Vec<Cmd> = d.subs.iter().map(leak_cmd).collect();
    Cmd {
        name: Box::leak(d.name.clone().into_boxed_str()),
        desc: Box::leak(d.desc.clone().into_boxed_str()),
        subs: Box::leak(subs.into_boxed_slice()),
    }
}
