use super::{State, auth, error};
use crate::hub::GlobalSubscription;
use agent_common::types::ServerEvent;
use axum::{
    extract::{
        Path, State as ExtractState,
        ws::{
            CloseFrame, Message, WebSocket, WebSocketUpgrade, rejection::WebSocketUpgradeRejection,
        },
    },
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::Stream;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn sse(
    stream: impl Stream<Item = Result<Event, axum::Error>> + Send + 'static,
    interval: Duration,
) -> Response {
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(interval).text("heartbeat"))
        .into_response();
    response.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("keep-alive"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", header::HeaderValue::from_static("no"));
    response
}

pub(super) async fn events(ExtractState(state): ExtractState<Arc<State>>) -> Response {
    // Register before the transactional snapshot; changes during its reads remain queued.
    let mut subscription = state.hub.subscribe_global();
    let snapshot = match state.store.snapshot(&state.shutdown).await {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            tracing::error!(error = %failure, "sse_snapshot_failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to get snapshot");
        }
    };
    let cancel = state.shutdown.clone();
    let stream = async_stream::stream! {
        if !cancel.is_cancelled() { yield Event::default().json_data(snapshot); }
        loop {
            let event = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                event = subscription.recv() => match event { Some(event) => event, None => break },
            };
            yield Event::default().json_data(&*event);
        }
    };
    sse(stream, state.config.heartbeat_interval)
}

pub(super) async fn tail(
    ExtractState(state): ExtractState<Arc<State>>,
    Path(id): Path<String>,
) -> Response {
    let mut subscription = state.hub.subscribe_tail(id);
    let cancel = state.shutdown.clone();
    let stream = async_stream::stream! {
        loop {
            let frame = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                frame = subscription.recv() => match frame { Some(frame) => frame, None => break },
            };
            yield Event::default().json_data(&*frame);
        }
    };
    sse(stream, state.config.heartbeat_interval)
}

pub(super) async fn websocket(
    ExtractState(state): ExtractState<Arc<State>>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !auth::websocket_origin(&headers, &state.config.cors_origins) {
        return error(StatusCode::FORBIDDEN, "origin not allowed");
    }
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let subscription = state.hub.subscribe_global();
    let snapshot = match state.store.snapshot(&state.shutdown).await {
        Ok(snapshot) => snapshot,
        Err(failure) => {
            tracing::error!(error = %failure, "ws_snapshot_failed");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to get snapshot");
        }
    };
    let tracking = state.tasks.token();
    upgrade.on_upgrade(move |socket| async move {
        let _tracking = tracking;
        websocket_loop(socket, subscription, snapshot, state.shutdown.clone()).await;
    })
}

async fn send(socket: &mut WebSocket, event: &ServerEvent, cancel: &CancellationToken) -> bool {
    let data = match serde_json::to_string(event) {
        Ok(data) => data,
        Err(_) => return false,
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => false,
        sent = socket.send(Message::Text(data.into())) => sent.is_ok(),
    }
}

async fn websocket_loop(
    mut socket: WebSocket,
    mut subscription: GlobalSubscription,
    snapshot: ServerEvent,
    cancel: CancellationToken,
) {
    if !send(&mut socket, &snapshot, &cancel).await {
        return;
    }
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Text(_) | Message::Binary(_))) => {
                    let close = Message::Close(Some(CloseFrame { code: 1008, reason: "unexpected data message".into() }));
                    tokio::select! { _ = cancel.cancelled() => {}, _ = socket.send(close) => {} }
                    return;
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
            },
            event = subscription.recv() => match event {
                Some(event) => if !send(&mut socket, &event, &cancel).await { return; },
                None => return,
            },
        }
    }
}
