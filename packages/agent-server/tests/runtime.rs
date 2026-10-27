use agent_server::{
    cli::{self, Action},
    runtime::{self, Daemon, Schedule},
};
use cli_command_kit::Mode;
use std::{io, path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UnixListener},
};
use tokio_util::sync::CancellationToken;

fn options(database: &Path, socket: &Path, addr: &str) -> cli::Options {
    let Action::Serve(options) = cli::parse_with_env(
        ["agent-server"],
        Mode::Agent,
        &mut Vec::new(),
        |_| None,
        cli::Paths { home: None },
    )
    .unwrap() else {
        panic!("expected service")
    };
    cli::Options {
        database: database.into(),
        socket: socket.into(),
        addr: addr.into(),
        ..*options
    }
}

fn schedule() -> Schedule {
    Schedule {
        header_timeout: Duration::from_millis(25),
        shutdown_timeout: Duration::from_secs(2),
        reconnect_wait: Duration::from_millis(1),
    }
}

#[tokio::test]
async fn daemon_serves_reads_reconciles_notifications_and_joins_streams_on_cancel() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    let writer = agent_status_store::open_writer(&database).unwrap();
    let socket = directory.path().join("notify.sock");
    let notify = UnixListener::bind(&socket).unwrap();
    let cancel = CancellationToken::new();
    let daemon = Daemon::bind(options(&database, &socket, "127.0.0.1:0"), &cancel)
        .await
        .unwrap();
    let addr = daemon.address();
    let hub = daemon.hub().clone();
    let child = cancel.clone();
    let run = tokio::spawn(async move { daemon.run(&child, schedule()).await });
    let (mut notification, _) = tokio::time::timeout(Duration::from_secs(3), notify.accept())
        .await
        .unwrap()
        .unwrap();
    let client = reqwest::Client::new();
    let status: serde_json::Value = client
        .get(format!("http://{addr}/api/v1/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["activeAgentCount"], 0);
    let mut subscription = hub.subscribe_global();
    writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('new','test','host','linux','user',123)", []).unwrap();
    notification
        .write_all(b"{\"kind\":\"hello\"}\n")
        .await
        .unwrap();
    let snapshot = tokio::time::timeout(Duration::from_secs(3), subscription.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.kind, "snapshot");
    assert_eq!(snapshot.environments[0].id, "new");
    let mut sse = client
        .get(format!("http://{addr}/api/v1/events"))
        .send()
        .await
        .unwrap();
    assert!(sse.chunk().await.unwrap().is_some());
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .unwrap();
    use futures_util::StreamExt;
    assert!(ws.next().await.unwrap().unwrap().is_text());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(hub.global_count(), 0);
    assert!(TcpStream::connect(addr).await.is_err());
    // The read-only service leaves the writer and its data intact.
    assert_eq!(
        writer
            .query_row("SELECT count(*) FROM environments", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn header_deadline_closes_partial_requests_without_bounding_event_streams() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let cancel = CancellationToken::new();
    let daemon = Daemon::bind(
        options(
            &database,
            &directory.path().join("missing.sock"),
            "127.0.0.1:0",
        ),
        &cancel,
    )
    .await
    .unwrap();
    let addr = daemon.address();
    let child = cancel.clone();
    let run = tokio::spawn(async move { daemon.run(&child, schedule()).await });
    let mut connection = TcpStream::connect(addr).await.unwrap();
    connection
        .write_all(b"GET /api/v1/status HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), connection.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(
        bytes.is_empty(),
        "an incomplete header receives no application response: {bytes:?}"
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn bind_failures_are_typed_and_missing_database_is_never_created() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let missing = directory.path().join("missing.db");
    let socket = directory.path().join("notify.sock");
    let cancel = CancellationToken::new();
    assert!(matches!(
        Daemon::bind(options(&missing, &socket, "127.0.0.1:0"), &cancel).await,
        Err(runtime::Error::Store(_))
    ));
    assert!(!missing.exists());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    assert!(
        matches!(Daemon::bind(options(&missing, &socket, &addr), &cancel).await, Err(runtime::Error::Io(error)) if error.kind()==io::ErrorKind::AddrInUse)
    );
    assert!(!missing.exists());
    for addr in [":invalid", "invalid address"] {
        assert!(matches!(
            Daemon::bind(options(&missing, &socket, addr), &cancel).await,
            Err(runtime::Error::Io(_))
        ));
    }
    cancel.cancel();
    runtime::run(&cancel, options(&missing, &socket, "invalid address"))
        .await
        .unwrap();
}

#[tokio::test]
async fn native_wildcard_listener_accepts_ipv4_and_shutdown_releases_queued_readers() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let cancel = CancellationToken::new();
    let daemon = Daemon::bind(
        options(&database, &directory.path().join("missing.sock"), ":0"),
        &cancel,
    )
    .await
    .unwrap();
    let port = daemon.address().port();
    let child = cancel.clone();
    let run = tokio::spawn(async move { daemon.run(&child, schedule()).await });
    let response = reqwest::get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .await
        .unwrap();
    assert!(response.status().is_success());
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn forced_shutdown_aborts_an_incomplete_body_and_never_repolls_joined_tasks() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let cancel = CancellationToken::new();
    let daemon = Daemon::bind(
        options(
            &database,
            &directory.path().join("missing.sock"),
            "127.0.0.1:0",
        ),
        &cancel,
    )
    .await
    .unwrap();
    let addr = daemon.address();
    let child = cancel.clone();
    let run = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        daemon
            .run(
                &child,
                Schedule {
                    shutdown_timeout: Duration::from_millis(20),
                    ..schedule()
                },
            )
            .await
    }));
    let mut connection = TcpStream::connect(addr).await.unwrap();
    connection.write_all(b"PATCH /api/v1/agents/a HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nExpect: 100-continue\r\n\r\n").await.unwrap();
    let mut interim = [0; 25];
    tokio::time::timeout(Duration::from_secs(3), connection.read_exact(&mut interim))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), run)
            .await
            .unwrap()
            .unwrap(),
        Err(runtime::Error::ShutdownTimeout)
    ));
    let mut remaining = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(3),
        connection.read_to_end(&mut remaining),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(remaining.is_empty());
    assert!(TcpStream::connect(addr).await.is_err());
}
