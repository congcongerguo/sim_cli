use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use super::{JsonFramer, Protocol, TransportEvent, TransportHandle};

/// Soft cap on bytes the framer is allowed to accumulate without producing a
/// complete message. Guards against an open quote or unbalanced brace eating
/// memory forever.
const MAX_PENDING: usize = 16 * 1024 * 1024;
const READ_CHUNK: usize = 4096;

pub fn spawn(addr: String, ev_tx: mpsc::Sender<TransportEvent>) -> TransportHandle {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);

    tokio::spawn(async move {
        let _ = ev_tx
            .send(TransportEvent::Connecting {
                protocol: Protocol::Tcp,
                addr: addr.clone(),
            })
            .await;
        let sock = match TcpStream::connect(&addr).await {
            Ok(s) => s,
            Err(e) => {
                let _ = ev_tx
                    .send(TransportEvent::Error(format!(
                        "connect failed: {:?}",
                        e.kind()
                    )))
                    .await;
                return;
            }
        };
        let _ = ev_tx
            .send(TransportEvent::Connected {
                protocol: Protocol::Tcp,
                addr: addr.clone(),
            })
            .await;

        let (mut read, mut write) = sock.into_split();
        let mut framer = JsonFramer::new();
        let mut chunk = [0u8; READ_CHUNK];

        loop {
            tokio::select! {
                n = read.read(&mut chunk) => match n {
                    Ok(0) => {
                        let _ = ev_tx.send(TransportEvent::Disconnected).await;
                        break;
                    }
                    Ok(n) => {
                        framer.feed(&chunk[..n]);
                        if framer.pending_bytes() > MAX_PENDING {
                            let _ = ev_tx
                                .send(TransportEvent::Error(format!(
                                    "buffer exceeded {MAX_PENDING} bytes without a complete message"
                                )))
                                .await;
                            break;
                        }
                        let mut closed = false;
                        while let Some(msg) = framer.next_message() {
                            match String::from_utf8(msg) {
                                Ok(s) => {
                                    if ev_tx
                                        .send(TransportEvent::Recv {
                                            encoding: "json".into(),
                                            text: s,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        closed = true;
                                        break;
                                    }
                                }
                                Err(e) => {
                                    let _ = ev_tx
                                        .send(TransportEvent::Error(format!("utf8 decode: {e}")))
                                        .await;
                                    closed = true;
                                    break;
                                }
                            }
                        }
                        if closed { break; }
                    }
                    Err(e) => {
                        let _ = ev_tx.send(TransportEvent::Error(format!("read: {:?}", e.kind()))).await;
                        break;
                    }
                },
                msg = out_rx.recv() => match msg {
                    Some(payload) => {
                        // Caller is expected to hand us a complete JSON value;
                        // we just push the raw bytes onto the stream.
                        if let Err(e) = write.write_all(payload.as_bytes()).await {
                            let _ = ev_tx.send(TransportEvent::Error(format!("write: {:?}", e.kind()))).await;
                            break;
                        }
                        if let Err(e) = write.flush().await {
                            let _ = ev_tx.send(TransportEvent::Error(format!("flush: {:?}", e.kind()))).await;
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    });

    TransportHandle {
        protocol: Protocol::Tcp,
        out_tx,
    }
}
