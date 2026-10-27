use std::{process::Stdio, time::Duration};
use tokio::process::Command;
fn isolated(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-ingester"));
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root)
        .env("AGENT", "1")
        .env("AGENT_LOG_TO_DISK", "false")
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}
#[tokio::test]
async fn binary_metadata_is_offline_and_errors_print_once() {
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    for args in [vec!["--help"], vec!["--version"], vec!["skill"]] {
        let output = isolated(dir.path()).args(args).output().await.unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.is_empty());
    }
    assert!(!dir.path().join("data").exists());
    for args in [
        vec!["--idle-grace-period", "bad"],
        vec!["--addr", "invalid-address"],
    ] {
        let output = isolated(dir.path()).args(args).output().await.unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            String::from_utf8_lossy(&output.stderr)
                .matches("command_failed")
                .count(),
            1
        );
        assert!(output.stdout.is_empty());
    }
}
#[tokio::test]
async fn real_daemon_joins_socket_http_and_database_on_interrupt_or_terminate() {
    for signal in ["-INT", "-TERM"] {
        // Keep the native Unix socket below sockaddr_un's path limit after nesting packages.
        let dir = tempfile::tempdir_in("../../.tmp").unwrap();
        let path = dir.path().canonicalize().unwrap();
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let child = isolated(&path)
            .args(["--addr", &address.to_string(), "--no-summarize"])
            .spawn()
            .unwrap();
        let client = reqwest::Client::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(response) = client.get(format!("http://{address}/healthz")).send().await
                    && response.status().is_success()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(path.join("runtime/agent-status/notify.sock").exists());
        assert!(
            Command::new("kill")
                .args([signal, &child.id().unwrap().to_string()])
                .status()
                .await
                .unwrap()
                .success()
        );
        let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        let logs = String::from_utf8(output.stderr).unwrap();
        assert!(logs.contains("server_starting"));
        assert!(logs.contains("server_shutting_down"));
        assert!(!path.join("runtime/agent-status/notify.sock").exists());
    }
}
