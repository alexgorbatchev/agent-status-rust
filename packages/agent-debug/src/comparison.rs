//! Compare replayed state with a read-only persisted session snapshot.
use crate::database::SessionRecord;
use agent_common::types::{AgentState, LifecycleState};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    pub replayed_value: String,
    pub persisted_value: String,
    pub status: String,
    pub is_drift: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub db_path: PathBuf,
    pub session_id: String,
    pub agent_id: String,
    pub fields: Vec<Field>,
    pub warnings: Vec<String>,
    pub has_drift: bool,
}
fn number(value: i128) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut result = if value < 0 { "-".into() } else { String::new() };
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}
fn duration(ms: i128) -> String {
    let seconds = ms.unsigned_abs() / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    match (hours, minutes % 60) {
        (0, _) if seconds < 60 => format!("{seconds}s"),
        (0, _) => format!("{minutes}m"),
        (_, 0) => format!("{hours}h"),
        (_, remainder) => format!("{hours}h {remainder}m"),
    }
}
fn timestamp(ms: i64) -> String {
    if ms == 0 {
        "-".into()
    } else {
        crate::output::timestamp(ms, "%Y-%m-%d %H:%M:%S UTC")
    }
}
fn field(
    name: &str,
    replayed: String,
    persisted: String,
    drift: bool,
    detail: Option<String>,
) -> Field {
    Field {
        name: name.into(),
        replayed_value: replayed,
        persisted_value: persisted,
        status: if drift {
            detail.map_or_else(|| "DRIFT".into(), |d| format!("DRIFT ({d})"))
        } else {
            "MATCH".into()
        },
        is_drift: drift,
    }
}
/// Differences use wide integer arithmetic so valid SQLite timestamps and usage never overflow.
pub fn compare(
    replayed: &AgentState,
    last_event_timestamp: i64,
    record: &SessionRecord,
) -> Comparison {
    let a = &replayed.activity;
    let b = &record.agent.activity;
    let tokens = i128::from(b.usage.total) - i128::from(a.usage.total);
    let rep_cost = a.cost.as_ref().map_or(0.0, |c| c.total);
    let db_cost = b.cost.as_ref().map_or(0.0, |c| c.total);
    let cost = db_cost - rep_cost;
    let rep_event = if last_event_timestamp == 0 {
        a.updated_at
    } else {
        last_event_timestamp
    };
    let db_event = record.last_event_timestamp.unwrap_or(b.updated_at);
    let updated = i128::from(b.updated_at) - i128::from(a.updated_at);
    let rep_run = a
        .current_run
        .as_ref()
        .filter(|run| run.ended_at.is_none())
        .map_or("NONE", |r| r.id.as_str());
    let db_run = b
        .current_run
        .as_ref()
        .filter(|run| run.ended_at.is_none())
        .map_or("NONE", |r| r.id.as_str());
    let fields = vec![
        field(
            "Lifecycle",
            a.lifecycle.as_str().into(),
            b.lifecycle.as_str().into(),
            a.lifecycle != b.lifecycle,
            None,
        ),
        field(
            "Activity",
            a.activity.as_str().into(),
            b.activity.as_str().into(),
            a.activity != b.activity,
            None,
        ),
        field(
            "Tokens Total",
            number(i128::from(a.usage.total)),
            number(i128::from(b.usage.total)),
            tokens != 0,
            Some(format!(
                "{}{value}",
                if tokens < 0 { "-" } else { "+" },
                value = number(tokens.abs())
            )),
        ),
        field(
            "Cost USD",
            format!("${rep_cost:.4}"),
            format!("${db_cost:.4}"),
            !cost.is_finite() || cost.abs() >= 0.0001,
            Some(format!(
                "{}{value:.4}",
                if cost < 0.0 { "-$" } else { "+$" },
                value = cost.abs()
            )),
        ),
        field(
            "Last Event Time",
            timestamp(rep_event),
            timestamp(db_event),
            (i128::from(db_event) - i128::from(rep_event)).abs() > 1000,
            None,
        ),
        field(
            "Updated At",
            timestamp(a.updated_at),
            timestamp(b.updated_at),
            updated.abs() > 1000,
            Some(format!(
                "{}{}",
                if updated < 0 { "-" } else { "+" },
                duration(updated)
            )),
        ),
        field(
            "Current Run ID",
            rep_run.into(),
            db_run.into(),
            rep_run != db_run,
            None,
        ),
    ];
    let mut warnings = Vec::new();
    let newer = i128::from(b.updated_at) - i128::from(rep_event);
    if newer > 60_000 {
        warnings.push(format!(
            "Database updated_at is {} newer than transcript last event time!",
            duration(newer)
        ));
    }
    if b.lifecycle == LifecycleState::Running && a.lifecycle == LifecycleState::Exited {
        warnings.push(
            "Database agent is still marked RUNNING while transcript replay finished!".into(),
        );
    }
    if let Some(run) = &b.current_run
        && run.ended_at.is_none()
        && a.current_run
            .as_ref()
            .is_none_or(|run| run.ended_at.is_some())
    {
        warnings.push(format!(
            "Database has active run {} while replayed transcript has no active run",
            run.id
        ));
    }
    Comparison {
        db_path: record.db_path.clone(),
        session_id: replayed.session_id.clone(),
        agent_id: replayed.id.clone(),
        has_drift: fields.iter().any(|f| f.is_drift),
        fields,
        warnings,
    }
}
