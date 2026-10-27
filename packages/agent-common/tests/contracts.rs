use agent_common::{models, types::*, usage};
use serde_json::json;

#[test]
fn snapshots_preserve_required_empty_arrays_and_roundtrip_data() {
    let event: ServerEvent = serde_json::from_value(json!({"type":"snapshot","at":100})).unwrap();
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(wire["agents"], json!([]));
    assert_eq!(wire["environments"], json!([]));
    let event: ServerEvent = serde_json::from_value(
        json!({"type":"agent_upsert","at":101,"agent":{"id":"a","harness":"pi"}}),
    )
    .unwrap();
    let wire = serde_json::to_value(&event).unwrap();
    assert!(wire.get("agents").is_none());
    assert_eq!(wire["agent"]["harness"], "pi");
    assert_eq!(serde_json::from_value::<ServerEvent>(wire).unwrap(), event);
}

#[test]
fn completion_wire_precedence_and_unknown_string_tags_are_preserved() {
    let activity: AgentActivity = serde_json::from_value(json!({"lifecycle":"RUNNING","activity":"RATE_LIMITED","hasUnseenCompletion":false,"has_unseen_completion":true,"completedAt":111,"completed_at":222,"rateLimitResetAt":123,"retryAfterSeconds":30})).unwrap();
    assert_eq!(activity.has_unseen_completion, Some(false));
    assert_eq!(activity.completed_at, Some(111));
    assert_eq!(activity.retry_after_seconds, Some(30));
    let activity: AgentActivity =
        serde_json::from_value(json!({"has_unseen_completion":true,"completed_at":222})).unwrap();
    assert_eq!(activity.has_unseen_completion, Some(true));
    assert_eq!(activity.completed_at, Some(222));
    let state: AgentState = serde_json::from_value(
        json!({"harness":"future-harness","activity":{"activity":"FUTURE"}}),
    )
    .unwrap();
    assert_eq!(state.harness.as_str(), "future-harness");
    assert_eq!(
        serde_json::to_value(state).unwrap()["activity"]["activity"],
        "FUTURE"
    );
    assert!(serde_json::from_value::<AgentActivity>(json!({"lifecycle":123})).is_err());
}

#[test]
fn observed_skill_invocations_are_deduplicated_by_native_call_identity() {
    let mut frame: TelemetryFrame = serde_json::from_value(json!({"agentId":"agent","harness":"claude-code","eventType":"tool_call","timestamp":1000,"data":{"toolName":"Skill","toolCallId":"call-1","toolInput":{"skill":"plugin:review","args":"sensitive arguments"}}})).unwrap();
    let first = usage::extract(&frame);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].name, "plugin:review");
    assert_eq!(first[0].operation, "invoke");
    assert_eq!(first[0].evidence.tool_call_id, "call-1");
    assert!(!format!("{:?}", first[0]).contains("sensitive arguments"));
    frame.timestamp = 2000;
    frame.seq = 500;
    assert_eq!(usage::extract(&frame)[0].id, first[0].id);
    frame.subagent_id = Some("child".into());
    assert_ne!(usage::extract(&frame)[0].id, first[0].id);
    frame.data.tool_call_id = None;
    let fallback = usage::extract(&frame)[0].id.clone();
    frame.seq += 1;
    assert_eq!(usage::extract(&frame)[0].id, fallback);
    frame.timestamp += 1;
    assert_ne!(usage::extract(&frame)[0].id, fallback);
    frame.timestamp = 0;
    assert!(usage::extract(&frame).is_empty());
}

#[test]
fn context_resolution_prefers_specific_models_and_reports_real_capacity() {
    for (name, expected) in [
        ("openai/GPT-5", 400_000),
        ("gpt-5.4", 1_050_000),
        ("gpt-5.5", 1_050_000),
        ("google/gemini-2.5-pro", 1_048_576),
        ("anthropic/claude-opus-5", 1_000_000),
        ("claude-sonnet-4-5", 200_000),
        ("gpt-4-32k", 32_768),
        ("o1-preview", 128_000),
        ("o1", 200_000),
        ("", 128_000),
    ] {
        assert_eq!(models::resolve_context_window(name), expected, "{name}");
    }
    assert_eq!(models::calculate_context_fullness(50_000, 200_000), 0.25);
    assert_eq!(models::calculate_context_fullness(50_000, 0), 0.0);
    for (model, expected) in [
        ("gpt-4-32k-custom", 32768),
        ("o1-preview-custom", 128000),
        ("gemini-1.5-pro-suffix", 2000000),
        ("claude-next", 200000),
        ("gemini-next", 1000000),
        ("gpt-3.5-next", 16385),
        ("gpt-4-next", 8192),
        ("gpt-4o-next", 128000),
        ("gpt-4-turbo-next", 128000),
        ("o1-new", 200000),
        ("o3-new", 200000),
        ("unknown", 128000),
        ("claude-sonnet-4-5:1m", 1000000),
    ] {
        assert_eq!(models::resolve_context_window(model), expected, "{model}");
    }
}

