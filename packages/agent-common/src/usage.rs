//! Extract only explicit transcript evidence, excluding conversational mentions.
use crate::types::{HarnessType, TelemetryFrame};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::LazyLock};

static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.:/-]*$").unwrap());
static SLASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(/[A-Za-z][A-Za-z0-9_.:-]*)(?:(?-u:\s)|$)").unwrap());
static CLAUDE_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<command-name>(?-u:\s)*(/?[A-Za-z][A-Za-z0-9_.:-]*)(?-u:\s)*</command-name>")
        .unwrap()
});
static PI_SKILL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^<skill name="([^"]+)" location="([^"]+)">\n"#).unwrap());
static CODEX_SKILL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^<skill>(?-u:\s)*<name>([^<]+)</name>(?-u:\s)*<path>([^<]+)</path>").unwrap()
});

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Evidence {
    pub source: String,
    pub event_type: String,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subagent_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tool_call_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub server: String,
    pub operation: String,
    pub evidence: Evidence,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub server: String,
    pub operation: String,
    pub count: i64,
    pub first_used_at: i64,
    pub last_used_at: i64,
    pub latest_evidence: Evidence,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub status: String,
    pub detail: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub agent_id: String,
    pub harness: HarnessType,
    pub skills: Vec<Entry>,
    pub commands: Vec<Entry>,
    pub mcp: Vec<Entry>,
    pub coverage: BTreeMap<String, Coverage>,
    pub history: String,
}
impl Report {
    pub fn new(agent_id: impl Into<String>, harness: HarnessType) -> Self {
        let descriptions = match &harness {
            HarnessType::ClaudeCode => Some((
                "Skill tool invocation attempts and direct SKILL.md read attempts; direct slash expansion may not identify the skill separately.",
                "Native command-name markers and recorded slash requests; commands omitted from transcripts are unknown.",
            )),
            HarnessType::Pi => Some((
                "Expanded skill blocks and direct SKILL.md read attempts.",
                "Expanded /skill:name commands and recorded slash requests; extension commands and prompt-template names may not be persisted.",
            )),
            HarnessType::Codex => Some((
                "Injected skill blocks and explicit read_file calls; skill reads through shell tools are excluded.",
                "Recorded slash requests only; client-side commands may not be persisted.",
            )),
            HarnessType::Opencode => Some((
                "Skill tool invocation attempts and direct SKILL.md read attempts.",
                "Recorded slash requests only; expanded commands may omit their original names.",
            )),
            HarnessType::Unknown(_) => None,
        };
        let coverage = ["skills", "commands", "mcp"].into_iter().map(|kind| {
            let detail = descriptions.map(|(skills, commands)| match kind {
                "skills" => skills, "commands" => commands,
                _ => "Explicit mcp__server__tool calls only; unqualified names and calls hidden inside other tools are unknown.",
            });
            (kind.into(), Coverage {
                status: if detail.is_some() { "partial" } else { "unavailable" }.into(),
                detail: detail.unwrap_or("No extractor is defined for this harness.").into(),
            })
        }).collect();
        Self { agent_id: agent_id.into(), harness, skills: Vec::new(), commands: Vec::new(), mcp: Vec::new(), coverage,
            history: "Includes retained telemetry backfilled at migration and subsequent observations. Older pruned history is unknown. Missing native call IDs use timestamp-based deduplication and may undercount identical observations at the same timestamp.".into(),
        }
    }
}

