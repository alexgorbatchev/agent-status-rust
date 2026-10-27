use agent_common::{
    types::{CostInfo, HarnessType, RunInfo},
    usage::{Entry, Evidence, Report},
};
use agent_debug::{
    audit::{self, AuditFilter, SessionInfo},
    comparison,
    database::{RunRecord, SessionRecord},
    output::{Format, SessionSummary},
    replay::{self, Options},
    validation,
};
use cli_command_kit::Mode;
use std::fs;
use tokio_util::sync::CancellationToken;

fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 100,
        color: false,
        home: Some("/fixture".into()),
    }
}

#[tokio::test]
async fn reports_render_real_replay_inspection_diff_usage_and_audit_in_both_modes() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("session.jsonl");
    fs::write(&path,"{\"type\":\"session\",\"cwd\":\"/fixture/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"usage\":{\"input\":10,\"output\":2,\"totalTokens\":12}}}\n").unwrap();
    let mut result = replay::run(
        &CancellationToken::new(),
        &path,
        &Options {
            diff: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let agent = result.agent.as_mut().unwrap();
    agent.activity.action = "Working".into();
    agent.activity.title = "Repair".into();
    agent.activity.subagent_count = 2;
    agent.activity.cost = Some(CostInfo {
        total: 0.25,
        ..Default::default()
    });
    agent.activity.current_run = Some(RunInfo {
        id: "run".into(),
        pid: 42,
        ..Default::default()
    });
    let record = SessionRecord {
        db_path: "/fixture/state.db".into(),
        agent: agent.clone(),
        runs: vec![
            RunRecord {
                id: "run".into(),
                agent_id: agent.id.clone(),
                pid: 42,
                harness_version: "3".into(),
                started_at: 1000,
                ended_at: None,
            },
            RunRecord {
                id: "old".into(),
                agent_id: agent.id.clone(),
                pid: 41,
                harness_version: "2".into(),
                started_at: 500,
                ended_at: Some(900),
            },
        ],
        event_count: 5,
        first_event_timestamp: Some(1000),
        last_event_timestamp: Some(2000),
    };
    let mut comparison = comparison::compare(agent, result.last_event_timestamp, &record);
    comparison.warnings.push("inspect /fixture/state.db".into());
    let report = validation::Report::replay(path, &result);
    let mut usage = Report::new("agent", HarnessType::Pi);
    let entry = Entry {
        name: "skill".into(),
        server: "server".into(),
        operation: "invoke".into(),
        count: 2,
        first_used_at: 1000,
        last_used_at: 2000,
        latest_evidence: Evidence {
            source: "/fixture/SKILL.md".into(),
            ..Default::default()
        },
    };
    usage.skills.push(entry.clone());
    usage.commands.push(Entry {
        name: "/command".into(),
        ..entry.clone()
    });
    usage.mcp.push(Entry {
        name: "tool".into(),
        ..entry
    });
    let audits = audit::audit(
        &[],
        &[SessionInfo {
            harness: HarnessType::Pi,
            session_id: "session".into(),
            cwd: "/fixture/repo".into(),
            ..Default::default()
        }],
        &AuditFilter::default(),
        3000,
    );
    let sessions = [SessionSummary {
        id: "session".into(),
        title: String::new(),
        directory: "/fixture/repo".into(),
        created_at: chrono::DateTime::from_timestamp_millis(1000).unwrap(),
        cost_usd: 0.25,
        tokens: 12,
    }];
    for mode in [Mode::Human, Mode::Agent] {
        let f = format(mode);
        let mut out = Vec::new();
        f.inspect(&mut out, &record).unwrap();
        f.replay_summary(&mut out, &result).unwrap();
        f.validation(&mut out, &report).unwrap();
        f.comparison(&mut out, &comparison).unwrap();
        f.usage(&mut out, &usage).unwrap();
        f.audit(&mut out, &audits).unwrap();
        f.session_list(&mut out, &sessions).unwrap();
        for diff in &result.diffs {
            f.diff(&mut out, diff).unwrap();
        }
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("~/repo"));
        assert!(text.contains("~/state.db"));
        assert!(text.contains("Repair"));
        assert!(text.contains("ORPHAN_SESSION"));
        assert!(text.contains("(untitled)"));
        assert!(text.contains("skill"));
        assert!(text.contains("synthetic"));
        assert!(!text.contains("/fixture/"));
        let mut empty = Vec::new();
        f.audit(&mut empty, &[]).unwrap();
        f.session_list(&mut empty, &[]).unwrap();
        f.usage(&mut empty, &Report::new("empty", HarnessType::Pi))
            .unwrap();
        let empty = String::from_utf8(empty).unwrap();
        assert!(empty.contains(if mode == Mode::Agent {
            "count:0"
        } else {
            "No processes"
        }));
        result.verification.passed = false;
        let mut failed = Vec::new();
        f.replay_summary(&mut failed, &result).unwrap();
        assert!(
            String::from_utf8(failed)
                .unwrap()
                .contains(if mode == Mode::Agent {
                    "verification:FAIL"
                } else {
                    "verification failed"
                })
        );
        result.verification.passed = true;
    }
}
