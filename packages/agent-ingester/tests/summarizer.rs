use agent_common::types::{ActivityState as A, AgentPhase as P};
use agent_ingester::summarizer::{self as s, SummaryInput, ToolExecution};
use serde_json::json;

#[test]
fn deterministic_activity_and_tool_summaries_keep_attention_and_context() {
    for (tool, target, expected, phase) in [
        ("Edit", "src/a.rs", "Editing src/a.rs", P::Editing),
        ("notebook_edit", "", "Editing code", P::Editing),
        ("write_file", "a", "Writing a", P::Editing),
        ("write", "", "Writing file", P::Editing),
        ("readfile", "a", "Reading a", P::Investigating),
        ("read", "", "Reading file", P::Investigating),
        ("exec_command", "cargo test", "Running tests", P::Testing),
        ("bash", "build", "Running build", P::Editing),
        ("sh", "", "Running shell command", P::Editing),
        (
            "ask_user_question",
            "",
            "Waiting for user input",
            P::Planning,
        ),
        ("web_search", "docs", "Searching docs", P::Investigating),
        ("codegraph", "", "Investigating codebase", P::Investigating),
        ("custom", "target", "Using custom on target", P::Editing),
        ("custom", "", "Using custom", P::Editing),
    ] {
        let input = SummaryInput {
            activity: A::Busy,
            active_tool: Some(tool.into()),
            active_target: Some(target.into()),
            ..Default::default()
        };
        let result = s::fallback(&input);
        assert_eq!(result.action, expected);
        assert_eq!(result.phase, Some(phase));
        assert!(result.is_fallback);
        let result = s::fallback(&SummaryInput {
            activity: A::RateLimited,
            ..input
        });
        assert_eq!(
            result.action,
            "Rate limit reached — backoff/retry in progress"
        );
        assert_eq!(result.phase, None);
    }
    for (input, action, phase) in [
        (
            SummaryInput {
                activity: A::WaitingQuestion,
                question: Some(" choose? ".into()),
                ..Default::default()
            },
            "choose?",
            Some(P::Planning),
        ),
        (
            SummaryInput {
                activity: A::WaitingQuestion,
                ..Default::default()
            },
            "Waiting for user input",
            Some(P::Planning),
        ),
        (
            SummaryInput {
                activity: A::Error,
                last_error: Some(" failed ".into()),
                ..Default::default()
            },
            "failed",
            Some(P::Debugging),
        ),
        (
            SummaryInput {
                activity: A::Error,
                ..Default::default()
            },
            "Encountered error",
            Some(P::Debugging),
        ),
        (
            SummaryInput {
                activity: A::Idle,
                ..Default::default()
            },
            "Idle",
            None,
        ),
        (
            SummaryInput {
                activity: A::Busy,
                last_assistant_response: "done".into(),
                ..Default::default()
            },
            "Responded to user",
            None,
        ),
        (
            SummaryInput {
                activity: A::Idle,
                last_assistant_response: "done".into(),
                ..Default::default()
            },
            "Responded to user",
            None,
        ),
        (
            SummaryInput {
                activity: A::Busy,
                current_action: "Existing task".into(),
                ..Default::default()
            },
            "Existing task",
            None,
        ),
        (
            SummaryInput {
                activity: A::Busy,
                recent_tools: vec![ToolExecution {
                    tool_name: "Read".into(),
                    target: "a".into(),
                }],
                ..Default::default()
            },
            "Reading a",
            Some(P::Investigating),
        ),
        (
            SummaryInput {
                activity: A::Busy,
                last_thinking_excerpt: "considering".into(),
                ..Default::default()
            },
            "Thinking",
            Some(P::Planning),
        ),
        (
            SummaryInput {
                activity: A::Busy,
                current_action: "Thinking".into(),
                ..Default::default()
            },
            "Thinking",
            Some(P::Planning),
        ),
        (
            SummaryInput {
                activity: A::Busy,
                ..Default::default()
            },
            "Working",
            None,
        ),
    ] {
        let result = s::fallback(&input);
        assert_eq!(result.action, action);
        assert_eq!(result.phase, phase);
    }
}

