use agent_debug::{cli, output::Format};
use cli_command_kit::Mode;
use std::{fs, path::PathBuf};
use tokio_util::sync::CancellationToken;

fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 80,
        color: false,
        home: Some(PathBuf::from("/fixture-home")),
    }
}
#[tokio::test]
async fn replay_validate_view_and_fleet_use_native_ingestion_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("fixture.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"cwd\":\"/fixture-home/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n").unwrap();
    let path = transcript.to_str().unwrap();
    for command in ["replay", "validate", "view"] {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = cli::run(
            ["agent-debug", "session", command, path, "--json"],
            &CancellationToken::new(),
            &format(Mode::Agent),
            &mut out,
            &mut err,
        )
        .await;
        assert_eq!(code, 0, "{command}: {}", String::from_utf8_lossy(&err));
        let json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        if command == "replay" {
            assert_eq!(json["sessionId"], "fixture");
            assert!(json["framesProcessed"].as_u64().unwrap() >= 2);
            assert_eq!(json["agent"]["workspace"]["cwd"], "~/repo");
        }
        if command == "validate" {
            assert_eq!(json["passed"], true);
            assert_eq!(json["checks"].as_array().unwrap().len(), 5);
        }
        if command == "view" {
            assert_eq!(json["sessionId"], "fixture");
        }
        assert!(err.is_empty());
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    assert_eq!(
        cli::run(
            [
                "agent-debug",
                "session",
                "validate",
                path,
                "--strict-lifecycle",
                "--json"
            ],
            &CancellationToken::new(),
            &format(Mode::Agent),
            &mut out,
            &mut err
        )
        .await,
        1
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&out).unwrap()["passed"],
        false
    );
    assert_eq!(String::from_utf8(err).unwrap().lines().count(), 1);
}

#[tokio::test]
async fn subject_first_fleet_and_events_preserve_native_database_results() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("pi.jsonl");
    let db = dir.path().join("events.db");
    fs::write(&path, "{\"type\":\"session\",\"cwd\":\"/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n").unwrap();
    for args in [
        vec![
            "session",
            "replay",
            path.to_str().unwrap(),
            "--export-db",
            db.to_str().unwrap(),
            "--json",
        ],
        vec!["fleet", "inspect", path.to_str().unwrap(), "--json"],
        vec![
            "event",
            "list",
            "pi",
            "--db",
            db.to_str().unwrap(),
            "--json",
        ],
        vec![
            "event",
            "inspect",
            "1",
            "--db",
            db.to_str().unwrap(),
            "--json",
        ],
    ] {
        let mut argv = vec!["agent-debug"];
        argv.extend(args);
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            cli::run(
                argv.clone(),
                &CancellationToken::new(),
                &format(Mode::Agent),
                &mut out,
                &mut err
            )
            .await,
            0,
            "{argv:?}: {}",
            String::from_utf8_lossy(&err)
        );
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        if argv[1] == "fleet" {
            assert_eq!(value.as_array().unwrap().len(), 1);
        }
        if argv[1] == "event" && argv[2] == "list" {
            assert!(!value.as_array().unwrap().is_empty());
        }
        if argv[1] == "event" && argv[2] == "inspect" {
            assert_eq!(value["id"], 1);
        }
        assert!(err.is_empty());
    }
}

#[test]
fn server_url_requires_enabled_remote_inventory_before_any_io() {
    assert!(
        cli::parse(
            [
                "agent-debug",
                "agent",
                "list",
                "--all=false",
                "--server-url",
                "http://127.0.0.1:1"
            ],
            &format(Mode::Agent),
            &mut Vec::new()
        )
        .is_err()
    );
}

#[test]
fn native_argument_errors_preserve_details_without_a_usage_screen() {
    for (args, detail) in [
        (
            vec!["agent-debug", "event", "inspect", "0"],
            "invalid value",
        ),
        (
            vec!["agent-debug", "event", "inspect"],
            "required arguments",
        ),
        (vec!["agent-debug", "unknown"], "unrecognized subcommand"),
    ] {
        let error = cli::parse(args, &format(Mode::Agent), &mut Vec::new()).unwrap_err();
        let text = error.to_string();
        assert!(text.contains(detail), "{text}");
        assert!(!text.contains("Usage:"), "{text}");
    }
}

#[tokio::test]
async fn open_ended_replay_does_not_hide_negative_usage_verification_errors() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("negative.jsonl");
    fs::write(&path,"{\"type\":\"session\",\"cwd\":\"/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"assistant\",\"content\":[],\"usage\":{\"input\":-1,\"output\":0,\"totalTokens\":-1}}}\n").unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    assert_eq!(
        cli::run(
            [
                "agent-debug",
                "session",
                "replay",
                path.to_str().unwrap(),
                "--strict-lifecycle",
                "--json"
            ],
            &CancellationToken::new(),
            &format(Mode::Agent),
            &mut out,
            &mut err
        )
        .await,
        1,
        "{}",
        String::from_utf8_lossy(&out)
    );
    assert!(
        String::from_utf8(err)
            .unwrap()
            .contains("verification failed")
    );
}

#[tokio::test]
async fn native_help_skill_version_and_argument_errors_are_complete() {
    for mode in [Mode::Human, Mode::Agent] {
        for args in [
            vec!["agent-debug", "--help"],
            vec!["agent-debug", "session", "--help"],
            vec!["agent-debug", "help", "agent"],
            vec!["agent-debug", "completion", "--help"],
        ] {
            let mut out = Vec::new();
            let mut err = Vec::new();
            assert_eq!(
                cli::run(
                    args,
                    &CancellationToken::new(),
                    &format(mode),
                    &mut out,
                    &mut err
                )
                .await,
                0
            );
            let text = String::from_utf8(out).unwrap();
            if mode == Mode::Agent {
                assert!(text.starts_with("ALERT: Agents must read `AGENT=1 agent-debug skill`"));
            }
            assert!(err.is_empty());
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            cli::run(
                ["agent-debug", "skill"],
                &CancellationToken::new(),
                &format(mode),
                &mut out,
                &mut err
            )
            .await,
            0
        );
        assert_eq!(String::from_utf8(out).unwrap(), cli::SKILL_POINTER);
    }
    for args in [
        vec!["agent-debug", "skill", "extra"],
        vec!["agent-debug", "session", "replay"],
        vec!["agent-debug", "agent", "list", "--all", "--db", "state.db"],
        vec![
            "agent-debug",
            "session",
            "replay",
            "missing",
            "--harness",
            "invalid",
        ],
        vec![
            "agent-debug",
            "session",
            "replay",
            "missing",
            "--until-line",
            "-1",
        ],
        vec!["agent-debug", "events"],
    ] {
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            cli::run(
                args,
                &CancellationToken::new(),
                &format(Mode::Agent),
                &mut out,
                &mut err
            )
            .await,
            1
        );
        assert!(out.is_empty());
        assert!(!err.is_empty());
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    assert_eq!(
        cli::run(
            ["agent-debug", "--version"],
            &CancellationToken::new(),
            &format(Mode::Agent),
            &mut out,
            &mut err
        )
        .await,
        0
    );
    assert_eq!(out, b"0.1.0\n");
    assert!(err.is_empty());
}
