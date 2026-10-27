use agent_common::types::HarnessType;
use agent_debug::{
    Error,
    audit::{AuditFilter, PairingStatus, SessionInfo, SessionPaths, audit, capture, sessions},
};
use std::{fs, path::Path};
use tokio_util::sync::CancellationToken;

fn temporary() -> tempfile::TempDir {
    fs::create_dir_all(".tmp").unwrap();
    tempfile::tempdir_in(".tmp").unwrap()
}

fn paths(root: &Path) -> SessionPaths {
    SessionPaths {
        pi: root.join("pi"),
        claude: vec![root.join("claude")],
        codex: root.join("codex"),
        opencode: root.join("missing-opencode.db"),
    }
}

#[test]
fn corrupt_transcript_headers_and_descriptors_do_not_discard_valid_sessions() {
    let temp = temporary();
    let paths = paths(temp.path());
    for (harness, directory, extension, valid) in [
        (
            HarnessType::Pi,
            paths.pi.join("project"),
            "jsonl",
            r#"{"type":"session","id":"pi-retained","timestamp":1758400000000,"cwd":"/repo/pi"}"#,
        ),
        (
            HarnessType::ClaudeCode,
            paths.claude[0].clone(),
            "json",
            r#"{"sessionId":"claude-retained","pid":12345,"startedAt":1758400000000,"cwd":"/repo/claude","harnessVersion":"2"}"#,
        ),
        (
            HarnessType::Codex,
            paths.codex.join("2026/09/21"),
            "jsonl",
            r#"{"type":"session_meta","payload":{"id":"codex-retained","timestamp":"2026-09-21T10:00:00Z","cwd":"/repo/codex","cli_version":"1"}}"#,
        ),
    ] {
        fs::create_dir_all(&directory).unwrap();
        let corrupt_path = directory.join(format!("a-corrupt.{extension}"));
        fs::write(&corrupt_path, b"{bad\n").unwrap();
        let valid_path = directory.join(format!("z-valid.{extension}"));
        fs::write(&valid_path, format!("{valid}\n")).unwrap();
        let inventory = sessions(&CancellationToken::new(), &paths, Some(&harness))
            .expect("one corrupt file must not discard another session");
        assert_eq!(inventory.diagnostics.len(), 1);
        assert_eq!(inventory.diagnostics[0].path, corrupt_path);
        assert!(!inventory.diagnostics[0].error.is_empty());
        let rows = inventory.sessions;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, valid_path);
        assert!(rows[0].session_id.ends_with("-retained"));
        assert_eq!(rows[0].harness, harness);
    }
}

#[test]
fn fractional_pi_header_milliseconds_are_truncated_instead_of_replaced_by_mtime() {
    let temp = temporary();
    let paths = paths(temp.path());
    let directory = paths.pi.join("project");
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("fractional.jsonl"),
        br#"{"type":"session","id":"fractional","timestamp":1758400000000.5,"cwd":"/repo"}"#,
    )
    .unwrap();
    let inventory = sessions(&CancellationToken::new(), &paths, Some(&HarnessType::Pi)).unwrap();
    assert!(inventory.diagnostics.is_empty());
    let rows = inventory.sessions;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].header_time, 1_758_400_000_000);
}

#[test]
fn missing_roots_are_empty_but_real_directory_and_database_failures_are_errors() {
    let temp = temporary();
    let mut paths = paths(temp.path());
    let inventory = sessions(&CancellationToken::new(), &paths, None).unwrap();
    assert!(inventory.sessions.is_empty());
    assert!(inventory.diagnostics.is_empty());
    fs::write(&paths.pi, b"not a directory").unwrap();
    assert!(matches!(
        sessions(&CancellationToken::new(), &paths, Some(&HarnessType::Pi)),
        Err(Error::Io(_))
    ));
    paths.opencode = temp.path().join("invalid.db");
    fs::write(&paths.opencode, b"not a SQLite database").unwrap();
    assert!(
        sessions(
            &CancellationToken::new(),
            &paths,
            Some(&HarnessType::Opencode)
        )
        .is_err()
    );
}

#[test]
fn cancelled_native_capture_and_session_discovery_return_cancellation() {
    let temp = temporary();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(capture(&cancel, 0), Err(Error::Cancelled)));
    assert!(matches!(
        sessions(&cancel, &paths(temp.path()), None),
        Err(Error::Cancelled)
    ));
}

#[test]
fn native_capture_retains_current_process_identity_and_all_harness_groups() {
    let now = chrono::Utc::now().timestamp_millis();
    let captured = capture(&CancellationToken::new(), now).unwrap();
    let pid = i32::try_from(std::process::id()).unwrap();
    let current = captured
        .snapshot
        .processes()
        .find(|process| process.pid == pid)
        .unwrap();
    assert!(current.alive);
    assert_eq!(current.cwd, std::env::current_dir().unwrap());
    assert!(current.create_time > 0 && current.create_time <= now);
    assert!(!current.cmdline.is_empty());
    for harness in [
        HarnessType::Pi,
        HarnessType::ClaudeCode,
        HarnessType::Codex,
        HarnessType::Opencode,
    ] {
        assert_eq!(
            captured
                .groups
                .iter()
                .filter(|(found, _)| found == &harness)
                .count(),
            1
        );
    }
}

#[test]
fn sessions_without_a_process_group_are_retained_as_orphans() {
    let session = SessionInfo {
        harness: HarnessType::Pi,
        session_id: "unpaired-pi".into(),
        cwd: "/repo/session-only".into(),
        file_path: "/sessions/pi.jsonl".into(),
        header_time: 1_758_400_000_000,
        ..Default::default()
    };
    let records = audit(&[], &[session], &AuditFilter::default(), 1_758_400_000_001);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, PairingStatus::OrphanSession);
    assert_eq!(records[0].harness, HarnessType::Pi);
    assert_eq!(records[0].session_id, "unpaired-pi");
    assert_eq!(records[0].cwd, Path::new("/repo/session-only"));
    assert_eq!(records[0].session_file, Path::new("/sessions/pi.jsonl"));
    assert_eq!(records[0].header_time, 1_758_400_000_000);
    assert_eq!(records[0].pid, 0);
}
