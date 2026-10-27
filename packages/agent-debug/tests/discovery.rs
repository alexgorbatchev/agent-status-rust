use agent_common::types::{AgentState, EnvironmentInfo, LifecycleState, RunInfo};
use agent_debug::{audit::SessionPaths, discovery};
use agent_watcher::scanner::{ProcessMeta, ProcessSnapshot};
use serde_json::json;
use std::fs;
use tokio_util::sync::CancellationToken;

fn process(pid: i32, name: &str, cwd: &str, time: i64) -> ProcessMeta {
    ProcessMeta {
        pid,
        name: name.into(),
        cwd: cwd.into(),
        create_time: time,
        alive: true,
        ..Default::default()
    }
}
#[test]
fn monitored_filters_lifecycle_pid_reuse_local_identity_and_remote_freshness() {
    let snapshot = ProcessSnapshot::from_processes([process(42, "pi", "/repo", 100000)]);
    let mut valid = AgentState {
        session_id: "valid".into(),
        environment: EnvironmentInfo {
            id: "local".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    valid.activity.lifecycle = LifecycleState::Running;
    valid.activity.current_run = Some(RunInfo {
        pid: 42,
        started_at: 100000,
        ..Default::default()
    });
    let mut cases = vec![valid.clone()];
    for (name, change) in [
        ("exited", 0),
        ("missing-run", 1),
        ("zero-pid", 2),
        ("ended", 3),
        ("reused", 4),
        ("huge-pid", 5),
        ("remote-stale", 6),
        ("remote-fresh", 7),
        ("dead", 8),
    ] {
        let mut a = valid.clone();
        a.session_id = name.into();
        match change {
            0 => a.activity.lifecycle = LifecycleState::Exited,
            1 => a.activity.current_run = None,
            2 => a.activity.current_run.as_mut().unwrap().pid = 0,
            3 => a.activity.current_run.as_mut().unwrap().ended_at = Some(200000),
            4 => a.activity.current_run.as_mut().unwrap().started_at = 1,
            5 => a.activity.current_run.as_mut().unwrap().pid = i64::MAX,
            6 | 7 => {
                a.environment.id = "remote".into();
                a.environment.is_stale = change == 6;
            }
            8 => a.activity.current_run.as_mut().unwrap().pid = 99,
            _ => unreachable!(),
        }
        cases.push(a);
    }
    assert_eq!(
        discovery::monitored(cases, "local", &snapshot)
            .iter()
            .map(|a| a.session_id.as_str())
            .collect::<Vec<_>>(),
        ["valid", "remote-fresh"]
    );
}

#[test]
fn local_inventory_uses_native_scanners_and_retains_verified_session_metadata() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let claude = dir.path().join("claude");
    let pi = dir.path().join("pi");
    let codex = dir.path().join("codex");
    for p in [&claude, &pi.join("project"), &codex] {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(claude.join("session.json"),json!({"pid":41,"sessionId":"claude-session","cwd":"/repo","startedAt":1700000000000i64,"name":"Claude title"}).to_string()).unwrap();
    fs::write(
        pi.join("project/pi.jsonl"),
        json!({"type":"session","id":"pi-session","cwd":"/repo","timestamp":"2023-11-14T22:13:20Z"})
            .to_string() + "\n",
    )
    .unwrap();
    fs::write(
        pi.join("project/inactive.jsonl"),
        json!({"type":"session","id":"inactive","cwd":"/other","timestamp":"2023-11-14T22:13:20Z"})
            .to_string()
            + "\n",
    )
    .unwrap();
    fs::write(codex.join("codex.jsonl"),json!({"type":"session_meta","timestamp":"2023-11-14T22:13:20Z","payload":{"id":"codex-session","cwd":"/repo","agent_nickname":"Worker"}}).to_string()+"\n").unwrap();
    let snapshot = ProcessSnapshot::from_processes([
        process(41, "claude", "/repo", 1700000000000),
        process(42, "pi", "/repo", 1700000000000),
        process(43, "codex", "/repo", 1700000000000),
    ]);
    let paths = SessionPaths {
        claude: vec![claude],
        pi,
        codex,
        opencode: dir.path().join("absent.db"),
    };
    let token = CancellationToken::new();
    let found = discovery::local(&token, &paths, &snapshot, "fixture-host").unwrap();
    assert_eq!(found.len(), 3, "{found:?}");
    for (id, pid, title) in [
        ("claude-session", 41, "Claude title"),
        ("pi-session", 42, ""),
        ("codex-session", 43, "Worker"),
    ] {
        let a = found.iter().find(|a| a.session_id == id).unwrap();
        assert_eq!(a.activity.current_run.as_ref().unwrap().pid, pid);
        assert_eq!(a.workspace.cwd, "/repo");
        assert_eq!(a.environment.hostname, "fixture-host");
        assert_eq!(a.activity.title, title);
    }
    token.cancel();
    assert!(matches!(
        discovery::local(&token, &paths, &snapshot, "host"),
        Err(agent_debug::Error::Cancelled)
    ));
}
