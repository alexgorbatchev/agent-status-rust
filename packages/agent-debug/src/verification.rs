use agent_common::types::{ActivityState, AgentState, LifecycleState};
use agent_status_store::read;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckScope {
    General,
    TranscriptTermination,
}
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub details: String,
    #[serde(skip)]
    scope: CheckScope,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub agent_id: String,
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentState>,
    pub log_count: usize,
    pub checks: Vec<Check>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}
impl Report {
    /// Replay permits an open-ended transcript, while every other check must pass.
    pub fn passes_replay(&self) -> bool {
        self.passed
            || (!self.checks.is_empty()
                && self
                    .checks
                    .iter()
                    .all(|check| check.passed || check.scope == CheckScope::TranscriptTermination))
    }
    pub fn add(&mut self, name: &str, passed: bool, details: impl Into<String>) {
        self.add_scoped(CheckScope::General, name, passed, details);
    }
    fn add_scoped(
        &mut self,
        scope: CheckScope,
        name: &str,
        passed: bool,
        details: impl Into<String>,
    ) {
        let details = details.into();
        if !passed {
            self.passed = false;
            if self.last_error.is_none() {
                self.last_error = Some(format!("{name}: {details}"));
            }
        }
        self.checks.push(Check {
            name: name.into(),
            passed,
            details,
            scope,
        });
    }
}

/// Exercise the same native read queries as the server, including complete typed deserialization.
pub fn verify(
    db: &Connection,
    id: &str,
    strict_lifecycle: bool,
    explicit_exit: bool,
    now: i64,
) -> Report {
    let mut report = Report {
        agent_id: id.into(),
        passed: true,
        agent: None,
        log_count: 0,
        checks: Vec::new(),
        last_error: None,
    };
    let agent = match read::agent(db, id, now) {
        Ok(agent) => agent,
        Err(error) => {
            report.add(
                "GetAgentByID deserialization",
                false,
                format!("query error: {error}"),
            );
            return report;
        }
    };
    report.add(
        "GetAgentByID deserialization",
        true,
        format!("loaded agent {}", agent.id),
    );
    report.add(
        "Environment scan",
        !agent.environment.id.is_empty() && !agent.environment.hostname.is_empty(),
        format!(
            "env {} ({})",
            agent.environment.id, agent.environment.platform
        ),
    );
    report.add(
        "Project & Workspace scan",
        !agent.project.name.is_empty() && !agent.workspace.cwd.is_empty(),
        format!(
            "project {} on {}",
            agent.project.name, agent.workspace.branch
        ),
    );
    report.add(
        "Lifecycle invariant",
        matches!(
            agent.activity.lifecycle,
            LifecycleState::Running | LifecycleState::Exited
        ),
        agent.activity.lifecycle.as_str(),
    );
    if strict_lifecycle {
        report.add_scoped(
            CheckScope::TranscriptTermination,
            "Transcript lifecycle termination",
            explicit_exit,
            if explicit_exit {
                "terminated with explicit harness exit event"
            } else {
                "transcript is open-ended (no explicit harness exit event)"
            },
        );
    }
    report.add(
        "Activity invariant",
        matches!(
            agent.activity.activity,
            ActivityState::Idle
                | ActivityState::Busy
                | ActivityState::WaitingQuestion
                | ActivityState::Error
                | ActivityState::RateLimited
        ),
        agent.activity.activity.as_str(),
    );
    let usage = &agent.activity.usage;
    report.add(
        "Token usage invariants",
        [
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write,
            usage.reasoning,
            usage.total,
        ]
        .iter()
        .all(|v| *v >= 0),
        format!(
            "in:{} out:{} total:{}",
            usage.input, usage.output, usage.total
        ),
    );
    report.add(
        "Pricing calculation",
        agent
            .activity
            .cost
            .as_ref()
            .is_none_or(|c| c.total.is_finite() && c.total >= 0.0),
        agent.activity.cost.as_ref().map_or_else(
            || "cost is nil (unpriced model or zero usage)".into(),
            |c| format!("${:.4} {}", c.total, c.currency),
        ),
    );
    report.agent = Some(agent);
    match read::events(db, id, 1000, 0, None, now) {
        Err(error) => report.add(
            "GetAgentLogs deserialization",
            false,
            format!("query error: {error}"),
        ),
        Ok(logs) => {
            report.log_count = logs.len();
            report.add(
                "GetAgentLogs deserialization",
                !logs.is_empty(),
                format!("{} telemetry log records", logs.len()),
            );
            if !logs.is_empty() {
                report.add(
                    "GetAgentLogs framing",
                    logs.iter()
                        .all(|f| f.timestamp > 0 && !f.event_type.is_empty()),
                    format!("{} log frames deserialized", logs.len()),
                );
            }
        }
    }
    report
}
