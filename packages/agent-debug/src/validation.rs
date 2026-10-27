//! Whole-pipeline validation of replayed telemetry.
use crate::replay::ReplayResult;
use agent_common::types::{HarnessType, LifecycleState};
use serde::Serialize;
use std::path::PathBuf;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub details: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub target: PathBuf,
    pub harness: HarnessType,
    pub passed: bool,
    pub checks: Vec<Check>,
}
impl Report {
    pub fn replay(target: PathBuf, result: &ReplayResult) -> Self {
        let a = result.agent.as_ref();
        let lifecycle = a.map_or_else(
            || "nil agent state".into(),
            |a| {
                format!(
                    "Lifecycle={}{}",
                    a.activity.lifecycle.as_str(),
                    if result.has_synthetic_end {
                        " (synthetic)"
                    } else if result.has_native_end {
                        " (native)"
                    } else {
                        " (open-ended transcript, no explicit harness exit event)"
                    }
                )
            },
        );
        let cost = a
            .and_then(|a| a.activity.cost.as_ref())
            .map_or(0.0, |c| c.total);
        let usage_valid = a.is_some_and(|a| {
            let u = &a.activity.usage;
            [
                u.input,
                u.output,
                u.cache_read,
                u.cache_write,
                u.reasoning,
                u.total,
            ]
            .iter()
            .all(|n| *n >= 0)
        }) && cost.is_finite()
            && cost >= 0.0;
        let mut report = Self {
            target,
            harness: result.harness.clone(),
            passed: true,
            checks: Vec::new(),
        };
        for (name, passed, details) in [
            ("Harness Detection", true, result.harness.as_str().into()),
            (
                "Telemetry Event Framing",
                result.frames_processed > 0 && result.errors.is_empty(),
                format!(
                    "{} frames processed ({} errors)",
                    result.frames_processed,
                    result.errors.len()
                ),
            ),
            (
                "Lifecycle State Invariant",
                a.is_some_and(|a| a.activity.lifecycle == LifecycleState::Exited)
                    && (result.has_synthetic_end || result.has_native_end),
                lifecycle,
            ),
            (
                "Usage & Pricing Invariants",
                usage_valid,
                format!(
                    "{} total tokens, ${cost:.4} USD",
                    a.map_or(0, |a| a.activity.usage.total)
                ),
            ),
            (
                "Server Read Verification",
                result.verification.passed,
                result
                    .verification
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "GetAgentByID & GetAgentLogs scanned cleanly".into()),
            ),
        ] {
            report.push(name, passed, details);
        }
        report
    }
    pub fn failed(target: PathBuf, error: &crate::Error) -> Self {
        Self {
            target,
            harness: HarnessType::default(),
            passed: false,
            checks: vec![Check {
                name: "Replay Pipeline Execution".into(),
                passed: false,
                details: error.to_string(),
            }],
        }
    }
    pub fn push(&mut self, name: &str, passed: bool, details: String) {
        self.passed &= passed;
        self.checks.push(Check {
            name: name.into(),
            passed,
            details,
        });
    }
}
