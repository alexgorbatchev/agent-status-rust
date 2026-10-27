use agent_common::types::{ActivityState, AgentState, CostInfo, LifecycleState, RunInfo};
use agent_debug::{comparison, database::SessionRecord};

#[test]
fn compares_all_fields_with_tolerances_and_recovery_warnings() {
    let mut replayed = AgentState::default();
    replayed.activity.lifecycle = LifecycleState::Exited;
    replayed.activity.activity = ActivityState::Idle;
    replayed.activity.updated_at = 1000;
    replayed.activity.usage.total = 1200;
    replayed.activity.cost = Some(CostInfo {
        total: 1.0,
        ..Default::default()
    });
    let mut record = SessionRecord {
        db_path: "state.db".into(),
        agent: replayed.clone(),
        runs: vec![],
        event_count: 1,
        first_event_timestamp: Some(1000),
        last_event_timestamp: Some(2000),
    };
    let result = comparison::compare(&replayed, 1000, &record);
    assert!(!result.has_drift);
    assert_eq!(result.fields.len(), 7);
    assert!(result.warnings.is_empty());
    record.agent.activity.lifecycle = LifecycleState::Running;
    record.agent.activity.activity = ActivityState::Busy;
    record.agent.activity.usage.total = 2200;
    record.agent.activity.cost.as_mut().unwrap().total = 1.1;
    record.agent.activity.updated_at = 121000;
    record.agent.activity.current_run = Some(RunInfo {
        id: "active-run".into(),
        ..Default::default()
    });
    record.last_event_timestamp = Some(2010);
    let result = comparison::compare(&replayed, 1000, &record);
    assert!(result.has_drift);
    assert!(result.fields.iter().all(|f| f.is_drift));
    assert_eq!(result.warnings.len(), 3);
    assert!(result.fields[2].status.contains("+1,000"));
    assert!(result.fields[5].status.contains("+2m"));
}

#[test]
fn comparison_math_handles_full_timestamp_and_token_ranges() {
    let mut replayed = AgentState::default();
    replayed.activity.usage.total = i64::MIN;
    replayed.activity.updated_at = i64::MIN;
    let mut persisted = replayed.clone();
    persisted.activity.usage.total = i64::MAX;
    persisted.activity.updated_at = i64::MAX;
    let record = SessionRecord {
        db_path: "state.db".into(),
        agent: persisted,
        runs: vec![],
        event_count: 0,
        first_event_timestamp: None,
        last_event_timestamp: None,
    };
    let result = comparison::compare(&replayed, 0, &record);
    assert!(
        result.fields[2]
            .status
            .contains("18,446,744,073,709,551,615")
    );
    assert!(result.has_drift);
    assert_eq!(result.warnings.len(), 1);
}
