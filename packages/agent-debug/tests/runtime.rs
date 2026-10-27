use std::{fs, process::Command};

fn command(dir: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-debug"));
    command
        .env("AGENT", "1")
        .env("AGENT_LOG_DIR", dir)
        .env("AGENT_LOG_TO_DISK", "1")
        .env_remove("AGENT_LOG_FILE");
    command
}

#[test]
fn delivered_cli_preserves_raw_version_help_quiet_replay_and_single_logged_failure() {
    fs::create_dir_all(".tmp").unwrap();
    let temp = tempfile::tempdir_in(".tmp").unwrap();
    let version = command(temp.path()).arg("--version").output().unwrap();
    assert!(version.status.success());
    assert_eq!(version.stdout, b"0.1.0\n");
    assert!(version.stderr.is_empty());
    let help = command(temp.path())
        .args(["session", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(
        String::from_utf8(help.stdout)
            .unwrap()
            .starts_with("ALERT: Agents must read `AGENT=1 agent-debug skill`")
    );
    let path = temp.path().join("pi.jsonl");
    fs::write(&path, "{\"type\":\"session\",\"cwd\":\"/repo\",\"timestamp\":\"2026-10-11T12:00:00Z\"}\n{\"type\":\"message\",\"timestamp\":\"2026-10-11T12:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"hello\"}}\n").unwrap();
    let replay = command(temp.path())
        .args(["session", "replay"])
        .arg(&path)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert!(replay.stderr.is_empty());
    let result: serde_json::Value = serde_json::from_slice(&replay.stdout).unwrap();
    assert_eq!(result["sessionId"], "pi");
    let failure = command(temp.path())
        .args(["session", "replay", "missing-session-fixture", "--json"])
        .output()
        .unwrap();
    assert_eq!(failure.status.code(), Some(1));
    assert!(failure.stdout.is_empty());
    let error = String::from_utf8(failure.stderr).unwrap();
    assert_eq!(error.lines().count(), 1, "{error}");
    assert!(error.contains("missing-session-fixture"));
    let log = fs::read_to_string(temp.path().join("agent-debug_rCURRENT.log")).unwrap();
    assert_eq!(log.matches("command_failed").count(), 1);
}

#[cfg(unix)]
#[test]
fn native_termination_cancels_an_inflight_http_read_and_drains_the_command() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        process::Stdio,
        sync::mpsc,
        time::{Duration, Instant},
    };
    fs::create_dir_all(".tmp").unwrap();
    let temp = tempfile::tempdir_in(".tmp").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut headers = Vec::new();
        let mut byte = [0];
        while !headers.ends_with(b"\r\n\r\n") {
            assert_eq!(stream.read(&mut byte).unwrap(), 1);
            headers.push(byte[0]);
        }
        assert!(
            String::from_utf8(headers)
                .unwrap()
                .starts_with("GET /api/v1/agents/cancel-fixture/usage ")
        );
        ready_tx.send(()).unwrap();
        // A real peer disconnect proves cancellation drops the in-flight body/header read.
        let end = stream.read(&mut byte);
        assert!(
            matches!(end, Ok(0))
                || end
                    .as_ref()
                    .is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionReset),
            "{end:?}"
        );
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
    });
    let mut child = command(temp.path())
        .args(["agent", "usage", "cancel-fixture", "--server-url", &base])
        .env_remove("AGENT_STATUS_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if ready_rx.recv_timeout(Duration::from_secs(5)).is_err() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child did not begin the local request");
    }
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cancelled command did not drain within five seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(error.lines().count(), 1, "{error}");
    assert!(error.contains("operation cancelled"), "{error}");
    server.join().unwrap();
}

#[test]
fn native_audit_commands_keep_valid_rows_and_expose_corrupt_peer_diagnostics() {
    fs::create_dir_all(".tmp").unwrap();
    let temp = tempfile::tempdir_in(".tmp").unwrap();
    let home = temp.path().join("home");
    let pi = home.join("pi/project");
    fs::create_dir_all(&pi).unwrap();
    fs::write(pi.join("good.jsonl"), "{\"type\":\"session\",\"id\":\"audit-fixture\",\"cwd\":\"/fixture/repo\",\"timestamp\":1758400000000}\n").unwrap();
    fs::write(pi.join("bad.jsonl"), "broken\n").unwrap();
    for (subject, verb) in [
        ("scanner", "audit"),
        ("scanner", "list"),
        ("process", "list"),
    ] {
        for json in [false, true] {
            let mut command = command(temp.path());
            command
                .args([
                    subject,
                    verb,
                    "--harness",
                    "pi",
                    "--cwd",
                    "/fixture",
                    "--debug",
                ])
                .env("HOME", &home)
                .env("XDG_DATA_HOME", home.join("data"))
                .env("PI_SESSIONS_DIR", pi.parent().unwrap());
            if json {
                command.arg("--json");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(
                text.contains("audit-fixture") && text.contains("ORPHAN_SESSION"),
                "{text}"
            );
            let err = String::from_utf8(output.stderr).unwrap();
            assert!(err.contains("WARN:") && err.contains("bad.jsonl"), "{err}");
            assert!(err.contains("action:command"), "{err}");
        }
    }
    let output = command(temp.path())
        .args(["agent", "list"])
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("PI_SESSIONS_DIR", pi.parent().unwrap())
        .env("CLAUDE_SESSIONS_DIR", home.join("claude"))
        .env("CODEX_SESSIONS_DIR", home.join("codex"))
        .env("OPENCODE_DB_PATH", home.join("opencode.db"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
}
