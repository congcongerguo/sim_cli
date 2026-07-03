//! Companion ZMQ pub/sub server for end-to-end verification of `con zmq`.
//!
//! Publishes every tick on BOTH wire encodings so a single run demonstrates
//! the topic-distinguishes-encoding scheme:
//!   - topic "json" → `["json", {"seq":N,"msg":".."}]`
//!   - topic "pb"   → `["pb",  <proto-encoded Tick>]`
//!
//! Also connects a SUB to the CLI's PUB (bound on `con zmq`) to receive what
//! the CLI's `send` publishes on topic "json".
//!
//! Run alongside the CLI:
//!     cargo run --bin zmq_pub_server
//! then in the CLI: `con zmq` (recv both encodings) and `send` (publishes back
//! here on "json").
//!
//! The SUB runs in its own task with `no_connect_timeout`, so this server can
//! start before the CLI's PUB (bound only once `con zmq` runs) is up — the SUB
//! just retries until it appears, while the PUB keeps publishing ticks.

#[path = "../proto.rs"]
mod proto;

use std::time::Duration;

use prost::Message;
use zeromq::{PubSocket, Socket, SocketOptions, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use proto::sim::Tick;

/// Server binds its PUB here; the CLI's SUB connects to this (the default
/// `con zmq` address).
const PUB_ADDR: &str = "tcp://127.0.0.1:5555";
/// The CLI binds its PUB here; this server connects a SUB to receive the CLI's
/// published `send` messages.
const CLI_PUB_ADDR: &str = "tcp://127.0.0.1:5556";

const TOPIC_JSON: &str = "json";
const TOPIC_PB: &str = "pb";

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
        if let Err(e) = sub.subscribe(TOPIC_JSON).await {
            eprintln!("sub subscribe failed: {e}");
            return;
        }
        println!("sub< listening on {CLI_PUB_ADDR} (topic {TOPIC_JSON})");
        while let Ok(m) = sub.recv().await {
            let frames = m.into_vec();
            let topic = frames
                .get(0)
                .map_or_else(String::new, |f| String::from_utf8_lossy(f.as_ref()).into_owned());
            let payload = frames
                .get(1)
                .map_or_else(String::new, |f| String::from_utf8_lossy(f.as_ref()).into_owned());
            println!("sub< [{topic}] {payload}");
        }
    });

    println!(
        "zmq pub server: publishing on {PUB_ADDR} (topics {TOPIC_JSON} + {TOPIC_PB}), subscribing on {CLI_PUB_ADDR}"
    );
    let mut seq = 0u64;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        seq += 1;
        let tick = Tick {
            seq,
            msg: format!("tick {seq}"),
        };

        // Protobuf topic: encode the Tick as proto bytes. (Borrows tick.)
        let pb = tick.encode_to_vec();
        let pb_len = pb.len();
        let mut pb_msg = ZmqMessage::from(TOPIC_PB);
        pb_msg.push_back(pb.into());
        let _ = pub_.send(pb_msg).await;
        println!("pub> [{TOPIC_PB}] <{pb_len} bytes>");

        // JSON topic: the same Tick serialized as JSON, so the two encodings
        // are directly comparable in the CLI's state panel.
        let json = serde_json::json!({"seq": tick.seq, "msg": tick.msg}).to_string();
        let mut json_msg = ZmqMessage::from(TOPIC_JSON);
        json_msg.push_back(json.clone().into());
        let _ = pub_.send(json_msg).await;
        println!("pub> [{TOPIC_JSON}] {json}");
    }
}
