use agent_common::{
    types::{AgentState, EnvironmentInfo, LifecycleState, RunInfo},
    usage::Report,
};
use agent_debug::{cli, output::Format};
use axum::{Router, routing::get};
use cli_command_kit::Mode;
use std::{
    fs,
    io::{self, Write},
};
use tokio_util::sync::CancellationToken;

fn format(mode: Mode) -> Format {
    Format {
        mode,
        width: 100,
        color: false,
        home: Some("/fixture".into()),
    }
}
async fn invoke(args: Vec<String>, mode: Mode) -> (i32, String, String) {
    let mut argv = vec!["agent-debug".to_owned()];
    argv.extend(args);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = cli::run(
        argv,
        &CancellationToken::new(),
        &format(mode),
        &mut out,
        &mut err,
    )
    .await;
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}
#[tokio::test]
async fn remote_agent_inventory_and_usage_render_real_server_responses() {
    let mut a = AgentState {
        session_id: "remote-session".into(),
        environment: EnvironmentInfo {
            id: "foreign-fixture".into(),
            hostname: "fixture-host".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    a.activity.lifecycle = LifecycleState::Running;
    a.activity.current_run = Some(RunInfo {
        pid: 42,
        ..Default::default()
    });
    a.workspace.cwd = "/fixture/repo".into();
    let router = Router::new()
        .route(
            "/api/v1/agents",
            get(move || {
                let a = a.clone();
                async move { axum::Json(vec![a]) }
            }),
        )
        .route(
            "/api/v1/agents/{id}/usage",
            get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    axum::Json(Report::new(id, agent_common::types::HarnessType::Pi))
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for mode in [Mode::Human, Mode::Agent] {
        let (code, out, err) = invoke(
            vec![
                "agent".into(),
                "list".into(),
                "--all".into(),
                "--server-url".into(),
                base.clone(),
            ],
            mode,
        )
        .await;
        assert_eq!(code, 0, "{err}");
        assert!(err.is_empty());
        assert!(out.contains("42\t"));
        assert!(out.contains("remote-session\tfixture-host\t~/repo"));
        for json in [false, true] {
            let mut args = vec![
                "agent".into(),
                "usage".into(),
                "env:pi:session".into(),
                "--server-url".into(),
                base.clone(),
            ];
            if json {
                args.push("--json".into());
            }
            let (code, out, err) = invoke(args, mode).await;
            assert_eq!(code, 0, "{err}");
            assert!(err.is_empty());
            if json {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&out).unwrap()["agentId"],
                    "env:pi:session"
                );
            } else {
                assert!(out.contains("coverage"));
            }
        }
    }
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn offline_database_commands_compare_exported_state_and_render_failures_once() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("pi.jsonl");
    let db = dir.path().join("status.db");
    fs::write(&path,"{\"type\":\"session\",\"cwd\":\"/fixture/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n").unwrap();
    let path = path.to_str().unwrap();
    let db = db.to_str().unwrap();
    assert_eq!(
        invoke(
            ["session", "replay", path, "--export-db", db, "--json"]
                .map(str::to_owned)
                .to_vec(),
            Mode::Agent
        )
        .await
        .0,
        0
    );
    for mode in [Mode::Human, Mode::Agent] {
        for args in [
            vec!["db", "inspect", "pi", "--db", db],
            vec!["db", "inspect", "pi", "--db", db, "--json"],
            vec!["fleet", "inspect", "--db", db],
            vec!["fleet", "inspect", db, "--json"],
            vec!["session", "replay", path, "--diff", "--db", db],
            vec!["session", "validate", path, "--db", db],
            vec!["session", "view", path],
            vec![
                "event",
                "list",
                "pi",
                "--db",
                db,
                "--type",
                "run_started",
                "--asc",
                "--limit",
                "0",
            ],
            vec!["event", "inspect", "1", "--db", db],
            vec!["agent", "list", "--db", db],
        ] {
            let (code, out, err) = invoke(args.iter().map(|s| s.to_string()).collect(), mode).await;
            assert_eq!(code, 0, "{args:?}: {err}\n{out}");
            assert!(err.is_empty());
        }
        for args in [
            vec!["event", "inspect", "99999", "--db", db],
            vec!["session", "validate", path, "--db", "absent.db"],
            vec!["session", "validate", "absent.jsonl", "--json"],
        ] {
            let (code, _, err) = invoke(args.iter().map(|s| s.to_string()).collect(), mode).await;
            assert_eq!(code, 1, "{args:?}");
            assert_eq!(err.lines().count(), 1, "{err}");
        }
    }
}

#[test]
fn framework_metadata_and_console_writes_propagate_failures() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
    }
    for args in [
        vec!["agent-debug", "skill"],
        vec!["agent-debug", "--version"],
        vec!["agent-debug", "--help"],
        vec!["agent-debug", "completion", "bash"],
    ] {
        assert!(cli::parse(args, &format(Mode::Agent), &mut Broken).is_err());
    }
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let mut out = Vec::new();
        assert!(
            cli::parse(
                ["agent-debug", "completion", shell],
                &format(Mode::Agent),
                &mut out
            )
            .unwrap()
            .is_none()
        );
        assert!(String::from_utf8(out).unwrap().contains("agent-debug"));
    }
    let mut console = cli::Console::new(Vec::new(), format(Mode::Agent));
    let record = b"/fixture/repo\x1b[2J";
    assert_eq!(console.write(record).unwrap(), record.len());
    console.flush().unwrap();
    assert_eq!(
        console.write(&[0xff]).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    let mut console = cli::Console::new(Broken, format(Mode::Human));
    assert!(console.write(b"message").is_err());
    assert!(console.flush().is_err());
}
