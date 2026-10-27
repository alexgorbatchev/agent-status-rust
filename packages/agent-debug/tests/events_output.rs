use agent_common::types::{EventData, FileModification, TokenUsage};
use agent_debug::{
    database::EventRecord,
    output::{Format, event_label, event_summary, intermediate_tool_turn},
};
use cli_command_kit::Mode;
use serde_json::json;

fn event(kind: &str, data: serde_json::Value) -> EventRecord {
    EventRecord {
        id: 9,
        agent_id: "env:pi:s".into(),
        session_id: "s".into(),
        harness: "pi".into(),
        subagent_id: Some("worker".into()),
        event_type: kind.into(),
        timestamp: 1000,
        data_json: data.to_string(),
        data: serde_json::from_value(data).unwrap(),
        pid: 42,
    }
}
fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 100,
        color: false,
        home: Some("/fixture".into()),
    }
}

#[test]
fn event_summaries_preserve_native_tool_and_turn_semantics() {
    for (kind, data, expected) in [
        (
            "run_started",
            json!({"title":"Repair","model":"model"}),
            "pid=42 harness=pi title=\"Repair\" model=model",
        ),
        ("run_exited", json!({}), "pid=42"),
        (
            "turn_end",
            json!({"usage":{"input":2,"output":3}}),
            "tokens=5",
        ),
        (
            "turn_end",
            json!({"stopReason":"tool_use","usage":{"total":7}}),
            "(api_response) tokens=7",
        ),
        (
            "turn_end",
            json!({"stopReason":"tool_calls"}),
            "(api_response)",
        ),
        ("turn_end", json!({}), "turn ended"),
        (
            "tool_result",
            json!({"toolName":"Bash","exitCode":2}),
            "tool=Bash status=error exit=2",
        ),
        ("tool_result", json!({"isError":true}), "status=error"),
        ("tool_result", json!({}), "status=success"),
        (
            "tool_call",
            json!({"toolName":"read","toolInput":{"path":"/fixture/file"}}),
            "tool=read path=/fixture/file",
        ),
        (
            "tool_call",
            json!({"toolInput":{"command":"echo hello\nworld"}}),
            "cmd=\"echo hello world\"",
        ),
        (
            "tool_call",
            json!({"toolInput":{"file_path":"file"}}),
            "file=file",
        ),
        (
            "tool_call",
            json!({"toolInput":{"pattern":"*.rs"}}),
            "pattern=\"*.rs\"",
        ),
        ("tool_call", json!({}), "tool call"),
        ("thinking", json!({"content":"界a"}), "len=2 chars"),
        ("thinking", json!({}), "thinking"),
        ("user_message", json!({"lastPrompt":"hello"}), "\"hello\""),
        (
            "user_message",
            json!({"content":"fallback"}),
            "\"fallback\"",
        ),
        ("user_message", json!({}), "user message"),
        ("assistant_message", json!({"content":"done"}), "\"done\""),
        ("assistant_message", json!({}), "assistant message"),
        ("error", json!({"content":"bad\u{001b}[2J"}), "bad"),
        ("error", json!({}), "error"),
        (
            "question_prompt",
            json!({"question":"Continue?"}),
            "question=\"Continue?\"",
        ),
        ("waiting_question", json!({}), "waiting question"),
        ("custom", json!({}), "custom"),
    ] {
        assert_eq!(event_summary(&event(kind, data)), expected, "{kind}");
    }
    let mut r = event("run_started", json!({}));
    r.pid = 0;
    r.harness.clear();
    assert_eq!(event_summary(&r), "run started");
    r.event_type = "run_exited".into();
    assert_eq!(event_summary(&r), "run exited");
    r.event_type = "tool_call".into();
    r.data.file_modification = Some(FileModification {
        file_path: "changed.rs".into(),
        ..Default::default()
    });
    assert_eq!(event_summary(&r), "file=changed.rs");
    r.event_type = "turn_end".into();
    r.data.stop_reason = Some("toolUse".into());
    assert!(intermediate_tool_turn(&r));
    assert_eq!(event_label(&r), "API RESPONSE");
    r.event_type = "turn_start".into();
    assert_eq!(event_label(&r), "USER PROMPT");
    r.event_type = "text".into();
    r.data.role = Some("user".into());
    assert_eq!(event_label(&r), "USER PROMPT");
    r.event_type = "tool_result".into();
    assert_eq!(event_label(&r), "tool_result");
}

#[test]
fn event_list_details_and_json_keep_payload_identity_and_fold_paths() {
    let r = event(
        "turn_end",
        json!({"stopReason":"tool_call","content":"/fixture/file","usage":{"total":10}}),
    );
    for mode in [Mode::Human, Mode::Agent] {
        let f = format(mode);
        let mut out = Vec::new();
        f.events(&mut out, std::slice::from_ref(&r)).unwrap();
        f.event_detail(&mut out, &r).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("~/file"), "{text}");
        assert!(text.contains("worker"));
        assert!(text.contains(if mode == Mode::Agent {
            "tokens=10"
        } else {
            "API RESPONSE"
        }));
        let mut out = Vec::new();
        f.events_json(&mut out, std::slice::from_ref(&r), true)
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["id"], 9);
        assert_eq!(value["data"]["content"], "~/file");
        let mut out = Vec::new();
        f.events_json(&mut out, std::slice::from_ref(&r), false)
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&out)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let mut empty = Vec::new();
        f.events(&mut empty, &[]).unwrap();
        assert_eq!(empty.is_empty(), mode == Mode::Agent);
        assert!(f.events_json(&mut Vec::new(), &[], true).is_err());
        let mut invalid = r.clone();
        invalid.data_json = "{".into();
        assert!(f.event_detail(&mut Vec::new(), &invalid).is_err());
        let mut native = r.clone();
        native.data_json.clear();
        native.data = EventData {
            usage: Some(TokenUsage {
                total: 5,
                ..Default::default()
            }),
            ..Default::default()
        };
        f.event_detail(&mut Vec::new(), &native).unwrap();
    }
}
