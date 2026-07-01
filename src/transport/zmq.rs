use tokio::sync::mpsc;
use zeromq::{PubSocket, Socket, SocketOptions, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use super::{Protocol, TransportEvent, TransportHandle};

/// Local port the CLI's PUB socket binds to so its `send` messages reach any
/// external subscriber (e.g. the `zmq_pub_server` test binary). The SUB connect
/// address is the user-facing one passed in as `addr`.
const DEFAULT_ZMQ_PUB_ADDR: &str = "tcp://127.0.0.1:5556";

/// Spawn a ZMQ pub/sub transport.
///
/// Pub/sub is one-way per socket, so we run two sockets in two tasks:
///   - a PUB bound on [`DEFAULT_ZMQ_PUB_ADDR`] that publishes whatever the caller
///     pushes through `out_tx` (the `send` direction), and
///   - a SUB connected to `addr` that feeds `TransportEvent::Recv` (the recv
///     direction).
///
/// They run concurrently so neither blocks the other: the SUB's `connect` uses
/// `no_connect_timeout`, so it retries forever until the publisher appears —
/// meaning the CLI can `con zmq` before the server is up (or vice versa) without
/// a 30s connect timeout tearing the transport down. `Connected` fires once the
/// PUB is bound and the SUB task is started, i.e. "the transport is live; you can
/// `send` now and will receive as soon as the publisher publishes".
///
/// ZMQ delivers each `recv()` as a complete message, so (unlike TCP) there is no
/// byte-stream framing here — we just UTF-8 decode the frames.
pub fn spawn(addr: String, ev_tx: mpsc::Sender<TransportEvent>) -> TransportHandle {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);

    tokio::spawn(async move {
        let _ = ev_tx
            .send(TransportEvent::Connecting {
                protocol: Protocol::Zmq,
                addr: addr.clone(),
            })
            .await;

        // Bind the PUB first so external subscribers can attach as soon as the
        // transport starts, independent of whether the SUB's publisher is up yet.
        let mut pub_ = PubSocket::new();
        if let Err(e) = pub_.bind(DEFAULT_ZMQ_PUB_ADDR).await {
            let _ = ev_tx
                .send(TransportEvent::Error(format!(
                    "zmq pub bind {DEFAULT_ZMQ_PUB_ADDR}: {e}"
                )))
                .await;
            return;
        }

        // SUB runs in its own task so its (possibly long, retrying) connect
        // never blocks the PUB's send path. SubSocket re-sends subscriptions on
        // reconnect, so a single subscribe() covers the connection's lifetime.
        let sub_ev = ev_tx.clone();
        let sub_addr = addr.clone();
        let mut sub_task = tokio::spawn(async move {
            let mut sub_opts = SocketOptions::default();
            sub_opts.no_connect_timeout();
            let mut sub = SubSocket::with_options(sub_opts);
            // no_connect_timeout makes connect retry forever on retryable errors
            // (connection refused, host not up yet); only a permanent failure
            // (bad endpoint syntax, etc.) returns Err here.
            if let Err(e) = sub.connect(&sub_addr).await {
                let _ = sub_ev
                    .send(TransportEvent::Error(format!("zmq sub connect: {e}")))
                    .await;
                return;
            }
            if let Err(e) = sub.subscribe("").await {
                let _ = sub_ev
                    .send(TransportEvent::Error(format!("zmq subscribe: {e}")))
                    .await;
                return;
            }
            loop {
                match sub.recv().await {
                    Ok(m) => match decode(m) {
                        Ok(s) => {
                            if sub_ev.send(TransportEvent::Recv(s)).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = sub_ev.send(TransportEvent::Error(e)).await;
                        }
                    },
                    Err(e) => {
                        let _ = sub_ev
                            .send(TransportEvent::Error(format!("zmq recv: {e}")))
                            .await;
                        break;
                    }
                }
            }
        });

        let _ = ev_tx
            .send(TransportEvent::Connected {
                protocol: Protocol::Zmq,
                addr: addr.clone(),
            })
            .await;

        // Drain outgoing messages until the handle is dropped ('close'). If the
        // SUB task ends (permanent recv/connect failure) we also stop.
        loop {
            tokio::select! {
                line = out_rx.recv() => {
                    match line {
                        Some(payload) => {
                            // PUB silently drops messages when there are no
                            // subscribers; that is expected ZMQ semantics, not an
                            // error. Only a real send failure tears down the transport.
                            if let Err(e) = pub_.send(payload.into()).await {
                                let _ = ev_tx
                                    .send(TransportEvent::Error(format!("zmq publish: {e}")))
                                    .await;
                                break;
                            }
                        }
                        None => break, // TransportHandle dropped → 'close'
                    }
                }
                _ = &mut sub_task => { break; } // SUB task ended
            }
        }
        sub_task.abort();
    });

    TransportHandle {
        protocol: Protocol::Zmq,
        out_tx,
    }
}