#[test]
fn structured_output_and_semantic_deduplication_use_real_text() {
    for (raw, action, phase) in [
        (
            r#"{"action":" task ","phase":"plan","activeTarget":" a "}"#,
            "task",
            Some(P::Planning),
        ),
        (
            "before\n```json\n{\"action\":\"write\",\"phase\":\"writing\"}\n```\nafter",
            "write",
            Some(P::Editing),
        ),
        (
            r#"prefix {"action":"test","phase":"test"} suffix"#,
            "test",
            Some(P::Testing),
        ),
        (
            r#"{"action":"fix","phase":"fixing"}"#,
            "fix",
            Some(P::Debugging),
        ),
        (
            r#"{"action":"search","phase":"searching"}"#,
            "search",
            Some(P::Investigating),
        ),
        (r#"{"action":"none","phase":"unknown"}"#, "none", None),
    ] {
        let result = s::parse_output(raw).unwrap();
        assert_eq!(result.action, action);
        assert_eq!(result.phase, phase);
    }
    for raw in [
        "",
        "nothing",
        "{broken}",
        "```empty```",
        "[]",
        r#"{"action":""}"#,
    ] {
        assert!(s::parse_output(raw).is_err(), "{raw}");
    }
    assert_eq!(
        s::tokenize("Hello, 世界! Build42 + HELLO"),
        ["hello", "世界", "build42", "hello"]
    );
    for (a, b, jaccard, overlap) in [
        ("a b", "a b c", 2.0 / 3.0, 1.0),
        ("", "", 1.0, 1.0),
        ("!", "?", 0.0, 0.0),
        ("a", "", 0.0, 0.0),
    ] {
        assert!((s::jaccard_similarity(a, b) - jaccard).abs() < 1e-10);
        assert!((s::word_overlap_similarity(a, b) - overlap).abs() < 1e-10);
    }
    assert!(s::is_near_duplicate(
        "Building authentication service",
        "Building authentication service with database migrations",
        0.6
    ));
    assert!(!s::is_near_duplicate(
        "Thinking",
        "Thinking about building authentication",
        0.6
    ));
    assert!(s::is_near_duplicate(" same ", "SAME", 0.0));
    assert!(s::is_near_duplicate("", " ", 0.6));
    assert!(!s::is_near_duplicate("", "task", 0.6));
}

#[test]
fn prompt_strategies_bound_unicode_and_scrub_nested_payloads() {
    let nested = json!({"outer":{"inner":{"token":"deep-secret","path":"retained.rs"}},"list":[{"meta":{"password":"array-secret","name":"retained-name"}}]});
    let nested_preview = s::sanitize_tool_input(&nested, 10_000);
    assert!(!nested_preview.contains("deep-secret"), "{nested_preview}");
    assert!(!nested_preview.contains("array-secret"), "{nested_preview}");
    assert!(nested_preview.contains("retained.rs"));
    assert!(nested_preview.contains("retained-name"));
    let input = json!({"path":"src/a.rs","token":"secret-marker","BODY":"payload-marker","nested":{"PASSWORD":"nested-marker","enabled":true},"list":[{"api_key":"list-marker","name":"kept"}],"empty":null,"line":"a\nb","long":"界".repeat(150)});
    let preview = s::sanitize_tool_input(&input, 0);
    for secret in [
        "secret-marker",
        "payload-marker",
        "nested-marker",
        "list-marker",
    ] {
        assert!(!preview.contains(secret), "{preview}");
    }
    assert!(preview.contains("src/a.rs"));
    assert!(!preview.contains('\n'));
    assert!(preview.chars().count() <= 303);
    assert_eq!(s::sanitize_tool_input(&json!({"token":"secret"}), 100), "");
    let mut input = SummaryInput {
        activity: A::Busy,
        last_prompt: "界".repeat(1200),
        last_thinking_excerpt: "thought".into(),
        active_tool: Some("Read".into()),
        active_target: Some("a".into()),
        active_tool_input: json!({"path":"a"}),
        recent_tools: (0..5)
            .map(|i| ToolExecution {
                tool_name: format!("Tool{i}"),
                target: if i == 4 { "last".into() } else { String::new() },
            })
            .collect(),
        ..Default::default()
    };
    let prompt = s::build_user_prompt(&input);
    assert!(
        prompt
            .contains("Thinking: thought\nActive Tool: Read (Target: a)\nTool Arguments: path: a")
    );
    assert!(!prompt.contains("Recent Tool"));
    input.active_tool = None;
    assert!(!s::build_user_prompt(&input).contains("Active Tool"));
    input.last_thinking_excerpt.clear();
    input.active_tool = Some("Read".into());
    input.active_tool_args = Some("界".repeat(400));
    let prompt = s::build_user_prompt(&input);
    assert!(prompt.contains("- Tool2\n- Tool3\n- Tool4 on last"));
    assert!(!prompt.contains("- Tool0"));
    input.active_tool = None;
    input.last_assistant_response = "a".repeat(1100);
    input.question = Some("choice".into());
    input.last_error = Some("e".repeat(400));
    let prompt = s::build_user_prompt(&input);
    assert!(prompt.contains("Assistant Response: "));
    assert!(prompt.contains("Current Activity State: BUSY"));
    assert!(prompt.contains("Question Asked: choice"));
    assert!(prompt.contains("Last Error: "));
}
