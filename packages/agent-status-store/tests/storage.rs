use agent_common::types::*;
use agent_status_store::{open_reader, open_writer, read, write};

fn fixture() -> AgentState {
    AgentState {
        id: "env:claude-code:session".into(),
        session_id: "session".into(),
        harness: HarnessType::ClaudeCode,
        environment: EnvironmentInfo {
            id: "env".into(),
            label: "Local".into(),
            hostname: "host".into(),
            platform: "linux".into(),
            user: "user".into(),
            last_heartbeat_at: 1000,
            ..Default::default()
        },
        project: ProjectInfo {
            id: "project".into(),
            name: "Repository".into(),
            canonical_root: "/repo".into(),
        },
        workspace: WorkspaceInfo {
            branch: "main".into(),
            cwd: "/repo".into(),
            ..Default::default()
        },
        activity: AgentActivity {
            lifecycle: LifecycleState::Running,
            activity: ActivityState::Busy,
            title: "Working".into(),
            action: "Editing".into(),
            updated_at: 1000,
            first_started_at: 900,
            current_run: Some(RunInfo {
                id: "run".into(),
                pid: 42,
                harness_version: "1.0".into(),
                started_at: 900,
                ended_at: None,
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn native_transactions_round_trip_optional_fields_and_read_only_access() {
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("nested/state.sqlite");
    let mut db = open_writer(&path).unwrap();
    let mut agent = fixture();
    agent.activity.summary = Some("Recap".into());
    agent.activity.summary_updated_at = Some(1000);
    agent.activity.model = Some("model".into());
    agent.activity.context_window = Some(200_000);
    agent.activity.context_fullness = Some(0.0);
    agent.activity.cost = Some(CostInfo {
        currency: "USD".into(),
        total: 0.0,
        is_estimated: false,
    });
    agent.activity.active_tool_started_at = Some(1000);
    agent.activity.active_tool = Some("Read".into());
    agent.activity.question = Some(String::new());
    agent.activity.has_unseen_completion = Some(true);
    agent.activity.completed_at = Some(1000);
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &agent).unwrap();
    write::save_cursor(&tx, "env", 10).unwrap();
    tx.commit().unwrap();
    assert_eq!(read::agent(&db, &agent.id, 1000).unwrap(), agent);
    assert_eq!(read::last_sequence(&db, "env").unwrap(), 10);
    let reader = open_reader(&path).unwrap();
    assert_eq!(
        read::agents(&reader, &read::AgentFilter::default(), 1000).unwrap(),
        vec![agent.clone()]
    );
    assert!(reader.execute("DELETE FROM agents", []).is_err());
    let tx = db.transaction().unwrap();
    write::dismiss_completion(&tx, &agent.id).unwrap();
    write::set_visibility(&tx, &agent.id, true, Some(2000), 2000).unwrap();
    tx.commit().unwrap();
    let stored = read::agent(&db, &agent.id, 2000).unwrap();
    assert!(stored.is_hidden);
    assert_eq!(stored.activity.completed_at, None);
    assert_eq!(stored.activity.summary, agent.activity.summary);
    let tx = db.transaction().unwrap();
    write::end_current_run(&tx, &agent.id, 2100).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        read::agent(&db, &agent.id, 2100)
            .unwrap()
            .activity
            .lifecycle,
        LifecycleState::Exited
    );
}

#[test]
fn telemetry_usage_and_cursor_commit_or_roll_back_together() {
    let mut db = open_writer(":memory:").unwrap();
    let agent = fixture();
    let frame = TelemetryFrame {
        agent_id: agent.id.clone(),
        env_id: "env".into(),
        harness: agent.harness.clone(),
        event_type: "tool_call".into(),
        timestamp: 1000,
        data: EventData {
            tool_name: Some("Skill".into()),
            tool_call_id: Some("call".into()),
            tool_input: Some(serde_json::Map::from_iter([(
                "skill".into(),
                "review".into(),
            )])),
            ..Default::default()
        },
        ..Default::default()
    };
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &agent).unwrap();
    tx.commit().unwrap();
    db.execute_batch("CREATE TRIGGER fail_usage BEFORE INSERT ON usage_observations BEGIN SELECT RAISE(FAIL,'blocked'); END").unwrap();
    {
        let tx = db.transaction().unwrap();
        assert!(write::record_event(&tx, &frame).is_err());
        write::save_cursor(&tx, "env", 10).unwrap();
    }
    assert_eq!(read::last_sequence(&db, "env").unwrap(), 0);
    assert!(
        read::events(&db, &agent.id, 100, 0, None, 1000)
            .unwrap()
            .is_empty()
    );
    db.execute_batch("DROP TRIGGER fail_usage").unwrap();
    let tx = db.transaction().unwrap();
    for _ in 0..3 {
        write::record_event(&tx, &frame).unwrap();
    }
    write::save_cursor(&tx, "env", 10).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        read::usage(&db, &agent.id, 1000).unwrap().skills[0].count,
        1
    );
    let tx = db.transaction().unwrap();
    write::prune(&tx, 2000, 0).unwrap();
    tx.commit().unwrap();
    assert!(
        read::events(&db, &agent.id, 100, 0, None, 1000)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        read::usage(&db, &agent.id, 1000).unwrap().skills[0].count,
        1
    );
}

#[test]
fn ended_referenced_runs_are_exited_and_filtered_consistently() {
    let mut db = open_writer(":memory:").unwrap();
    let agent = fixture();
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &agent).unwrap();
    tx.commit().unwrap();
    db.execute("UPDATE runs SET ended_at=2000", []).unwrap();
    assert_eq!(
        read::agent(&db, &agent.id, 2000)
            .unwrap()
            .activity
            .lifecycle,
        LifecycleState::Exited
    );
    assert!(
        read::agents(
            &db,
            &read::AgentFilter {
                lifecycle: Some(LifecycleState::Running),
                ..Default::default()
            },
            2000
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        read::agents(
            &db,
            &read::AgentFilter {
                lifecycle: Some(LifecycleState::Exited),
                ..Default::default()
            },
            2000
        )
        .unwrap()
        .len(),
        1
    );
    let tx = db.transaction().unwrap();
    assert_eq!(read::status(&tx, 2000).unwrap().active_agent_count, 0);
}

#[test]
fn historical_attention_fixtures_match_native_snapshot_queries() {
    let fixtures: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/attention.json")).unwrap();
    for case in fixtures {
        let mut db = open_writer(":memory:").unwrap();
        let mut agent = fixture();
        agent.activity.lifecycle = serde_json::from_value(case["lifecycle"].clone()).unwrap();
        agent.activity.activity = serde_json::from_value(case["activity"].clone()).unwrap();
        agent.activity.updated_at = case["updatedAt"].as_i64().unwrap();
        agent.activity.active_tool = case["activeTool"].as_str().map(str::to_owned);
        agent.activity.active_tool_started_at = case["activeToolStartedAt"].as_i64();
        agent.activity.rate_limit_reset_at = case["rateLimitResetAt"].as_i64();
        agent.activity.retry_after_seconds = case["retryAfterSeconds"].as_i64();
        agent.environment.last_heartbeat_at = case["environmentLastHeartbeatAt"].as_i64().unwrap();
        if agent.activity.lifecycle == LifecycleState::Exited {
            agent.activity.current_run = None;
        }
        let tx = db.transaction().unwrap();
        write::save_agent(&tx, &agent).unwrap();
        let status = read::status(&tx, case["nowMs"].as_i64().unwrap()).unwrap();
        assert_eq!(
            status.requires_attention_count > 0,
            case["expectedRequiresAttention"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn filters_groups_environment_updates_and_missing_agent_errors() {
    let mut db = open_writer(":memory:").unwrap();
    let mut first = fixture();
    first.activity.cost = Some(CostInfo {
        currency: "USD".into(),
        total: 1.5,
        is_estimated: false,
    });
    let mut second = first.clone();
    second.id = "second".into();
    second.environment.id = "other".into();
    second.environment.label = "Other".into();
    second.activity.current_run = None;
    second.activity.updated_at = 2000;
    second.activity.activity = ActivityState::Idle;
    second.is_hidden = true;
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &first).unwrap();
    write::save_agent(&tx, &second).unwrap();
    write::heartbeat(&tx, "env", 3000).unwrap();
    write::dropped_frames(&tx, "env", 3).unwrap();
    write::dropped_frames(&tx, "env", 2).unwrap();
    write::save_cursor(&tx, "env", 20).unwrap();
    write::save_cursor(&tx, "env", 0).unwrap();
    assert!(matches!(
        write::set_visibility(&tx, "missing", true, None, 1),
        Err(agent_status_store::Error::NotFound)
    ));
    assert!(matches!(
        write::dismiss_completion(&tx, "missing"),
        Err(agent_status_store::Error::NotFound)
    ));
    write::end_current_run(&tx, "missing", 1).unwrap();
    write::end_current_run(&tx, "second", 1).unwrap();
    tx.commit().unwrap();
    assert_eq!(read::last_sequence(&db, "env").unwrap(), 0);
    assert_eq!(
        db.query_row(
            "SELECT dropped_frames FROM environments WHERE id='env'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        5
    );
    assert_eq!(
        read::environments(&db, 4000).unwrap()[0].last_heartbeat_at,
        3000
    );
    assert!(
        read::environments(&db, 40000)
            .unwrap()
            .iter()
            .all(|e| e.is_stale)
    );
    let filter = read::AgentFilter {
        project: Some("project".into()),
        environment: Some("env".into()),
        lifecycle: Some(LifecycleState::Running),
        activity: Some(ActivityState::Busy),
        hidden: Some(false),
    };
    assert_eq!(read::agents(&db, &filter, 4000).unwrap().len(), 1);
    let groups = read::projects(&db, 4000).unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].agents[0].id, "second");
    assert_eq!(groups[0].active_agent_count, 1);
    assert_eq!(groups[0].total_cost, 3.0);
    assert!(matches!(
        read::agent(&db, "missing", 4000),
        Err(agent_status_store::Error::NotFound)
    ));
    assert!(matches!(
        read::usage(&db, "missing", 4000),
        Err(agent_status_store::Error::NotFound)
    ));
    first.environment.platform = "darwin".into();
    first.environment.user = "new-user".into();
    first.activity.first_started_at = 9999;
    first.activity.current_run.as_mut().unwrap().ended_at = Some(5000);
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &first).unwrap();
    tx.commit().unwrap();
    let stored = read::agent(&db, &first.id, 4000).unwrap();
    assert_eq!(stored.environment.platform, "darwin");
    assert_eq!(stored.environment.user, "new-user");
    assert_eq!(stored.activity.first_started_at, 900);
    assert_eq!(stored.activity.lifecycle, LifecycleState::Exited);
    let tx = db.transaction().unwrap();
    write::prune(&tx, 0, 3000).unwrap();
    tx.commit().unwrap();
    assert!(
        read::agents(&db, &Default::default(), 4000)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn events_paginate_filter_metadata_and_preserve_usage_after_log_pruning() {
    let mut db = open_writer(":memory:").unwrap();
    let agent = fixture();
    let tx = db.transaction().unwrap();
    write::save_agent(&tx, &agent).unwrap();
    let mut ids = Vec::new();
    for (index, (event, tool, text)) in [
        ("tool_call", "Skill", ""),
        (
            "turn_start",
            "",
            "<command-name>/review</command-name>\n<command-message>review</command-message>",
        ),
        ("tool_call", "mcp__github__list_issues", ""),
    ]
    .into_iter()
    .enumerate()
    {
        let frame = TelemetryFrame {
            agent_id: agent.id.clone(),
            harness: agent.harness.clone(),
            subagent_id: Some(format!("sub{index}")),
            event_type: event.into(),
            timestamp: 1000 + index as i64,
            data: EventData {
                tool_name: Some(tool.into()),
                tool_call_id: Some(format!("call{index}")),
                content: Some(text.into()),
                tool_input: Some(serde_json::Map::from_iter([(
                    "skill".into(),
                    "review".into(),
                )])),
                ..Default::default()
            },
            ..Default::default()
        };
        ids.push(write::record_event(&tx, &frame).unwrap());
    }
    tx.commit().unwrap();
    let all = read::events(&db, &agent.id, 0, 0, None, 2000).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].seq, ids[2]);
    assert_eq!(all[0].pid, 42);
    assert_eq!(all[0].project, agent.project);
    assert_eq!(
        read::events(&db, &agent.id, 1, ids[2], None, 2000).unwrap()[0].seq,
        ids[1]
    );
    assert_eq!(
        read::events(&db, &agent.id, 10000, 0, Some("sub0"), 2000)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        read::events(&db, &agent.id, 10, 0, Some(""), 2000)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        read::distinct_subagents(&db, &agent.id).unwrap(),
        ["sub0", "sub1", "sub2"]
    );
    let report = read::usage(&db, &agent.id, 2000).unwrap();
    assert_eq!(report.skills[0].name, "review");
    assert_eq!(report.commands[0].name, "/review");
    assert_eq!(report.mcp[0].server, "github");
    assert_eq!(report.mcp[0].name, "list_issues");
    assert_eq!(report.mcp[0].operation, "invoke");
    db.execute(
        "UPDATE telemetry_events SET data_json='' WHERE id=?",
        [ids[0]],
    )
    .unwrap();
    assert_eq!(
        read::events(&db, &agent.id, 10, 0, Some("sub0"), 2000).unwrap()[0].data,
        EventData::default()
    );
    db.execute(
        "UPDATE telemetry_events SET data_json='{' WHERE id=?",
        [ids[0]],
    )
    .unwrap();
    assert!(matches!(
        read::events(&db, &agent.id, 10, 0, None, 2000),
        Err(agent_status_store::Error::Json(_))
    ));
    db.execute("UPDATE agents SET active_subagents='{'", [])
        .unwrap();
    assert!(matches!(
        read::agent(&db, &agent.id, 2000),
        Err(agent_status_store::Error::Json(_))
    ));
}

#[test]
fn literal_paths_missing_readers_and_filesystem_failures_are_native_errors() {
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("literal?mode=ro#state.sqlite");
    let db = open_writer(&path).unwrap();
    assert!(path.exists());
    assert!(open_reader(&path).is_ok());
    assert!(matches!(
        open_reader(dir.path().join("missing")),
        Err(agent_status_store::Error::Sql(_))
    ));
    assert!(!dir.path().join("missing").exists());
    drop(db);
    assert!(matches!(
        open_writer(path.join("not-a-directory/state")),
        Err(agent_status_store::Error::Io(_))
    ));
}
