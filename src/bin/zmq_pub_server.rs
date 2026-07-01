//! Companion ZMQ pub/sub server for end-to-end verification of `con zmq`.
//!
//! Plays both sides of the round trip so a single process tests both
//! directions against the CLI:
//!   - binds a PUB on `PUB_ADDR`        → the CLI's SUB receives these ticks.
//!   - connects a SUB to `CLI_PUB_ADDR` → receives what the CLI's `send` publishes.
//!
//! Run alongside the CLI:
//!     cargo run --bin zmq_pub_server
//! then in the CLI: `con zmq` (recv ticks) and `send` (publishes back here).
//!
//! The SUB runs in its own task with `no_connect_timeout`, so this server can
//! start before the CLI's PUB (bound only once `con zmq` runs) is up — the SUB
//! just retries until it appears, while the PUB keeps publishing ticks.

use std::time::Duration;

use zeromq::{PubSocket, Socket, SocketOptions, SocketRecv, SocketSend, SubSocket};

/// Server binds its PUB here; the CLI's SUB connects to this (the default
/// `con zmq` address).
const PUB_ADDR: &str = "tcp://127.0.0.1:5555";
/// The CLI binds its PUB here; this server connects a SUB to receive the CLI's
/// published `send` messages.
const CLI_PUB_ADDR: &str = "tcp://127.0.0.1:5556";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut pub_ = PubSocket::new();
    pub_.bind(PUB_ADDR).await?;

    // SUB in its own task: connect with no timeout so it retries until the CLI's
    // PUB appears, without blocking the PUB's tick loop above.
    tokio::spawn(async move {
        let mut sub_opts = SocketOptions::default();
        sub_opts.no_connect_timeout();
        let mut sub = SubSocket::with_options(sub_opts);
        if let Err(e) = sub.connect(CLI_PUB_ADDR).await {
            eprintln!("sub connect failed: {e}");
            return;
        }
        if let Err(e) = sub.subscribe("").await {
            eprintln!("sub subscribe failed: {e}");
            return;
        }
        println!("sub< listening on {CLI_PUB_ADDR}");
        while let Ok(m) = sub.recv().await {
            let bytes: Vec<u8> = m.into_vec().into_iter().flatten().collect();
            println!("sub< {}", String::from_utf8_lossy(&bytes));
        }
    });

    println!("zmq pub server: publishing on {PUB_ADDR}, subscribing on {CLI_PUB_ADDR}");
    let mut seq = 0u64;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        seq += 1;
        let msg = serde_json::json!({
            "seq": seq,
            "msg": format!("tick {seq}"),
        })
        .to_string();
        let _ = pub_.send(msg.clone().into()).await;
        println!("pub> {msg}");
    }
}
