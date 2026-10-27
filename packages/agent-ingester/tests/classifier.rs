use agent_common::types::*;
use agent_ingester::classifier::{Classifier, Config};
use std::time::Duration;

fn frame(event: &str, time: i64) -> TelemetryFrame {
    TelemetryFrame {
        version: "1.0".into(),
        seq: time,
        env_id: "env".into(),
        environment: EnvironmentInfo {
            id: "env".into(),
            ..Default::default()
        },
        agent_id: "agent".into(),
        session_id: "session".into(),
        pid: 123,
        harness: HarnessType::Pi,
        event_type: event.into(),
        timestamp: time,
        ..Default::default()
    }
}
fn apply(c: &mut Classifier, state: &mut Option<AgentState>, event: TelemetryFrame) -> AgentState {
    let outcome = c.process(state.take(), &event, &[], event.timestamp);
    let result = outcome.agent.clone();
    *state = Some(outcome.agent);
    result
}
#[test]
fn root_turns_keep_tools_and_attention_then_mark_terminal_completion() {
    let mut c = Classifier::new(Config {
        idle_grace: Duration::ZERO,
        ..Default::default()
    });
    let mut state = None;
    let initial = apply(&mut c, &mut state, frame("run_started", 1000));
    let repeated = apply(&mut c, &mut state, frame("run_started", 2000));
    assert_eq!(initial.activity.current_run, repeated.activity.current_run);
    assert_eq!(
        apply(&mut c, &mut state, frame("turn_start", 3000))
            .activity
            .action,
        "Thinking"
    );
    let mut tool = frame("tool_call", 4000);
    tool.data.tool_name = Some("bash".into());
    tool.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"command":"cargo test"})).unwrap());
    let running = apply(&mut c, &mut state, tool);
    assert_eq!(running.activity.action, "Running tests");
    assert_eq!(
        running.activity.active_target.as_deref(),
        Some("cargo test")
    );
    let mut end = frame("turn_end", 5000);
    end.data.stop_reason = Some("tool_use".into());
    assert_eq!(
        apply(&mut c, &mut state, end).activity.activity,
        ActivityState::Busy
    );
    assert!(
        apply(&mut c, &mut state, frame("tool_result", 6000))
            .activity
            .active_tool
            .is_none()
    );
    let mut end = frame("turn_end", 7000);
    end.data.content = Some("Fixed".into());
    end.data.stop_reason = Some("end_turn".into());
    let done = apply(&mut c, &mut state, end);
    assert_eq!(done.activity.activity, ActivityState::Idle);
    assert_eq!(done.activity.has_unseen_completion, Some(true));
    assert_eq!(done.activity.completed_at, Some(7000));
    let mut recap = frame("summary", 8000);
    recap.data.content = Some("Session recap".into());
    let recapped = apply(&mut c, &mut state, recap);
    assert_eq!(recapped.activity.summary.as_deref(), Some("Session recap"));
    assert_eq!(recapped.activity.updated_at, 7000);
    assert_eq!(recapped.activity.completed_at, Some(7000));
    assert!(
        apply(&mut c, &mut state, frame("turn_start", 9000))
            .activity
            .has_unseen_completion
            .is_none()
    );
    let mut question = frame("question_prompt", 10000);
    question.data.question = Some("Proceed?".into());
    apply(&mut c, &mut state, question);
    let waiting = apply(&mut c, &mut state, frame("turn_end", 11000));
    assert_eq!(waiting.activity.activity, ActivityState::WaitingQuestion);
    assert_eq!(waiting.activity.question.as_deref(), Some("Proceed?"));
    let exited = apply(&mut c, &mut state, frame("run_exited", 12000));
    assert_eq!(exited.activity.lifecycle, LifecycleState::Exited);
    assert_eq!(exited.activity.current_run.unwrap().ended_at, Some(12000));
}
#[test]
fn rate_limit_metadata_clears_and_subagent_precedence_is_deterministic() {
    let mut c = Classifier::new(Config {
        idle_grace: Duration::ZERO,
        subagent_dwell: Duration::from_millis(30),
        ..Default::default()
    });
    let mut state = None;
    apply(&mut c, &mut state, frame("run_started", 1000));
    let mut limited = frame("error", 2000);
    limited.data.retry_after_seconds = Some(90);
    limited.data.content = Some("quota".into());
    assert_eq!(
        apply(&mut c, &mut state, limited).activity.activity,
        ActivityState::RateLimited
    );
    let ready = apply(&mut c, &mut state, frame("thinking", 3000));
    assert_eq!(ready.activity.activity, ActivityState::Busy);
    assert!(ready.activity.retry_after_seconds.is_none());
    let mut end = frame("turn_end", 4000);
    end.data.stop_reason = Some("stop".into());
    apply(&mut c, &mut state, end);
    let mut sub = frame("tool_call", 5000);
    sub.subagent_id = Some("z".into());
    sub.data.tool_name = Some("read".into());
    let busy = apply(&mut c, &mut state, sub);
    assert_eq!(busy.activity.action, "Running subagent z: read");
    assert_eq!(busy.activity.subagent_count, 1);
    let mut question = frame("waiting_question", 6000);
    question.subagent_id = Some("b".into());
    question.data.content = Some(" Confirm ".into());
    assert_eq!(
        apply(&mut c, &mut state, question)
            .activity
            .question
            .as_deref(),
        Some("[Subagent b] Confirm")
    );
    let mut error = frame("error", 7000);
    error.subagent_id = Some("a".into());
    error.data.content = Some("failed".into());
    error.data.is_rate_limit = Some(false);
    error.data.retry_after_seconds = Some(200);
    let failed = apply(&mut c, &mut state, error);
    assert_eq!(failed.activity.activity, ActivityState::Error);
    assert_eq!(
        failed.activity.last_error.as_deref(),
        Some("[Subagent a] failed")
    );
    assert!(failed.activity.retry_after_seconds.is_none());
    assert_eq!(
        failed
            .activity
            .active_subagents
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "z"]
    );
    for (i, id) in ["a", "b", "z"].into_iter().enumerate() {
        let mut end = frame("turn_end", 8000 + i as i64);
        end.subagent_id = Some(id.into());
        apply(&mut c, &mut state, end);
    }
    let final_state = state.unwrap();
    assert_eq!(final_state.activity.activity, ActivityState::Idle);
    assert_eq!(final_state.activity.subagent_count, 3);
}

