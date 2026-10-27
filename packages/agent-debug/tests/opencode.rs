use agent_common::types::HarnessType;
use agent_debug::{
    cli,
    output::Format,
    transcript::{self, LoadOptions},
};
use cli_command_kit::Mode;
use std::{fs, path::Path};
use tokio_util::sync::CancellationToken;

fn fixture(path: &Path) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch("CREATE TABLE session(id TEXT,project_id TEXT,parent_id TEXT,slug TEXT,directory TEXT,title TEXT,version TEXT,agent TEXT,model TEXT,cost REAL,tokens_input INTEGER,tokens_output INTEGER,tokens_reasoning INTEGER,tokens_cache_read INTEGER,tokens_cache_write INTEGER,time_created INTEGER,time_updated INTEGER,time_archived INTEGER,metadata TEXT); CREATE TABLE message(id TEXT,session_id TEXT,time_created INTEGER,data TEXT); CREATE TABLE part(id TEXT,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);
    INSERT INTO session VALUES('shared','project',NULL,NULL,'/fixture/repo','Repair','1','build','model',0.25,10,2,0,0,0,1000,2000,NULL,'{}');
    INSERT INTO session VALUES('child','project','shared',NULL,'/fixture/repo','Child','1','build','model',0,0,0,0,0,0,1000,2000,NULL,'{}');
    INSERT INTO message VALUES('user','shared',1100,'{\"role\":\"user\"}'); INSERT INTO part VALUES('text','user','shared',1100,'{\"type\":\"text\",\"text\":\"hello\"}');").unwrap();
}
#[tokio::test]
async fn native_database_loading_selects_session_and_cli_renders_list_and_replay() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let path = dir.path().join("opencode.db");
    fixture(&path);
    let token = CancellationToken::new();
    let loaded = transcript::load_session(
        &token,
        &path,
        &LoadOptions {
            session_id: Some("shared".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(loaded.harness, HarnessType::Opencode);
    assert_eq!(loaded.session_id, "shared");
    assert_eq!(loaded.cwd, Path::new("/fixture/repo"));
    assert!(
        loaded
            .events
            .iter()
            .any(|e| e.event.event_type == "user_prompt")
    );
    assert!(
        transcript::load_session(
            &token,
            &path,
            &LoadOptions {
                session_id: Some("missing".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
    let format = Format {
        mode: Mode::Agent,
        width: 80,
        color: false,
        home: Some("/fixture".into()),
    };
    for args in [
        vec!["session", "list", path.to_str().unwrap()],
        vec![
            "session",
            "replay",
            path.to_str().unwrap(),
            "--session-id",
            "shared",
            "--json",
        ],
    ] {
        let mut argv = vec!["agent-debug"];
        argv.extend(args);
        let mut out = Vec::new();
        let mut err = Vec::new();
        assert_eq!(
            cli::run(argv, &token, &format, &mut out, &mut err).await,
            0,
            "{}",
            String::from_utf8_lossy(&err)
        );
        assert!(String::from_utf8(out).unwrap().contains("shared"));
        assert!(err.is_empty());
    }
    token.cancel();
    assert!(matches!(
        transcript::load_session(&token, &path, &LoadOptions::default()),
        Err(agent_debug::Error::Cancelled)
    ));
}

#[test]
fn session_list_resolves_only_opencode_even_when_other_harness_has_same_id() {
    fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let db = dir.path().join("opencode.db");
    fixture(&db);
    let pi = dir.path().join("pi/project");
    fs::create_dir_all(&pi).unwrap();
    fs::write(
        pi.join("shared.jsonl"),
        "{\"type\":\"session\",\"id\":\"shared\",\"cwd\":\"/repo\"}\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-debug"))
        .args(["session", "list", "shared"])
        .env("AGENT", "1")
        .env("AGENT_LOG_TO_DISK", "0")
        .env("OPENCODE_DB_PATH", &db)
        .env("PI_SESSIONS_DIR", pi.parent().unwrap())
        .env("CLAUDE_PROJECTS_DIR", dir.path().join("missing-claude"))
        .env("CODEX_SESSIONS_DIR", dir.path().join("missing-codex"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("session_id:shared"));
    assert!(text.contains("session_id:child"));
}
