//! Authenticated telemetry and internal mutation endpoints.
mod http;
mod websocket;
use crate::{
    dispatch::Dispatcher, herdr, ingestion::Batch, notify::Publisher, owner::Owner,
    summarizer::Summarizer,
};
use agent_common::types::ServerEvent;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{any, patch, post},
};
use std::sync::Arc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

struct Inner {
    owner: Owner,
    publisher: Arc<Publisher>,
    summaries: Dispatcher,
    token: String,
    herdr: Arc<herdr::Client>,
    cancel: CancellationToken,
    connections: TaskTracker,
}
#[derive(Clone)]
pub struct Receiver(Arc<Inner>);
impl Receiver {
    pub fn new(
        owner: Owner,
        publisher: Arc<Publisher>,
        summarizer: Arc<Summarizer>,
        token: String,
        herdr: Arc<herdr::Client>,
    ) -> Self {
        let summaries = Dispatcher::new(owner.clone(), summarizer, publisher.clone());
        Self(Arc::new(Inner {
            owner,
            publisher,
            summaries,
            token,
            herdr,
            cancel: CancellationToken::new(),
            connections: TaskTracker::new(),
        }))
    }
    pub fn router(&self) -> Router {
        let internal = Router::new()
            .route(
                "/api/v1/internal/agents/{id}/visibility",
                patch(http::visibility),
            )
            .route(
                "/api/v1/internal/agents/{id}/dismiss-completion",
                patch(http::dismiss).post(http::dismiss),
            )
            .route("/api/v1/internal/agents/{id}", patch(http::change))
            .route("/api/v1/internal/agents/{id}/focus", post(http::focus))
            .route_layer(middleware::from_fn_with_state(
                self.clone(),
                http::authorize,
            ));
        internal
            .route("/ws/ingest", any(websocket::upgrade))
            .route("/healthz", any(|| async { "ok\n" }))
            .layer(DefaultBodyLimit::disable())
            .with_state(self.clone())
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.0.cancel.clone()
    }
    pub async fn shutdown(&self) {
        self.0.cancel.cancel();
        self.0.connections.close();
        self.0.summaries.shutdown().await;
        self.0.connections.wait().await;
    }
    pub async fn flush_summaries(&self) {
        self.0.summaries.flush().await;
    }
    pub fn publish_batch(&self, batch: Batch) -> i64 {
        for (index, agent) in batch.updates.into_iter().enumerate() {
            let id = agent.id.clone();
            self.publish(ServerEvent {
                kind: "agent_upsert".into(),
                at: wall_time(),
                agent: Some(agent),
                ..Default::default()
            });
            if let Some(frame) = batch.events.get(index) {
                self.publish(ServerEvent {
                    kind: "log_append".into(),
                    at: wall_time(),
                    agent_id: id,
                    event: Some(frame.clone()),
                    ..Default::default()
                });
            }
        }
        self.0.summaries.enqueue(batch.summaries);
        batch.up_to
    }
    fn publish(&self, event: ServerEvent) {
        if let Err(error) = self.0.publisher.publish(&event) {
            tracing::error!(%error,"notification serialization failed");
        }
    }
}
pub fn wall_time() -> i64 {
    crate::notify::now()
}

fn authorized(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    use subtle::ConstantTimeEq;
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| bool::from(provided.as_bytes().ct_eq(expected.as_bytes())))
}
