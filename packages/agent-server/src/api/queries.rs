use super::{State, auth::query_value, error};
use crate::store::{self, Error};
use agent_status_store::read::{self, AgentFilter};
use axum::{
    Json,
    extract::{Path, RawQuery, State as ExtractState},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;

fn result<T: Serialize>(result: Result<T, Error>, message: &str, missing: bool) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(Error::Database(agent_status_store::Error::NotFound)) if missing => {
            error(StatusCode::NOT_FOUND, "agent not found")
        }
        Err(failure) => {
            tracing::error!(error = %failure, "query_failed");
            error(StatusCode::INTERNAL_SERVER_ERROR, message)
        }
    }
}

pub(super) async fn status(ExtractState(state): ExtractState<Arc<State>>) -> Response {
    result(
        state
            .store
            .query(&state.shutdown, |tx| read::status(tx, store::now()))
            .await,
        "failed to get status",
        false,
    )
}
pub(super) async fn projects(ExtractState(state): ExtractState<Arc<State>>) -> Response {
    result(
        state
            .store
            .query(&state.shutdown, |tx| read::projects(tx, store::now()))
            .await,
        "failed to get projects",
        false,
    )
}
pub(super) async fn environments(ExtractState(state): ExtractState<Arc<State>>) -> Response {
    result(
        state
            .store
            .query(&state.shutdown, |tx| read::environments(tx, store::now()))
            .await,
        "failed to get environments",
        false,
    )
}
pub(super) async fn agents(
    ExtractState(state): ExtractState<Arc<State>>,
    RawQuery(query): RawQuery,
) -> Response {
    let get = |key| query_value(query.as_deref(), key).filter(|value| !value.is_empty());
    let filter = AgentFilter {
        project: get("project"),
        environment: get("environment"),
        lifecycle: get("lifecycle")
            .and_then(|value| serde_json::from_value(serde_json::Value::String(value)).ok()),
        activity: get("activity")
            .and_then(|value| serde_json::from_value(serde_json::Value::String(value)).ok()),
        hidden: get("hidden").and_then(|value| match value.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }),
    };
    result(
        state
            .store
            .query(&state.shutdown, move |tx| {
                read::agents(tx, &filter, store::now())
            })
            .await,
        "failed to get agents",
        false,
    )
}
pub(super) async fn agent(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
) -> Response {
    result(
        state
            .store
            .query(&state.shutdown, move |tx| {
                read::agent(tx, &id, store::now())
            })
            .await,
        "failed to get agent",
        true,
    )
}
pub(super) async fn usage(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
) -> Response {
    result(
        state
            .store
            .query(&state.shutdown, move |tx| {
                read::usage(tx, &id, store::now())
            })
            .await,
        "failed to get agent usage",
        true,
    )
}
pub(super) async fn logs(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
) -> Response {
    let positive = |name, default| {
        query_value(query.as_deref(), name)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(default)
    };
    let limit = positive("limit", 100);
    let before = positive("before", 0);
    let subagent = query_value(query.as_deref(), "subagent").unwrap_or_default();
    result(
        state
            .store
            .query(&state.shutdown, move |tx| {
                read::events(tx, &id, limit, before, Some(&subagent), store::now())
            })
            .await,
        "failed to get agent logs",
        false,
    )
}
