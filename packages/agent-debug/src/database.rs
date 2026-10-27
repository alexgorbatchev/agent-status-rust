use crate::{Error, Result, check_cancel};
use agent_common::types::{AgentState, EventData};
use agent_status_store::read;
use path_clean::PathClean;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub agent_id: String,
    pub pid: i64,
    pub harness_version: String,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub db_path: PathBuf,
    pub agent: AgentState,
    pub runs: Vec<RunRecord>,
    pub event_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_event_timestamp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_timestamp: Option<i64>,
}
fn open(path: &Path, cancel: &CancellationToken) -> Result<Connection> {
    check_cancel(cancel)?;
    let db = agent_status_store::open_reader(path)?;
    let token = cancel.clone();
    db.progress_handler(1000, Some(move || token.is_cancelled()))?;
    Ok(db)
}
pub fn inspect(
    cancel: &CancellationToken,
    path: &Path,
    session_id: &str,
    now: i64,
) -> Result<SessionRecord> {
    let mut db = open(path, cancel)?;
    let tx = db.transaction()?;
    let id: String = tx.query_row("SELECT id FROM agents WHERE session_id=? OR id=? ORDER BY updated_at DESC,id ASC LIMIT 1", [session_id, session_id], |row| row.get(0)).optional()?.ok_or_else(|| Error::InvalidInput(format!("session not found in database: {session_id}")))?;
    let agent = read::agent(&tx, &id, now)?;
    let runs = tx.prepare("SELECT id,agent_id,pid,harness_version,started_at,ended_at FROM runs WHERE agent_id=? ORDER BY started_at ASC,id ASC")?.query_map([&id], |row| Ok(RunRecord { id: row.get(0)?, agent_id: row.get(1)?, pid: row.get(2)?, harness_version: row.get(3)?, started_at: row.get(4)?, ended_at: row.get(5)? }))?.collect::<std::result::Result<Vec<_>, _>>()?;
    let (event_count, first_event_timestamp, last_event_timestamp) = tx.query_row(
        "SELECT COUNT(*),MIN(timestamp),MAX(timestamp) FROM telemetry_events WHERE agent_id=?",
        [&id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    check_cancel(cancel)?;
    Ok(SessionRecord {
        db_path: path.into(),
        agent,
        runs,
        event_count,
        first_event_timestamp,
        last_event_timestamp,
    })
}
pub fn list_agents(cancel: &CancellationToken, path: &Path, now: i64) -> Result<Vec<AgentState>> {
    let mut db = open(path, cancel)?;
    let tx = db.transaction()?;
    let agents = read::agents(&tx, &read::AgentFilter::default(), now)?;
    check_cancel(cancel)?;
    Ok(agents)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub id: i64,
    pub agent_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub harness: String,
    pub subagent_id: Option<String>,
    pub event_type: String,
    pub timestamp: i64,
    pub data_json: String,
    pub data: EventData,
    #[serde(skip_serializing_if = "is_zero")]
    pub pid: i64,
}
fn is_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Clone, Debug)]
pub struct EventQuery {
    pub limit: i64,
    pub event_types: Vec<String>,
    pub subagent_id: Option<String>,
    pub include_subagents: bool,
    pub descending: bool,
    pub event_id: Option<i64>,
}
impl Default for EventQuery {
    fn default() -> Self {
        Self {
            limit: 20,
            event_types: Vec::new(),
            subagent_id: None,
            include_subagents: true,
            descending: true,
            event_id: None,
        }
    }
}
pub fn events(
    cancel: &CancellationToken,
    path: &Path,
    session_id: &str,
    options: &EventQuery,
) -> Result<Vec<EventRecord>> {
    let db = open(path, cancel)?;
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if let Some(id) = options.event_id.filter(|v| *v > 0) {
        clauses.push("te.id=?".into());
        values.push(Value::Integer(id));
    }
    if !session_id.is_empty() {
        clauses.push("(a.session_id=? OR a.session_id LIKE ? OR a.id=? OR a.id LIKE ? OR te.agent_id=? OR te.agent_id LIKE ?)".into());
        for _ in 0..3 {
            values.extend([
                Value::Text(session_id.into()),
                Value::Text(format!("%{session_id}%")),
            ]);
        }
    }
    let types = options
        .event_types
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if !types.is_empty() {
        clauses.push(format!(
            "te.event_type IN ({})",
            vec!["?"; types.len()].join(",")
        ));
        values.extend(types.into_iter().map(|v| Value::Text(v.into())));
    }
    if let Some(id) = options.subagent_id.as_ref().filter(|s| !s.is_empty()) {
        clauses.push("te.subagent_id=?".into());
        values.push(Value::Text(id.clone()));
    }
    if !options.include_subagents {
        clauses.push("(te.subagent_id IS NULL OR te.subagent_id='')".into());
    }
    let condition = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let mut query = format!(
        "SELECT te.id,te.agent_id,te.subagent_id,te.event_type,te.timestamp,te.data_json,COALESCE(a.session_id,''),COALESCE(a.harness,''),COALESCE(r.pid,0) FROM telemetry_events te LEFT JOIN agents a ON te.agent_id=a.id LEFT JOIN runs r ON a.current_run_id=r.id{condition} ORDER BY te.id {}",
        if options.descending { "DESC" } else { "ASC" }
    );
    if options.limit > 0 {
        query.push_str(" LIMIT ?");
        values.push(Value::Integer(options.limit));
    }
    let mut statement = db.prepare(&query)?;
    let mut rows = statement.query(params_from_iter(values))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        check_cancel(cancel)?;
        let data_json: String = row.get(5)?;
        // Invalid stored data must surface as a diagnostic, not silently become empty telemetry.
        let raw: serde_json::Value = if data_json.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&data_json)?
        };
        let data = serde_json::from_value(raw.clone())?;
        let agent_id: String = row.get(1)?;
        let mut session_id: String = row.get(6)?;
        let mut harness: String = row.get(7)?;
        let mut pid: i64 = row.get(8)?;
        if pid <= 0 {
            pid = raw
                .get("pid")
                .and_then(serde_json::Value::as_i64)
                .filter(|v| *v > 0)
                .unwrap_or_default();
        }
        if harness.is_empty() {
            harness = raw
                .get("harness")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .into();
        }
        let identity = agent_id.splitn(3, ':').collect::<Vec<_>>();
        if identity.len() == 3 && session_id.is_empty() {
            session_id = identity[2].into();
            if harness.is_empty() {
                harness = identity[1].into();
            }
        }
        result.push(EventRecord {
            id: row.get(0)?,
            agent_id,
            session_id,
            harness,
            subagent_id: row.get::<_, Option<String>>(2)?.filter(|s| !s.is_empty()),
            event_type: row.get(3)?,
            timestamp: row.get(4)?,
            data_json,
            data,
            pid,
        });
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug)]
pub enum DatabaseSearch {
    Sessions,
    Events,
}

