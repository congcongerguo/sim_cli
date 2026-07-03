use prost::Message;
use tokio::sync::mpsc;
use zeromq::{PubSocket, Socket, SocketOptions, SocketRecv, SocketSend, SubSocket, ZmqMessage};

use super::{Protocol, TransportEvent, TransportHandle};
use crate::proto::sim;

/// Local port the CLI's PUB socket binds to so its `send` messages reach any
/// external subscriber (e.g. the `zmq_pub_server` test binary). The SUB connect
/// address is the user-facing one passed in as `addr`.
const DEFAULT_ZMQ_PUB_ADDR: &str = "tcp://127.0.0.1:5556";

/// Wire topics. The first frame of every published message is one of these,
/// and it doubles as the encoding label surfaced to the rest of the app.
const TOPIC_JSON: &str = "json";
const TOPIC_PB: &str = "pb";

/// Spawn a ZMQ pub/sub transport.
///
/// Pub/sub is one-way per socket, so we run two sockets in two tasks:
///   - a PUB bound on [`DEFAULT_ZMQ_PUB_ADDR`] that publishes whatever the caller
///     pushes through `out_tx` (the `send` direction), and
///   - a SUB connected to `addr` that feeds `TransportEvent::Recv` (the recv
///     direction).
///
/// Every message on the wire is a 2-frame multipart `[topic, payload]` where
/// `topic` is `TOPIC_JSON` or `TOPIC_PB` and selects how `payload` is encoded:
/// a UTF-8 JSON string or a proto-encoded `sim::Tick`. The SUB subscribes to
/// both topics explicitly — that subscription filter *is* the "topic
/// distinguishes json vs protobuf" mechanism.
///
/// The two sockets run concurrently so neither blocks the other: the SUB's
/// `connect` uses `no_connect_timeout`, so it retries forever until the
/// publisher appears — meaning the CLI can `con zmq` before the server is up
/// (or vice versa) without a 30s connect timeout tearing the transport down.
/// `Connected` fires once the PUB is bound and the SUB task is started, i.e.
/// "the transport is live; you can `send` now and will receive as soon as the
/// publisher publishes".
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
        // reconnect, so subscribing once per topic covers the connection's
        // lifetime.
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
            for topic in [TOPIC_JSON, TOPIC_PB] {
                if let Err(e) = sub.subscribe(topic).await {
                    let _ = sub_ev
                        .send(TransportEvent::Error(format!("zmq subscribe {topic}: {e}")))
                        .await;
                    return;
                }
            }
            loop {
                match sub.recv().await {
                    Ok(m) => match decode(m) {
                        Ok((encoding, text)) => {
                            if sub_ev
                                .send(TransportEvent::Recv { encoding, text })
                                .await
                                .is_err()
                            {
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
                            // Frame as [topic, payload] so subscribers can
                            // filter by encoding. The CLI's `send` always emits
                            // JSON, so the topic is "json". PUB silently drops
                            // messages when there are no subscribers; that is
                            // expected ZMQ semantics, not an error. Only a real
                            // send failure tears down the transport.
                            let mut msg = ZmqMessage::from(TOPIC_JSON);
                            msg.push_back(payload.into());
                            if let Err(e) = pub_.send(msg).await {
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

/// Decode a 2-frame `[topic, payload]` multipart message into
/// `(topic, json_text)`.
///
/// - `json` topic: payload is a UTF-8 JSON string, returned verbatim.
/// - `pb` topic:   payload is a proto-encoded `sim::Tick`, decoded and
///   re-serialized as a JSON string here so the rest of the app only ever
///   sees JSON text regardless of wire encoding.
fn decode(msg: ZmqMessage) -> Result<(String, String), String> {
    let frames = msg.into_vec();
    if frames.len() < 2 {
        return Err(format!(
            "expected [topic, payload] multipart, got {} frame(s)",
            frames.len()
        ));
    }
    let topic = String::from_utf8(frames[0].to_vec())
        .map_err(|e| format!("topic utf8: {e}"))?;
    let payload = frames[1].to_vec();
    let text = match topic.as_str() {
        "json" => String::from_utf8(payload).map_err(|e| format!("payload utf8: {e}")),
        "pb" => {
            let tick = sim::Tick::decode(payload.as_slice())
                .map_err(|e| format!("proto decode Tick: {e}"))?;
            Ok(serde_json::json!({"seq": tick.seq, "msg": tick.msg}).to_string())
        }
        other => Err(format!("unknown topic: {other}")),
    }?;
    Ok((topic, text))
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
    /// control and assert the Connecting → Connected → Recv event sequence over
    /// a 2-frame `[topic, payload]` publish. This covers the receive half of
    /// the round trip.
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
        let mut msg = ZmqMessage::from(TOPIC_JSON);
        msg.push_back("{\"k\":42}".into());
        upstream.send(msg).await.unwrap();

        let mut got = None;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(200), ev_rx.recv()).await {
                Ok(Some(TransportEvent::Recv { encoding, text })) => {
                    assert_eq!(encoding, TOPIC_JSON);
                    assert_eq!(text, "{\"k\":42}");
                    got = Some(text);
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
    /// socket (bound on 5556 by `spawn`) as a 2-frame `["json", payload]`
    /// message, observable from a SUB we attach to it.
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
        cli_sub.subscribe(TOPIC_JSON).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;

        handle.out_tx.send("{\"out\":1}".to_string()).await.unwrap();
        let msg = tokio::time::timeout(Duration::from_secs(3), cli_sub.recv())
            .await
            .expect("recv timed out")
            .unwrap();
        let frames = msg.into_vec();
        assert_eq!(frames.len(), 2, "expected [topic, payload] multipart");
        assert_eq!(String::from_utf8_lossy(frames[0].as_ref()), TOPIC_JSON);
        assert_eq!(String::from_utf8_lossy(frames[1].as_ref()), "{\"out\":1}");

        drop(handle);
        drop(dummy_pub);
    }

    #[test]
    fn decode_json_topic() {
        let mut msg = ZmqMessage::from(TOPIC_JSON);
        msg.push_back("{\"a\":1}".into());
        assert_eq!(
            decode(msg).unwrap(),
            ("json".into(), "{\"a\":1}".into())
        );
    }

    #[test]
    fn decode_pb_topic_decodes_tick() {
        let tick = sim::Tick {
            seq: 7,
            msg: "hi".into(),
        };
        let bytes = tick.encode_to_vec();
        let mut msg = ZmqMessage::from(TOPIC_PB);
        msg.push_back(bytes.into());
        let (topic, text) = decode(msg).unwrap();
        assert_eq!(topic, TOPIC_PB);
        // Compare structurally — serde_json key order is not guaranteed.
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["seq"], 7);
        assert_eq!(v["msg"], "hi");
    }

    #[test]
    fn decode_rejects_single_frame() {
        let msg = ZmqMessage::from("not multipart");
        assert!(decode(msg).is_err());
    }

    #[test]
    fn decode_rejects_unknown_topic() {
        let mut msg = ZmqMessage::from("xml");
        msg.push_back("<a/>".into());
        assert!(decode(msg).is_err());
    }
}