#[test]
fn timers_respect_root_generations_and_silence_never_announces_completion() {
    let config = Config {
        idle_grace: Duration::from_millis(50),
        turn_floor: Duration::from_millis(20),
        silence_timeout: Duration::from_millis(100),
        subagent_dwell: Duration::from_millis(30),
        rate_limit_buffer: Duration::ZERO,
        ..Default::default()
    };
    let mut c = Classifier::new(config);
    let mut state = None;
    apply(&mut c, &mut state, frame("turn_start", 1000));
    let mut end = frame("turn_end", 1010);
    end.data.stop_reason = Some("stop".into());
    apply(&mut c, &mut state, end);
    assert!(c.due_agents(1059).is_empty());
    assert_eq!(c.due_agents(1060), ["agent"]);
    let mut sub = frame("tool_call", 1030);
    sub.subagent_id = Some("sub".into());
    sub.data.tool_name = Some("read".into());
    apply(&mut c, &mut state, sub);
    let after = c.tick(state.take().unwrap(), 1060).unwrap().agent;
    assert_eq!(after.activity.activity, ActivityState::Busy);
    assert!(after.activity.has_unseen_completion.is_none());
    state = Some(after);
    let mut end = frame("turn_end", 1061);
    end.subagent_id = Some("sub".into());
    let done = apply(&mut c, &mut state, end);
    assert_eq!(done.activity.activity, ActivityState::Idle);
    assert_eq!(done.activity.has_unseen_completion, Some(true));
    apply(&mut c, &mut state, frame("turn_start", 2000));
    let silent = c.tick(state.take().unwrap(), 2100).unwrap().agent;
    assert_eq!(silent.activity.activity, ActivityState::Idle);
    assert!(silent.activity.has_unseen_completion.is_none());
    state = Some(silent);
    apply(&mut c, &mut state, frame("turn_start", 3000));
    let mut end = frame("turn_end", 3010);
    end.data.stop_reason = Some("stop".into());
    apply(&mut c, &mut state, end);
    apply(&mut c, &mut state, frame("thinking", 3030));
    assert!(c.tick(state.clone().unwrap(), 3060).is_none());
    assert_eq!(state.unwrap().activity.activity, ActivityState::Busy);
}

#[test]
fn summary_cadence_trailing_work_and_generation_guards_preserve_fresh_rows() {
    use agent_ingester::summarizer::SummaryResult;
    let mut c = Classifier::new(Config {
        summarize: true,
        debounce: Duration::from_millis(100),
        ..Default::default()
    });
    let mut first = frame("tool_call", 1000);
    first.data.tool_name = Some("edit".into());
    first.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"path":"a.rs"})).unwrap());
    let initial = c.process(None, &first, &[], 1000);
    let job = initial.summary.unwrap();
    let mut state = initial.agent;
    assert!(c.apply_summary(
        &mut state,
        &job,
        SummaryResult {
            action: "Reworking parser".into(),
            phase: Some(AgentPhase::Editing),
            active_target: Some("a.rs".into()),
            is_fallback: false
        },
        1001
    ));
    assert_eq!(state.activity.updated_at, 1001);
    assert!(!c.apply_summary(
        &mut state,
        &job,
        SummaryResult {
            action: "Reworking parser".into(),
            phase: Some(AgentPhase::Editing),
            active_target: Some("a.rs".into()),
            is_fallback: false
        },
        1002
    ));
    let repeated = c.process(Some(state.clone()), &first, &[], 1002);
    assert!(repeated.summary.is_none());
    first.timestamp = 1020;
    first.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"path":"b.rs"})).unwrap());
    let pending = c.process(Some(repeated.agent), &first, &[], 1020);
    assert!(pending.summary.is_none());
    assert!(pending.agent.activity.action_stale);
    assert_eq!(c.next_deadline(), Some(1100));
    let flushed = c.tick(pending.agent, 1100).unwrap();
    let trailing = flushed.summary.unwrap();
    let mut latest = flushed.agent;
    assert!(!c.apply_summary(
        &mut latest,
        &job,
        SummaryResult {
            action: "Stale update".into(),
            ..Default::default()
        },
        1101
    ));
    assert!(c.apply_summary(
        &mut latest,
        &trailing,
        SummaryResult {
            action: "Testing parser".into(),
            phase: Some(AgentPhase::Testing),
            active_target: Some("b.rs".into()),
            is_fallback: false
        },
        1102
    ));
    assert_eq!(latest.activity.action, "Testing parser");
    assert!(!latest.activity.action_stale);
    first.timestamp = 1030;
    first.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"path":"c.rs"})).unwrap());
    let checkpoint = c.checkpoint("agent");
    let pending = c.process(Some(latest.clone()), &first, &[], 1030);
    assert!(pending.agent.activity.action_stale);
    c.rollback("agent", checkpoint);
    assert_eq!(c.next_deadline(), Some(1_801_020));
    let pending = c.process(Some(latest), &first, &[], 1030);
    assert!(pending.agent.activity.action_stale);
    c.flush_trailing("agent", 1031);
    let flushed = c.tick(pending.agent, 1031).unwrap();
    assert!(flushed.summary.is_some());
    c.remove("agent");
    assert!(c.next_deadline().is_none());
    assert!(c.tick(flushed.agent, 1040).is_none());
}

