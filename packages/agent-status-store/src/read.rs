//! Snapshot-safe dashboard queries. Every row retains native SQL NULL distinctions.
use crate::{Error, Result};
use agent_common::{
    types::*,
    usage::{self, Entry, Evidence, Report},
};
use rusqlite::{
    Connection, OptionalExtension, Row, Transaction, params, params_from_iter, types::Value,
};

#[derive(Clone, Debug, Default)]
pub struct AgentFilter {
    pub project: Option<String>,
    pub environment: Option<String>,
    pub lifecycle: Option<LifecycleState>,
    pub activity: Option<ActivityState>,
    pub hidden: Option<bool>,
}

const AGENT_QUERY: &str = "SELECT a.*,e.label AS env_label,e.hostname AS env_hostname,e.platform AS env_platform,
    e.user AS env_user,e.last_heartbeat_at AS env_heartbeat,
    r.id AS run_id,r.pid AS run_pid,r.harness_version AS run_version,r.started_at AS run_started,r.ended_at AS run_ended
    FROM agents a JOIN environments e ON a.environment_id=e.id LEFT JOIN runs r ON a.current_run_id=r.id";

fn string_enum<T: serde::de::DeserializeOwned>(value: String) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(value))?)
}

fn scan_agent(row: &Row<'_>, now: i64) -> Result<AgentState> {
    let run_id: Option<String> = row.get("run_id")?;
    let current_run = run_id
        .map(|id| -> Result<RunInfo> {
            Ok(RunInfo {
                id,
                pid: row.get("run_pid")?,
                harness_version: row.get("run_version")?,
                started_at: row.get("run_started")?,
                ended_at: row.get("run_ended")?,
            })
        })
        .transpose()?;
    let lifecycle = if current_run
        .as_ref()
        .is_some_and(|run| run.ended_at.is_none())
    {
        LifecycleState::Running
    } else {
        LifecycleState::Exited
    };
    let heartbeat: i64 = row.get("env_heartbeat")?;
    let subagents: Option<String> = row.get("active_subagents")?;
    let active_subagents = subagents
        .filter(|s| !s.trim().is_empty())
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    let phase: Option<String> = row.get("phase")?;
    let cost: Option<f64> = row.get("cost_usd")?;
    let model: Option<String> = row.get("model")?;
    Ok(AgentState {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        harness: string_enum(row.get("harness")?)?,
        environment: EnvironmentInfo {
            id: row.get("environment_id")?,
            label: row.get("env_label")?,
            hostname: row.get("env_hostname")?,
            platform: row.get("env_platform")?,
            user: row.get("env_user")?,
            is_stale: heartbeat <= 0 || now.saturating_sub(heartbeat) > 30000,
            last_heartbeat_at: heartbeat,
        },
        project: ProjectInfo {
            id: row.get("project_id")?,
            name: row.get("project_name")?,
            canonical_root: row.get("canonical_root")?,
        },
        workspace: WorkspaceInfo {
            branch: row.get("branch")?,
            is_detached: row.get("is_detached")?,
            is_worktree: row.get("is_worktree")?,
            rel_path: row.get("worktree_rel_path")?,
            cwd: row.get("cwd")?,
        },
        activity: AgentActivity {
            lifecycle,
            current_run,
            activity: string_enum(row.get("activity")?)?,
            title: row.get("title")?,
            action: row.get("action")?,
            action_stale: row.get("action_stale")?,
            phase: phase.map(string_enum).transpose()?,
            active_tool: row.get("active_tool")?,
            active_target: row.get("active_target")?,
            active_tool_started_at: row.get("active_tool_started_at")?,
            question: row.get("question")?,
            last_thinking_excerpt: row.get("last_thinking_excerpt")?,
            last_error: row.get("last_error")?,
            rate_limit_reset_at: row.get("rate_limit_reset_at")?,
            retry_after_seconds: row.get("retry_after_seconds")?,
            model: model.filter(|s| !s.is_empty()),
            context_window: row.get("context_window")?,
            context_fullness: row.get("context_fullness")?,
            usage: TokenUsage {
                input: row.get("tokens_input")?,
                output: row.get("tokens_output")?,
                cache_read: row.get("tokens_cache_read")?,
                cache_write: row.get("tokens_cache_write")?,
                reasoning: row.get("tokens_reasoning")?,
                total: row.get("tokens_total")?,
            },
            cost: cost.map(|total| CostInfo {
                currency: "USD".into(),
                total,
                is_estimated: false,
            }),
            subagent_count: row.get("subagent_count")?,
            active_subagents,
            first_started_at: row.get("first_started_at")?,
            updated_at: row.get("updated_at")?,
            has_unseen_completion: if row.get::<_, bool>("has_unseen_completion")? {
                Some(true)
            } else {
                None
            },
            completed_at: row.get("completed_at")?,
            summary: row.get("summary")?,
            summary_updated_at: row.get("summary_updated_at")?,
        },
        is_hidden: row.get("is_hidden")?,
        hidden_at: row.get("hidden_at")?,
    })
}

