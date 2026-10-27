use crate::{
    config::Paths,
    git::Resolver,
    shipper::{self, Shipper},
};
use agent_common::types::{EnvironmentInfo, HarnessType as TelemetryHarness, TelemetryFrame};
use agent_watcher::{Config, Event, Handler};
use std::{io, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("watching agent sessions failed")]
    Watcher(#[source] io::Error),
    #[error(transparent)]
    Shipper(#[from] shipper::Error),
}

pub async fn telemetry_frame(
    cancel: &CancellationToken,
    environment: &EnvironmentInfo,
    resolver: &Resolver,
    event: Event,
) -> io::Result<TelemetryFrame> {
    let (project, workspace) = resolver
        .resolve(cancel, &event.parsed.cwd, &event.parsed.git_branch)
        .await?;
    Ok(TelemetryFrame {
        version: "1.0".into(),
        env_id: environment.id.clone(),
        environment: environment.clone(),
        agent_id: format!("{}:{}:{}", environment.id, event.harness, event.session_id),
        session_id: event.session_id,
        subagent_id: event.subagent_id,
        pid: i64::from(event.pid),
        harness: match event.harness {
            agent_watcher::Harness::ClaudeCode => TelemetryHarness::ClaudeCode,
            agent_watcher::Harness::Pi => TelemetryHarness::Pi,
            agent_watcher::Harness::Codex => TelemetryHarness::Codex,
            agent_watcher::Harness::Opencode => TelemetryHarness::Opencode,
        },
        harness_version: event.harness_version,
        project,
        workspace,
        timestamp: event.parsed.timestamp,
        event_type: event.parsed.event_type,
        data: event.parsed.data,
        ..Default::default()
    })
}

/// Watches and ships until cancellation. Watcher workers join and persist checkpoints before
/// transport shutdown; unacknowledged transcript data stays available for restart replay.
pub async fn run(
    cancel: &CancellationToken,
    mut watcher: Config,
    shipper_config: shipper::Config,
) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    let paths = Paths::capture();
    if watcher.cursor_path.as_os_str().is_empty() {
        watcher.cursor_path = paths.observer_cache("observer-cursors.json");
    }
    if watcher.session_state_path.as_os_str().is_empty() {
        watcher.session_state_path = paths.observer_cache("observer-sessions.json");
    }
    let environment = Arc::new(shipper_config.environment.clone());
    let shipper = Arc::new(Shipper::new(shipper_config)?);
    let resolver = Arc::new(Resolver::default());
    let handler: Handler = Arc::new({
        let shipper = shipper.clone();
        move |cancel, event, acknowledgement| {
            let shipper = shipper.clone();
            let resolver = resolver.clone();
            let environment = environment.clone();
            Box::pin(async move {
                let frame = telemetry_frame(cancel, &environment, &resolver, event).await?;
                shipper
                    .enqueue(cancel, frame, acknowledgement)
                    .await
                    .map_err(io::Error::other)?;
                Ok(())
            })
        }
    });
    let watcher_cancel = cancel.child_token();
    let transport_cancel = CancellationToken::new();
    let watching = agent_watcher::run(&watcher_cancel, watcher, handler);
    let shipping = shipper.run(&transport_cancel);
    tokio::pin!(watching, shipping);
    tokio::select! {
        result = &mut watching => {
            shipper.stop();
            shipping.await?;
            result.map_err(Error::Watcher)
        },
        result = &mut shipping => {
            watcher_cancel.cancel();
            let watched = watching.await;
            result?;
            watched.map_err(Error::Watcher)
        },
    }
}
