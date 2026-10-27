use super::{Format, quote, timestamp};
use crate::{
    audit::{AuditRecord, PairingStatus},
    database::SessionRecord,
    diff::StateDiff,
};
use agent_common::usage::Report;
use cli_command_kit::Mode;
use std::io::{self, Write};

impl Format {
    pub fn audit(&self, out: &mut impl Write, records: &[AuditRecord]) -> io::Result<()> {
        if records.is_empty() {
            if self.mode == Mode::Agent {
                return self.line(out, "scanner_audit count:0");
            }
            self.divider(out, '-')?;
            self.line(out, "No processes or sessions found matching filter.")?;
            return self.divider(out, '-');
        }
        if self.mode == Mode::Agent {
            for r in records {
                self.line(out, &format!("scanner_audit harness:{} pid:{} created_at:{} cwd:{} session_id:{} delta_ms:{} status:{} status_details:{}", r.harness.as_str(), r.pid, r.created_at, quote(&self.text(&r.cwd.to_string_lossy())), quote(&self.text(&r.session_id)), r.delta_ms.map_or_else(|| "-".into(), |v| v.to_string()), r.status.as_str(), quote(&self.text(&r.status_details))))?;
            }
            return Ok(());
        }
        self.divider(out, '-')?;
        self.line(out, "Process Discovery & Session Pairing Audit")?;
        self.divider(out, '-')?;
        self.table(
            out,
            &[
                "Harness",
                "PID",
                "Created At",
                "CWD",
                "Session ID",
                "Delta",
                "Status",
            ],
            records.iter().map(|r| {
                vec![
                    r.harness.as_str().into(),
                    if r.pid > 0 {
                        r.pid.to_string()
                    } else {
                        "-".into()
                    },
                    if r.created_at > 0 {
                        timestamp(r.created_at, "%Y-%m-%d %H:%M:%S")
                    } else {
                        "-".into()
                    },
                    r.cwd.to_string_lossy().into_owned(),
                    if r.session_id.is_empty() {
                        "-".into()
                    } else {
                        r.session_id.clone()
                    },
                    r.delta_ms.map_or_else(
                        || "-".into(),
                        |v| {
                            if v > 0 {
                                format!("+{v}ms")
                            } else {
                                format!("{v}ms")
                            }
                        },
                    ),
                    if r.status == PairingStatus::Paired && !r.status_details.is_empty() {
                        format!("PAIRED ({})", r.status_details)
                    } else {
                        r.status.as_str().into()
                    },
                ]
            }),
        )
    }
    pub fn usage(&self, out: &mut impl Write, report: &Report) -> io::Result<()> {
        if self.mode == Mode::Human {
            self.line(
                out,
                &format!(
                    "Usage for {} ({})",
                    self.text(&report.agent_id),
                    self.text(report.harness.as_str())
                ),
            )?;
        }
        let mut rows = Vec::new();
        for (kind, entries) in [
            ("skill", &report.skills),
            ("command", &report.commands),
            ("mcp", &report.mcp),
        ] {
            for entry in entries {
                if self.mode == Mode::Agent {
                    self.line(out, &format!("kind:{kind} name:{} server:{} operation:{} count:{} first:{} last:{} source:{}", quote(&self.text(&entry.name)), quote(&self.text(&entry.server)), quote(&self.text(&entry.operation)), entry.count, entry.first_used_at, entry.last_used_at, quote(&self.text(&entry.latest_evidence.source))))?;
                } else {
                    rows.push(vec![
                        kind.into(),
                        entry.name.clone(),
                        entry.server.clone(),
                        entry.operation.clone(),
                        entry.count.to_string(),
                        timestamp(entry.first_used_at, "%Y-%m-%dT%H:%M:%SZ"),
                        timestamp(entry.last_used_at, "%Y-%m-%dT%H:%M:%SZ"),
                    ]);
                }
            }
        }
        if self.mode == Mode::Human {
            if rows.is_empty() {
                self.line(out, "No usage observations recorded.")?;
            } else {
                self.table(
                    out,
                    &[
                        "Kind",
                        "Name",
                        "Server",
                        "Operation",
                        "Count",
                        "First used (UTC)",
                        "Last used (UTC)",
                    ],
                    rows,
                )?;
            }
        }
        for kind in ["skills", "commands", "mcp"] {
            let coverage = report.coverage.get(kind).cloned().unwrap_or_default();
            let status = self.text(&coverage.status);
            let detail = self.text(&coverage.detail);
            if self.mode == Mode::Agent {
                self.line(
                    out,
                    &format!(
                        "coverage:{kind} status:{} detail:{}",
                        quote(&status),
                        quote(&detail)
                    ),
                )?;
            } else {
                self.line(out, &format!("{kind} coverage: {status} — {detail}"))?;
            }
        }
        self.line(
            out,
            &format!("history:{}", quote(&self.text(&report.history))),
        )
    }
    pub fn diff(&self, out: &mut impl Write, diff: &StateDiff) -> io::Result<()> {
        let mut label = if self.mode == Mode::Agent {
            format!(
                "diff step:{} event:{}",
                diff.step_index,
                self.text(&diff.event_type)
            )
        } else {
            format!("Step {} ({})", diff.step_index, self.text(&diff.event_type))
        };
        if diff.is_synthetic {
            label.push_str(if self.mode == Mode::Agent {
                " synthetic:true"
            } else {
                " (synthetic)"
            });
        }
        if diff.line_number > 0 {
            label.push_str(&if self.mode == Mode::Agent {
                format!(" line:{}", diff.line_number)
            } else {
                format!(" (line {})", diff.line_number)
            });
        }
        if let Some(id) = &diff.subagent_id {
            label.push_str(&if self.mode == Mode::Agent {
                format!(" subagent:{}", self.text(id))
            } else {
                format!(" [subagent {}]", self.text(id))
            });
        }
        if self.mode == Mode::Agent {
            let changes = diff
                .mutations
                .iter()
                .map(|m| {
                    format!(
                        "{}:{}->{}",
                        self.text(&m.field),
                        self.text(&m.from),
                        self.text(&m.to)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            label.push_str(&format!(" mutations:[{changes}]"));
            if diff.input_delta != 0 || diff.output_delta != 0 || diff.cost_delta != 0.0 {
                label.push_str(&format!(
                    " in:+{} out:+{} cost:+${:.4}",
                    diff.input_delta, diff.output_delta, diff.cost_delta
                ));
            }
            self.line(out, &label)
        } else {
            self.line(out, &label)?;
            for change in &diff.mutations {
                self.line(
                    out,
                    &format!(
                        "  {:<16} {} -> {}",
                        self.text(&change.field),
                        self.text(&change.from),
                        self.text(&change.to)
                    ),
                )?;
            }
            if diff.input_delta != 0 || diff.output_delta != 0 {
                self.line(
                    out,
                    &format!(
                        "  Tokens           in:+{} out:+{}",
                        diff.input_delta, diff.output_delta
                    ),
                )?;
            }
            if diff.cost_delta != 0.0 {
                self.line(out, &format!("  Cost             +${:.4}", diff.cost_delta))?;
            }
            self.line(out, "")
        }
    }
    pub fn inspect(&self, out: &mut impl Write, record: &SessionRecord) -> io::Result<()> {
        let agent = &record.agent;
        let a = &agent.activity;
        let cost = a.cost.as_ref().map_or(0.0, |c| c.total);
        if self.mode == Mode::Agent {
            self.line(out, &format!("db_agent id:{} session_id:{} harness:{} env:{} lifecycle:{} activity:{} title:{} action:{} tokens:{} cost_usd:{cost:.4} updated_at:{} runs:{} events:{}", self.text(&agent.id), self.text(&agent.session_id), self.text(agent.harness.as_str()), self.text(&agent.environment.label), a.lifecycle.as_str(), a.activity.as_str(), quote(&self.text(&a.title)), quote(&self.text(&a.action)), a.usage.total, a.updated_at, record.runs.len(), record.event_count))?;
            for run in &record.runs {
                self.line(
                    out,
                    &format!(
                        "db_run id:{} pid:{} harness_version:{} started_at:{} ended_at:{}",
                        self.text(&run.id),
                        run.pid,
                        self.text(&run.harness_version),
                        run.started_at,
                        run.ended_at
                            .map_or_else(|| "running".into(), |v| v.to_string())
                    ),
                )?;
            }
            return Ok(());
        }
        self.divider(out, '-')?;
        self.line(
            out,
            &format!("Persisted Agent Record ({})", self.text(&agent.session_id)),
        )?;
        self.divider(out, '-')?;
        let mut fields = vec![
            ("Agent ID", agent.id.clone()),
            ("Session ID", agent.session_id.clone()),
            ("Harness", agent.harness.as_str().into()),
            (
                "Environment",
                format!(
                    "{} ({})",
                    agent.environment.label, agent.environment.hostname
                ),
            ),
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
            ("Lifecycle", a.lifecycle.as_str().into()),
            ("Activity", a.activity.as_str().into()),
            (
                "Token Usage",
                format!(
                    "Input: {} | Output: {} | Total: {}",
                    a.usage.input, a.usage.output, a.usage.total
                ),
            ),
        ]);
        if a.cost.is_some() {
            fields.push(("Total Cost", format!("${cost:.4} USD")));
        }
        if let Some(run) = &a.current_run {
            fields.push(("Current Run", format!("{} (PID: {})", run.id, run.pid)));
        }
        fields.push(("Historical Runs", record.runs.len().to_string()));
        for (name, value) in fields {
            self.line(out, &format!("  {name:<16} {}", self.text(&value)))?;
        }
        for run in &record.runs {
            self.line(
                out,
                &format!(
                    "    - Run {}: PID {}, started {}, ended {}",
                    self.text(&run.id),
                    run.pid,
                    timestamp(run.started_at, "%Y-%m-%d %H:%M:%S UTC"),
                    run.ended_at.map_or_else(
                        || "RUNNING".into(),
                        |v| timestamp(v, "%Y-%m-%d %H:%M:%S UTC")
                    )
                ),
            )?;
        }
        self.line(
            out,
            &format!("  Telemetry Events: {} recorded", record.event_count),
        )?;
        if let Some((first, last)) = record
            .first_event_timestamp
            .zip(record.last_event_timestamp)
        {
            self.line(
                out,
                &format!(
                    "  Event Window:    {} to {}",
                    timestamp(first, "%Y-%m-%d %H:%M:%S UTC"),
                    timestamp(last, "%Y-%m-%d %H:%M:%S UTC")
                ),
            )?;
        }
        self.line(
            out,
            &format!(
                "  Last Updated:    {}",
                timestamp(a.updated_at, "%Y-%m-%d %H:%M:%S UTC")
            ),
        )?;
        self.divider(out, '-')
    }
}
