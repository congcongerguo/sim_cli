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

/// 端点参数(平台无关的原始输入)。`--socket` 走 Unix socket(仅 unix),
/// `--tcp`(或形如 `host:port` 的目标)走 TCP(跨平台)。
#[derive(Default)]
#[cfg_attr(not(feature = "serve"), allow(dead_code))]
struct EndpointSpec {
    socket: Option<String>,
    tcp: Option<String>,
}

/// 启动模式,由命令行参数决定。
enum RunMode {
    /// 本地单进程 TUI(默认,与历史行为一致)。
    Local,
    /// 无头后端 daemon(`--serve`)。需 `serve` feature。
    #[cfg_attr(not(feature = "serve"), allow(dead_code))]
    Serve(EndpointSpec),
    /// 远端 TUI 客户端(`--connect`),连上无头后端。需 `serve` feature。
    #[cfg_attr(not(feature = "serve"), allow(dead_code))]
    Connect(EndpointSpec),
}

/// 极简参数解析(不引入 clap,保持体积)。
///
/// - `sim_cli`                      本地 TUI
/// - `sim_cli --serve [--socket P | --tcp A]`   无头后端
/// - `sim_cli --connect [目标] [--socket P | --tcp A]`  远端 TUI
///
/// 目标 / `--socket`:含 `:` 视为 TCP `host:port`,否则视为 Unix socket 路径。
fn parse_mode() -> RunMode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut serve = false;
    let mut connect = false;
    let mut spec = EndpointSpec::default();
    let mut i = 0;
    // 目标既可能来自 --connect 的位置参数,也可能来自 --socket/--tcp。
    let classify = |spec: &mut EndpointSpec, t: String| {
        if t.contains(':') {
            spec.tcp = Some(t);
        } else {
            spec.socket = Some(t);
        }
    };
    while i < args.len() {
        match args[i].as_str() {
            "--serve" => serve = true,
            "--connect" => {
                connect = true;
                if let Some(t) = args.get(i + 1).filter(|a| !a.starts_with("--")).cloned() {
                    classify(&mut spec, t);
                    i += 1;
                }
            }
            "--socket" => {
                if let Some(p) = args.get(i + 1).cloned() {
                    spec.socket = Some(p);
                    i += 1;
                }
            }
            "--tcp" => {
                if let Some(a) = args.get(i + 1).cloned() {
                    spec.tcp = Some(a);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if serve {
        RunMode::Serve(spec)
    } else if connect {
        RunMode::Connect(spec)
    } else {
        RunMode::Local
    }
}

/// 把原始端点参数解析成传输端点。`--tcp`/`host:port` 优先;否则 unix 用 socket
/// 路径,非 unix 平台(如 Windows)回退到默认 TCP 回环并给出提示。
#[cfg(feature = "serve")]
fn resolve_endpoint(spec: EndpointSpec) -> serve::Endpoint {
    if let Some(addr) = spec.tcp {
        return serve::Endpoint::Tcp(addr);
    }
    #[cfg(unix)]
    {
        let path = spec.socket.unwrap_or_else(serve::default_sock_path);
        serve::Endpoint::Unix(path)
    }
    #[cfg(not(unix))]
    {
        // 非 unix(如 Windows)无 Unix socket:回退到 TCP。若用户用 --socket 传了
        // host:port 也当 TCP 地址用,否则用默认回环地址。
        let addr = spec.socket.unwrap_or_else(serve::default_tcp_addr);
        eprintln!(
            "note: Unix sockets are unavailable on this platform; using TCP {addr}. \
             Pass --tcp <host:port> to change it."
        );
        serve::Endpoint::Tcp(addr)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    match parse_mode() {
        RunMode::Local => run_local().await,
        #[cfg(feature = "serve")]
        RunMode::Serve(spec) => serve::run(resolve_endpoint(spec)).await,
        #[cfg(feature = "serve")]
        RunMode::Connect(spec) => client::run(resolve_endpoint(spec)).await,
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
