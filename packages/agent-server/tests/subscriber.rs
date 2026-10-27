#![cfg(unix)]
use agent_common::types::ServerEvent;
use agent_server::{
    hub::{GlobalSubscription, Hub},
    store::ReadStore,
    subscriber::{Subscriber, SubscriberConfig},
};
use std::{path::Path, time::Duration};
use tokio::{io::AsyncWriteExt, net::UnixListener};
use tokio_util::sync::CancellationToken;

async fn receive(subscription: &mut GlobalSubscription) -> std::sync::Arc<ServerEvent> {
    tokio::time::timeout(Duration::from_secs(3), subscription.recv())
        .await
        .unwrap()
        .unwrap()
}

fn config(path: &Path) -> SubscriberConfig {
    SubscriberConfig {
        socket_path: path.to_owned(),
        reconnect_wait: Duration::from_millis(1),
    }
}

#[tokio::test]
async fn real_unix_hello_and_reconnect_refresh_before_live_events() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    let writer = agent_status_store::open_writer(&database).unwrap();
    let socket = directory.path().join("notify.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&database, &cancel).await.unwrap();
    let hub = Hub::default();
    let mut events = hub.subscribe_global();
    let subscriber = Subscriber::new(config(&socket), store.clone(), hub.clone());
    let task = tokio::spawn(subscriber.run(cancel.clone()));
    let (mut connection, _) = listener.accept().await.unwrap();
    connection
        .write_all(b"\nnot-json\n{\"type\":\"hello\"}\n{\"type\":\"agent_upsert\",\"at\":1}\n")
        .await
        .unwrap();
    assert_eq!(receive(&mut events).await.kind, "snapshot");
    assert_eq!(receive(&mut events).await.at, 1);
    writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('new','test','host','linux','user',123)", []).unwrap();
    drop(connection);
    let (mut connection, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    connection
        .write_all(b"{\"type\":\"hello\"}\n")
        .await
        .unwrap();
    let snapshot = receive(&mut events).await;
    assert_eq!(snapshot.kind, "snapshot");
    assert_eq!(snapshot.environments[0].id, "new");
    // Hello is a control frame, so it never reaches global subscribers.
    connection
        .write_all(b"{\"type\":\"log_append\",\"at\":2}\n")
        .await
        .unwrap();
    assert_eq!(receive(&mut events).await.at, 2);
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    store.shutdown().await;
}

#[tokio::test]
async fn failed_snapshot_is_retried_on_next_event_without_dropping_live_events() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    let writer = agent_status_store::open_writer(&database).unwrap();
    let socket = directory.path().join("notify.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&database, &cancel).await.unwrap();
    let hub = Hub::default();
    let mut events = hub.subscribe_global();
    writer
        .execute("ALTER TABLE environments RENAME TO unavailable", [])
        .unwrap();
    let task =
        tokio::spawn(Subscriber::new(config(&socket), store.clone(), hub).run(cancel.clone()));
    let (mut connection, _) = listener.accept().await.unwrap();
    connection
        .write_all(b"{\"type\":\"hello\"}\n{\"type\":\"agent_upsert\",\"at\":1}\n")
        .await
        .unwrap();
    assert_eq!(receive(&mut events).await.at, 1);
    writer
        .execute("ALTER TABLE unavailable RENAME TO environments", [])
        .unwrap();
    // An old ingester may omit hello: the first ordinary event also reconciles.
    connection
        .write_all(b"{\"type\":\"agent_upsert\",\"at\":2}\n")
        .await
        .unwrap();
    assert_eq!(receive(&mut events).await.kind, "snapshot");
    assert_eq!(receive(&mut events).await.at, 2);
    cancel.cancel();
    task.await.unwrap().unwrap();
    store.shutdown().await;
}

#[tokio::test]
async fn fragmented_lines_final_unterminated_line_and_size_limit_are_handled() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let socket = directory.path().join("notify.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&database, &cancel).await.unwrap();
    let hub = Hub::default();
    let mut events = hub.subscribe_global();
    let task =
        tokio::spawn(Subscriber::new(config(&socket), store.clone(), hub).run(cancel.clone()));
    let (mut connection, _) = listener.accept().await.unwrap();
    connection.write_all(b"{\"type\":\"agent_").await.unwrap();
    connection.write_all(b"upsert\",\"at\":9}").await.unwrap();
    connection.shutdown().await.unwrap();
    assert_eq!(receive(&mut events).await.kind, "snapshot");
    assert_eq!(receive(&mut events).await.at, 9);
    let (mut connection, _) = listener.accept().await.unwrap();
    let oversized = vec![b'x'; 16 * 1024 * 1024 + 1];
    // The peer may close as soon as the bound is crossed.
    let _ = connection.write_all(&oversized).await;
    drop(connection);
    let (mut connection, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
        .await
        .unwrap()
        .unwrap();
    connection
        .write_all(b"{\"type\":\"hello\"}\r\n")
        .await
        .unwrap();
    assert_eq!(receive(&mut events).await.kind, "snapshot");
    cancel.cancel();
    task.await.unwrap().unwrap();
    store.shutdown().await;
}

#[tokio::test]
async fn cancellation_stops_missing_socket_backoff_and_idle_socket_reads() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&database, &cancel).await.unwrap();
    let token = CancellationToken::new();
    let subscriber = Subscriber::new(
        SubscriberConfig {
            socket_path: directory.path().join("absent.sock"),
            reconnect_wait: Duration::from_secs(60),
        },
        store.clone(),
        Hub::default(),
    );
    let task = tokio::spawn(subscriber.run(token.clone()));
    token.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let socket = directory.path().join("notify.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let token = CancellationToken::new();
    let task = tokio::spawn(
        Subscriber::new(config(&socket), store.clone(), Hub::default()).run(token.clone()),
    );
    let (_connection, _) = listener.accept().await.unwrap();
    token.cancel();
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    store.shutdown().await;
}