pub fn agent(db: &Connection, id: &str, now: i64) -> Result<AgentState> {
    let query = format!("{AGENT_QUERY} WHERE a.id=?");
    let mut statement = db.prepare(&query)?;
    let mut rows = statement.query([id])?;
    rows.next()?
        .map(|row| scan_agent(row, now))
        .unwrap_or(Err(Error::NotFound))
}

pub fn agents(db: &Connection, filter: &AgentFilter, now: i64) -> Result<Vec<AgentState>> {
    let mut where_clauses = Vec::new();
    let mut values = Vec::new();
    for (column, value) in [
        ("a.project_id", filter.project.as_ref()),
        ("a.environment_id", filter.environment.as_ref()),
    ] {
        if let Some(value) = value.filter(|s| !s.is_empty()) {
            where_clauses.push(format!("{column}=?"));
            values.push(Value::Text(value.clone()));
        }
    }
    if let Some(activity) = filter.activity.as_ref().filter(|a| !a.as_str().is_empty()) {
        where_clauses.push("a.activity=?".into());
        values.push(Value::Text(activity.as_str().into()));
    }
    match filter.lifecycle {
        Some(LifecycleState::Running) => {
            where_clauses.push("r.id IS NOT NULL AND r.ended_at IS NULL".into())
        }
        Some(LifecycleState::Exited) => {
            where_clauses.push("(r.id IS NULL OR r.ended_at IS NOT NULL)".into())
        }
        _ => {}
    }
    if let Some(hidden) = filter.hidden {
        where_clauses.push("a.is_hidden=?".into());
        values.push(Value::Integer(i64::from(hidden)));
    }
    let condition = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let query = format!("{AGENT_QUERY}{condition} ORDER BY a.updated_at DESC,a.id ASC");
    let mut statement = db.prepare(&query)?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push(scan_agent(row, now)?);
    }
    Ok(result)
}

pub fn last_sequence(db: &Connection, environment: &str) -> Result<i64> {
    Ok(db
        .query_row(
            "SELECT last_seq FROM observer_cursors WHERE environment_id=?",
            [environment],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0))
}

pub fn environments(db: &Connection, now: i64) -> Result<Vec<EnvironmentInfo>> {
    let mut statement = db.prepare("SELECT id,label,hostname,platform,user,last_heartbeat_at FROM environments ORDER BY label ASC,id ASC")?;
    Ok(statement
        .query_map([], |row| {
            let heartbeat: i64 = row.get(5)?;
            Ok(EnvironmentInfo {
                id: row.get(0)?,
                label: row.get(1)?,
                hostname: row.get(2)?,
                platform: row.get(3)?,
                user: row.get(4)?,
                last_heartbeat_at: heartbeat,
                is_stale: heartbeat <= 0 || now.saturating_sub(heartbeat) > 30000,
            })
        })?
        .collect::<std::result::Result<_, _>>()?)
}

/// Reads environment and aggregate state within the caller's snapshot transaction.
pub fn status(tx: &Transaction<'_>, now: i64) -> Result<StatusResponse> {
    let environments = environments(tx, now)?;
    Ok(tx.query_row(
        include_str!("sql/status.sql"),
        params![now, now, now, now, now, now],
        |row| {
            Ok(StatusResponse {
                status: "ok".into(),
                environments,
                active_agent_count: row.get(0)?,
                requires_attention_count: row.get(1)?,
                total_fleet_spend: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            })
        },
    )?)
}

