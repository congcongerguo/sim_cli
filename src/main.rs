mod backend;
mod commands;
mod event;
mod frontend;
mod json_framer;
mod message;
mod mock_llm;
mod terminal;
mod transport;
mod ui;

use anyhow::Result;
use clap::Parser;
use tokio::sync::{mpsc, watch};

use crate::backend::{Command, ViewState};

#[derive(Parser, Debug)]
#[command(name = "sim_cli", about = "Claude Code 风格交互 CLI 演示")]
struct Args {
    #[arg(long, default_value = "mock-claude")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let _guard = terminal::install()?;
    let mut term = terminal::new_terminal()?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let (view_tx, view_rx) = watch::channel(ViewState::initial(args.model.clone()));

    let backend_handle = tokio::spawn(backend::run(cmd_rx, view_tx, args.model));

    let mut fe = frontend::Frontend::new(cmd_tx, view_rx);
    let res = fe.run(&mut term).await;

    backend_handle.abort();
    drop(_guard);
    res
}