/// Concatenate all frames of a multipart message and UTF-8 decode. Single-frame
/// JSON publishes (the common case, and what `zmq_pub_server` emits) come out as
/// the original JSON string verbatim.
fn decode(msg: ZmqMessage) -> Result<String, String> {
    let mut bytes = Vec::new();
    for frame in msg.into_vec() {
        bytes.extend_from_slice(&frame);
    }
    String::from_utf8(bytes).map_err(|e| format!("utf8 decode: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc;

    /// `spawn` always binds the CLI's PUB on the fixed port 5556, so the two
    /// tests that exercise `spawn` must not run concurrently. This lock makes
    /// them mutually exclusive even under cargo's parallel test runner.
    static SPAWN_LOCK: Mutex<()> = Mutex::new(());

    /// `con zmq` connects a SUB and binds a PUB. Drive `spawn` against a PUB we
    /// control and assert the Connecting → Connected → Recv event sequence.
    /// This covers the receive half of the round trip.
    #[tokio::test(flavor = "current_thread")]
    async fn spawn_emits_connecting_connected_recv() {
        let _guard = SPAWN_LOCK.lock().unwrap();
        let mut upstream = PubSocket::new();
        let ep = upstream.bind("tcp://127.0.0.1:0").await.unwrap();
        // `ep` already formats as `tcp://host:port` — don't re-prefix.
        let sub_addr = ep.to_string();

        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        let handle = spawn(sub_addr.clone(), ev_tx);

        let mut saw_connecting = false;
        let mut saw_connected = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await {
                Ok(Some(TransportEvent::Connecting { protocol, addr })) => {
                    assert_eq!(protocol, Protocol::Zmq);
                    assert_eq!(addr, sub_addr);
                    saw_connecting = true;
                }
                Ok(Some(TransportEvent::Connected { protocol, addr })) => {
                    assert_eq!(protocol, Protocol::Zmq);
                    assert_eq!(addr, sub_addr);
                    saw_connected = true;
                    break;
                }
                Ok(Some(TransportEvent::Error(e))) => panic!("unexpected error: {e}"),
                Ok(Some(other)) => panic!("unexpected event before connected: {other:?}"),
                Ok(None) => panic!("event stream closed"),
                Err(_) => {}
            }
        }
        assert!(saw_connecting, "expected Connecting event");
        assert!(saw_connected, "expected Connected event");

        // Let the SUB finish its subscribe handshake before publishing.
        tokio::time::sleep(Duration::from_millis(300)).await;
        upstream.send("{\"k\":42}".into()).await.unwrap();

        let mut got = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await {
                Ok(Some(TransportEvent::Recv(s))) => {
                    got = Some(s);
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(got.as_deref(), Some("{\"k\":42}"), "got {got:?}");

        drop(handle);
        drop(upstream);
    }

    /// Send half: pushing into `out_tx` must publish through the CLI's PUB
    /// socket (bound on 5556 by `spawn`), observable from a SUB we attach to it.
    #[tokio::test(flavor = "current_thread")]
    async fn spawn_send_publishes_through_pub() {
        let _guard = SPAWN_LOCK.lock().unwrap();
        // Point the transport's SUB at a throwaway PUB so spawn() reaches the
        // bind-PUB step; we only care about the PUB side here.
        let mut dummy_pub = PubSocket::new();
        let ep = dummy_pub.bind("tcp://127.0.0.1:0").await.unwrap();
        let dummy_addr = ep.to_string();

        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        let handle = spawn(dummy_addr, ev_tx);

        // Wait for Connected so the CLI's PUB (5556) is bound.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await {
                Ok(Some(TransportEvent::Connected { .. })) => break,
                Ok(Some(TransportEvent::Error(e))) => panic!("connect error: {e}"),
                _ => {}
            }
        }

        let mut cli_sub = SubSocket::new();
        cli_sub.connect("tcp://127.0.0.1:5556").await.unwrap();
        cli_sub.subscribe("").await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        handle.out_tx.send("{\"out\":1}".to_string()).await.unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(3), cli_sub.recv())
            .await
            .expect("recv timed out")
            .unwrap();
        let bytes: Vec<u8> = msg.into_vec().into_iter().flatten().collect();
        assert_eq!(String::from_utf8_lossy(&bytes), "{\"out\":1}");

        drop(handle);
        drop(dummy_pub);
    }

    #[test]
    fn decode_single_frame_json() {
        let msg = ZmqMessage::from("{\"a\":1}");
        assert_eq!(decode(msg).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn decode_concatenates_multipart_frames() {
        let mut msg = ZmqMessage::from("{\"a\":");
        msg.push_back("1}".into());
        assert_eq!(decode(msg).unwrap(), "{\"a\":1}");
    }
}