pub fn projects(db: &Connection, now: i64) -> Result<Vec<ProjectGroup>> {
    let mut groups: Vec<ProjectGroup> = Vec::new();
    for agent in agents(db, &AgentFilter::default(), now)? {
        let index = groups
            .iter()
            .position(|group| group.project.id == agent.project.id)
            .unwrap_or_else(|| {
                groups.push(ProjectGroup {
                    project: agent.project.clone(),
                    ..Default::default()
                });
                groups.len() - 1
            });
        let group = &mut groups[index];
        group.total_cost += agent.activity.cost.as_ref().map_or(0.0, |cost| cost.total);
        group.active_agent_count += i64::from(agent.activity.lifecycle == LifecycleState::Running);
        group.agents.push(agent);
    }
    Ok(groups)
}

pub fn distinct_subagents(db: &Connection, id: &str) -> Result<Vec<String>> {
    Ok(db.prepare("SELECT DISTINCT subagent_id FROM telemetry_events WHERE agent_id=? AND subagent_id IS NOT NULL AND subagent_id!='' ORDER BY subagent_id ASC")?
        .query_map([id],|row|row.get(0))?.collect::<std::result::Result<_,_>>()?)
}

/// Event IDs are returned in `seq`, matching the paginated dashboard log contract.
pub fn events(
    db: &Connection,
    id: &str,
    limit: i64,
    before: i64,
    subagent: Option<&str>,
    now: i64,
) -> Result<Vec<TelemetryFrame>> {
    let limit = if limit <= 0 { 100 } else { limit.min(1000) };
    let query = "SELECT te.id,te.subagent_id,te.event_type,te.timestamp,te.data_json FROM telemetry_events te
        WHERE te.agent_id=? AND (?<=0 OR te.id<?) AND (? IS NULL OR te.subagent_id=?) ORDER BY te.id DESC LIMIT ?";
    let mut statement = db.prepare(query)?;
    let mut rows = statement.query(params![
        id,
        before,
        before,
        subagent.filter(|s| !s.is_empty()),
        subagent,
        limit
    ])?;
    let mut result = Vec::new();
    // Reading metadata after the first event keeps both reads within one SQLite statement snapshot.
    let mut metadata = None;
    while let Some(row) = rows.next()? {
        let agent = match &metadata {
            Some(agent) => agent,
            None => metadata.insert(agent(db, id, now)?),
        };
        let data: String = row.get(4)?;
        result.push(TelemetryFrame {
            version: "1.0".into(),
            seq: row.get(0)?,
            agent_id: agent.id.clone(),
            session_id: agent.session_id.clone(),
            env_id: agent.environment.id.clone(),
            environment: agent.environment.clone(),
            harness: agent.harness.clone(),
            project: agent.project.clone(),
            workspace: agent.workspace.clone(),
            pid: agent.activity.current_run.as_ref().map_or(0, |run| run.pid),
            harness_version: agent
                .activity
                .current_run
                .as_ref()
                .map_or_else(String::new, |run| run.harness_version.clone()),
            subagent_id: row.get::<_, Option<String>>(1)?.filter(|s| !s.is_empty()),
            event_type: row.get(2)?,
            timestamp: row.get(3)?,
            data: if data.is_empty() {
                Default::default()
            } else {
                serde_json::from_str(&data)?
            },
        });
    }
    Ok(result)
}

pub fn usage(db: &Connection, id: &str, now: i64) -> Result<Report> {
    let agent = agent(db, id, now)?;
    let mut report = usage::Report::new(agent.id, agent.harness);
    let mut statement = db.prepare(include_str!("sql/read-usage.sql"))?;
    let mut rows = statement.query([id])?;
    while let Some(row) = rows.next()? {
        let kind: String = row.get(0)?;
        let entry = Entry {
            name: row.get(1)?,
            server: row.get(2)?,
            operation: row.get(3)?,
            count: row.get(4)?,
            first_used_at: row.get(5)?,
            last_used_at: row.get(6)?,
            latest_evidence: Evidence {
                source: row.get(7)?,
                event_type: row.get(8)?,
                timestamp: row.get(9)?,
                subagent_id: row.get(10)?,
                tool_call_id: row.get(11)?,
            },
        };
        match kind.as_str() {
            "skill" => report.skills.push(entry),
            "command" => report.commands.push(entry),
            "mcp" => report.mcp.push(entry),
            _ => {}
        }
    }
    Ok(report)
}
