use agent_common::types::{EnvironmentInfo, IngesterMessage, ObserverMessage, TelemetryFrame};
use agent_observer::shipper::{Config, Shipper};
use agent_watcher::Acknowledgement;
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

fn config(url: String) -> Config {
    Config {
        ingester_url: url.parse().unwrap(),
        environment: EnvironmentInfo {
            id: "environment".into(),
            ..Default::default()
        },
        bearer_token: "local-fixture".into(),
        buffer_size: 2,
        heartbeat_interval: Duration::from_millis(20),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(80),
        sustained_health: Duration::from_millis(40),
        backoff_randomization: 0.0,
    }
}

fn frame() -> TelemetryFrame {
    TelemetryFrame {
        version: "1.0".into(),
        env_id: "environment".into(),
        agent_id: "agent".into(),
        event_type: "thinking".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn successfully_sent_unacknowledged_frames_replay_after_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cfg = config(format!("ws://{}/ws/ingest", listener.local_addr().unwrap()));
    let shipper = Arc::new(Shipper::new(cfg).unwrap());
    let cancel = CancellationToken::new();
    let acknowledgements = Arc::new(Mutex::new(Vec::new()));
    let seen = acknowledgements.clone();
    shipper
        .enqueue(
            &cancel,
            frame(),
            Some(Acknowledgement::new(move || {
                seen.lock().unwrap().push(1);
                Ok(())
            })),
        )
        .await
        .unwrap();
    let (done_tx, done_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut first = accept_async(socket).await.unwrap();
        let hello: ObserverMessage =
            serde_json::from_slice(&first.next().await.unwrap().unwrap().into_data()).unwrap();
        assert_eq!(hello.kind, "hello");
        assert_eq!(hello.since, 0);
        let original: ObserverMessage =
            serde_json::from_slice(&first.next().await.unwrap().unwrap().into_data()).unwrap();
        assert_eq!(original.frames[0].seq, 1);
        drop(first);
        let (socket, _) = listener.accept().await.unwrap();
        let mut second = accept_async(socket).await.unwrap();
        let hello: ObserverMessage =
            serde_json::from_slice(&second.next().await.unwrap().unwrap().into_data()).unwrap();
        assert_eq!(hello.since, 0);
        let replay: ObserverMessage =
            serde_json::from_slice(&second.next().await.unwrap().unwrap().into_data()).unwrap();
        assert_eq!(replay.frames, original.frames);
        second
            .send(Message::Text(
                serde_json::to_string(&IngesterMessage {
                    kind: "ack".into(),
                    up_to: 1,
                    ..Default::default()
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        done_tx.send(()).unwrap();
        while second.next().await.is_some() {}
    });
    let runner = tokio::spawn({
        let s = shipper.clone();
        let c = cancel.clone();
        async move { s.run(&c).await }
    });
    timeout(Duration::from_secs(3), done_rx)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(2), async {
        while shipper.unacked_count() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*acknowledgements.lock().unwrap(), vec![1]);
    cancel.cancel();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn capacity_counts_unacknowledged_frames_and_stop_unblocks_waiters() {
    let shipper = Arc::new(Shipper::new(config("ws://127.0.0.1:1/ws/ingest".into())).unwrap());
    let cancel = CancellationToken::new();
    shipper.enqueue(&cancel, frame(), None).await.unwrap();
    shipper.enqueue(&cancel, frame(), None).await.unwrap();
    assert_eq!(shipper.unacked_count(), 2);
    assert!(
        timeout(
            Duration::from_millis(30),
            shipper.enqueue(&cancel, frame(), None)
        )
        .await
        .is_err()
    );
    shipper.stop();
    assert!(shipper.enqueue(&cancel, frame(), None).await.is_err());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(shipper.enqueue(&cancelled, frame(), None).await.is_err());
    assert_eq!(shipper.unacked_count(), 2);
}

#[tokio::test]
// The native tungstenite handshake callback requires its unboxed HTTP response error.
#[allow(clippy::result_large_err)]
async fn bearer_auth_heartbeats_malformed_messages_and_ordered_acknowledgements() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = config(format!("ws://{}/ws/ingest", listener.local_addr().unwrap()));
    cfg.buffer_size = 3;
    let shipper = Arc::new(Shipper::new(cfg).unwrap());
    let cancel = CancellationToken::new();
    let seen = Arc::new(Mutex::new(Vec::new()));
    for index in 1..=3 {
        let seen = seen.clone();
        shipper
            .enqueue(
                &cancel,
                frame(),
                Some(Acknowledgement::new(move || {
                    seen.lock().unwrap().push(index);
                    Ok(())
                })),
            )
            .await
            .unwrap();
    }
    let (done_tx, done_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_hdr_async(
            socket,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(request.headers()["Authorization"], "Bearer local-fixture");
                Ok(response)
            },
        )
        .await
        .unwrap();
        ws.send(Message::Text("malformed".into())).await.unwrap();
        ws.send(Message::Binary(vec![1, 2].into())).await.unwrap();
        ws.send(Message::Ping(vec![42].into())).await.unwrap();
        ws.send(Message::Text(r#"{"type":"error","code":"fixture"}"#.into()))
            .await
            .unwrap();
        let mut received = Vec::new();
        let mut heartbeat = false;
        let mut pong = false;
        while let Some(Ok(message)) = ws.next().await {
            if message.is_pong() {
                pong = true;
                continue;
            }
            if !message.is_text() {
                continue;
            }
            let message: ObserverMessage = serde_json::from_slice(&message.into_data()).unwrap();
            if message.kind == "hello" && message.environment.unwrap().last_heartbeat_at > 0 {
                heartbeat = true;
            }
            received.extend(message.frames.into_iter().map(|f| f.seq));
            if received.len() == 3 && heartbeat && pong {
                break;
            }
        }
        assert_eq!(received, vec![1, 2, 3]);
        ws.send(Message::Text(r#"{"type":"ack","upTo":2}"#.into()))
            .await
            .unwrap();
        loop {
            let message = ws.next().await.unwrap().unwrap();
            if !message.is_text() {
                continue;
            }
            let message: ObserverMessage = serde_json::from_slice(&message.into_data()).unwrap();
            if message.kind == "hello" && message.since == 2 {
                break;
            }
        }
        ws.send(Message::Text(r#"{"type":"ack","upTo":3}"#.into()))
            .await
            .unwrap();
        done_tx.send(()).unwrap();
        while ws.next().await.is_some() {}
    });
    let runner = tokio::spawn({
        let s = shipper.clone();
        let c = cancel.clone();
        async move { s.run(&c).await }
    });
    timeout(Duration::from_secs(2), done_rx)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(1), async {
        while shipper.unacked_count() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![1, 2, 3]);
    shipper.stop();
    timeout(Duration::from_secs(1), runner)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn checkpoint_failure_stops_delivery_without_discarding_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let shipper = Shipper::new(config(format!(
        "ws://{}/ws/ingest",
        listener.local_addr().unwrap()
    )))
    .unwrap();
    let cancel = CancellationToken::new();
    shipper
        .enqueue(
            &cancel,
            frame(),
            Some(Acknowledgement::new(|| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "checkpoint fixture",
                ))
            })),
        )
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(socket).await.unwrap();
        ws.next().await.unwrap().unwrap();
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(r#"{"type":"ack","upTo":1}"#.into()))
            .await
            .unwrap();
        while ws.next().await.is_some() {}
    });
    assert!(matches!(
        timeout(Duration::from_secs(2), shipper.run(&cancel))
            .await
            .unwrap(),
        Err(agent_observer::shipper::Error::Checkpoint(_))
    ));
    assert_eq!(shipper.unacked_count(), 1);
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn disconnected_transport_cancels_and_duplicate_run_is_rejected() {
    let shipper = Arc::new(Shipper::new(config("ws://127.0.0.1:1/ws/ingest".into())).unwrap());
    let cancel = CancellationToken::new();
    let worker = tokio::spawn({
        let s = shipper.clone();
        let c = cancel.clone();
        async move { s.run(&c).await }
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(matches!(
        shipper.run(&cancel).await,
        Err(agent_observer::shipper::Error::AlreadyRunning)
    ));
    cancel.cancel();
    timeout(Duration::from_secs(1), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    shipper.stop();
    shipper.run(&CancellationToken::new()).await.unwrap();
}

#[test]
fn invalid_connection_settings_are_rejected_before_network_access() {
    let mut cfg = config("http://example.invalid/".into());
    assert!(Shipper::new(cfg.clone()).is_err());
    cfg.ingester_url = "ws://example.invalid/".parse().unwrap();
    cfg.heartbeat_interval = Duration::ZERO;
    assert!(Shipper::new(cfg.clone()).is_err());
    cfg.heartbeat_interval = Duration::from_secs(1);
    cfg.backoff_randomization = 2.0;
    assert!(Shipper::new(cfg.clone()).is_err());
    cfg.backoff_randomization = 0.5;
    cfg.buffer_size = usize::MAX;
    assert!(Shipper::new(cfg.clone()).is_err());
    cfg.buffer_size = 0;
    assert!(Shipper::new(cfg).is_ok());
}
