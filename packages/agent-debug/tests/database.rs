use agent_debug::{
    Error,
    database::{self, DatabaseSearch, EventQuery},
};
use std::{fs, path::Path};
use tokio_util::sync::CancellationToken;

fn temporary() -> tempfile::TempDir {
    fs::create_dir_all(".tmp").unwrap();
    tempfile::tempdir_in(".tmp").unwrap()
}
fn fixture(path: &Path) -> rusqlite::Connection {
    let db = agent_status_store::open_writer(path).unwrap();
    db.execute_batch("INSERT INTO environments VALUES('env','Local','host','linux','user',0,1000);
    INSERT INTO runs VALUES('run','env:pi:session:with:colons',42,'3',1000,NULL);
    INSERT INTO agents(id,session_id,environment_id,harness,project_id,project_name,canonical_root,branch,cwd,current_run_id,first_started_at,updated_at,activity)
    VALUES('env:pi:session:with:colons','session:with:colons','env','pi','project','Project','/repo','main','/repo','run',1000,3000,'BUSY');
    INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('env:pi:session:with:colons','user_prompt',1000,'{\"text\":\"hello\"}');
    INSERT INTO telemetry_events(agent_id,subagent_id,event_type,timestamp,data_json) VALUES('env:pi:session:with:colons','worker','tool_call',2000,'{\"toolName\":\"Bash\",\"toolInput\":{\"command\":\"ls\"}}');
    INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('env:pi:session:with:colons','text',3000,'{\"text\":\"done\"}');").unwrap();
    db
}

#[test]
fn native_database_queries_preserve_projection_filters_order_and_read_only_state() {
    let temp = temporary();
    let path = temp.path().join("status.db");
    drop(fixture(&path));
    let original = fs::read(&path).unwrap();
    let cancel = CancellationToken::new();
    for id in ["session:with:colons", "env:pi:session:with:colons"] {
        let record = database::inspect(&cancel, &path, id, 3000).unwrap();
        assert_eq!(record.agent.workspace.cwd, "/repo");
        assert_eq!(record.runs[0].pid, 42);
        assert_eq!(record.event_count, 3);
        assert_eq!(record.first_event_timestamp, Some(1000));
        assert_eq!(record.last_event_timestamp, Some(3000));
    }
    assert_eq!(
        database::list_agents(&cancel, &path, 3000).unwrap().len(),
        1
    );
    let events = database::events(&cancel, &path, "with", &EventQuery::default()).unwrap();
    assert_eq!(events.iter().map(|e| e.id).collect::<Vec<_>>(), [3, 2, 1]);
    assert_eq!(events[0].pid, 42);
    assert_eq!(events[1].subagent_id.as_deref(), Some("worker"));
    let asc = EventQuery {
        descending: false,
        limit: 1,
        ..Default::default()
    };
    assert_eq!(
        database::events(&cancel, &path, "session", &asc).unwrap()[0].id,
        1
    );
    let root = EventQuery {
        include_subagents: false,
        limit: 0,
        ..Default::default()
    };
    assert_eq!(
        database::events(&cancel, &path, "", &root).unwrap().len(),
        2
    );
    let filtered = EventQuery {
        event_types: vec![" tool_call ".into(), "".into()],
        subagent_id: Some("worker".into()),
        ..Default::default()
    };
    let rows = database::events(&cancel, &path, "", &filtered).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].data.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        database::events(
            &cancel,
            &path,
            "",
            &EventQuery {
                event_id: Some(2),
                ..Default::default()
            }
        )
        .unwrap()[0]
            .event_type,
        "tool_call"
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(database::inspect(&cancel, &path, "missing", 3000).is_err());
    cancel.cancel();
    assert!(matches!(
        database::inspect(&cancel, &path, "session", 3000),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        database::events(&cancel, &path, "", &EventQuery::default()),
        Err(Error::Cancelled)
    ));
}

#[test]
fn event_identity_fallback_retains_colons_and_invalid_json_is_an_error() {
    let temp = temporary();
    let path = temp.path().join("status.db");
    let db = fixture(&path);
    db.pragma_update(None, "foreign_keys", false).unwrap();
    db.execute("INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('orphan:codex:session:colon','text',4000,?)",[r#"{"pid":99,"text":"orphan"}"#]).unwrap();
    let rows = database::events(
        &CancellationToken::new(),
        &path,
        "orphan",
        &EventQuery::default(),
    )
    .unwrap();
    assert_eq!(rows[0].session_id, "session:colon");
    assert_eq!(rows[0].harness, "codex");
    assert_eq!(rows[0].pid, 99);
    db.execute(
        "UPDATE telemetry_events SET data_json='invalid' WHERE id=4",
        [],
    )
    .unwrap();
    assert!(matches!(
        database::events(
            &CancellationToken::new(),
            &path,
            "orphan",
            &EventQuery::default()
        ),
        Err(Error::Json(_))
    ));
    db.execute("UPDATE telemetry_events SET data_json='' WHERE id=4", [])
        .unwrap();
    assert_eq!(
        database::events(
            &CancellationToken::new(),
            &path,
            "orphan",
            &EventQuery::default()
        )
        .unwrap()[0]
            .pid,
        0
    );
    assert!(
        database::events(
            &CancellationToken::new(),
            &temp.path().join("missing"),
            "",
            &EventQuery::default()
        )
        .is_err()
    );
}

#[test]
fn explicit_tilde_and_environment_paths_follow_source_search_precedence() {
    let temp = temporary();
    let home = temp.path().join("home");
    let cwd = temp.path().join("cwd");
    let xdg = temp.path().join("xdg");
    let dev = cwd.join(".tmp/dev-stack/data/agent-status/agent-status.db");
    let data = xdg.join("agent-status/agent-status.db");
    for path in [&dev, &data, &home.join("direct.db")] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"fixture").unwrap();
    }
    let lookup = |key: &str| (key == "XDG_DATA_HOME").then(|| xdg.to_str().unwrap().to_owned());
    assert_eq!(
        database::resolve_path("auto", DatabaseSearch::Sessions, &home, &cwd, lookup).unwrap(),
        data
    );
    assert_eq!(
        database::resolve_path("auto", DatabaseSearch::Events, &home, &cwd, lookup).unwrap(),
        dev
    );
    assert_eq!(
        database::resolve_path("~/direct.db", DatabaseSearch::Sessions, &home, &cwd, |_| {
            None
        })
        .unwrap(),
        home.join("direct.db")
    );
    assert_eq!(
        database::resolve_path("", DatabaseSearch::Sessions, &home, &cwd, |key| (key
            == "AGENT_DB_PATH")
            .then(|| dev.to_str().unwrap().into()))
        .unwrap(),
        dev
    );
    assert!(
        database::resolve_path(
            home.to_str().unwrap(),
            DatabaseSearch::Sessions,
            &home,
            &cwd,
            |_| None
        )
        .is_err()
    );
    assert!(
        database::resolve_path("missing.db", DatabaseSearch::Sessions, &home, &cwd, |_| {
            None
        })
        .is_err()
    );
    assert!(
        database::resolve_path("auto", DatabaseSearch::Sessions, &home, &cwd, |key| (key
            == "AGENT_DB_PATH")
            .then(|| "missing.db".into()))
        .is_err()
    );
    let empty = temp.path().join("empty");
    assert!(
        database::resolve_path("auto", DatabaseSearch::Events, &empty, &empty, |_| None).is_err()
    );
}
