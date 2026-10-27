//! Local harness discovery and monitored-process identity checks.
use crate::{Error, Result, audit::SessionPaths, check_cancel};
use agent_common::types::{
    AgentState, EnvironmentInfo, HarnessType, LifecycleState, RunInfo, WorkspaceInfo,
};
use agent_watcher::scanner::{
    ClaudeCodeScanner, CodexScanner, OpencodeScanner, PiScanner, ProcessSnapshot,
};
use tokio_util::sync::CancellationToken;

pub fn local(
    cancel: &CancellationToken,
    paths: &SessionPaths,
    snapshot: &ProcessSnapshot,
    hostname: &str,
) -> Result<Vec<AgentState>> {
    check_cancel(cancel)?;
    let groups = [
        (
            HarnessType::ClaudeCode,
            ClaudeCodeScanner::new(paths.claude.clone()).scan(cancel, snapshot)?,
        ),
        (
            HarnessType::Pi,
            PiScanner::new(paths.pi.clone()).scan(cancel, snapshot)?,
        ),
        (
            HarnessType::Codex,
            CodexScanner::new(paths.codex.clone()).scan(cancel, snapshot)?,
        ),
        (
            HarnessType::Opencode,
            OpencodeScanner::new(paths.opencode.clone()).scan(cancel, snapshot)?,
        ),
    ];
    let mut agents = Vec::new();
    for (harness, sessions) in groups {
        for s in sessions {
            check_cancel(cancel)?;
            if harness != HarnessType::ClaudeCode && !s.is_active
                || !snapshot.verify_process_match(s.pid, s.started_at)
            {
                continue;
            }
            let cwd = s
                .cwd
                .to_str()
                .ok_or_else(|| {
                    Error::InvalidInput("session working directory is not valid UTF-8".into())
                })?
                .to_owned();
            let title = match harness {
                HarnessType::ClaudeCode => s.name,
                HarnessType::Codex => s.agent_nickname,
                HarnessType::Opencode => s.title,
                _ => String::new(),
            };
            let mut agent = AgentState {
                session_id: s.session_id,
                harness: harness.clone(),
                environment: EnvironmentInfo {
                    hostname: hostname.into(),
                    ..Default::default()
                },
                workspace: WorkspaceInfo {
                    cwd,
                    ..Default::default()
                },
                ..Default::default()
            };
            agent.activity.title = title;
            agent.activity.current_run = Some(RunInfo {
                pid: i64::from(s.pid),
                started_at: s.started_at,
                ..Default::default()
            });
            agents.push(agent);
        }
    }
    Ok(agents)
}
/// Remote PIDs are checked by heartbeat freshness; local PIDs additionally require start-time identity.
pub fn monitored(
    agents: Vec<AgentState>,
    local_id: &str,
    snapshot: &ProcessSnapshot,
) -> Vec<AgentState> {
    agents
        .into_iter()
        .filter(|a| {
            if a.activity.lifecycle != LifecycleState::Running {
                return false;
            }
            let Some(run) = &a.activity.current_run else {
                return false;
            };
            if run.pid <= 0 || run.ended_at.is_some() {
                return false;
            }
            if a.environment.id == local_id {
                i32::try_from(run.pid)
                    .is_ok_and(|pid| snapshot.verify_process_match(pid, run.started_at))
            } else {
                !a.environment.is_stale
            }
        })
        .collect()
}
