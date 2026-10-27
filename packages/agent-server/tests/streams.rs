use agent_common::types::{ServerEvent, TelemetryFrame};
use agent_server::{
    api::{Api, ApiConfig},
    hub::Hub,
    store::ReadStore,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;

struct Fixture {
    _directory: tempfile::TempDir,
    writer: rusqlite::Connection,
    store: ReadStore,
    api: Api,
    hub: Hub,
    cancel: CancellationToken,
    address: std::net::SocketAddr,
    server: JoinHandle<std::io::Result<()>>,
}
impl Fixture {
    async fn new(config: ApiConfig) -> Self {
        let directory = tempfile::tempdir_in(".tmp").unwrap();
        let path = directory.path().join("state.db");
        let writer = agent_status_store::open_writer(&path).unwrap();
        let cancel = CancellationToken::new();
        let store = ReadStore::open(&path, &cancel).await.unwrap();
        let hub = Hub::default();
        let api = Api::new(store.clone(), hub.clone(), config, cancel.clone()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = api.router();
        let shutdown = cancel.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await
        });
        Self {
            _directory: directory,
            writer,
            store,
            api,
            hub,
            cancel,
            address,
            server,
        }
    }
    async fn close(self) {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(3), self.api.shutdown())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), self.server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.store.shutdown().await;
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
    fn ws(&self) -> String {
        format!("ws://{}/ws/events", self.address)
    }
}

struct SseReader {
    response: reqwest::Response,
    pending: Vec<u8>,
}
impl SseReader {
    async fn frame(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(end) = self.pending.windows(2).position(|bytes| bytes == b"\n\n") {
                    let bytes: Vec<_> = self.pending.drain(..end + 2).collect();
                    return String::from_utf8(bytes).unwrap();
                }
                self.pending.extend_from_slice(
                    &self
                        .response
                        .chunk()
                        .await
                        .unwrap()
                        .expect("stream ended before frame"),
                );
            }
        })
        .await
        .unwrap()
    }
    async fn event(&mut self) -> Value {
        loop {
            let frame = self.frame().await;
            if let Some(data) = frame.strip_prefix("data: ") {
                return serde_json::from_str(data.trim()).unwrap();
            }
        }
    }
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn real_sse_has_transactional_snapshot_live_delivery_heartbeat_and_owned_disconnect() {
    let fixture = Fixture::new(ApiConfig {
        heartbeat_interval: Duration::from_millis(5),
        rest_timeout: Duration::from_millis(1),
        ..Default::default()
    })
    .await;
    let response = reqwest::get(fixture.url("/api/v1/events")).await.unwrap();
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut stream = SseReader {
        response,
        pending: Vec::new(),
    };
    let snapshot = stream.event().await;
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["agents"], json!([]));
    assert_eq!(snapshot["environments"], json!([]));
    for _ in 0..3 {
        assert!(stream.frame().await.contains("heartbeat"));
    }
    fixture.hub.broadcast(ServerEvent {
        kind: "agent_removed".into(),
        agent_id: "a".into(),
        at: 42,
        ..Default::default()
    });
    assert_eq!(stream.event().await["at"], 42);
    drop(stream);
    until(|| fixture.hub.global_count() == 0).await;
    fixture.close().await;
}

