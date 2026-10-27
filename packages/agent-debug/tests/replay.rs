use agent_common::types::{ActivityState, LifecycleState};
use agent_debug::{
    replay::{self, Options},
    transcript::LoadOptions,
};
use std::fs;
use tokio_util::sync::CancellationToken;

fn fixture(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("pi.jsonl");
    fs::write(&path, "{\"type\":\"session\",\"id\":\"s\",\"cwd\":\"/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:02Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"done\"}],\"usage\":{\"input\":10,\"output\":2,\"totalTokens\":12}}}\n").unwrap();
    path
}

#[tokio::test]
async fn replay_commits_real_native_ingestion_and_read_queries() {
    let temp = tempfile::tempdir().unwrap();
    let path = fixture(temp.path());
    let export = temp.path().join("export.db");
    let result = replay::run(
        &CancellationToken::new(),
        &path,
        &Options {
            diff: true,
            export_db: Some(export.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(result.verification.passed, "{:?}", result.verification);
    assert_eq!(
        result.agent.as_ref().unwrap().activity.lifecycle,
        LifecycleState::Exited
    );
    assert!(result.has_synthetic_end);
    assert!(!result.has_native_end);
    assert_eq!(result.diffs.len(), result.frames_processed);
    assert!(
        result.diffs[0]
            .mutations
            .iter()
            .any(|m| m.field == "Lifecycle")
    );
    let db = agent_status_store::open_reader(&export).unwrap();
    let stored = agent_status_store::read::agent(
        &db,
        &result.agent_id,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap();
    assert_eq!(stored.activity.usage, result.agent.unwrap().activity.usage);
    assert_eq!(result.verification.log_count, result.frames_processed);
}

#[tokio::test]
async fn early_breakpoints_report_only_processed_lifecycle_and_timestamps() {
    let temp = tempfile::tempdir().unwrap();
    let path = fixture(temp.path());
    let result = replay::run(
        &CancellationToken::new(),
        &path,
        &Options {
            until_line: Some(1),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!result.has_synthetic_end);
    assert!(!result.has_native_end);
    assert_eq!(result.last_event_timestamp, 1791720000000);
    assert_eq!(
        result.agent.unwrap().activity.lifecycle,
        LifecycleState::Running
    );
    let strict = replay::run(
        &CancellationToken::new(),
        &path,
        &Options {
            load: LoadOptions {
                strict_lifecycle: true,
                ..Default::default()
            },
            until_event: Some("run_started".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!strict.verification.passed);
    assert!(!strict.has_synthetic_start);
    assert!(!strict.has_synthetic_end);
}

#[tokio::test]
async fn parsing_errors_are_visible_and_stop_on_error_is_fatal() {
    let temp = tempfile::tempdir().unwrap();
    let path = fixture(temp.path());
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("broken\n");
    fs::write(&path, text).unwrap();
    let cancel = CancellationToken::new();
    let result = replay::run(&cancel, &path, &Options::default())
        .await
        .unwrap();
    assert!(!result.errors.is_empty());
    assert!(
        replay::run(
            &cancel,
            &path,
            &Options {
                stop_on_error: true,
                ..Default::default()
            }
        )
        .await
        .is_err()
    );
    assert!(
        replay::run_observed(&cancel, &path, &Options::default(), |_, _, _| Err(
            std::io::Error::other("output closed")
        ))
        .await
        .is_err()
    );
    cancel.cancel();
    assert!(
        replay::run(&cancel, &path, &Options::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rate_limited_is_a_valid_native_activity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("codex.jsonl");
    fs::write(&path, "{\"type\":\"session_meta\",\"timestamp\":\"2026-10-11T12:00:00Z\",\"payload\":{\"id\":\"s\",\"cwd\":\"/repo\"}}\n{\"type\":\"event_msg\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"payload\":{\"type\":\"error\",\"message\":\"Rate limit exceeded\",\"codex_error_info\":\"RateLimitExceeded\"}}\n").unwrap();
    let result = replay::run(
        &CancellationToken::new(),
        &path,
        &Options {
            until_event: Some("error".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        result.agent.unwrap().activity.activity,
        ActivityState::RateLimited
    );
    assert!(
        result
            .verification
            .checks
            .iter()
            .find(|c| c.name == "Activity invariant")
            .unwrap()
            .passed
    );
}