#[test]
fn telemetry_and_server_events_preserve_nested_data_and_optional_fields() {
    let activity = json!({"lifecycle":"EXITED","activity":"WAITING_QUESTION","title":"Review","action":"testing","summary":"Completed","summaryUpdatedAt":12,"actionStale":false,"phase":"testing","activeTool":"shell","activeTarget":"src","activeToolStartedAt":10,"question":"Proceed?","lastThinkingExcerpt":"checking","lastError":"failure","rateLimitResetAt":20,"retryAfterSeconds":3,"model":"model","contextWindow":200000,"contextFullness":0.25,"cost":{"currency":"USD","total":1.25,"isEstimated":true},"subagentCount":1,"activeSubagents":[{"id":"child","activity":"BUSY","activeTool":"read","action":"investigating","lastError":"last","status":"settling","startedAt":9,"completedAt":11}],"currentRun":{"id":"run","pid":123,"harnessVersion":"1","startedAt":1,"endedAt":12},"firstStartedAt":1,"updatedAt":12,"hasUnseenCompletion":true,"completedAt":12});
    let agent:AgentState=serde_json::from_value(json!({"id":"a","sessionId":"session","harness":"codex","activity":activity,"isHidden":true,"hiddenAt":13,"workspace":{"isWorktree":true,"branch":"main","isDetached":false,"relPath":null,"cwd":"/project"}})).unwrap();
    assert_eq!(agent.activity.lifecycle, LifecycleState::Exited);
    assert_eq!(
        agent.activity.active_subagents[0].status,
        SubagentStatus::Settling
    );
    assert_eq!(
        serde_json::from_value::<AgentState>(serde_json::to_value(&agent).unwrap()).unwrap(),
        agent
    );
    let frame:TelemetryFrame=serde_json::from_value(json!({"version":"1","seq":4,"envId":"env","agentId":"a","sessionId":"session","subagentId":"child","pid":123,"harness":"codex","harnessVersion":"1","timestamp":12,"eventType":"agent_exit","data":{"toolCallId":"call"}})).unwrap();
    let server = ServerEvent {
        kind: "snapshot".into(),
        at: 13,
        agent: Some(agent.clone()),
        agents: vec![agent.clone()],
        agent_id: "a".into(),
        environment: Some(EnvironmentInfo::default()),
        environments: vec![EnvironmentInfo::default()],
        event: Some(frame.clone()),
    };
    assert_eq!(
        serde_json::from_value::<ServerEvent>(serde_json::to_value(&server).unwrap()).unwrap(),
        server
    );
    let observer = ObserverMessage {
        kind: "frames".into(),
        env_id: "env".into(),
        environment: Some(EnvironmentInfo::default()),
        since: 2,
        dropped: 1,
        frames: vec![frame],
    };
    assert_eq!(
        serde_json::from_value::<ObserverMessage>(serde_json::to_value(&observer).unwrap())
            .unwrap(),
        observer
    );
    let response:StatusResponse=serde_json::from_value(json!({"status":"ok","environments":[{"id":"env"}],"activeAgentCount":2,"requiresAttentionCount":1,"totalFleetSpend":1.25})).unwrap();
    assert_eq!(response.environments[0].id, "env");
    let group = ProjectGroup {
        agents: vec![agent],
        total_cost: 1.25,
        active_agent_count: 1,
        ..Default::default()
    };
    assert_eq!(
        serde_json::from_value::<ProjectGroup>(serde_json::to_value(&group).unwrap()).unwrap(),
        group
    );
    let ingester: IngesterMessage = serde_json::from_value(json!({"type":"ack","upTo":4})).unwrap();
    assert_eq!(
        serde_json::to_value(ingester).unwrap(),
        json!({"type":"ack","upTo":4})
    );
    for phase in [
        "planning",
        "editing",
        "testing",
        "debugging",
        "investigating",
        "future",
    ] {
        let parsed: AgentPhase = serde_json::from_value(json!(phase)).unwrap();
        assert_eq!(parsed.as_str(), phase);
        assert_eq!(serde_json::to_value(parsed).unwrap(), json!(phase));
    }
    for status in ["running", "completed", "settling", ""] {
        let parsed: SubagentStatus = serde_json::from_value(json!(status)).unwrap();
        assert_eq!(parsed.as_str(), status);
    }
    let empty: AgentActivity = serde_json::from_value(json!({"activeSubagents":null})).unwrap();
    assert!(empty.active_subagents.is_empty());
}
