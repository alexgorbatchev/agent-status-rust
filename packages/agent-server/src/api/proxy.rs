use super::{State, error};
use axum::{
    body::Bytes,
    extract::{Path, State as ExtractState, rejection::BytesRejection},
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Patch {
    is_hidden: Option<bool>,
    dismiss_completion: Option<bool>,
    acknowledge_completion: Option<bool>,
}

pub(super) async fn patch(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        Err(_) => return error(StatusCode::BAD_REQUEST, "failed to read request body"),
    };
    let fields = match serde_json::from_slice::<Option<Patch>>(&body) {
        Ok(fields) => fields.unwrap_or_default(),
        Err(_) => return error(StatusCode::BAD_REQUEST, "invalid request body"),
    };
    if fields.is_hidden.is_none()
        && fields.dismiss_completion != Some(true)
        && fields.acknowledge_completion != Some(true)
    {
        return error(StatusCode::BAD_REQUEST, "no recognized patch fields");
    }
    forward(&state, Method::PATCH, &id, false, headers, body).await
}

pub(super) async fn focus(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    forward(&state, Method::POST, &id, true, headers, Bytes::new()).await
}

async fn forward(
    state: &State,
    method: Method,
    id: &str,
    focus: bool,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut target = match url::Url::parse(&state.config.ingester_url) {
        Ok(target) => target,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create proxy request",
            );
        }
    };
    let Ok(mut segments) = target.path_segments_mut() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create proxy request",
        );
    };
    segments
        .pop_if_empty()
        .extend(["api", "v1", "internal", "agents"])
        .push(id);
    if focus {
        segments.push("focus");
    }
    drop(segments);
    let mut request = state
        .client
        .request(method, target)
        .header(header::CONTENT_TYPE, "application/json")
        .body(body);
    if !state.config.auth_token.is_empty() {
        request = request.bearer_auth(&state.config.auth_token);
    } else if let Some(value) = headers.get(header::AUTHORIZATION) {
        request = request.header(header::AUTHORIZATION, value);
    }
    let response = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => return error(StatusCode::BAD_GATEWAY, "failed to reach ingester"),
        response = request.send() => match response {
            Ok(response) => response,
            Err(_) => return error(StatusCode::BAD_GATEWAY, "failed to reach ingester"),
        },
    };
    let status = response.status();
    let body = tokio::select! {
        biased;
        _ = state.shutdown.cancelled() => return error(StatusCode::BAD_GATEWAY, "failed to read ingester response"),
        body = response.bytes() => match body {
            Ok(body) => body,
            Err(_) => return error(StatusCode::BAD_GATEWAY, "failed to read ingester response"),
        },
    };
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}
