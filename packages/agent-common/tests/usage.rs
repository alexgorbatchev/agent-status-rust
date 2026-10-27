use agent_common::{types::*, usage};
use serde_json::{Value, json};

fn extract(
    harness: &str,
    event: &str,
    tool: &str,
    content: Option<&str>,
    input: Value,
) -> Vec<usage::Observation> {
    let frame=serde_json::from_value(json!({"agentId":"agent","harness":harness,"eventType":event,"timestamp":1000,"data":{"toolName":tool,"content":content,"toolCallId":"call-1","toolInput":input}})).unwrap();
    usage::extract(&frame)
}
#[test]
fn explicit_metadata_is_extracted_across_all_supported_harnesses() {
    for (harness, tool, key, name) in [
        ("claude-code", "Skill", "skill", "plugin:review"),
        ("opencode", "skill", "name", "review"),
    ] {
        let observations = extract(
            harness,
            "tool_call",
            tool,
            None,
            json!({key:name,"args":"private arguments"}),
        );
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].name, name);
        assert_eq!(observations[0].evidence.source, "skill_tool");
    }
    for (harness, tool, key) in [
        ("claude-code", "Read", "file_path"),
        ("pi", "read", "path"),
        ("opencode", "read", "filePath"),
        ("codex", "read_file", "path"),
    ] {
        let observations = extract(
            harness,
            "tool_use",
            tool,
            None,
            json!({key:"skills/review/SKILL.md/"}),
        );
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].operation, "load");
        assert_eq!(observations[0].evidence.event_type, "tool_use");
    }
    for (harness, event, content, names, source) in [
        (
            "pi",
            "turn_start",
            "<skill name=\"review\" location=\"/skills/review/SKILL.md\">\nInstructions\n</skill>\n\nprivate arguments",
            vec!["review", "/skill:review"],
            "skill_expansion",
        ),
        (
            "codex",
            "turn_start",
            "<skill>\n<name>review</name>\n<path>/skills/review/SKILL.md</path>\nInstructions\n</skill>",
            vec!["review"],
            "skill_injection",
        ),
        (
            "claude-code",
            "turn_start",
            "<command-name>/review</command-name>\n<command-message>review</command-message>\n<command-args>private arguments</command-args>",
            vec!["/review"],
            "command_metadata",
        ),
        (
            "claude-code",
            "turn_start",
            "<local-command-caveat>local</local-command-caveat><command-name>review</command-name>",
            vec!["/review"],
            "command_metadata",
        ),
        (
            "pi",
            "turn_start",
            "/review private arguments",
            vec!["/review"],
            "slash_prompt",
        ),
        (
            "codex",
            "turn_start",
            "/review",
            vec!["/review"],
            "slash_prompt",
        ),
        (
            "opencode",
            "user_prompt",
            "/review",
            vec!["/review"],
            "slash_prompt",
        ),
    ] {
        let observations = extract(harness, event, "", Some(content), Value::Null);
        assert_eq!(
            observations
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            names
        );
        assert!(
            observations
                .iter()
                .all(|o| o.evidence.source == source && o.evidence.tool_call_id.is_empty())
        );
    }
    for harness in ["pi", "codex", "claude-code", "opencode"] {
        let observed = extract(
            harness,
            "tool_call",
            "mcp__plugin_git_github__list_issues",
            None,
            Value::Null,
        );
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].server, "plugin_git_github");
        assert_eq!(observed[0].name, "list_issues");
    }
}

#[test]
fn inferred_mentions_results_shell_commands_and_malformed_metadata_are_excluded() {
    for (harness, event, tool, content, input) in [
        (
            "codex",
            "tool_call",
            "exec_command",
            "",
            json!({"cmd":"cat /skills/review/SKILL.md"}),
        ),
        (
            "claude-code",
            "tool_result",
            "Skill",
            "",
            json!({"skill":"review"}),
        ),
        (
            "pi",
            "turn_start",
            "",
            "<available_skills>\n<skill name=\"review\" location=\"/skills/review/SKILL.md\">\nInstructions\n</skill>\n</available_skills>",
            Value::Null,
        ),
        (
            "pi",
            "turn_start",
            "",
            "Please use /review and $review",
            Value::Null,
        ),
        ("pi", "turn_start", "", "/etc/passwd", Value::Null),
        ("codex", "tool_call", "mcp__github__", "", Value::Null),
        ("codex", "tool_call", "mcp____tool", "", Value::Null),
        (
            "opencode",
            "tool_use",
            "github_list_issues",
            "",
            Value::Null,
        ),
        ("claude-code", "tool_call", "Skill", "", json!({"skill":7})),
        (
            "claude-code",
            "tool_call",
            "Skill",
            "",
            json!({"skill":"bad name"}),
        ),
        ("pi", "tool_call", "Skill", "", json!({"skill":"review"})),
        ("pi", "tool_call", "read", "", json!({"path":"README.md"})),
        (
            "pi",
            "tool_call",
            "read",
            "",
            json!({"path":"skills/\nreview/SKILL.md"}),
        ),
        ("pi", "tool_call", "", "", Value::Null),
        (
            "claude-code",
            "turn_start",
            "",
            "Example: <command-name>/review</command-name>",
            Value::Null,
        ),
        ("codex", "agent_message", "", "/review", Value::Null),
        (
            "codex",
            "turn_start",
            "",
            "<skill><name>review</name><path>/x</path>",
            Value::Null,
        ),
        (
            "pi",
            "turn_start",
            "",
            "<skill name=\"bad name\" location=\"/x\">\n</skill>",
            Value::Null,
        ),
        (
            "pi",
            "turn_start",
            "",
            "<skill name=\"review\" location=\"/x\">\nInstructions",
            Value::Null,
        ),
        (
            "pi",
            "turn_start",
            "",
            "/review\u{a0}arguments",
            Value::Null,
        ),
    ] {
        assert!(
            extract(harness, event, tool, Some(content), input).is_empty(),
            "{harness} {event} {tool} {content}"
        );
    }
    assert!(extract("pi", "turn_start", "", None, Value::Null).is_empty());
}

#[test]
fn reports_distinguish_missing_coverage_and_preserve_wire_data() {
    for harness in [
        HarnessType::ClaudeCode,
        HarnessType::Pi,
        HarnessType::Codex,
        HarnessType::Opencode,
        HarnessType::Unknown("future".into()),
    ] {
        let mut report = usage::Report::new("a", harness.clone());
        assert!(report.coverage.values().all(|c| c.status
            == if matches!(harness, HarnessType::Unknown(_)) {
                "unavailable"
            } else {
                "partial"
            }));
        let wire = serde_json::to_value(&report).unwrap();
        assert_eq!(wire["skills"], json!([]));
        assert_eq!(wire["commands"], json!([]));
        assert_eq!(wire["mcp"], json!([]));
        report.skills.push(usage::Entry {
            name: "review".into(),
            count: 2,
            first_used_at: 100,
            last_used_at: 200,
            latest_evidence: usage::Evidence {
                source: "skill_tool".into(),
                timestamp: 200,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(
            serde_json::from_value::<usage::Report>(serde_json::to_value(&report).unwrap())
                .unwrap(),
            report
        );
    }
}
