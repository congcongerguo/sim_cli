mod backend;
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
mod proto;
mod terminal;
mod tool;
mod transport;
mod ui;

use anyhow::Result;
use tokio::sync::{mpsc, watch};

use crate::backend::{Command, ViewState};

#[tokio::main]
async fn main() -> Result<()> {

    let _guard = terminal::install()?;
    let mut term = terminal::new_terminal()?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let (view_tx, view_rx) = watch::channel(ViewState::initial());

    let backend_handle = tokio::spawn(backend::run(cmd_rx, view_tx));

    let mut fe = frontend::Frontend::new(cmd_tx, view_rx);
    let res = fe.run(&mut term).await;

    backend_handle.abort();
    drop(_guard);
    res
}
