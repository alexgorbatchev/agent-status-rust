use agent_common::types::HarnessType;
use agent_debug::transcript::detect_harness;
use agent_debug::{
    Error,
    transcript::{
        LoadOptions, ReplayEvent, SearchRoot, ensure_lifecycle, load_session, resolve_target,
    },
};
use agent_parser::ParsedEvent;
use std::fs;
use tokio_util::sync::CancellationToken;

#[test]
fn detects_structured_metadata_independent_of_json_whitespace() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp")
        .join(format!("transcript-initial-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("session.jsonl");
    fs::write(
        &path,
        "{\"type\" : \"session\", \"id\": \"pi-session\", \"version\": 3, \"cwd\": \"/project\"}\n",
    )
    .unwrap();
    assert_eq!(detect_harness(&path).unwrap().as_str(), "pi");
    fs::write(
        &path,
        "{\"type\" : \"session_meta\", \"payload\": {\"id\": \"codex-session\"}}\n",
    )
    .unwrap();
    assert_eq!(detect_harness(&path).unwrap().as_str(), "codex");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reports_invalid_input_and_retains_line_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("pi.jsonl");
    fs::write(&path, "\n{\"type\":\"session\",\"id\":\"p\",\"cwd\":\"/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\ninvalid json\n").unwrap();
    let loaded = load_session(&CancellationToken::new(), &path, &LoadOptions::default()).unwrap();
    assert_eq!(loaded.harness, HarnessType::Pi);
    assert_eq!(loaded.session_id, "pi");
    assert_eq!(loaded.events[0].event.event_type, "run_started");
    assert_eq!(loaded.events.last().unwrap().event.event_type, "run_exited");
    assert!(loaded.diagnostics[0].contains(":3:"));
    let empty = temp.path().join("empty");
    fs::write(&empty, "\n").unwrap();
    assert!(matches!(
        detect_harness(&empty),
        Err(Error::UnknownHarness(_))
    ));
    fs::write(&empty, "unknown").unwrap();
    assert!(detect_harness(&empty).is_err());
    assert!(detect_harness(&temp.path().join("missing")).is_err());
    fs::write(&empty, vec![b'a'; 20 * 1024 * 1024 + 1]).unwrap();
    assert!(matches!(
        detect_harness(&empty),
        Err(Error::InvalidInput(_))
    ));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        load_session(&cancelled, &path, &LoadOptions::default()),
        Err(Error::Cancelled)
    ));
}

