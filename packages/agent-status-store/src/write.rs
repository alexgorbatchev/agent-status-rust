//! Mutations require a native transaction; the caller controls the complete commit boundary.
use crate::{Error, Result, schema};
use agent_common::types::{AgentState, EnvironmentInfo, TelemetryFrame};
use rusqlite::{OptionalExtension, Transaction, params};

pub fn upsert_environment(tx: &Transaction<'_>, environment: &EnvironmentInfo) -> Result<()> {
    tx.execute("INSERT INTO environments(id,label,hostname,platform,user,dropped_frames,last_heartbeat_at)
        VALUES(?,?,?,?,?,0,?) ON CONFLICT(id) DO UPDATE SET label=excluded.label,hostname=excluded.hostname,
        platform=excluded.platform,user=excluded.user,last_heartbeat_at=excluded.last_heartbeat_at",
        params![environment.id,environment.label,environment.hostname,environment.platform,environment.user,environment.last_heartbeat_at])?;
    Ok(())
}

pub fn heartbeat(tx: &Transaction<'_>, environment: &str, timestamp: i64) -> Result<()> {
    tx.execute(
        "UPDATE environments SET last_heartbeat_at=? WHERE id=?",
        params![timestamp, environment],
    )?;
    Ok(())
}

pub fn dropped_frames(tx: &Transaction<'_>, environment: &str, count: i64) -> Result<()> {
    tx.execute(
        "UPDATE environments SET dropped_frames=dropped_frames+? WHERE id=?",
        params![count, environment],
    )?;
    Ok(())
}

pub fn save_cursor(tx: &Transaction<'_>, environment: &str, sequence: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO observer_cursors(environment_id,last_seq) VALUES(?,?)
        ON CONFLICT(environment_id) DO UPDATE SET last_seq=excluded.last_seq",
        params![environment, sequence],
    )?;
    Ok(())
}

pub fn record_event(tx: &Transaction<'_>, frame: &TelemetryFrame) -> Result<i64> {
    tx.execute("INSERT INTO telemetry_events(agent_id,subagent_id,event_type,timestamp,data_json) VALUES(?,?,?,?,?)",
        params![frame.agent_id,frame.subagent_id,frame.event_type,frame.timestamp,serde_json::to_string(&frame.data)?])?;
    let id = tx.last_insert_rowid();
    schema::record_usage(tx, frame)?;
    Ok(id)
}

pub fn save_agent(tx: &Transaction<'_>, agent: &AgentState) -> Result<()> {
    upsert_environment(tx, &agent.environment)?;
    let activity = &agent.activity;
    if let Some(run) = &activity.current_run {
        tx.execute("INSERT INTO runs(id,agent_id,pid,harness_version,started_at,ended_at) VALUES(?,?,?,?,?,?)
            ON CONFLICT(id) DO UPDATE SET pid=excluded.pid,harness_version=excluded.harness_version,
            started_at=excluded.started_at,ended_at=excluded.ended_at",
            params![run.id,agent.id,run.pid,run.harness_version,run.started_at,run.ended_at])?;
    }
    let run_id = activity
        .current_run
        .as_ref()
        .filter(|run| run.ended_at.is_none())
        .map(|run| &run.id);
    let active_subagents = if activity.active_subagents.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&activity.active_subagents)?)
    };
    tx.execute(
        include_str!("sql/save-agent.sql"),
        params![
            agent.id,
            agent.session_id,
            agent.environment.id,
            agent.harness.as_str(),
            agent.project.id,
            agent.project.name,
            agent.project.canonical_root,
            agent.workspace.branch,
            agent.workspace.is_detached,
            agent.workspace.is_worktree,
            agent.workspace.rel_path,
            agent.workspace.cwd,
            run_id,
            activity.activity.as_str(),
            activity.title,
            activity.action,
            activity.action_stale,
            activity.phase.as_ref().map(|phase| phase.as_str()),
            activity.active_tool,
            activity.active_target,
            activity.active_tool_started_at,
            activity.question,
            activity.last_thinking_excerpt,
            activity.last_error,
            activity.rate_limit_reset_at,
            activity.retry_after_seconds,
            activity.usage.input,
            activity.usage.output,
            activity.usage.cache_read,
            activity.usage.cache_write,
            activity.usage.reasoning,
            activity.usage.total,
            activity.cost.as_ref().map(|cost| cost.total),
            activity.subagent_count,
            active_subagents,
            activity.first_started_at,
            activity.updated_at,
            agent.is_hidden,
            agent.hidden_at,
            activity.model,
            activity.context_window,
            activity.context_fullness,
            activity.has_unseen_completion.unwrap_or(false),
            activity.completed_at,
            activity.summary,
            activity.summary_updated_at
        ],
    )?;
    Ok(())
}

pub fn set_visibility(
    tx: &Transaction<'_>,
    id: &str,
    hidden: bool,
    hidden_at: Option<i64>,
    now: i64,
) -> Result<()> {
    if tx.execute(
        "UPDATE agents SET is_hidden=?,hidden_at=?,updated_at=? WHERE id=?",
        params![hidden, hidden_at, now, id],
    )? == 0
    {
        return Err(Error::NotFound);
    }
    Ok(())
}

pub fn dismiss_completion(tx: &Transaction<'_>, id: &str) -> Result<()> {
    if tx.execute(
        "UPDATE agents SET has_unseen_completion=0,completed_at=NULL WHERE id=?",
        [id],
    )? == 0
    {
        return Err(Error::NotFound);
    }
    Ok(())
}

pub fn end_current_run(tx: &Transaction<'_>, id: &str, ended_at: i64) -> Result<()> {
    let current: Option<Option<String>> = tx
        .query_row(
            "SELECT current_run_id FROM agents WHERE id=?",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(Some(run)) = current {
        tx.execute(
            "UPDATE runs SET ended_at=? WHERE id=?",
            params![ended_at, run],
        )?;
    }
    tx.execute(
        "UPDATE agents SET current_run_id=NULL,active_tool_started_at=NULL WHERE id=?",
        [id],
    )?;
    Ok(())
}

pub fn prune(tx: &Transaction<'_>, events_before: i64, agents_before: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM telemetry_events WHERE timestamp<?",
        [events_before],
    )?;
    tx.execute("DELETE FROM agents WHERE updated_at<?", [agents_before])?;
    tx.execute("DELETE FROM runs WHERE (ended_at IS NOT NULL AND ended_at<?) OR agent_id NOT IN (SELECT id FROM agents)",[agents_before])?;
    Ok(())
}
