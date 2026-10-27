//! Native Axum query, mutation-forwarding, SSE, and WebSocket endpoints.
mod auth;
mod proxy;
mod queries;
mod streams;

use crate::{hub::Hub, store::ReadStore};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, Method, StatusCode, header},
    middleware,
    routing::{get, post},
};
use std::{sync::Arc, time::Duration};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    timeout::TimeoutLayer,
};

#[derive(Clone)]
pub struct ApiConfig {
    pub ingester_url: String,
    pub auth_token: String,
    pub cors_origins: Vec<String>,
    pub rest_timeout: Duration,
    pub proxy_timeout: Duration,
    pub heartbeat_interval: Duration,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            ingester_url: "http://127.0.0.1:4501".into(),
            auth_token: String::new(),
            cors_origins: vec!["http://localhost:3000".into()],
            rest_timeout: Duration::from_secs(15),
            proxy_timeout: Duration::from_secs(5),
            heartbeat_interval: Duration::from_secs(15),
        }
    }
}

struct State {
    store: ReadStore,
    hub: Hub,
    config: ApiConfig,
    client: reqwest::Client,
    shutdown: CancellationToken,
    tasks: TaskTracker,
}

/// The owner must call shutdown to cancel and join upgraded connection handlers.
pub struct Api {
    router: Router,
    state: Arc<State>,
}
impl Api {
    pub fn new(
        store: ReadStore,
        hub: Hub,
        mut config: ApiConfig,
        cancel: CancellationToken,
    ) -> Result<Self, reqwest::Error> {
        if config.ingester_url.is_empty() {
            config.ingester_url = ApiConfig::default().ingester_url;
        }
        if config.cors_origins.is_empty() {
            config.cors_origins = ApiConfig::default().cors_origins;
        }
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .redirect(reqwest::redirect::Policy::none())
            .timeout(config.proxy_timeout)
            .build()?;
        let state = Arc::new(State {
            store,
            hub,
            config,
            client,
            shutdown: cancel.child_token(),
            tasks: TaskTracker::new(),
        });
        let rest = Router::new()
            .route("/api/v1/status", get(queries::status))
            .route("/api/v1/projects", get(queries::projects))
            .route("/api/v1/environments", get(queries::environments))
            .route("/api/v1/agents", get(queries::agents))
            .route(
                "/api/v1/agents/{id}",
                get(queries::agent).patch(proxy::patch),
            )
            .route("/api/v1/agents/{id}/usage", get(queries::usage))
            .route("/api/v1/agents/{id}/logs", get(queries::logs))
            .route("/api/v1/agents/{id}/focus", post(proxy::focus))
            // Preserve the source mutation contract, which reads the entire JSON body.
            .layer(DefaultBodyLimit::disable())
            .layer(TimeoutLayer::with_status_code(
                StatusCode::GATEWAY_TIMEOUT,
                state.config.rest_timeout,
            ));
        let origins = state.config.cors_origins.clone();
        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
                auth::cors_matches(origin, &origins)
            }))
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::ACCEPT,
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::HeaderName::from_static("x-csrf-token"),
            ])
            .expose_headers([header::LINK])
            .allow_credentials(true)
            .max_age(Duration::from_secs(300));
        let router = rest
            .route("/api/v1/events", get(streams::events))
            .route("/api/v1/agents/{id}/live-tail", get(streams::tail))
            .route("/ws/events", get(streams::websocket))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                auth::authorize,
            ))
            .layer(cors)
            .with_state(state.clone());
        Ok(Self { router, state })
    }
    pub fn router(&self) -> Router {
        self.router.clone()
    }
    pub async fn shutdown(&self) {
        self.state.shutdown.cancel();
        self.state.tasks.close();
        self.state.tasks.wait().await;
    }
}

fn error(status: StatusCode, message: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, axum::Json(serde_json::json!({"error":message}))).into_response()
}
