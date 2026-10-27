use agent_common::types::HarnessType;
use agent_debug::audit::{AuditFilter, PairingStatus, ProcessInfo, SessionInfo, audit, match_cwd};
use std::path::PathBuf;

fn process(pid: i32, at: i64) -> ProcessInfo {
    ProcessInfo {
        pid,
        created_at: at,
        cwd: "/repo".into(),
        cmdline: "pi --session x".into(),
        memory_mb: 4.0,
        cpu_percent: 12.5,
    }
}
fn session(id: &str, at: i64, pid: i32) -> SessionInfo {
    SessionInfo {
        harness: HarnessType::Pi,
        session_id: id.into(),
        cwd: "/repo".into(),
        file_path: PathBuf::from(format!("/sessions/{id}.jsonl")),
        header_time: at,
        harness_version: "1".into(),
        pid,
    }
}

#[test]
fn pairs_closest_unused_session_and_keeps_orphans_and_metadata() {
    let processes = vec![(
        HarnessType::Pi,
        vec![process(1, 100_000), process(2, 99_000), process(3, 500_000)],
    )];
    let sessions = vec![
        session("early", 95_001, 0),
        session("closest", 100_100, 0),
        session("too-old", 1, 0),
    ];
    let records = audit(&processes, &sessions, &AuditFilter::default(), 4_000_000);
    assert_eq!(records.len(), 4);
    assert_eq!(records[0].session_id, "closest");
    assert_eq!(records[0].delta_ms, Some(100));
    assert_eq!(records[0].memory_mb, 4.0);
    assert_eq!(records[0].cpu_percent, 12.5);
    assert_eq!(records[0].status_details, "IDLE 1h");
    assert_eq!(records[1].session_id, "early");
    assert_eq!(records[2].status, PairingStatus::OrphanProcess);
    assert_eq!(records[3].status, PairingStatus::OrphanSession);
}

#[test]
fn claude_does_not_pair_reused_pid_and_prefers_newest_descriptor() {
    let mut old = session("old", 100_000, 9);
    old.harness = HarnessType::ClaudeCode;
    let mut recent = session("new", 200_001, 9);
    recent.harness = HarnessType::ClaudeCode;
    let mut other = recent.clone();
    other.session_id = "duplicate".into();
    other.header_time = 200_000;
    let groups = vec![(HarnessType::ClaudeCode, vec![process(9, 200_000)])];
    let records = audit(
        &groups,
        &[old, other, recent],
        &AuditFilter::default(),
        200_100,
    );
    assert_eq!(records[0].status, PairingStatus::Paired);
    assert_eq!(records[0].session_id, "new");
    assert_eq!(
        records
            .iter()
            .filter(|r| r.status == PairingStatus::Paired)
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .find(|r| r.session_id == "old")
            .unwrap()
            .status,
        PairingStatus::OrphanSession
    );
    let mut stale = session("reused", 100_000, 9);
    stale.harness = HarnessType::ClaudeCode;
    let records = audit(&groups, &[stale], &AuditFilter::default(), 300_000);
    assert!(records.iter().all(|r| r.status != PairingStatus::Paired));
}

#[test]
fn filters_harness_aliases_and_whole_path_segments() {
    assert!(match_cwd(
        std::path::Path::new("/repo/sub/../sub"),
        Some(std::path::Path::new("/repo"))
    ));
    assert!(!match_cwd(
        std::path::Path::new("/repository"),
        Some(std::path::Path::new("/repo"))
    ));
    let mut claude = session("c", 1000, 1);
    claude.harness = HarnessType::ClaudeCode;
    let groups = vec![
        (HarnessType::ClaudeCode, vec![process(1, 1000)]),
        (HarnessType::Pi, vec![process(2, 1000)]),
    ];
    let records = audit(
        &groups,
        &[claude, session("p", 1000, 0)],
        &AuditFilter {
            harness: Some(HarnessType::ClaudeCode),
            cwd: Some("/repo".into()),
        },
        1000,
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].session_id, "c");
    assert_eq!(records[0].status_details, "ACTIVE");
    assert!(
        audit(
            &groups,
            &[],
            &AuditFilter {
                cwd: Some("/absent".into()),
                ..Default::default()
            },
            1000
        )
        .is_empty()
    );
}
