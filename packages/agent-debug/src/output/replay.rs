use super::{Format, quote, truncate, truncate_left};
use crate::replay::ReplayResult;
use agent_common::types::LifecycleState;
use cli_command_kit::Mode;
use serde::Serialize;
use std::io::{self, Write};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    pub tokens: i64,
}
impl Format {
    pub fn session_list(
        &self,
        out: &mut impl Write,
        sessions: &[SessionSummary],
    ) -> io::Result<()> {
        if self.mode == Mode::Agent {
            for s in sessions {
                self.line(
                    out,
                    &format!(
                        "session_id:{} title:{} cwd:{} created_at:{} cost_usd:{:.4} tokens:{}",
                        self.text(&s.id),
                        quote(&self.text(if s.title.is_empty() {
                            "(untitled)"
                        } else {
                            &s.title
                        })),
                        self.text(&s.directory),
                        s.created_at.timestamp_millis(),
                        s.cost_usd,
                        s.tokens
                    ),
                )?;
            }
            Ok(())
        } else if sessions.is_empty() {
            self.line(out, "No sessions found.")
        } else {
            self.table(
                out,
                &[
                    "Session ID",
                    "Title",
                    "Directory",
                    "Created",
                    "Cost ($)",
                    "Tokens",
                ],
                sessions.iter().map(|s| {
                    vec![
                        s.id.clone(),
                        truncate(
                            &self.text(if s.title.is_empty() {
                                "(untitled)"
                            } else {
                                &s.title
                            }),
                            35,
                            "...",
                        ),
                        truncate_left(&self.text(&s.directory), 35, "..."),
                        s.created_at
                            .with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string(),
                        format!("${:.4}", s.cost_usd),
                        s.tokens.to_string(),
                    ]
                }),
            )
        }
    }
    pub fn replay_summary(&self, out: &mut impl Write, result: &ReplayResult) -> io::Result<()> {
        let empty = agent_common::types::AgentState::default();
        let agent = result.agent.as_ref().unwrap_or(&empty);
        let a = &agent.activity;
        let cost = a.cost.as_ref().map_or(0.0, |c| c.total);
        let suffix = if a.lifecycle == LifecycleState::Exited {
            if result.has_synthetic_end {
                "synthetic"
            } else {
                "native"
            }
        } else {
            ""
        };
        let status = if result.verification.passed {
            "OK"
        } else {
            "FAIL"
        };
        if self.mode == Mode::Agent {
            let lifecycle = if suffix.is_empty() {
                a.lifecycle.as_str().into()
            } else {
                format!("{}({suffix})", a.lifecycle.as_str())
            };
            return self.line(out, &format!("harness:{} session_id:{} agent_id:{} project:{} branch:{} activity:{} lifecycle:{lifecycle} tokens_in:{} tokens_out:{} tokens_total:{} cost_usd:{cost:.4} subagents:{} frames:{} verification:{status} ver_logs:{} duration_ms:{}{}", result.harness.as_str(), self.text(&result.session_id), self.text(&result.agent_id), self.text(&agent.project.name), self.text(&agent.workspace.branch), a.activity.as_str(), a.usage.input, a.usage.output, a.usage.total, a.subagent_count, result.frames_processed, result.verification.log_count, result.duration / 1_000_000, if super::completed(agent) {" has_unseen_completion:true"} else {""}));
        }
        self.divider(out, '-')?;
        self.line(
            out,
            &format!("Session Replay Summary ({})", result.harness.as_str()),
        )?;
        self.divider(out, '-')?;
        let mut fields = vec![
            ("Session ID", result.session_id.clone()),
            ("Agent ID", result.agent_id.clone()),
            (
                "Project",
                format!(
                    "{} (branch: {})",
                    agent.project.name, agent.workspace.branch
                ),
            ),
            ("Directory", agent.workspace.cwd.clone()),
        ];
        if !a.title.is_empty() {
            fields.push(("Title", a.title.clone()));
        }
        if !a.action.is_empty() {
            fields.push(("Action", a.action.clone()));
        }
        fields.extend([
            (
                "Lifecycle",
                if suffix.is_empty() {
                    a.lifecycle.as_str().into()
                } else {
                    format!("{} ({suffix})", a.lifecycle.as_str())
                },
            ),
            (
                "Activity",
                if super::completed(agent) {
                    "Completed".into()
                } else {
                    a.activity.as_str().into()
                },
            ),
            (
                "Token Usage",
                format!(
                    "Input: {} | Output: {} | Total: {}",
                    a.usage.input, a.usage.output, a.usage.total
                ),
            ),
            ("Total Cost", format!("${cost:.4} USD")),
        ]);
        if a.subagent_count > 0 {
            fields.push((
                "Subagents",
                format!("{} active/completed", a.subagent_count),
            ));
        }
        fields.push((
            "Frames",
            format!(
                "{} frames replayed in {} ms",
                result.frames_processed,
                result.duration / 1_000_000
            ),
        ));
        for (name, value) in fields {
            self.line(out, &format!("  {name:<16} {}", self.text(&value)))?;
        }
        self.divider(out, '-')?;
        if result.verification.passed {
            self.line(
                out,
                &format!(
                    "[OK] Read-side query verification passed (GetAgentByID, {} logs scanned)",
                    result.verification.log_count
                ),
            )?;
        } else {
            self.line(out, "[ERROR] Read-side query verification failed!")?;
        }
        self.divider(out, '-')
    }
}
