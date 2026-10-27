use agent_common::types::*;
use agent_ingester::{
    classifier::Config,
    ingestion::{Ingestion, StateChange},
};
use agent_status_store::read;
use rusqlite::Connection;
fn frame(seq: i64, event: &str) -> TelemetryFrame {
    TelemetryFrame {
        version: "1.0".into(),
        seq,
        env_id: "env".into(),
        environment: EnvironmentInfo {
            id: "env".into(),
            label: "Local".into(),
            platform: "linux".into(),
            ..Default::default()
        },
        agent_id: "agent".into(),
        session_id: "session".into(),
        pid: 12,
        harness: HarnessType::Pi,
        event_type: event.into(),
        timestamp: 1000 + seq,
        ..Default::default()
    }
}
fn ingester() -> Ingestion {
    let mut db = Connection::open_in_memory().unwrap();
    agent_status_store::schema::migrate(&mut db).unwrap();
    Ingestion::from_connection(db, Config::default())
}
#[test]
fn telemetry_state_usage_and_delivery_cursor_commit_as_one_transaction() {
    let mut ingest = ingester();
    let mut first = frame(1, "tool_call");
    first.harness = HarnessType::ClaudeCode;
    first.data.tool_name = Some("Skill".into());
    first.data.tool_input =
        Some(serde_json::from_value(serde_json::json!({"skill":"review"})).unwrap());
    first.data.usage = Some(TokenUsage {
        input: 10,
        output: 2,
        ..Default::default()
    });
    ingest.connection().execute_batch("CREATE TRIGGER reject_events BEFORE INSERT ON telemetry_events BEGIN SELECT RAISE(ABORT,'controlled failure'); END;").unwrap();
    assert!(
        ingest
            .process_frames("env", &[first.clone()], 1001)
            .is_err()
    );
    assert_eq!(read::last_sequence(ingest.connection(), "env").unwrap(), 0);
    assert!(read::agent(ingest.connection(), "agent", 1001).is_err());
    ingest
        .connection()
        .execute_batch("DROP TRIGGER reject_events")
        .unwrap();
    let batch = ingest
        .process_frames("env", &[first.clone()], 1001)
        .unwrap();
    assert_eq!(batch.up_to, 1);
    assert_eq!(batch.updates.len(), 1);
    assert_eq!(batch.events.len(), 1);
    assert_eq!(batch.updates[0].activity.usage.input, 10);
    let replay = ingest.process_frames("env", &[first], 1002).unwrap();
    assert_eq!(replay.up_to, 1);
    assert!(replay.updates.is_empty());
    assert_eq!(
        read::events(ingest.connection(), "agent", 100, 0, None, 1002)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        read::usage(ingest.connection(), "agent", 1002)
            .unwrap()
            .skills[0]
            .count,
        1
    );
}
#[test]
fn batches_validate_environment_binding_and_atomic_state_changes() {
    let mut ingest = ingester();
    ingest
        .process_frames("env", &[frame(1, "turn_start")], 1001)
        .unwrap();
    let mut bad = frame(3, "thinking");
    bad.env_id = "other".into();
    assert!(
        ingest
            .process_frames("env", &[frame(2, "tool_result"), bad], 1003)
            .is_err()
    );
    assert_eq!(read::last_sequence(ingest.connection(), "env").unwrap(), 1);
    let mut invalid = frame(2, "thinking");
    invalid.version = "2.0".into();
    assert!(ingest.process_frames("env", &[invalid], 1003).is_err());
    let hidden = ingest
        .change_state(
            "agent",
            StateChange {
                is_hidden: Some(true),
                ..Default::default()
            },
            2000,
        )
        .unwrap();
    assert!(hidden.is_hidden);
    assert_eq!(hidden.hidden_at, Some(2000));
    assert!(
        ingest
            .change_state(
                "missing",
                StateChange {
                    dismiss_completion: Some(true),
                    ..Default::default()
                },
                2000
            )
            .is_err()
    );
    let shown = ingest
        .change_state(
            "agent",
            StateChange {
                is_hidden: Some(false),
                dismiss_completion: Some(true),
                ..Default::default()
            },
            2100,
        )
        .unwrap();
    assert!(!shown.is_hidden);
    assert!(shown.hidden_at.is_none());
}