pub fn resolve_path(
    custom: &str,
    mode: DatabaseSearch,
    home: &Path,
    cwd: &Path,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<PathBuf> {
    if !custom.is_empty() && custom != "auto" {
        let path = if let Some(relative) = custom.strip_prefix("~/") {
            home.join(relative)
        } else {
            PathBuf::from(custom)
        }
        .clean();
        fs_exists(&path)?;
        return Ok(path);
    }
    let value = |name| lookup(name).filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut candidates = Vec::new();
    match mode {
        DatabaseSearch::Sessions => {
            if let Some(explicit) = value("AGENT_DB_PATH") {
                fs_exists(&explicit)?;
                return Ok(explicit);
            }
            candidates.push(
                value("XDG_DATA_HOME")
                    .unwrap_or_else(|| home.join(".local/share"))
                    .join("agent-status/agent-status.db"),
            );
            candidates.push(cwd.join(".tmp/dev-stack/data/agent-status/agent-status.db"));
            candidates.push(home.join(".local/state/agent-status/agent-status.db"));
        }
        DatabaseSearch::Events => {
            candidates.extend([
                cwd.join(".tmp/dev-stack/data/agent-status/agent-status.db"),
                cwd.join(".tmp/dev-stack/data/agent-status.db"),
            ]);
            for name in ["AGENT_DATA_DIR", "XDG_DATA_HOME"] {
                if let Some(root) = value(name) {
                    let pair = [
                        root.join("agent-status.db"),
                        root.join("agent-status/agent-status.db"),
                    ];
                    if name == "AGENT_DATA_DIR" {
                        candidates.extend(pair);
                    } else {
                        candidates.extend(pair.into_iter().rev());
                    }
                }
            }
            candidates.push(home.join(".local/share/agent-status/agent-status.db"));
        }
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        Error::InvalidInput("agent-status SQLite database not found; specify --db <path>".into())
    })
}
fn fs_exists(path: &Path) -> Result<()> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::InvalidInput(format!(
            "database path is not a file: {}",
            path.display()
        )));
    }
    Ok(())
}