#[test]
fn metadata_usage_hidden_state_and_restart_recovery_keep_independent_root_state() {
    let mut c = Classifier::new(Config::default());
    let mut event = frame("thinking", 1000);
    event.data.model = Some(" gpt-4o-mini ".into());
    event.data.title = Some(" Task ".into());
    event.data.usage = Some(TokenUsage {
        input: 100,
        output: 10,
        cache_read: 5,
        cache_write: 3,
        reasoning: 2,
        total: 0,
    });
    let mut agent = c.process(None, &event, &[], 1000).agent;
    assert_eq!(agent.activity.title, "Task");
    assert_eq!(agent.activity.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(agent.activity.context_window, Some(128000));
    assert_eq!(agent.activity.usage.total, 118);
    assert!(agent.activity.cost.unwrap().total > 0.0);
    agent.activity.cost = None;
    agent.is_hidden = true;
    agent.hidden_at = Some(2000);
    event.timestamp = 1500;
    let still_hidden = c.process(Some(agent), &event, &[], 1500).agent;
    assert!(still_hidden.is_hidden);
    event.timestamp = 2100;
    let mut unhidden = c.process(Some(still_hidden), &event, &[], 2100).agent;
    assert!(!unhidden.is_hidden);
    assert!(unhidden.hidden_at.is_none());
    unhidden.activity.activity = ActivityState::Busy;
    unhidden.activity.action = "Running subagent sub: read".into();
    unhidden.activity.active_tool = None;
    unhidden.activity.subagent_count = 8;
    unhidden.activity.active_subagents = vec![SubagentSummary {
        id: "sub".into(),
        activity: ActivityState::Busy,
        active_tool: Some("read".into()),
        ..Default::default()
    }];
    let mut restored = Classifier::new(Config {
        idle_grace: Duration::ZERO,
        subagent_dwell: Duration::ZERO,
        ..Default::default()
    });
    let mut ended = frame("turn_end", 2200);
    ended.subagent_id = Some("sub".into());
    ended.data.model = Some("claude-3-opus".into());
    ended.data.usage = Some(TokenUsage {
        input: 1,
        output: 2,
        total: 10,
        ..Default::default()
    });
    let result = restored
        .process(Some(unhidden), &ended, &["old".into()], 2200)
        .agent;
    assert_eq!(result.activity.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(result.activity.activity, ActivityState::Idle);
    assert!(result.activity.active_subagents.is_empty());
    assert_eq!(result.activity.subagent_count, 8);
    assert_eq!(result.activity.usage.total, 364);
    restored.dismiss_completion("agent");
    let recap = restored
        .process(Some(result), &frame("message", 2300), &[], 2300)
        .agent;
    assert!(recap.activity.has_unseen_completion.is_none());
}

#[test]
fn rate_limit_recovery_duration_uses_event_time_and_bounded_wait() {
    use agent_ingester::classifier::rate_limit_silence;
    let config = Config::default();
    assert_eq!(
        rate_limit_silence(&config, None, None, 1000),
        Duration::from_secs(1800)
    );
    assert_eq!(
        rate_limit_silence(&config, None, Some(1790), 1000),
        Duration::from_secs(1800)
    );
    assert_eq!(
        rate_limit_silence(&config, Some(2_001_000), None, 1000),
        Duration::from_secs(2120)
    );
    assert_eq!(
        rate_limit_silence(&config, Some(i64::MAX), Some(i64::MAX), 1),
        Duration::from_secs(86520)
    );
    assert_eq!(
        rate_limit_silence(
            &Config {
                silence_timeout: Duration::ZERO,
                ..config
            },
            Some(i64::MAX),
            None,
            1
        ),
        Duration::ZERO
    );
}
