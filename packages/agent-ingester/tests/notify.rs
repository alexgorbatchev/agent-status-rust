use agent_common::types::ServerEvent;
use agent_ingester::notify::Publisher;
use std::{
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
};

async fn line(reader: &mut BufReader<UnixStream>) -> ServerEvent {
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut text))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn ordered_notifications_shutdown_and_socket_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private/notify.sock");
    let publisher = Publisher::bind(&path).await.unwrap();
    assert_eq!(
        std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let mut first = BufReader::new(UnixStream::connect(&path).await.unwrap());
    let mut second = BufReader::new(UnixStream::connect(&path).await.unwrap());
    assert_eq!(line(&mut first).await.kind, "hello");
    assert_eq!(line(&mut second).await.kind, "hello");
    assert_eq!(publisher.client_count(), 2);
    for at in 1..=5 {
        publisher
            .publish(&ServerEvent {
                kind: "agent_upsert".into(),
                at,
                agent_id: "agent".into(),
                ..Default::default()
            })
            .unwrap();
    }
    for at in 1..=5 {
        assert_eq!(line(&mut first).await.at, at);
        assert_eq!(line(&mut second).await.at, at);
    }
    assert!(Publisher::bind(&path).await.is_err());
    assert!(std::fs::metadata(&path).unwrap().file_type().is_socket());
    publisher.shutdown().await.unwrap();
    publisher.shutdown().await.unwrap();
    assert!(!path.exists());
    assert!(UnixStream::connect(&path).await.is_err());
    assert_eq!(publisher.client_count(), 0);
}

#[tokio::test]
async fn preserves_files_and_reclaims_only_stale_sockets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notify.sock");
    std::fs::write(&path, "unrelated data").unwrap();
    assert_eq!(
        Publisher::bind(&path).await.err().unwrap().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "unrelated data");
    std::fs::remove_file(&path).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    let publisher = Publisher::bind(&path).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "replacement").unwrap();
    publisher.shutdown().await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "replacement");
    let publisher = Publisher::bind(dir.path().join("drop.sock")).await.unwrap();
    drop(publisher);
    tokio::time::timeout(Duration::from_secs(2), async {
        while dir.path().join("drop.sock").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn slow_clients_cannot_block_publication_or_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let publisher = Publisher::bind(dir.path().join("notify.sock"))
        .await
        .unwrap();
    let mut reader = BufReader::new(UnixStream::connect(publisher.path()).await.unwrap());
    line(&mut reader).await;
    let event = ServerEvent {
        kind: "agent_upsert".into(),
        agent_id: "x".repeat(1_000_000),
        ..Default::default()
    };
    for _ in 0..105 {
        publisher.publish(&event).unwrap();
    }
    assert_eq!(publisher.client_count(), 0);
    tokio::time::timeout(Duration::from_secs(1), publisher.shutdown())
        .await
        .unwrap()
        .unwrap();
}
