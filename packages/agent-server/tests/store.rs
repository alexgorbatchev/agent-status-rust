use agent_server::store::{Error, ReadStore};
use agent_status_store::read::{self, AgentFilter};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn real_reader_never_creates_or_mutates_and_observes_writer_commits() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let missing = directory.path().join("missing.db");
    let cancel = CancellationToken::new();
    assert!(ReadStore::open(&missing, &cancel).await.is_err());
    assert!(!missing.exists());
    let path = directory.path().join("state.db");
    let writer = agent_status_store::open_writer(&path).unwrap();
    let store = ReadStore::open(&path, &cancel).await.unwrap();
    assert!(
        store
            .query(&cancel, |tx| {
                tx.execute("CREATE TABLE forbidden (id INTEGER)", [])?;
                Ok(())
            })
            .await
            .is_err()
    );
    let first = store.snapshot(&cancel).await.unwrap();
    assert_eq!(first.kind, "snapshot");
    assert!(first.agents.is_empty());
    assert!(first.environments.is_empty());
    writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('e','test','host','linux','user',123)", []).unwrap();
    assert_eq!(
        store.snapshot(&cancel).await.unwrap().environments[0].id,
        "e"
    );
    assert!(
        store
            .query(&cancel, |tx| read::agents(tx, &AgentFilter::default(), 123))
            .await
            .unwrap()
            .is_empty()
    );
    store.shutdown().await;
    assert!(matches!(
        store.query(&cancel, |_| Ok(())).await,
        Err(Error::Cancelled)
    ));
}

#[tokio::test]
async fn native_progress_handler_cancels_executing_sqlite_and_drain_joins_it() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let path = directory.path().join("state.db");
    agent_status_store::open_writer(&path).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&path, &cancel).await.unwrap();
    let worker = store.clone();
    let child = cancel.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let query = tokio::spawn(async move {
        worker.query(&child, move |tx| {
            let _ = entered.send(());
            Ok(tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0))?)
        }).await
    });
    ready.await.unwrap();
    cancel.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), query)
            .await
            .unwrap()
            .unwrap(),
        Err(Error::Cancelled)
    ));
    tokio::time::timeout(Duration::from_secs(2), store.shutdown())
        .await
        .unwrap();
}

#[tokio::test]
async fn dropped_request_interrupts_owned_work_and_parallel_queries_are_bounded() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let path = directory.path().join("state.db");
    agent_status_store::open_writer(&path).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&path, &cancel).await.unwrap();
    let worker = store.clone();
    let child = cancel.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let query = tokio::spawn(async move {
        worker.query(&child, move |tx| {
            let _ = entered.send(());
            Ok(tx.query_row("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n", [], |row| row.get::<_, i64>(0))?)
        }).await
    });
    ready.await.unwrap();
    query.abort();
    let _ = query.await;
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let mut queries = tokio::task::JoinSet::new();
    for _ in 0..24 {
        let worker = store.clone();
        let child = cancel.clone();
        let active = active.clone();
        let maximum = maximum.clone();
        queries.spawn(async move {
            worker
                .query(&child, move |tx| {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    let count = tx.query_row("SELECT count(*) FROM environments", [], |row| {
                        row.get::<_, i64>(0)
                    })?;
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(count)
                })
                .await
        });
    }
    while let Some(query) = queries.join_next().await {
        assert_eq!(query.unwrap().unwrap(), 0);
    }
    assert!((1..=4).contains(&maximum.load(Ordering::SeqCst)));
    tokio::time::timeout(Duration::from_secs(2), store.shutdown())
        .await
        .unwrap();
}

#[tokio::test]
async fn snapshot_queries_share_one_sqlite_transaction() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let path = directory.path().join("state.db");
    let writer = agent_status_store::open_writer(&path).unwrap();
    let cancel = CancellationToken::new();
    let store = ReadStore::open(&path, &cancel).await.unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let worker = store.clone();
    let child = cancel.clone();
    let query = tokio::spawn(async move {
        worker
            .query(&child, move |tx| {
                let before = read::environments(tx, 123)?;
                let _ = entered.send(());
                resumed.recv_timeout(Duration::from_secs(2)).unwrap();
                let after = read::environments(tx, 123)?;
                Ok((before, after))
            })
            .await
    });
    ready.await.unwrap();
    writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('new','test','host','linux','user',123)", []).unwrap();
    resume.send(()).unwrap();
    let (before, after) = query.await.unwrap().unwrap();
    assert!(before.is_empty());
    assert!(after.is_empty());
    assert_eq!(store.snapshot(&cancel).await.unwrap().environments.len(), 1);
    store.shutdown().await;
}
