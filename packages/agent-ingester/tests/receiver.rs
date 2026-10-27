use agent_common::types::*;
use agent_ingester::{
    classifier::Config, herdr, ingestion::Ingestion, notify::Publisher, owner::Owner,
    receiver::Receiver, summarizer::Summarizer,
};
use agent_status_store::read;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::{TcpListener, UnixStream},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

struct Fixture {
    _dir: tempfile::TempDir,
    owner: Owner,
    publisher: Arc<Publisher>,
    receiver: Receiver,
    url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(config: Config) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let owner =
            Owner::start(Ingestion::open(dir.path().join("data.db"), config).unwrap()).unwrap();
        let publisher = Arc::new(
            Publisher::bind(dir.path().join("notify.sock"))
                .await
                .unwrap(),
        );
        let summary = Arc::new(Summarizer::new(Default::default()).await.unwrap());
        let receiver = Receiver::new(
            owner.clone(),
            publisher.clone(),
            summary,
            "fixture-token".into(),
            Arc::new(herdr::Client::new(dir.path().join("herdr.sock"))),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = receiver.router();
        let cancel = receiver.cancellation_token();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(cancel.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            _dir: dir,
            owner,
            publisher,
            receiver,
            url,
            task,
        }
    }
    async fn socket(
        &self,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = format!("{}/ws/ingest", self.url.replace("http:", "ws:"))
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Authorization", "Bearer fixture-token".parse().unwrap());
        connect_async(request).await.unwrap().0
    }
    async fn shutdown(self) {
        self.receiver.shutdown().await;
        self.task.await.unwrap();
        self.publisher.shutdown().await.unwrap();
        self.owner.shutdown().await.unwrap();
    }
}
fn frame(seq: i64, event: &str) -> TelemetryFrame {
    TelemetryFrame {
        version: "1.0".into(),
        seq,
        env_id: "env".into(),
        environment: EnvironmentInfo {
            id: "env".into(),
            platform: "linux".into(),
            ..Default::default()
        },
        agent_id: "agent".into(),
        session_id: "session".into(),
        harness: HarnessType::Pi,
        pid: i64::from(std::process::id()),
        event_type: event.into(),
        timestamp: agent_ingester::receiver::wall_time(),
        ..Default::default()
    }
}
async fn message<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let value = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(value.to_text().unwrap()).unwrap()
}

