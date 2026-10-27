//! Reconnecting, bounded newline protocol from the ingester's notification socket.
use crate::{hub::Hub, store::ReadStore};
use agent_common::types::ServerEvent;
use futures_util::StreamExt;
use std::{io, path::PathBuf, time::Duration};
use tokio::net::UnixStream;
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    sync::CancellationToken,
};

pub struct SubscriberConfig {
    pub socket_path: PathBuf,
    pub reconnect_wait: Duration,
}
impl Default for SubscriberConfig {
    fn default() -> Self {
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        Self {
            socket_path: directory.join("agent-status/notify.sock"),
            reconnect_wait: Duration::from_secs(1),
        }
    }
}

pub struct Subscriber {
    config: SubscriberConfig,
    store: ReadStore,
    hub: Hub,
}
impl Subscriber {
    pub fn new(mut config: SubscriberConfig, store: ReadStore, hub: Hub) -> Self {
        if config.socket_path.as_os_str().is_empty() {
            config.socket_path = SubscriberConfig::default().socket_path;
        }
        if config.reconnect_wait.is_zero() {
            config.reconnect_wait = Duration::from_secs(1);
        }
        Self { config, store, hub }
    }

    pub async fn run(self, cancel: CancellationToken) -> io::Result<()> {
        loop {
            let connection = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                connection = UnixStream::connect(&self.config.socket_path) => connection,
            };
            match connection {
                Ok(connection) => {
                    tracing::info!("notify_socket_connected");
                    self.read(connection, &cancel).await;
                }
                Err(error) => tracing::debug!(%error, "notify_socket_connect_failed"),
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(()),
                _ = tokio::time::sleep(self.config.reconnect_wait) => {},
            }
        }
    }

    async fn read(&self, connection: UnixStream, cancel: &CancellationToken) {
        let mut lines = FramedRead::new(
            connection,
            LinesCodec::new_with_max_length(16 * 1024 * 1024),
        );
        let mut reconciled = false;
        loop {
            let line = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                line = lines.next() => line,
            };
            let line = match line {
                Some(Ok(line)) => line,
                Some(Err(error)) => {
                    tracing::warn!(%error, "notify_socket_read_error");
                    return;
                }
                None => return,
            };
            if line.is_empty() {
                continue;
            }
            let event: ServerEvent = match serde_json::from_str(&line) {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(%error, length = line.len(), "notify_event_malformed");
                    continue;
                }
            };
            if event.kind == "hello" || !reconciled {
                match self.store.snapshot(cancel).await {
                    Ok(snapshot) => {
                        self.hub.broadcast(snapshot);
                        reconciled = true;
                    }
                    Err(error) => tracing::error!(%error, "notify_snapshot_resync_failed"),
                }
            }
            if event.kind != "hello" {
                self.hub.broadcast(event);
            }
        }
    }
}
