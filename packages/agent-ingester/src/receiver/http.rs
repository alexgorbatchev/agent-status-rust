use super::*;
use crate::{
    ingestion::{self, StateChange},
    owner,
};
use agent_common::types::LifecycleState;
use agent_status_store::read;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

pub(super) async fn authorize(
    State(receiver): State<Receiver>,
    request: Request,
    next: Next,
) -> Response {
    if !receiver.0.token.is_empty() && !authorized(request.headers(), &receiver.0.token) {
        if request.uri().path().ends_with("/focus") {
            return json_error(StatusCode::UNAUTHORIZED, "unauthorized");
        }
        return (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response();
    }
    next.run(request).await
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Visibility {
    is_hidden: bool,
}
pub(super) async fn visibility(
    State(receiver): State<Receiver>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let Ok(body) = serde_json::from_slice::<Visibility>(&body) else {
        return invalid_body();
    };
    mutate(
        receiver,
        id,
        StateChange {
            is_hidden: Some(body.is_hidden),
            ..Default::default()
        },
    )
    .await
}
pub(super) async fn dismiss(State(receiver): State<Receiver>, Path(id): Path<String>) -> Response {
    mutate(
        receiver,
        id,
        StateChange {
            dismiss_completion: Some(true),
            ..Default::default()
        },
    )
    .await
}
pub(super) async fn change(
    State(receiver): State<Receiver>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let Ok(body) = serde_json::from_slice::<StateChange>(&body) else {
        return invalid_body();
    };
    mutate(receiver, id, body).await
}
async fn mutate(receiver: Receiver, id: String, change: StateChange) -> Response {
    match receiver
        .0
        .owner
        .call(move |ingestion| ingestion.change_state(&id, change, wall_time()))
        .await
    {
        Ok(agent) => {
            receiver.publish(ServerEvent {
                kind: "agent_upsert".into(),
                at: wall_time(),
                agent: Some(agent.clone()),
                ..Default::default()
            });
            Json(agent).into_response()
        }
        Err(error) => store_error(error),
    }
}
fn store_error(error: owner::Error) -> Response {
    let status = match &error {
        owner::Error::Ingestion(ingestion::Error::Store(agent_status_store::Error::NotFound)) => {
            StatusCode::NOT_FOUND
        }
        owner::Error::Ingestion(ingestion::Error::EmptyState) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, format!("{error}\n")).into_response()
}
fn invalid_body() -> Response {
    (StatusCode::BAD_REQUEST, "invalid request body\n").into_response()
}
fn json_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
pub(super) async fn focus(State(receiver): State<Receiver>, Path(id): Path<String>) -> Response {
    let agent = match receiver
        .0
        .owner
        .call(move |ingestion| Ok(read::agent(ingestion.connection(), &id, wall_time())?))
        .await
    {
        Ok(agent) => agent,
        Err(owner::Error::Ingestion(ingestion::Error::Store(
            agent_status_store::Error::NotFound,
        ))) => return json_error(StatusCode::NOT_FOUND, "agent not found"),
        Err(error) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if agent.activity.lifecycle == LifecycleState::Exited {
        return json_error(StatusCode::CONFLICT, "agent is exited");
    }
    let client = &receiver.0.herdr;
    if client.ping(&receiver.0.cancel).await.is_err() {
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "herdr multiplexer daemon is not running",
        );
    }
    let pid = agent.activity.current_run.as_ref().map_or(0, |run| run.pid);
    let pane = match client
        .find_pane(
            &receiver.0.cancel,
            pid,
            &agent.workspace.cwd,
            agent.harness.as_str(),
        )
        .await
    {
        Ok(pane) => pane,
        Err(herdr::Error::PaneNotFound) => {
            return json_error(
                StatusCode::NOT_FOUND,
                "no matching herdr pane found for agent",
            );
        }
        Err(error) => {
            return json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("locating herdr pane: {error}"),
            );
        }
    };
    if let Err(error) = client.focus_pane(&receiver.0.cancel, &pane.pane_id).await {
        return json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("focusing herdr pane: {error}"),
        );
    }
    Json(json!({"ok":true,"paneId":pane.pane_id,"workspaceId":pane.workspace_id,"tabId":pane.tab_id})).into_response()
}