#[tokio::test]
async fn subscription_precedes_snapshot_so_updates_during_queued_query_are_delivered() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    let mut workers = tokio::task::JoinSet::new();
    let mut releases = Vec::new();
    for _ in 0..4 {
        let store = fixture.store.clone();
        let cancel = fixture.cancel.clone();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        releases.push(release);
        workers.spawn(async move {
            store
                .query(&cancel, move |_| {
                    let _ = entered.send(());
                    resume.recv_timeout(Duration::from_secs(3)).unwrap();
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
    }
    let url = fixture.url("/api/v1/events");
    let request = tokio::spawn(async move { reqwest::get(url).await.unwrap() });
    until(|| fixture.hub.global_count() == 1).await;
    fixture.writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('new','test','host','linux','user',123)", []).unwrap();
    fixture.hub.broadcast(ServerEvent {
        kind: "environment_upsert".into(),
        at: 99,
        ..Default::default()
    });
    for release in releases {
        release.send(()).unwrap();
    }
    while let Some(worker) = workers.join_next().await {
        worker.unwrap().unwrap();
    }
    let response = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .unwrap()
        .unwrap();
    let mut stream = SseReader {
        response,
        pending: Vec::new(),
    };
    assert_eq!(stream.event().await["environments"][0]["id"], "new");
    assert_eq!(stream.event().await["at"], 99);
    drop(stream);
    fixture.close().await;
}

#[tokio::test]
async fn live_tail_has_no_snapshot_and_sends_only_matching_native_frames() {
    let fixture = Fixture::new(ApiConfig {
        heartbeat_interval: Duration::from_millis(5),
        ..Default::default()
    })
    .await;
    let response = reqwest::get(fixture.url("/api/v1/agents/env%3Aclaude-code%3Aa/live-tail"))
        .await
        .unwrap();
    let mut stream = SseReader {
        response,
        pending: Vec::new(),
    };
    assert!(stream.frame().await.contains("heartbeat"));
    for (kind, id, seq) in [
        ("agent_upsert", "env:claude-code:a", 1),
        ("log_append", "other", 2),
        ("log_append", "env:claude-code:a", 3),
    ] {
        fixture.hub.broadcast(ServerEvent {
            kind: kind.into(),
            agent_id: id.into(),
            event: Some(TelemetryFrame {
                seq,
                event_type: "tool_call".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
    }
    let frame = stream.event().await;
    assert_eq!(frame["seq"], 3);
    assert_eq!(frame["eventType"], "tool_call");
    assert!(frame.get("type").is_none());
    drop(stream);
    until(|| fixture.hub.tail_count("env:claude-code:a") == 0).await;
    fixture.close().await;
}

async fn ws_json(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Value {
    let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(message.to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn real_websocket_snapshot_event_ping_and_data_policy_are_native() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    let (mut socket, _) = connect_async(fixture.ws()).await.unwrap();
    assert_eq!(ws_json(&mut socket).await["type"], "snapshot");
    fixture.hub.broadcast(ServerEvent {
        kind: "agent_removed".into(),
        at: 10,
        ..Default::default()
    });
    assert_eq!(ws_json(&mut socket).await["at"], 10);
    socket.send(Message::Ping(vec![1, 2].into())).await.unwrap();
    let pong = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(pong, Message::Pong(vec![1, 2].into()));
    socket
        .send(Message::Text("unexpected".into()))
        .await
        .unwrap();
    let close = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Close(Some(close)) = close else {
        panic!("expected policy close")
    };
    assert_eq!(u16::from(close.code), 1008);
    assert_eq!(close.reason, "unexpected data message");
    drop(socket);
    until(|| fixture.hub.global_count() == 0).await;
    fixture.close().await;
}

#[tokio::test]
async fn websocket_auth_origin_grammar_and_shutdown_are_enforced() {
    for (pattern, origin, allowed) in [
        ("http://*.example.test", "http://ui.example.test", true),
        ("http://*.example.test", "https://ui.example.test", false),
        ("ui[0-9].example.test", "https://ui1.example.test", true),
        ("ui[^0-9].example.test", "https://ui1.example.test", false),
        ("[invalid", "https://ui.example.test", false),
        ("*", "https://any.example.test", true),
    ] {
        let fixture = Fixture::new(ApiConfig {
            auth_token: "configured".into(),
            cors_origins: vec![pattern.into()],
            ..Default::default()
        })
        .await;
        let mut request = format!("{}?token=configured", fixture.ws())
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let result = tokio::time::timeout(Duration::from_secs(3), connect_async(request))
            .await
            .unwrap();
        if allowed {
            let (mut socket, _) = result.unwrap();
            assert_eq!(ws_json(&mut socket).await["type"], "snapshot");
            fixture.cancel.cancel();
            let ended = tokio::time::timeout(Duration::from_secs(3), socket.next())
                .await
                .unwrap();
            assert!(
                ended.is_none()
                    || ended.is_some_and(
                        |result| result.is_err() || matches!(result, Ok(Message::Close(_)))
                    )
            );
            drop(socket);
        } else {
            let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = result else {
                panic!("expected denied upgrade");
            };
            assert_eq!(response.status(), 403);
        }
        fixture.close().await;
    }
}

#[tokio::test]
async fn snapshot_failure_returns_safe_http_error_before_stream_headers() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    fixture
        .writer
        .execute("ALTER TABLE environments RENAME TO unavailable", [])
        .unwrap();
    let response = reqwest::get(fixture.url("/api/v1/events")).await.unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"failed to get snapshot"})
    );
    let result = connect_async(fixture.ws()).await;
    let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = result else {
        panic!("expected HTTP error");
    };
    assert_eq!(response.status(), 500);
    assert_eq!(fixture.hub.global_count(), 0);
    fixture.close().await;
}