#[test]
fn persistent_lifecycle_hello_heartbeat_timers_summaries_and_pruning() {
    use agent_ingester::summarizer::SummaryResult;
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut ingest = Ingestion::open(
        &path,
        Config {
            idle_grace: Duration::from_millis(10),
            summarize: true,
            ..Default::default()
        },
    )
    .unwrap();
    let env = EnvironmentInfo {
        id: "env".into(),
        platform: "linux".into(),
        label: "Local".into(),
        ..Default::default()
    };
    assert!(ingest.hello(EnvironmentInfo::default(), 0, 1000).is_err());
    assert_eq!(
        ingest
            .hello(env.clone(), 0, 1000)
            .unwrap()
            .last_heartbeat_at,
        1000
    );
    ingest
        .classifier_mut()
        .pricing_mut()
        .merge_json(br#"{"local":{"input_cost_per_token":0.5}}"#)
        .unwrap();
    let mut start = frame(1, "tool_call");
    start.data.tool_name = Some("read".into());
    start.data.model = Some("local".into());
    start.data.usage = Some(TokenUsage {
        input: 2,
        ..Default::default()
    });
    let batch = ingest.process_frames("env", &[start], 1001).unwrap();
    assert_eq!(batch.updates[0].activity.cost.as_ref().unwrap().total, 1.0);
    let job = &batch.summaries[0];
    let result = SummaryResult {
        action: "Investigating migration".into(),
        phase: Some(AgentPhase::Investigating),
        ..Default::default()
    };
    let updated = ingest
        .apply_summary("agent", job, result.clone(), 1002)
        .unwrap()
        .unwrap();
    assert_eq!(updated.activity.action, "Investigating migration");
    assert!(
        ingest
            .apply_summary("agent", job, result, 1003)
            .unwrap()
            .is_none()
    );
    assert!(
        ingest
            .apply_summary("missing", job, SummaryResult::default(), 1003)
            .unwrap()
            .is_none()
    );
    let mut end = frame(2, "turn_end");
    end.data.stop_reason = Some("stop".into());
    ingest.process_frames("env", &[end], 1002).unwrap();
    assert!(ingest.tick(1011).unwrap().updates.is_empty());
    let complete = ingest.tick(1012).unwrap();
    assert_eq!(
        complete.updates[0].activity.has_unseen_completion,
        Some(true)
    );
    ingest.connection().execute_batch("CREATE TRIGGER reject_visibility BEFORE UPDATE OF is_hidden ON agents BEGIN SELECT RAISE(ABORT,'controlled'); END;").unwrap();
    assert!(
        ingest
            .change_state(
                "agent",
                StateChange {
                    is_hidden: Some(true),
                    dismiss_completion: Some(true),
                    ..Default::default()
                },
                1013
            )
            .is_err()
    );
    assert_eq!(
        read::agent(ingest.connection(), "agent", 1013)
            .unwrap()
            .activity
            .has_unseen_completion,
        Some(true)
    );
    ingest
        .connection()
        .execute_batch("DROP TRIGGER reject_visibility")
        .unwrap();
    ingest
        .change_state(
            "agent",
            StateChange {
                acknowledge_completion: Some(true),
                ..Default::default()
            },
            1014,
        )
        .unwrap();
    ingest.heartbeat("env", 1020, 3).unwrap();
    assert_eq!(
        read::environments(ingest.connection(), 1020).unwrap()[0].last_heartbeat_at,
        1020
    );
    assert_eq!(
        ingest
            .connection()
            .query_row(
                "SELECT dropped_frames FROM environments WHERE id='env'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        3
    );
    ingest.hello(env.clone(), 2, 1021).unwrap();
    assert_eq!(read::last_sequence(ingest.connection(), "env").unwrap(), 2);
    ingest.hello(env, 0, 1022).unwrap();
    assert_eq!(read::last_sequence(ingest.connection(), "env").unwrap(), 0);
    assert!(
        ingest
            .change_state("agent", StateChange::default(), 1022)
            .is_err()
    );
    drop(ingest);
    let mut reopened = Ingestion::open(&path, Config::default()).unwrap();
    assert!(
        read::agent(reopened.connection(), "agent", 1023)
            .unwrap()
            .activity
            .has_unseen_completion
            .is_none()
    );
    reopened.prune(31 * 24 * 3600 * 1000).unwrap();
    assert!(read::agent(reopened.connection(), "agent", 31 * 24 * 3600 * 1000).is_err());
}

#[test]
fn failed_background_batch_cannot_commit_updates_without_delivering_them() {
    use std::time::Duration;
    let mut ingest = Ingestion::from_connection(
        agent_status_store::open_writer(":memory:").unwrap(),
        Config {
            idle_grace: Duration::from_millis(10),
            ..Default::default()
        },
    );
    let first = frame(1, "turn_start");
    let mut second = frame(2, "turn_start");
    second.agent_id = "second".into();
    second.session_id = "second-session".into();
    ingest
        .process_frames("env", &[first, second], 1002)
        .unwrap();
    let mut end_first = frame(3, "turn_end");
    end_first.data.stop_reason = Some("stop".into());
    let mut end_second = frame(4, "turn_end");
    end_second.agent_id = "second".into();
    end_second.session_id = "second-session".into();
    end_second.data.stop_reason = Some("stop".into());
    ingest
        .process_frames("env", &[end_first, end_second], 1004)
        .unwrap();
    ingest.connection().execute_batch("CREATE TRIGGER reject_second BEFORE UPDATE ON agents WHEN NEW.id='second' BEGIN SELECT RAISE(ABORT,'controlled failure'); END;").unwrap();
    assert!(ingest.tick(1014).is_err());
    assert_eq!(
        read::agent(ingest.connection(), "agent", 1014)
            .unwrap()
            .activity
            .activity,
        ActivityState::Busy
    );
    ingest
        .connection()
        .execute_batch("DROP TRIGGER reject_second")
        .unwrap();
    let batch = ingest.tick(1014).unwrap();
    assert_eq!(batch.updates.len(), 2);
    assert!(
        batch
            .updates
            .iter()
            .all(|agent| agent.activity.has_unseen_completion == Some(true))
    );
}
