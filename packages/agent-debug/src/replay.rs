use crate::{
    Error, Result, check_cancel,
    diff::{self, StateDiff},
    transcript::{self, LoadOptions},
    verification,
};
use agent_common::types::{AgentState, EnvironmentInfo, TelemetryFrame};
use agent_ingester::{
    classifier,
    ingestion::Ingestion,
    summarizer::{self, Summarizer},
};
use agent_observer::git::Resolver;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
pub struct Options {
    pub load: LoadOptions,
    pub environment: String,
    pub diff: bool,
    pub until_line: Option<usize>,
    pub until_event: Option<String>,
    pub stop_on_error: bool,
    pub export_db: Option<PathBuf>,
    pub summarize: bool,
    pub summarizer: Option<Arc<Summarizer>>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayResult {
    pub harness: agent_common::types::HarnessType,
    pub session_id: String,
    pub agent_id: String,
    pub agent: Option<AgentState>,
    pub frames_processed: usize,
    pub event_counts: BTreeMap<String, usize>,
    pub subagents: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<StateDiff>,
    pub verification: verification::Report,
    /// Nanoseconds, matching the source JSON duration unit.
    pub duration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export_db_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    pub has_native_start: bool,
    pub has_native_end: bool,
    pub has_synthetic_start: bool,
    pub has_synthetic_end: bool,
    pub strict_lifecycle: bool,
    pub last_event_timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<crate::comparison::Comparison>,
}

pub async fn run(
    cancel: &CancellationToken,
    path: &Path,
    options: &Options,
) -> Result<ReplayResult> {
    run_observed(cancel, path, options, |_, _, _| Ok(())).await
}

/// Callbacks run after each committed ingestion step; errors stop replay and preserve its export.
pub async fn run_observed(
    cancel: &CancellationToken,
    path: &Path,
    options: &Options,
    mut observer: impl FnMut(usize, &TelemetryFrame, Option<&StateDiff>) -> std::io::Result<()>,
) -> Result<ReplayResult> {
    let start = Instant::now();
    let token = cancel.clone();
    let source = path.to_owned();
    let load = options.load.clone();
    let session =
        tokio::task::spawn_blocking(move || transcript::load_session(&token, &source, &load))
            .await
            .map_err(Error::Task)??;
    if options.stop_on_error && !session.diagnostics.is_empty() {
        return Err(Error::InvalidInput(session.diagnostics.join("; ")));
    }
    let mut ingestion = Ingestion::open(
        options
            .export_db
            .as_deref()
            .unwrap_or(Path::new(":memory:")),
        classifier::Config {
            idle_grace: Duration::ZERO,
            synchronous: true,
            summarize: true,
            debounce: Duration::ZERO,
            ..Default::default()
        },
    )?;
    let summaries = if let Some(engine) = &options.summarizer {
        engine.clone()
    } else {
        Arc::new(
            Summarizer::new(summarizer::Config::from_lookup(
                (!options.summarize).then_some(summarizer::DisableSource::Flag),
                |name| std::env::var(name).ok(),
            ))
            .await?,
        )
    };
    let label = if options.environment.is_empty() {
        "debug"
    } else {
        &options.environment
    };
    let now = chrono::Utc::now().timestamp_millis();
    let environment = ingestion.hello(
        EnvironmentInfo {
            id: format!("env-{label}"),
            label: label.into(),
            hostname: label.into(),
            platform: if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            }
            .into(),
            user: "local".into(),
            last_heartbeat_at: now,
            ..Default::default()
        },
        0,
        now,
    )?;
    let agent_id = format!(
        "{}:{}:{}",
        environment.id,
        session.harness.as_str(),
        session.session_id
    );
    let resolver = Resolver::new(Duration::ZERO);
    let mut result = ReplayResult {
        harness: session.harness.clone(),
        session_id: session.session_id.clone(),
        agent_id: agent_id.clone(),
        agent: None,
        frames_processed: 0,
        event_counts: BTreeMap::new(),
        subagents: session.subagents,
        diffs: Vec::new(),
        verification: verification::Report {
            agent_id: agent_id.clone(),
            passed: false,
            agent: None,
            log_count: 0,
            checks: Vec::new(),
            last_error: None,
        },
        duration: 0,
        export_db_path: options.export_db.clone(),
        errors: session.diagnostics,
        has_native_start: false,
        has_native_end: false,
        has_synthetic_start: false,
        has_synthetic_end: false,
        strict_lifecycle: options.load.strict_lifecycle,
        last_event_timestamp: 0,
        comparison: None,
    };
    for (index, event) in session.events.iter().enumerate() {
        check_cancel(cancel)?;
        if options
            .until_line
            .is_some_and(|line| line > 0 && event.source_line > line)
        {
            break;
        }
        let (project, workspace) = resolver
            .resolve(cancel, &event.event.cwd, &event.event.git_branch)
            .await?;
        let frame = TelemetryFrame {
            version: "1.0".into(),
            seq: i64::try_from(index + 1)
                .map_err(|_| Error::InvalidInput("replay sequence overflow".into()))?,
            env_id: environment.id.clone(),
            environment: environment.clone(),
            agent_id: agent_id.clone(),
            session_id: session.session_id.clone(),
            subagent_id: event.subagent_id.clone(),
            pid: 1000,
            harness: session.harness.clone(),
            harness_version: session.harness_version.clone(),
            project,
            workspace,
            timestamp: event.event.timestamp,
            event_type: event.event.event_type.clone(),
            data: event.event.data.clone(),
        };
        let previous = result.agent.clone();
        match ingestion.process_frames(&environment.id, std::slice::from_ref(&frame), now) {
            Ok(batch) => {
                result.agent = batch.updates.into_iter().last().or(result.agent);
                for job in batch.summaries {
                    let summary = summaries.summarize(cancel, &job.input).await;
                    check_cancel(cancel)?;
                    if let Some(agent) =
                        ingestion.apply_summary(&agent_id, &job, summary, frame.timestamp)?
                    {
                        result.agent = Some(agent);
                    }
                }
            }
            Err(error) => {
                if options.stop_on_error {
                    return Err(error.into());
                }
                result.errors.push(format!(
                    "step {} (line {}) event {}: {error}",
                    index + 1,
                    event.source_line,
                    frame.event_type
                ));
            }
        }
        result.frames_processed += 1;
        *result
            .event_counts
            .entry(frame.event_type.clone())
            .or_default() += 1;
        result.last_event_timestamp = frame.timestamp;
        if event.subagent_id.as_ref().is_none_or(String::is_empty) {
            match (frame.event_type.as_str(), event.is_synthetic) {
                ("run_started", false) => result.has_native_start = true,
                ("run_started", true) => result.has_synthetic_start = true,
                ("run_exited", false) => result.has_native_end = true,
                ("run_exited", true) => result.has_synthetic_end = true,
                _ => {}
            }
        }
        let state_diff = options
            .diff
            .then(|| diff::compute(index + 1, event, previous.as_ref(), result.agent.as_ref()));
        observer(index + 1, &frame, state_diff.as_ref())?;
        result.diffs.extend(state_diff);
        if options
            .until_event
            .as_ref()
            .is_some_and(|kind| kind == &frame.event_type)
        {
            break;
        }
    }
    result.verification = verification::verify(
        ingestion.connection(),
        &agent_id,
        options.load.strict_lifecycle,
        result.has_native_end,
        now,
    );
    result.duration = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
    Ok(result)
}
