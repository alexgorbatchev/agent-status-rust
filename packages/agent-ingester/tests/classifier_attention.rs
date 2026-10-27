use agent_common::types::*;
use agent_ingester::classifier::{Classifier, Config};
use std::time::Duration;
fn frame(event: &str, at: i64) -> TelemetryFrame {
    TelemetryFrame {
        agent_id: "agent".into(),
        event_type: event.into(),
        timestamp: at,
        pid: 1,
        ..Default::default()
    }
}
fn send(c: &mut Classifier, state: &mut Option<AgentState>, frame: TelemetryFrame) -> AgentState {
    let outcome = c.process(state.take(), &frame, &[], frame.timestamp.max(1));
    let agent = outcome.agent;
    *state = Some(agent.clone());
    agent
}
#[test]
fn root_attention_preserves_questions_errors_and_rate_limit_until_real_recovery() {
    let mut c = Classifier::new(Config::default());
    let mut state = None;
    send(&mut c, &mut state, frame("turn_start", 1000));
    assert_eq!(
        send(&mut c, &mut state, frame("turn_end", 1001))
            .activity
            .action,
        "Thinking"
    );
    let mut thinking = frame("thinking", 1002);
    thinking.data.content = Some("Investigating".into());
    thinking.data.last_prompt = Some(" Fix parser ".into());
    thinking.data.model_context_window = Some(1000);
    thinking.data.context_fullness = Some(0.5);
    let thinking = send(&mut c, &mut state, thinking);
    assert_eq!(
        thinking.activity.last_thinking_excerpt.as_deref(),
        Some("Investigating")
    );
    assert_eq!(thinking.activity.context_window, Some(1000));
    assert_eq!(thinking.activity.context_fullness, Some(0.5));
    let mut question = frame("tool_call", 1003);
    question.data.tool_name = Some("AskUserQuestion".into());
    question.data.question = Some("Choose parser?".into());
    assert_eq!(
        send(&mut c, &mut state, question).activity.activity,
        ActivityState::WaitingQuestion
    );
    let mut end = frame("turn_end", 1004);
    end.data.stop_reason = Some("tool_use".into());
    assert_eq!(
        send(&mut c, &mut state, end).activity.activity,
        ActivityState::WaitingQuestion
    );
    let mut error = frame("error", 1005);
    error.data.content = Some("界".repeat(2500));
    error.data.is_rate_limit = Some(false);
    error.data.retry_after_seconds = Some(100);
    let failed = send(&mut c, &mut state, error);
    assert_eq!(failed.activity.activity, ActivityState::Error);
    assert_eq!(failed.activity.last_error.unwrap().chars().count(), 2000);
    let mut end = frame("turn_end", 1006);
    end.data.stop_reason = Some("stop".into());
    assert_eq!(
        send(&mut c, &mut state, end).activity.activity,
        ActivityState::Error
    );
    let mut limit = frame("error", 1007);
    limit.data.is_rate_limit = Some(true);
    send(&mut c, &mut state, limit);
    let mut end = frame("turn_end", 1008);
    end.data.retry_after_seconds = Some(3600);
    end.data.rate_limit_reset_at = Some(4_000_000);
    let limited = send(&mut c, &mut state, end);
    assert_eq!(limited.activity.activity, ActivityState::RateLimited);
    assert_eq!(limited.activity.retry_after_seconds, Some(3600));
    send(&mut c, &mut state, frame("turn_start", 1010));
    let mut tool = frame("tool_call", 1011);
    tool.data.tool_name = Some("edit".into());
    tool.data.file_modification = Some(FileModification {
        file_path: "file.rs".into(),
        ..Default::default()
    });
    assert_eq!(
        send(&mut c, &mut state, tool)
            .activity
            .active_target
            .as_deref(),
        Some("file.rs")
    );
    let mut next = frame("tool_call", 1012);
    next.data.tool_name = Some("read".into());
    assert!(
        send(&mut c, &mut state, next)
            .activity
            .active_target
            .is_none()
    );
    let exited = send(&mut c, &mut state, frame("run_exited", 1013));
    let ended = exited.activity.current_run.unwrap();
    let restarted = send(&mut c, &mut state, frame("run_started", 1014));
    assert_ne!(restarted.activity.current_run.unwrap().id, ended.id);
    let mut old = frame("summary", 10);
    old.data.content = Some("old".into());
    send(&mut c, &mut state, old);
    let mut recent = frame("summary", 20);
    recent.data.content = Some("recent".into());
    send(&mut c, &mut state, recent);
    let mut old = frame("summary", 15);
    old.data.content = Some("stale".into());
    assert_eq!(
        send(&mut c, &mut state, old).activity.summary.as_deref(),
        Some("recent")
    );
    let mut fresh = Classifier::new(Config::default());
    assert_eq!(
        fresh
            .process(None, &frame("run_exited", 0), &[], 50)
            .agent
            .activity
            .first_started_at,
        50
    );
    let mut initial = frame("run_started", 20);
    initial.pid = 0;
    let init = fresh.process(None, &initial, &[], 20).agent;
    initial.pid = 10;
    assert_eq!(
        fresh
            .process(Some(init), &initial, &[], 21)
            .agent
            .activity
            .current_run
            .unwrap()
            .pid,
        10
    );
}