#[tokio::test]
async fn durable_websocket_ack_replay_binding_and_ordered_notifications() {
    let fixture = Fixture::new(Config::default()).await;
    let http = reqwest::Client::new();
    assert_eq!(
        http.get(format!("{}/ws/ingest", fixture.url))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        http.get(format!("{}/ws/ingest", fixture.url))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        http.get(format!("{}/healthz", fixture.url))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok\n"
    );
    assert_eq!(
        http.post(format!("{}/healthz", fixture.url))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok\n"
    );
    assert!(
        connect_async(format!("{}/ws/ingest", fixture.url.replace("http:", "ws:")))
            .await
            .is_err()
    );
    let mut notifications =
        BufReader::new(UnixStream::connect(fixture.publisher.path()).await.unwrap());
    let mut line = String::new();
    notifications.read_line(&mut line).await.unwrap();
    let mut socket = fixture.socket().await;
    socket.send(Message::Text("{".into())).await.unwrap();
    assert_eq!(message(&mut socket).await["code"], "malformed");
    socket
        .send(Message::Binary(vec![1, 2].into()))
        .await
        .unwrap();
    socket.send(Message::Text(json!({"type":"hello","envId":"env","environment":{"id":"env","platform":"linux","label":"Fixture"},"since":0,"dropped":3}).to_string().into())).await.unwrap();
    socket
        .send(Message::Text(
            json!({"type":"frames","frames":[frame(1,"turn_start")]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(message(&mut socket).await, json!({"type":"ack","upTo":1}));
    for expected in ["environment_upsert", "agent_upsert", "log_append"] {
        line.clear();
        tokio::time::timeout(Duration::from_secs(2), notifications.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["type"],
            expected
        );
    }
    socket
        .send(Message::Text(
            json!({"type":"frames","frames":[frame(1,"turn_start")]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(message(&mut socket).await["upTo"], 1);
    let mut bad = frame(2, "thinking");
    bad.env_id = "other".into();
    socket
        .send(Message::Text(
            json!({"type":"frames","envId":"other","frames":[bad]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(message(&mut socket).await["code"], "environment");
    let mut bad = frame(2, "thinking");
    bad.version = "2.0".into();
    socket
        .send(Message::Text(
            json!({"type":"frames","frames":[bad]}).to_string().into(),
        ))
        .await
        .unwrap();
    assert_eq!(message(&mut socket).await["code"], "unsupported_version");
    fixture
        .owner
        .call(|ingestion| {
            assert_eq!(read::last_sequence(ingestion.connection(), "env")?, 1);
            assert_eq!(
                read::events(ingestion.connection(), "agent", 100, 0, None, 0)?.len(),
                1
            );
            let dropped: i64 = ingestion.connection().query_row(
                "SELECT dropped_frames FROM environments WHERE id='env'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(dropped, 3);
            Ok(())
        })
        .await
        .unwrap();
    fixture.shutdown().await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), socket.next()).await,
        Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_))))
    ));
}

#[tokio::test]
async fn failures_never_acknowledge_or_partially_commit_and_http_mutations_are_typed() {
    let fixture = Fixture::new(Config {
        idle_grace: Duration::ZERO,
        ..Default::default()
    })
    .await;
    fixture.owner.call(|ingestion|{ingestion.connection().execute_batch("CREATE TRIGGER reject_events BEFORE INSERT ON telemetry_events BEGIN SELECT RAISE(ABORT,'controlled failure'); END;")?;Ok(())}).await.unwrap();
    let mut socket = fixture.socket().await;
    socket
        .send(Message::Text(
            json!({"type":"frames","envId":"env","frames":[frame(1,"turn_start")]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    assert_eq!(message(&mut socket).await["type"], "error");
    fixture
        .owner
        .call(|ingestion| {
            assert_eq!(read::last_sequence(ingestion.connection(), "env")?, 0);
            assert!(read::agent(ingestion.connection(), "agent", 0).is_err());
            ingestion
                .connection()
                .execute_batch("DROP TRIGGER reject_events")?;
            Ok(())
        })
        .await
        .unwrap();
    socket.send(Message::Text(json!({"type":"frames","envId":"env","frames":[frame(1,"turn_start"),frame(2,"turn_end")]}).to_string().into())).await.unwrap();
    assert_eq!(message(&mut socket).await["upTo"], 2);
    let client = reqwest::Client::new();
    let base = format!("{}/api/v1/internal/agents", fixture.url);
    assert_eq!(
        client
            .patch(format!("{base}/agent"))
            .json(&json!({"isHidden":true}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let result = client
        .patch(format!("{base}/agent"))
        .bearer_auth("fixture-token")
        .json(&json!({"isHidden":true,"acknowledgeCompletion":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), 200);
    let agent: AgentState = result.json().await.unwrap();
    assert!(agent.is_hidden);
    assert_eq!(agent.activity.has_unseen_completion, None);
    assert_eq!(
        client
            .patch(format!("{base}/agent"))
            .bearer_auth("fixture-token")
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .patch(format!("{base}/agent"))
            .bearer_auth("fixture-token")
            .body("{")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(
        client
            .patch(format!("{base}/missing/visibility"))
            .bearer_auth("fixture-token")
            .json(&json!({"isHidden":false}))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .patch(format!("{base}/agent/visibility"))
            .bearer_auth("fixture-token")
            .json(&json!({"isHidden":false}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/agent/dismiss-completion"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/agent/focus"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    assert_eq!(
        client
            .post(format!("{base}/missing/focus"))
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn focus_resolves_real_herdr_rpc_and_distinguishes_service_and_storage_failures() {
    use std::sync::atomic::{AtomicU8, Ordering};
    use tokio::io::AsyncWriteExt;
    let fixture = Fixture::new(Config::default()).await;
    fixture
        .owner
        .call(|ingestion| {
            ingestion.process_frames(
                "env",
                &[frame(1, "turn_start")],
                agent_ingester::receiver::wall_time(),
            )
        })
        .await
        .unwrap();
    let listener = tokio::net::UnixListener::bind(fixture._dir.path().join("herdr.sock")).unwrap();
    let mode = Arc::new(AtomicU8::new(0));
    let state = mode.clone();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            let method = request["method"].as_str().unwrap();
            let result = match method {
                "ping" => json!({"ok":true}),
                "pane.list" if state.load(Ordering::SeqCst) == 1 => json!([]),
                "pane.list" => json!({"panes":[{"pane_id":"p","workspace_id":"w","tab_id":"t"}]}),
                "pane.process_info" => json!({"foreground_processes":[{"pid":std::process::id()}]}),
                "pane.focus" => {
                    assert_eq!(request["params"]["pane_id"], "p");
                    json!({"ok":true})
                }
                _ => panic!("unexpected RPC"),
            };
            let response = if (state.load(Ordering::SeqCst) == 2 && method == "pane.list")
                || (state.load(Ordering::SeqCst) == 3 && method == "pane.focus")
            {
                json!({"id":request["id"],"error":{"code":7,"message":"controlled RPC failure"}})
            } else {
                json!({"id":request["id"],"result":result})
            };
            stream
                .get_mut()
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/internal/agents/agent/focus", fixture.url);
    let unauthorized = client.post(&url).send().await.unwrap();
    assert_eq!(unauthorized.status(), 401);
    assert_eq!(
        unauthorized.json::<Value>().await.unwrap()["error"],
        "unauthorized"
    );
    let success = client
        .post(&url)
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap();
    assert_eq!(success.status(), 200);
    assert_eq!(
        success.json::<Value>().await.unwrap(),
        json!({"ok":true,"paneId":"p","workspaceId":"w","tabId":"t"})
    );
    for (scenario, status, description) in [
        (1, 404, "no matching"),
        (2, 500, "locating"),
        (3, 500, "focusing"),
    ] {
        mode.store(scenario, Ordering::SeqCst);
        let response = client
            .post(&url)
            .bearer_auth("fixture-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert!(
            response.json::<Value>().await.unwrap()["error"]
                .as_str()
                .unwrap()
                .contains(description)
        );
    }
    fixture.owner.call(|ingestion| {
        ingestion.connection().execute_batch("CREATE TRIGGER reject_change BEFORE UPDATE ON agents BEGIN SELECT RAISE(ABORT,'controlled update failure'); END;")?;
        Ok(())
    }).await.unwrap();
    let response = client
        .patch(format!(
            "{}/api/v1/internal/agents/agent/visibility",
            fixture.url
        ))
        .bearer_auth("fixture-token")
        .json(&json!({"isHidden":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    fixture
        .owner
        .call(|ingestion| {
            assert!(!read::agent(ingestion.connection(), "agent", 0)?.is_hidden);
            ingestion
                .connection()
                .execute_batch("DROP TRIGGER reject_change")?;
            ingestion.process_frames(
                "env",
                &[frame(2, "run_exited")],
                agent_ingester::receiver::wall_time(),
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let response = client
        .post(&url)
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "agent is exited"
    );
    fixture.shutdown().await;
    server.abort();
}