pub fn extract(frame: &TelemetryFrame) -> Vec<Observation> {
    if frame.timestamp <= 0 {
        return Vec::new();
    }
    let mut observations = Vec::new();
    let is_tool = matches!(frame.event_type.as_str(), "tool_call" | "tool_use");
    let mut add = |kind: &str, name: &str, server: &str, operation: &str, source: &str| {
        if name.is_empty() || name.chars().any(char::is_control) {
            return;
        }
        let evidence = Evidence {
            source: source.into(),
            event_type: frame.event_type.clone(),
            timestamp: frame.timestamp,
            subagent_id: frame.subagent_id.clone().unwrap_or_default(),
            tool_call_id: if is_tool {
                frame.data.tool_call_id.clone().unwrap_or_default()
            } else {
                String::new()
            },
        };
        let mut identity = vec![
            serde_json::json!(frame.agent_id),
            serde_json::json!(evidence.subagent_id),
            serde_json::json!(kind),
            serde_json::json!(name),
            serde_json::json!(server),
            serde_json::json!(operation),
        ];
        if !evidence.tool_call_id.is_empty() {
            identity.extend([
                serde_json::json!("call"),
                serde_json::json!(evidence.tool_call_id),
            ]);
        } else {
            identity.extend([
                serde_json::json!("timestamp"),
                serde_json::json!(frame.timestamp),
                serde_json::json!(source),
            ]);
        }
        // Persisted Go observation IDs use encoding/json's HTML-safe string encoding.
        let encoded = serde_json::to_string(&identity)
            .expect("JSON values are strings and integers")
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e")
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        let id = format!("{:x}", Sha256::digest(encoded.as_bytes()));
        observations.push(Observation {
            id,
            kind: kind.into(),
            name: name.into(),
            server: server.into(),
            operation: operation.into(),
            evidence,
        });
    };
    if is_tool {
        let tool = frame.data.tool_name.as_deref().unwrap_or("");
        let input = |key: &str| {
            frame
                .data
                .tool_input
                .as_ref()
                .and_then(|v| v.get(key))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        };
        if (frame.harness == HarnessType::ClaudeCode && tool == "Skill")
            || (frame.harness == HarnessType::Opencode && tool == "skill")
        {
            let name = input(if frame.harness == HarnessType::Opencode {
                "name"
            } else {
                "skill"
            });
            if IDENTIFIER.is_match(name) {
                add("skill", name, "", "invoke", "skill_tool");
            }
        }
        let key = match (&frame.harness, tool) {
            (HarnessType::ClaudeCode, "Read") => Some("file_path"),
            (HarnessType::Pi, "read") | (HarnessType::Codex, "read_file") => Some("path"),
            (HarnessType::Opencode, "read") => Some("filePath"),
            _ => None,
        };
        if let Some(key) = key {
            let file = input(key);
            // Transcript paths are slash-delimited independent of the host OS.
            if file.trim_end_matches('/').rsplit('/').next() == Some("SKILL.md") {
                add("skill", file, "", "load", "skill_file_read");
            }
        }
        if let Some((server, name)) = tool.strip_prefix("mcp__").and_then(|s| s.split_once("__"))
            && IDENTIFIER.is_match(server)
            && IDENTIFIER.is_match(name)
        {
            add("mcp", name, server, "invoke", "mcp_tool");
        }
        return observations;
    }
    if !matches!(frame.event_type.as_str(), "turn_start" | "user_prompt") {
        return Vec::new();
    }
    let Some(content) = frame.data.content.as_deref() else {
        return Vec::new();
    };
    let content = content.trim();
    if frame.harness == HarnessType::ClaudeCode
        && (content.starts_with("<command-") || content.starts_with("<local-command-caveat>"))
        && let Some(m) = CLAUDE_COMMAND.captures(content)
    {
        add(
            "command",
            &format!("/{}", m[1].trim_start_matches('/')),
            "",
            "invoke",
            "command_metadata",
        );
        return observations;
    }
    if frame.harness == HarnessType::Pi
        && let Some(m) = PI_SKILL.captures(content)
        && IDENTIFIER.is_match(&m[1])
        && content.contains("\n</skill>")
    {
        add("skill", &m[1], "", "load", "skill_expansion");
        add(
            "command",
            &format!("/skill:{}", &m[1]),
            "",
            "invoke",
            "skill_expansion",
        );
        return observations;
    }
    if frame.harness == HarnessType::Codex
        && let Some(m) = CODEX_SKILL.captures(content)
        && IDENTIFIER.is_match(&m[1])
        && content.contains("</skill>")
    {
        add("skill", &m[1], "", "load", "skill_injection");
        return observations;
    }
    if let Some(m) = SLASH.captures(content) {
        add("command", &m[1], "", "request", "slash_prompt");
    }
    observations
}
