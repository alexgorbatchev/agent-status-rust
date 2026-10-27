use super::*;
use crate::{ingestion, owner};
use agent_common::types::{IngesterMessage, ObserverMessage};
use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket, rejection::WebSocketUpgradeRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

pub(super) async fn upgrade(
    State(receiver): State<Receiver>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !authorized(&headers, &receiver.0.token) {
        return (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response();
    }
    if receiver.0.cancel.is_cancelled() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let upgrade = match upgrade {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let guard = receiver.0.connections.token();
    upgrade
        .max_message_size(64 * 1024 * 1024)
        .max_frame_size(64 * 1024 * 1024)
        .on_failed_upgrade(|error| tracing::debug!(%error,"WebSocket upgrade failed"))
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            receiver.connection(socket).await
        })
}
impl Receiver {
    async fn connection(&self, mut socket: WebSocket) {
        let mut environment = String::new();
        loop {
            let message = tokio::select! {biased;_=self.0.cancel.cancelled()=>return,message=socket.recv()=>match message {Some(Ok(message))=>message,_=>return}};
            let Message::Text(text) = message else {
                continue;
            };
            let response = match serde_json::from_str::<ObserverMessage>(&text) {
                Ok(message) => match self.message(&mut environment, message).await {
                    Ok(response) => response,
                    Err(error) => Some(failure(&error)),
                },
                Err(_) => Some(IngesterMessage {
                    kind: "error".into(),
                    code: "malformed".into(),
                    message: "invalid json payload".into(),
                    ..Default::default()
                }),
            };
            if let Some(response) = response {
                let text =
                    serde_json::to_string(&response).expect("ingester response is serializable");
                tokio::select! {biased;_=self.0.cancel.cancelled()=>return,result=socket.send(Message::Text(text.into()))=>if result.is_err() {return;}}
            }
        }
    }
    async fn message(
        &self,
        bound: &mut String,
        mut message: ObserverMessage,
    ) -> Result<Option<IngesterMessage>, owner::Error> {
        match message.kind.as_str() {
            "hello" => {
                let id = if message.env_id.is_empty() {
                    message
                        .environment
                        .as_ref()
                        .map(|env| env.id.clone())
                        .unwrap_or_default()
                } else {
                    message.env_id
                };
                if !bound.is_empty() && *bound != id {
                    return Err(ingestion::Error::Environment.into());
                }
                let save_id = id.clone();
                let environment = self
                    .0
                    .owner
                    .call(move |ingestion| {
                        ingestion.accept_hello(
                            message.environment,
                            &save_id,
                            message.since,
                            message.dropped,
                            wall_time(),
                        )
                    })
                    .await?;
                *bound = id;
                if let Some(environment) = environment {
                    self.publish(ServerEvent {
                        kind: "environment_upsert".into(),
                        at: wall_time(),
                        environment: Some(environment),
                        ..Default::default()
                    });
                }
                Ok(None)
            }
            "frames" => {
                let id = if !message.env_id.is_empty() {
                    message.env_id
                } else if !bound.is_empty() {
                    bound.clone()
                } else {
                    message
                        .frames
                        .first()
                        .map(|frame| frame.env_id.clone())
                        .unwrap_or_default()
                };
                if !bound.is_empty() && *bound != id {
                    return Err(ingestion::Error::Environment.into());
                }
                for frame in &mut message.frames {
                    if frame.env_id.is_empty() {
                        frame.env_id = id.clone();
                    }
                }
                let save_id = id.clone();
                let batch = self
                    .0
                    .owner
                    .call(move |ingestion| {
                        ingestion.process_frames(&save_id, &message.frames, wall_time())
                    })
                    .await?;
                *bound = id;
                let up_to = self.publish_batch(batch);
                Ok((up_to > 0).then(|| IngesterMessage {
                    kind: "ack".into(),
                    up_to,
                    ..Default::default()
                }))
            }
            _ => Ok(None),
        }
    }
}
fn failure(error: &owner::Error) -> IngesterMessage {
    let (code, message) = match error {
        owner::Error::Ingestion(ingestion::Error::Version) => {
            ("unsupported_version", "frame version must be 1.0")
        }
        owner::Error::Ingestion(ingestion::Error::Environment) => (
            "environment",
            "telemetry environment does not match this observer connection",
        ),
        owner::Error::Ingestion(ingestion::Error::Identity) => {
            ("identity", "invalid telemetry agent identity")
        }
        owner::Error::Ingestion(ingestion::Error::Sequence) => {
            ("sequence", "telemetry sequence must be positive")
        }
        _ => {
            tracing::error!(%error,"telemetry batch was not committed");
            (
                "storage",
                "telemetry batch was not committed; retry without advancing its cursor",
            )
        }
    };
    IngesterMessage {
        kind: "error".into(),
        code: code.into(),
        message: message.into(),
        ..Default::default()
    }
}
