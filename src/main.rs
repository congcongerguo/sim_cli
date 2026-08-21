mod backend;
#[cfg(feature = "serve")]
mod client;
#[cfg(feature = "mock-llm")]
mod event;
mod filter;
mod frontend;
mod json_framer;
mod log_buffer;
mod scroll;
mod message;
mod msg_log;
#[cfg(feature = "mock-llm")]
mod mock_llm;
#[cfg(feature = "serve")]
mod protocol;
#[cfg(feature = "serve")]
mod serve;
mod terminal;
mod tool;
mod ui;

use anyhow::Result;
use tokio::sync::{mpsc, watch};

use crate::backend::{Command, ViewState};

/// 启动模式,由命令行参数决定。
enum RunMode {
    /// 本地单进程 TUI(默认,与历史行为一致)。
    Local,
    /// 无头后端 daemon(`--serve`),监听 socket。需 `serve` feature。
    /// (无 `serve` feature 时字段不被读取,仅用于给出友好报错。)
    #[cfg_attr(not(feature = "serve"), allow(dead_code))]
    Serve(String),
    /// 远端 TUI 客户端(`--connect <sock>`),连上无头后端。需 `serve` feature。
    #[cfg_attr(not(feature = "serve"), allow(dead_code))]
    Connect(String),
}

/// 极简参数解析(不引入 clap,保持体积)。
///
/// - `sim_cli`                     本地 TUI
/// - `sim_cli --serve [--socket P]` 无头后端(默认 socket 见 `default_sock_path`)
/// - `sim_cli --connect [P]`        远端 TUI(P 缺省用默认 socket)
fn parse_mode() -> RunMode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = RunMode::Local;
    let mut socket: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--serve" => mode = RunMode::Serve(String::new()),
            "--connect" => {
                // 可选地紧跟一个路径参数。
                let path = args.get(i + 1).filter(|a| !a.starts_with("--")).cloned();
                if let Some(p) = path {
                    mode = RunMode::Connect(p);
                    i += 1;
                } else {
                    mode = RunMode::Connect(String::new());
                }
            }
            "--socket" => {
                socket = args.get(i + 1).cloned();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    // 把 --socket 合并进 serve/connect 的路径。
    match mode {
        RunMode::Serve(_) => RunMode::Serve(socket.unwrap_or_default()),
        RunMode::Connect(p) if p.is_empty() => RunMode::Connect(socket.unwrap_or_default()),
        other => other,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match parse_mode() {
        RunMode::Local => run_local().await,
        #[cfg(feature = "serve")]
        RunMode::Serve(p) => {
            let path = if p.is_empty() { serve::default_sock_path() } else { p };
            serve::run(path).await
        }
        #[cfg(feature = "serve")]
        RunMode::Connect(p) => {
            let path = if p.is_empty() { serve::default_sock_path() } else { p };
            client::run(path).await
        }
        #[cfg(not(feature = "serve"))]
        RunMode::Serve(_) | RunMode::Connect(_) => {
            anyhow::bail!(
                "--serve / --connect require the `serve` feature: \
                 rebuild with `cargo build --features serve`"
            )
        }
    }
}

/// 本地单进程 TUI:后端与前端在同一进程,通过内存 channel 通信。
async fn run_local() -> Result<()> {
    let _guard = terminal::install()?;
    let mut term = terminal::new_terminal()?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let (view_tx, view_rx) = watch::channel(ViewState::initial());

    let backend_handle = tokio::spawn(backend::run(cmd_rx, view_tx));

    let mut fe = frontend::Frontend::new(cmd_tx, view_rx);
    let res = fe.run(&mut term).await;

    backend_handle.abort();
    drop(_guard);
    // Flush any buffered log records to disk before exiting.
    msg_log::flush();
    res
}