#[test]
fn subagent_questions_errors_rate_limits_results_and_dwell_keep_root_state_separate() {
    let mut c = Classifier::new(Config {
        subagent_dwell: Duration::from_millis(100),
        silence_timeout: Duration::from_millis(10),
        rate_limit_buffer: Duration::ZERO,
        ..Default::default()
    });
    let mut state = None;
    send(&mut c, &mut state, frame("run_started", 1000));
    let mut sub = frame("turn_start", 1001);
    sub.subagent_id = Some("sub".into());
    send(&mut c, &mut state, sub.clone());
    sub.event_type = "tool_call".into();
    sub.timestamp = 1002;
    sub.data.tool_name = Some("AskUserQuestion".into());
    sub.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"prompt":" Confirm? "})).unwrap());
    assert_eq!(
        send(&mut c, &mut state, sub.clone())
            .activity
            .question
            .as_deref(),
        Some("[Subagent sub] Confirm?")
    );
    sub.event_type = "error".into();
    sub.timestamp = 1003;
    sub.data.content = Some("quota".into());
    sub.data.is_rate_limit = Some(true);
    assert_eq!(
        send(&mut c, &mut state, sub.clone()).activity.activity,
        ActivityState::RateLimited
    );
    let quiet = c.tick(state.take().unwrap(), 1013).unwrap().agent;
    assert_eq!(quiet.activity.activity, ActivityState::Idle);
    assert!(quiet.activity.has_unseen_completion.is_none());
    assert_eq!(
        quiet.activity.active_subagents[0].status,
        SubagentStatus::Completed
    );
    state = Some(quiet);
    sub.event_type = "tool_result".into();
    sub.timestamp = 1014;
    sub.data.is_error = Some(true);
    let resumed = send(&mut c, &mut state, sub.clone());
    assert_eq!(resumed.activity.activity, ActivityState::Busy);
    assert!(resumed.activity.active_subagents[0].completed_at.is_none());
    sub.event_type = "error".into();
    sub.timestamp = 1015;
    sub.data.is_rate_limit = Some(false);
    sub.data.content = None;
    sub.data.tool_output = Some("bad result".into());
    assert_eq!(
        send(&mut c, &mut state, sub.clone())
            .activity
            .last_error
            .as_deref(),
        Some("[Subagent sub] bad result")
    );
    let mut root_error = frame("error", 1016);
    root_error.data.content = Some("root failed".into());
    assert_eq!(
        send(&mut c, &mut state, root_error)
            .activity
            .last_error
            .as_deref(),
        Some("root failed")
    );
    let mut question = frame("question_prompt", 1017);
    question.data.question = Some("root question".into());
    assert_eq!(
        send(&mut c, &mut state, question).activity.activity,
        ActivityState::Error
    );
    sub.event_type = "tool_result".into();
    sub.timestamp = 1018;
    assert_eq!(
        send(&mut c, &mut state, sub.clone())
            .activity
            .question
            .as_deref(),
        Some("root question")
    );
    sub.event_type = "turn_end".into();
    sub.timestamp = 1020;
    send(&mut c, &mut state, sub);
    assert_eq!(c.next_deadline(), Some(1101));
    let evicted = c.tick(state.take().unwrap(), 1101).unwrap().agent;
    assert!(evicted.activity.active_subagents.is_empty());
    assert_eq!(evicted.activity.activity, ActivityState::WaitingQuestion);
}
