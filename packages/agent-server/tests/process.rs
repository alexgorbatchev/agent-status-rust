use std::{process::Stdio, time::Duration};
use tokio::process::Command;

fn isolated(root: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-server"));
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
async fn binary_metadata_is_offline_and_failures_are_printed_once() {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["skill"],
        vec!["completion", "fish"],
    ] {
        let output = isolated(directory.path())
            .args(args)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    assert!(!directory.path().join("data").exists());
    for args in [
        vec!["--unknown"],
        vec!["--log-level", "invalid"],
        vec!["--cors-origin", "\"unclosed"],
        vec!["--addr", "invalid-address"],
        vec![],
    ] {
        let output = isolated(directory.path())
            .args(args)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.matches("command_failed").count(), 1, "{stderr}");
        assert!(!stderr.contains("panicked"));
        assert!(
            !stderr.contains("Usage:"),
            "daemon failures suppress usage: {stderr}"
        );
    }
    assert!(!directory.path().join("data").exists());
}

#[tokio::test]
async fn real_daemon_preserves_database_and_joins_on_both_unix_signals() {
    for signal in ["-INT", "-TERM"] {
        let directory = tempfile::tempdir_in(".tmp").unwrap();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("state.db");
        let writer = agent_status_store::open_writer(&database).unwrap();
        writer.execute("INSERT INTO environments(id,label,hostname,platform,user,last_heartbeat_at) VALUES ('retained','test','host','linux','user',123)", []).unwrap();
        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = reservation.local_addr().unwrap();
        drop(reservation);
        let child = isolated(&root)
            .arg("--addr")
            .arg(addr.to_string())
            .arg("--db")
            .arg(&database)
            .spawn()
            .unwrap();
        let client = reqwest::Client::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(response) = client
                    .get(format!("http://{addr}/api/v1/status"))
                    .send()
                    .await
                    && response.status().is_success()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
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
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stderr}");
        assert!(output.stdout.is_empty());
        assert!(stderr.contains("server_starting"));
        assert!(stderr.contains("server_shutting_down"));
        assert!(stderr.contains("~/state.db"));
        assert!(!stderr.contains(&root.display().to_string()));
        assert!(tokio::net::TcpStream::connect(addr).await.is_err());
        assert_eq!(
            writer
                .query_row("SELECT id FROM environments", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "retained"
        );
    }
}
