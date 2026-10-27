use agent_common::types::*;
use agent_ingester::{
    cli::{self, Action},
    config::Paths,
    runtime::{Daemon, Schedule},
};
use cli_command_kit::Mode;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
fn options(dir: &std::path::Path) -> cli::Options {
    let Action::Serve(options) = cli::parse_with_env(
        [
            "agent-ingester",
            "--addr",
            "127.0.0.1:0",
            "--no-summarize",
            "--db",
            dir.join("data.db").to_str().unwrap(),
            "--notify-sock",
            dir.join("notify.sock").to_str().unwrap(),
            "--token",
            "local",
            "--idle-grace-period",
            "25ms",
        ],
        Mode::Agent,
        &mut Vec::new(),
        |_| None,
        Paths::new(None),
    )
    .unwrap() else {
        panic!()
    };
    *options
}
#[tokio::test]
async fn wildcard_listener_accepts_both_native_address_families() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = options(dir.path());
    config.addr = ":0".into();
    let daemon = Daemon::bind(config).await.unwrap();
    let port = daemon.address().port();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task = tokio::spawn(async move { daemon.run(&stop, Schedule::default()).await });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    for host in ["127.0.0.1", "[::1]"] {
        assert_eq!(
            http.get(format!("http://{host}:{port}/healthz"))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            "ok\n"
        );
    }
    cancel.cancel();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn daemon_serves_commits_timer_transitions_and_joins_all_resources_on_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::bind(options(dir.path())).await.unwrap();
    let address = daemon.address();
    let owner = daemon.owner().clone();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task = tokio::spawn(async move { daemon.run(&stop, Schedule::default()).await });
    let mut request = format!("ws://{address}/ws/ingest")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("Authorization", "Bearer local".parse().unwrap());
    let (mut socket, _) = connect_async(request).await.unwrap();
    let now = agent_ingester::receiver::wall_time();
    let base = TelemetryFrame {
        version: "1.0".into(),
        seq: 1,
        env_id: "env".into(),
        environment: EnvironmentInfo {
            id: "env".into(),
            platform: "linux".into(),
            hostname: hostname::get().unwrap().to_string_lossy().into_owned(),
            ..Default::default()
        },
        agent_id: "agent".into(),
        session_id: "session".into(),
        harness: HarnessType::Pi,
        pid: i64::from(std::process::id()),
        event_type: "turn_start".into(),
        timestamp: now,
        ..Default::default()
    };
    let end = TelemetryFrame {
        seq: 2,
        event_type: "turn_end".into(),
        timestamp: now + 1,
        data: EventData {
            stop_reason: Some("end_turn".into()),
            ..Default::default()
        },
        ..base.clone()
    };
    socket
        .send(Message::Text(
            json!({"type":"frames","envId":"env","frames":[base,end]})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let ack = socket.next().await.unwrap().unwrap();
    assert!(ack.to_text().unwrap().contains("\"upTo\":2"));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let idle = owner
                .call(|ingestion| {
                    Ok(
                        agent_status_store::read::agent(ingestion.connection(), "agent", 0)?
                            .activity
                            .activity
                            == ActivityState::Idle,
                    )
                })
                .await
                .unwrap();
            if idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!dir.path().join("notify.sock").exists());
    assert!(owner.call(|_| Ok(())).await.is_err());
    assert!(TcpStream::connect(address).await.is_err());
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), socket.next()).await,
        Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_))))
    ));
}
#[tokio::test]
async fn native_header_deadline_closes_partial_requests_and_periodic_maintenance_runs() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::bind(options(dir.path())).await.unwrap();
    let address = daemon.address();
    let owner = daemon.owner().clone();
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task = tokio::spawn(async move {
        daemon
            .run(
                &stop,
                Schedule {
                    header_timeout: Duration::from_millis(25),
                    reconcile: Duration::from_millis(20),
                    prune: Duration::from_millis(30),
                    ..Default::default()
                },
            )
            .await
    });
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: local\r\n")
        .await
        .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.is_empty() || String::from_utf8_lossy(&bytes).contains("408"));
    let frame = TelemetryFrame {
        version: "1.0".into(),
        seq: 1,
        env_id: "env".into(),
        environment: EnvironmentInfo {
            id: "env".into(),
            platform: "linux".into(),
            ..Default::default()
        },
        agent_id: "abandoned".into(),
        session_id: "session".into(),
        harness: HarnessType::Pi,
        pid: 0,
        event_type: "run_exited".into(),
        timestamp: 1,
        ..Default::default()
    };
    owner
        .call(move |ingestion| ingestion.process_frames("env", &[frame], 1))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let count = owner
                .call(|ingestion| {
                    Ok(ingestion.connection().query_row(
                        "SELECT count(*) FROM agents WHERE id='abandoned'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .await
                .unwrap();
            if count == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn startup_failures_and_precancellation_do_not_leak_socket_or_database_workers() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = options(dir.path());
    config.addr = listener.local_addr().unwrap().to_string();
    assert!(Daemon::bind(config).await.is_err());
    assert!(!dir.path().join("notify.sock").exists());
    let mut config = options(dir.path());
    config.database = dir.path().to_owned();
    assert!(Daemon::bind(config).await.is_err());
    let config = options(dir.path());
    std::fs::write(&config.notify_socket, b"unrelated").unwrap();
    assert!(Daemon::bind(config).await.is_err());
    assert_eq!(
        std::fs::read(dir.path().join("notify.sock")).unwrap(),
        b"unrelated"
    );
    std::fs::remove_file(dir.path().join("notify.sock")).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    agent_ingester::runtime::run(&token, options(dir.path()))
        .await
        .unwrap();
    assert!(!dir.path().join("notify.sock").exists());
}
