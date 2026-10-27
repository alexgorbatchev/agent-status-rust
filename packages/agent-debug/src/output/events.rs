use super::{Format, quote, sanitize, timestamp, truncate};
use crate::database::EventRecord;
use cli_command_kit::Mode;
use serde::Serialize;
use std::io::{self, Write};

fn present(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}
pub fn intermediate_tool_turn(record: &EventRecord) -> bool {
    if record.event_type != "turn_end" {
        return false;
    }
    let raw: serde_json::Value = serde_json::from_str(&record.data_json).unwrap_or_default();
    let reason = record
        .data
        .stop_reason
        .as_deref()
        .or_else(|| raw["stopReason"].as_str())
        .or_else(|| raw["stop_reason"].as_str());
    matches!(
        reason,
        Some("tool_use" | "toolUse" | "tool_calls" | "tool_call")
    )
}
pub fn event_label(record: &EventRecord) -> &str {
    if intermediate_tool_turn(record) {
        "API RESPONSE"
    } else if record.event_type != "tool_result"
        && (record.event_type == "turn_start" || record.data.role.as_deref() == Some("user"))
    {
        "USER PROMPT"
    } else {
        &record.event_type
    }
}
pub fn event_summary(record: &EventRecord) -> String {
    let d = &record.data;
    let text = |value: &str, width| {
        quote(&truncate(
            &sanitize(value).trim().replace('\n', " "),
            width,
            "...",
        ))
    };
    match record.event_type.as_str() {
        "run_started" => {
            let mut parts = Vec::new();
            if record.pid > 0 {
                parts.push(format!("pid={}", record.pid));
            }
            if !record.harness.is_empty() {
                parts.push(format!("harness={}", sanitize(&record.harness)));
            }
            if let Some(value) = present(&d.title) {
                parts.push(format!("title={}", quote(&sanitize(value))));
            }
            if let Some(value) = present(&d.model) {
                parts.push(format!("model={}", sanitize(value)));
            }
            if parts.is_empty() {
                "run started".into()
            } else {
                parts.join(" ")
            }
        }
        "run_exited" => {
            if record.pid > 0 {
                format!("pid={}", record.pid)
            } else {
                "run exited".into()
            }
        }
        "turn_end" => {
            let prefix = if intermediate_tool_turn(record) {
                "(api_response) "
            } else {
                ""
            };
            if let Some(usage) = &d.usage {
                format!(
                    "{prefix}tokens={}",
                    if usage.total != 0 {
                        i128::from(usage.total)
                    } else {
                        i128::from(usage.input) + i128::from(usage.output)
                    }
                )
            } else if prefix.is_empty() {
                "turn ended".into()
            } else {
                prefix.trim().into()
            }
        }
        "tool_result" => {
            let mut parts = Vec::new();
            if let Some(tool) = present(&d.tool_name) {
                parts.push(format!("tool={}", sanitize(tool)));
            }
            parts.push(format!(
                "status={}",
                if d.is_error == Some(true) || d.exit_code.is_some_and(|c| c != 0) {
                    "error"
                } else {
                    "success"
                }
            ));
            if let Some(code) = d.exit_code.filter(|c| *c != 0) {
                parts.push(format!("exit={code}"));
            }
            parts.join(" ")
        }
        "tool_call" => {
            let mut parts = Vec::new();
            if let Some(tool) = present(&d.tool_name) {
                parts.push(format!("tool={}", sanitize(tool)));
            }
            if let Some(file) = d
                .file_modification
                .as_ref()
                .filter(|f| !f.file_path.is_empty())
            {
                parts.push(format!("file={}", sanitize(&file.file_path)));
            } else if let Some(input) = &d.tool_input {
                for (key, label) in [
                    ("command", "cmd"),
                    ("path", "path"),
                    ("file_path", "file"),
                    ("pattern", "pattern"),
                ] {
                    if let Some(value) = input
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        let value = match key {
                            "command" => text(value, 30),
                            "pattern" => quote(&sanitize(value)),
                            _ => sanitize(value),
                        };
                        parts.push(format!("{label}={value}"));
                        break;
                    }
                }
            }
            if parts.is_empty() {
                "tool call".into()
            } else {
                parts.join(" ")
            }
        }
        "thinking" => d.content.as_ref().map_or_else(
            || "thinking".into(),
            |s| format!("len={} chars", s.chars().count()),
        ),
        "user_message" => present(&d.last_prompt)
            .or_else(|| present(&d.content))
            .map_or_else(|| "user message".into(), |s| text(s, 40)),
        "assistant_message" => {
            present(&d.content).map_or_else(|| "assistant message".into(), |s| text(s, 40))
        }
        "error" => present(&d.content).map_or_else(|| "error".into(), sanitize),
        "waiting_question" | "question_prompt" => present(&d.question).map_or_else(
            || "waiting question".into(),
            |s| format!("question={}", quote(&sanitize(s))),
        ),
        _ => sanitize(&record.event_type),
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventItem<'a> {
    id: i64,
    agent_id: &'a str,
    session_id: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    harness: &'a str,
    subagent_id: &'a Option<String>,
    event_type: &'a str,
    timestamp: i64,
    #[serde(rename = "timestampUTC")]
    timestamp_utc: String,
    summary: String,
    data: serde_json::Value,
}
impl EventRecord {
    fn output_item(&self) -> Result<EventItem<'_>, serde_json::Error> {
        Ok(EventItem {
            id: self.id,
            agent_id: &self.agent_id,
            session_id: &self.session_id,
            harness: &self.harness,
            subagent_id: &self.subagent_id,
            event_type: &self.event_type,
            timestamp: self.timestamp,
            timestamp_utc: timestamp(self.timestamp, "%Y-%m-%dT%H:%M:%S%.3fZ"),
            summary: event_summary(self),
            data: if self.data_json.is_empty() {
                serde_json::to_value(&self.data)?
            } else {
                serde_json::from_str(&self.data_json)?
            },
        })
    }
}
impl Format {
    pub fn events(&self, out: &mut impl Write, events: &[EventRecord]) -> io::Result<()> {
        if self.mode == Mode::Agent {
            for record in events {
                self.line(
                    out,
                    &format!(
                        "id:{} ts:{} type:{} subagent:{} summary:{}",
                        record.id,
                        record.timestamp,
                        self.text(&record.event_type),
                        self.text(record.subagent_id.as_deref().unwrap_or("-")),
                        quote(&self.text(&event_summary(record)))
                    ),
                )?;
            }
            Ok(())
        } else if events.is_empty() {
            self.line(out, "No telemetry events found.")
        } else {
            self.table(
                out,
                &["ID", "Timestamp (UTC)", "Event Type", "Subagent", "Summary"],
                events.iter().map(|record| {
                    vec![
                        record.id.to_string(),
                        timestamp(record.timestamp, "%Y-%m-%d %H:%M:%S%.3f"),
                        if intermediate_tool_turn(record) {
                            "turn_end (API RESPONSE)".into()
                        } else {
                            record.event_type.clone()
                        },
                        record.subagent_id.clone().unwrap_or_else(|| "-".into()),
                        event_summary(record),
                    ]
                }),
            )
        }
    }
    pub fn event_detail(&self, out: &mut impl Write, record: &EventRecord) -> crate::Result<()> {
        let data = record.output_item()?.data;
        if self.mode == Mode::Agent {
            self.line(
                out,
                &format!(
                    "id:{} session_id:{} agent_id:{} type:{} ts:{} subagent:{} summary:{} data:{}",
                    record.id,
                    self.text(&record.session_id),
                    self.text(&record.agent_id),
                    self.text(&record.event_type),
                    record.timestamp,
                    self.text(record.subagent_id.as_deref().unwrap_or("-")),
                    quote(&self.text(&event_summary(record))),
                    serde_json::to_string(&data)?
                ),
            )?;
        } else {
            self.divider(out, '-')?;
            let mut fields = vec![
                ("Event ID", record.id.to_string()),
                ("Session ID", record.session_id.clone()),
                ("Agent ID", record.agent_id.clone()),
            ];
            if !record.harness.is_empty() {
                fields.push(("Harness", record.harness.clone()));
            }
            if record.pid > 0 {
                fields.push(("PID", record.pid.to_string()));
            }
            fields.extend([
                (
                    "Event Type",
                    if intermediate_tool_turn(record) {
                        "turn_end (API RESPONSE)".into()
                    } else {
                        record.event_type.clone()
                    },
                ),
                (
                    "Timestamp",
                    format!(
                        "{} UTC ({})",
                        timestamp(record.timestamp, "%Y-%m-%d %H:%M:%S%.3f"),
                        record.timestamp
                    ),
                ),
                (
                    "Subagent",
                    record.subagent_id.clone().unwrap_or_else(|| "-".into()),
                ),
                ("Summary", event_summary(record)),
            ]);
            for (label, value) in fields {
                self.line(out, &format!("{label:<16} {}", self.text(&value)))?;
            }
            self.divider(out, '-')?;
            self.line(out, "Payload Data:")?;
            self.line(out, &self.text(&serde_json::to_string_pretty(&data)?))?;
            self.divider(out, '-')?;
        }
        Ok(())
    }
    pub fn events_json(
        &self,
        out: &mut impl Write,
        events: &[EventRecord],
        single: bool,
    ) -> crate::Result<()> {
        if single {
            let record = events
                .first()
                .ok_or_else(|| crate::Error::InvalidInput("telemetry event not found".into()))?;
            self.json(out, &record.output_item()?)
        } else {
            self.json(
                out,
                &events
                    .iter()
                    .map(EventRecord::output_item)
                    .collect::<Result<Vec<_>, _>>()?,
            )
        }
    }
}
