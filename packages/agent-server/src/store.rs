//! Native read-only SQLite transactions with bounded blocking work and cooperative cancellation.
use agent_common::types::ServerEvent;
use agent_status_store::read::{self, AgentFilter};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database request cancelled")]
    Cancelled,
    #[error(transparent)]
    Database(#[from] agent_status_store::Error),
    #[error("database worker failed")]
    Worker(#[from] tokio::task::JoinError),
}

struct Inner {
    path: PathBuf,
    permits: Arc<Semaphore>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

#[derive(Clone)]
pub struct ReadStore(Arc<Inner>);

impl ReadStore {
    pub async fn open(path: impl AsRef<Path>, cancel: &CancellationToken) -> Result<Self, Error> {
        let store = Self(Arc::new(Inner {
            path: path.as_ref().to_owned(),
            permits: Arc::new(Semaphore::new(4)),
            shutdown: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }));
        store
            .query(cancel, |tx| {
                tx.query_row("SELECT 1", [], |_| Ok(()))?;
                Ok(())
            })
            .await?;
        Ok(store)
    }

    /// Each call sees one SQLite transaction. Dropping the future interrupts its owned work.
    /// At most four native blocking operations execute concurrently; shutdown joins them.
    pub async fn query<T, F>(&self, cancel: &CancellationToken, operation: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&rusqlite::Transaction<'_>) -> agent_status_store::Result<T> + Send + 'static,
    {
        // Register before waiting for capacity, so shutdown also owns queued requests.
        let _request_tracking = self.0.tasks.token();
        let permit = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = self.0.shutdown.cancelled() => return Err(Error::Cancelled),
            permit = self.0.permits.clone().acquire_owned() => permit.map_err(|_| Error::Cancelled)?,
        };
        let request = cancel.child_token();
        let _request_guard = request.clone().drop_guard();
        let shutdown = self.0.shutdown.clone();
        let path = self.0.path.clone();
        let worker_request = request.clone();
        let worker_shutdown = shutdown.clone();
        let mut worker = self.0.tasks.spawn_blocking(move || {
            let _permit = permit;
            if worker_request.is_cancelled() || worker_shutdown.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let mut connection = agent_status_store::open_reader(path)?;
            let progress_request = worker_request.clone();
            let progress_shutdown = worker_shutdown.clone();
            connection
                .progress_handler(
                    1000,
                    Some(move || {
                        progress_request.is_cancelled() || progress_shutdown.is_cancelled()
                    }),
                )
                .map_err(agent_status_store::Error::from)?;
            let transaction = connection
                .transaction()
                .map_err(agent_status_store::Error::from)?;
            let result = operation(&transaction);
            if worker_request.is_cancelled() || worker_shutdown.is_cancelled() {
                Err(Error::Cancelled)
            } else {
                result.map_err(Error::from)
            }
        });
        tokio::select! {
            biased;
            _ = request.cancelled() => { request.cancel(); let _ = worker.await; Err(Error::Cancelled) },
            _ = shutdown.cancelled() => { request.cancel(); let _ = worker.await; Err(Error::Cancelled) },
            result = &mut worker => result?,
        }
    }

    pub async fn snapshot(&self, cancel: &CancellationToken) -> Result<ServerEvent, Error> {
        self.query(cancel, |tx| {
            let at = now();
            Ok(ServerEvent {
                kind: "snapshot".into(),
                at,
                agents: read::agents(tx, &AgentFilter::default(), at)?,
                environments: read::environments(tx, at)?,
                ..Default::default()
            })
        })
        .await
    }

    pub async fn shutdown(&self) {
        self.0.shutdown.cancel();
        self.0.permits.close();
        self.0.tasks.close();
        self.0.tasks.wait().await;
    }
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}
