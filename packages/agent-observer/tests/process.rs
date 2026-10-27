use futures_util::StreamExt;
use std::{process::Stdio, time::Duration};
use tokio::{
    net::TcpListener,
    process::{Child, Command},
    time::timeout,
};

fn isolated(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-observer"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root)
        .env("AGENT_LOG_TO_DISK", "false")
        .env("AGENT", "1")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("data"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

async fn stop(mut child: Child, signal: &str) -> std::process::Output {
    let status = Command::new("kill")
        .args([signal, &child.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    assert!(status.success());
    timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap();
    child.wait_with_output().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_sends_hello_then_joins_and_flushes_on_interrupt_or_terminate() {
    for signal in ["-TERM", "-INT"] {
        let root = tempfile::tempdir_in(".tmp").unwrap();
        let root = root.path().canonicalize().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut command = isolated(&root);
        command.args([
            "--ingester-url",
            &format!("ws://{}/ws/ingest", listener.local_addr().unwrap()),
            "--env",
            "fixture",
            "--scan-interval",
            "10ms",
        ]);
        for flag in [
            "--claude-sessions-dir",
            "--claude-projects-dir",
            "--pi-sessions-dir",
            "--codex-sessions-dir",
        ] {
            command.arg(flag).arg(root.join(flag));
        }
        command
            .arg("--opencode-db-path")
            .arg(root.join("missing.db"));
        let child = command.spawn().unwrap();
        let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let hello = timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let hello: serde_json::Value = serde_json::from_slice(&hello.into_data()).unwrap();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["environment"]["label"], "fixture");
        let output = stop(child, signal).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("observer_starting"));
        assert!(stderr.contains("observer_shutting_down"));
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test]
async fn binary_metadata_is_offline_and_runtime_failures_print_once() {
    let root = tempfile::tempdir_in(".tmp").unwrap();
    for args in [vec!["--version"], vec!["skill"], vec!["--help"]] {
        let output = isolated(root.path()).args(args).output().await.unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
    }
    assert!(!root.path().join("cache").exists());
    let invalid = isolated(root.path())
        .args(["--scan-interval", "wrong"])
        .output()
        .await
        .unwrap();
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&invalid.stderr)
            .matches("command_failed")
            .count(),
        1
    );
    let blocked = root.path().join("blocked");
    std::fs::write(&blocked, "file").unwrap();
    let failed = timeout(
        Duration::from_secs(5),
        isolated(root.path())
            .arg("--cursor-path")
            .arg(blocked.join("cursor"))
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let stderr = String::from_utf8(failed.stderr).unwrap();
    assert_eq!(stderr.matches("command_failed").count(), 1);
    assert!(stderr.contains("observer_shutting_down"));
}