#[test]
fn claude_interleaves_deduplicated_subagents_and_keeps_native_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    fs::write(&path, "{\"type\":\"ai-title\",\"title\":\"Early title\"}\n{\"type\":\"user\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"cwd\":\"/old\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n").unwrap();
    for directory in [
        temp.path().join("session/subagents"),
        temp.path().join("subagents"),
    ] {
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("worker.jsonl"), "{\"type\":\"assistant\",\"timestamp\":\"2026-10-11T12:00:02Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n").unwrap();
        fs::write(directory.join("ignored.txt"), "bad").unwrap();
    }
    let loaded = load_session(
        &CancellationToken::new(),
        &path,
        &LoadOptions {
            cwd: Some("/new/../repo".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.subagents, ["worker"]);
    assert!(
        loaded
            .events
            .iter()
            .any(|e| e.subagent_id.as_deref() == Some("worker"))
    );
    assert!(loaded.events.iter().all(|e| e.event.cwd == "/repo"));
    assert!(
        loaded
            .events
            .windows(2)
            .all(|e| e[0].event.timestamp <= e[1].event.timestamp)
    );
    let strict = load_session(
        &CancellationToken::new(),
        &path,
        &LoadOptions {
            strict_lifecycle: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(strict.events.iter().all(|e| !e.is_synthetic));
}

#[test]
fn codex_parser_state_survives_lines() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    fs::write(&path, "{\"type\":\"session_meta\",\"timestamp\":\"2026-06-12T16:08:43Z\",\"payload\":{\"id\":\"s\",\"cwd\":\"/repo\"}}\n{\"type\":\"event_msg\",\"timestamp\":\"2026-06-12T16:08:44Z\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"total_tokens\":100,\"prompt_tokens\":80,\"completion_tokens\":20},\"last_token_usage\":{\"total_tokens\":100,\"prompt_tokens\":80,\"completion_tokens\":20}},\"rate_limits\":{\"remaining_tokens\":0,\"reset_tokens\":\"60s\"}}}\n{\"type\":\"event_msg\",\"timestamp\":\"2026-06-12T16:08:45Z\",\"payload\":{\"type\":\"error\",\"message\":\"Rate limit exceeded\",\"codex_error_info\":\"RateLimitExceeded\"}}\n").unwrap();
    let loaded = load_session(&CancellationToken::new(), &path, &LoadOptions::default()).unwrap();
    assert_eq!(
        loaded
            .events
            .iter()
            .find(|e| e.event.event_type == "error")
            .unwrap()
            .event
            .data
            .retry_after_seconds,
        Some(59)
    );
}

#[test]
fn resolves_files_directories_ids_and_rejects_unknowns() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("sessions");
    fs::create_dir(&root).unwrap();
    let path = root.join("a-session-token.jsonl");
    fs::write(&path, "{\"type\":\"model_change\"}\n").unwrap();
    let exact = root.join("session-token.jsonl");
    fs::write(&exact, "{\"type\":\"session\",\"cwd\":\"/repo\"}\n").unwrap();
    let roots = [SearchRoot {
        harness: HarnessType::Pi,
        path: root.clone(),
    }];
    let cancel = CancellationToken::new();
    assert_eq!(
        resolve_target(&cancel, std::path::Path::new("session-token"), None, &roots)
            .unwrap()
            .file_path,
        exact
    );
    assert_eq!(
        resolve_target(
            &cancel,
            std::path::Path::new("a-session-token"),
            None,
            &roots
        )
        .unwrap()
        .file_path,
        path
    );
    assert_eq!(
        resolve_target(&cancel, &root, None, &roots)
            .unwrap()
            .file_path,
        path
    );
    assert_eq!(
        resolve_target(&cancel, &path, Some(HarnessType::Codex), &roots)
            .unwrap()
            .harness,
        HarnessType::Codex
    );
    assert!(resolve_target(&cancel, std::path::Path::new("missing"), None, &roots).is_err());
    assert!(resolve_target(&cancel, std::path::Path::new(""), None, &roots).is_err());
    assert!(
        resolve_target(
            &cancel,
            std::path::Path::new("session-token"),
            Some(HarnessType::ClaudeCode),
            &roots
        )
        .is_err()
    );
    let event = |kind: &str, subagent_id| ReplayEvent {
        event: ParsedEvent {
            event_type: kind.into(),
            timestamp: 1000,
            ..Default::default()
        },
        source_line: 1,
        subagent_id,
        is_synthetic: false,
    };
    let mut events = vec![
        event("run_started", Some("worker".into())),
        event("run_exited", Some("worker".into())),
    ];
    ensure_lifecycle(&mut events, 500, 2000, temp.path(), false).unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].event.timestamp, 500);
    assert_eq!(events[3].event.timestamp, 2000);
    let mut native = vec![event("run_started", None), event("run_exited", None)];
    ensure_lifecycle(&mut native, 0, 0, temp.path(), false).unwrap();
    assert_eq!(native.len(), 2);
    let mut overflow = vec![ReplayEvent {
        event: ParsedEvent {
            timestamp: i64::MAX,
            ..Default::default()
        },
        ..event("thinking", None)
    }];
    assert!(ensure_lifecycle(&mut overflow, 0, 0, temp.path(), false).is_err());
}

#[test]
fn partial_session_lookup_preserves_identity_and_rejects_ambiguity_across_roots() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let path = first.join("canonical-abc-123.jsonl");
    fs::write(&path, "{\"type\":\"session\",\"cwd\":\"/repo\"}\n").unwrap();
    let roots = [
        SearchRoot {
            harness: HarnessType::Pi,
            path: first.clone(),
        },
        SearchRoot {
            harness: HarnessType::ClaudeCode,
            path: second.clone(),
        },
    ];
    let cancel = CancellationToken::new();
    let resolved = resolve_target(&cancel, std::path::Path::new("abc"), None, &roots).unwrap();
    let loaded = load_session(
        &cancel,
        &resolved.file_path,
        &LoadOptions {
            harness: Some(resolved.harness),
            session_id: resolved.session_id,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.session_id, "canonical-abc-123");
    fs::write(
        second.join("different-abc-456.jsonl"),
        "{\"type\":\"user\"}\n",
    )
    .unwrap();
    assert!(resolve_target(&cancel, std::path::Path::new("abc"), None, &roots).is_err());
    let exact = second.join("abc.jsonl");
    fs::write(&exact, "{\"type\":\"user\"}\n").unwrap();
    assert_eq!(
        resolve_target(&cancel, std::path::Path::new("abc"), None, &roots)
            .unwrap()
            .file_path,
        exact
    );
}

#[cfg(unix)]
#[test]
fn lifecycle_rejects_lossy_native_working_directory() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, path::PathBuf};
    let cwd = PathBuf::from(OsString::from_vec(vec![b'/', b'r', 0xff]));
    let mut events = vec![ReplayEvent {
        event: ParsedEvent {
            event_type: "thinking".into(),
            timestamp: 1000,
            ..Default::default()
        },
        source_line: 1,
        subagent_id: None,
        is_synthetic: false,
    }];
    assert!(ensure_lifecycle(&mut events, 0, 0, &cwd, false).is_err());
    assert_eq!(events.len(), 1, "validation must precede mutation");
}
