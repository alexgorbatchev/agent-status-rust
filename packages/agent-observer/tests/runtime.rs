use agent_common::types::{EnvironmentInfo, ObserverMessage};
use agent_observer::{runtime, shipper};
use agent_watcher::{
    Config, Harness,
    scanner::{ProcessMeta, ProcessSnapshot},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_events_ship_with_workspace_metadata_and_acknowledged_cursors_flush_on_shutdown() {
    std::fs::create_dir_all(".tmp").unwrap();
    let root = tempfile::tempdir_in(".tmp").unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let sessions = root_path.join("pi/project");
    std::fs::create_dir_all(&sessions).unwrap();
    let transcript = sessions.join("session.jsonl");
    let stamp = "2026-10-14T20:00:00Z";
    let records = [
        json!({"type":"session","id":"session","version":3,"cwd":root_path,"timestamp":stamp}),
        json!({"type":"message","timestamp":stamp,"message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stopReason":"stop","usage":{"input":10,"output":2,"totalTokens":12}}}),
    ];
    std::fs::write(
        &transcript,
        records.iter().map(|r| format!("{r}\n")).collect::<String>(),
    )
    .unwrap();
    let process_cwd = root_path.clone();
    let cfg = Config {
        harnesses: vec![Harness::Pi],
        scan_interval: Duration::from_millis(10),
        cursor_flush_interval: Duration::from_millis(5),
        pi_sessions_dir: root_path.join("pi"),
        cursor_path: root_path.join("cursors.json"),
        session_state_path: root_path.join("sessions.json"),
        process_snapshot_provider: Arc::new(move |_: &CancellationToken| {
            Ok(ProcessSnapshot::from_processes([ProcessMeta {
                pid: 42,
                name: "pi".into(),
                alive: true,
                cwd: process_cwd.clone(),
                ..Default::default()
            }]))
        }),
        ..Default::default()
    };
    let cursor_path = cfg.cursor_path.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let environment = EnvironmentInfo {
        id: "local-environment".into(),
        label: "fixture".into(),
        ..Default::default()
    };
    let ship_cfg = shipper::Config::new(
        format!("ws://{}/ws/ingest", listener.local_addr().unwrap())
            .parse()
            .unwrap(),
        environment,
    );
    let cancel = CancellationToken::new();
    let worker = tokio::spawn({
        let c = cancel.clone();
        async move { runtime::run(&c, cfg, ship_cfg).await }
    });
    let (finished_tx, finished_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut websocket = accept_async(socket).await.unwrap();
        let mut saw_turn_end = false;
        while let Some(Ok(message)) = websocket.next().await {
            if !message.is_text() {
                continue;
            }
            let message: ObserverMessage = serde_json::from_slice(&message.into_data()).unwrap();
            for frame in message.frames {
                assert_eq!(frame.agent_id, "local-environment:pi:session");
                assert_eq!(frame.session_id, "session");
                assert_eq!(frame.environment.label, "fixture");
                assert_eq!(frame.workspace.cwd, root_path.to_str().unwrap());
                assert_eq!(frame.pid, 42);
                saw_turn_end |= frame.event_type == "turn_end";
                websocket
                    .send(Message::Text(
                        json!({"type":"ack","upTo":frame.seq}).to_string().into(),
                    ))
                    .await
                    .unwrap();
            }
            if saw_turn_end {
                break;
            }
        }
        assert!(saw_turn_end);
        finished_tx.send(()).unwrap();
        while websocket.next().await.is_some() {}
    });
    timeout(Duration::from_secs(3), finished_rx)
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(bytes) = std::fs::read(&cursor_path)
                && let Ok(cursors) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && cursors
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|c| c["offset"].as_u64().unwrap() > 0)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
    assert!(cursor_path.exists());
}

#[tokio::test]
async fn pre_cancelled_observer_returns_without_creating_state() {
    let cancel = CancellationToken::new();
    cancel.cancel();
    let ship_cfg = shipper::Config::new(
        "ws://127.0.0.1:1/ws/ingest".parse().unwrap(),
        EnvironmentInfo::default(),
    );
    runtime::run(&cancel, Config::default(), ship_cfg)
        .await
        .unwrap();
}

#[tokio::test]
async fn initialization_failure_joins_transport_and_returns_watcher_error() {
    std::fs::create_dir_all(".tmp").unwrap();
    let root = tempfile::tempdir_in(".tmp").unwrap();
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, "file").unwrap();
    let cfg = Config {
        cursor_path: blocked.join("cursor.json"),
        session_state_path: blocked.join("sessions.json"),
        ..Default::default()
    };
    let ship_cfg = shipper::Config::new(
        "ws://127.0.0.1:1/ws/ingest".parse().unwrap(),
        EnvironmentInfo::default(),
    );
    assert!(
        timeout(
            Duration::from_secs(1),
            runtime::run(&CancellationToken::new(), cfg, ship_cfg)
        )
        .await
        .unwrap()
        .is_err()
    );
}
