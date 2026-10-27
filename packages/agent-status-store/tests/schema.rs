use agent_status_store::schema::migrate;
use rusqlite::Connection;

#[test]
fn migration_preserves_historical_rows_and_foreign_keys() {
    let mut db = Connection::open_in_memory().unwrap();
    db.pragma_update(None, "foreign_keys", true).unwrap();
    db.execute_batch(include_str!("../src/sql/v1.sql")).unwrap();
    db.execute_batch("INSERT INTO environments VALUES('env','Local','host','linux','user',0,1000);
        INSERT INTO agents(id,session_id,environment_id,harness,project_id,project_name,canonical_root,branch,cwd,first_started_at,updated_at,activity)
        VALUES('agent','session','env','claude-code','project','project','/repo','main','/repo',1000,1000,'BUSY');
        INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('agent','tool_call',1000,'{\"toolName\":\"Skill\",\"toolCallId\":\"call\",\"toolInput\":{\"skill\":\"review\"}}');
        PRAGMA user_version=1;").unwrap();
    migrate(&mut db).unwrap();
    migrate(&mut db).unwrap();
    assert_eq!(
        db.query_row("SELECT activity FROM agents", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "BUSY"
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM telemetry_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT name FROM usage_observations", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "review"
    );
    db.execute("UPDATE agents SET activity='RATE_LIMITED'", [])
        .unwrap();
    assert!(db.execute("INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('missing','event',1,'{}')", []).is_err());
}

#[test]
fn failed_migration_rolls_back_data_and_version() {
    let mut db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIEW agents AS SELECT 1 AS id")
        .unwrap();
    assert!(migrate(&mut db).is_err());
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(db.prepare("SELECT * FROM environments").is_err());
}

fn version_five(db: &Connection) {
    db.execute_batch(include_str!("../src/sql/v1.sql")).unwrap();
    db.execute_batch(
        "ALTER TABLE agents ADD COLUMN is_hidden INTEGER NOT NULL DEFAULT 0;
        ALTER TABLE agents ADD COLUMN hidden_at INTEGER;
        ALTER TABLE agents ADD COLUMN active_tool_started_at INTEGER;
        ALTER TABLE agents ADD COLUMN active_subagents TEXT;
        ALTER TABLE agents ADD COLUMN model TEXT;
        ALTER TABLE agents ADD COLUMN context_window INTEGER;
        ALTER TABLE agents ADD COLUMN context_fullness REAL;
        PRAGMA user_version=5;",
    )
    .unwrap();
}

#[test]
fn rebuild_failure_restores_foreign_keys_and_preserves_original_schema() {
    let mut db = Connection::open_in_memory().unwrap();
    version_five(&db);
    db.pragma_update(None, "foreign_keys", false).unwrap();
    db.execute("INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('orphan','event',1000,'{}')",[]).unwrap();
    assert!(matches!(
        migrate(&mut db),
        Err(agent_status_store::Error::ForeignKey { .. })
    ));
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        5
    );
    assert!(
        db.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM telemetry_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(db.prepare("SELECT * FROM agents_new").is_err());
}

#[test]
fn partially_added_rate_limit_and_completion_columns_are_recovered() {
    for (version, columns) in [
        (
            6,
            "ALTER TABLE agents ADD COLUMN rate_limit_reset_at INTEGER;",
        ),
        (
            6,
            "ALTER TABLE agents ADD COLUMN rate_limit_reset_at INTEGER; ALTER TABLE agents ADD COLUMN retry_after_seconds INTEGER;",
        ),
        (
            7,
            "ALTER TABLE agents ADD COLUMN has_unseen_completion INTEGER NOT NULL DEFAULT 0;",
        ),
        (
            7,
            "ALTER TABLE agents ADD COLUMN has_unseen_completion INTEGER NOT NULL DEFAULT 0; ALTER TABLE agents ADD COLUMN completed_at INTEGER;",
        ),
    ] {
        let mut db = Connection::open_in_memory().unwrap();
        version_five(&db);
        db.execute_batch(include_str!("../src/sql/v6.sql")).unwrap();
        if version == 7 {
            db.execute_batch("ALTER TABLE agents ADD COLUMN rate_limit_reset_at INTEGER; ALTER TABLE agents ADD COLUMN retry_after_seconds INTEGER;").unwrap();
        }
        db.pragma_update(None, "user_version", version).unwrap();
        db.execute_batch(columns).unwrap();
        migrate(&mut db).unwrap();
        db.execute_batch("INSERT INTO environments VALUES('env','label','host','linux','user',0,1000);
            INSERT INTO agents(id,session_id,environment_id,harness,project_id,project_name,canonical_root,branch,cwd,first_started_at,updated_at,activity,rate_limit_reset_at,retry_after_seconds,has_unseen_completion,completed_at)
            VALUES('agent','session','env','pi','project','project','/repo','main','/repo',1000,1000,'RATE_LIMITED',1100,10,1,1000);").unwrap();
        assert_eq!(
            db.query_row("SELECT retry_after_seconds FROM agents", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            10
        );
    }
}

#[test]
fn invalid_historical_usage_rolls_back_and_newer_schema_is_rejected() {
    let mut db = Connection::open_in_memory().unwrap();
    migrate(&mut db).unwrap();
    db.execute_batch("DROP TABLE usage_observations; PRAGMA user_version=9;
        INSERT INTO environments VALUES('env','label','host','linux','user',0,1000);
        INSERT INTO agents(id,session_id,environment_id,harness,project_id,project_name,canonical_root,branch,cwd,first_started_at,updated_at,activity)
        VALUES('agent','session','env','pi','project','project','/repo','main','/repo',1000,1000,'IDLE');
        INSERT INTO telemetry_events(agent_id,event_type,timestamp,data_json) VALUES('agent','tool_call',1000,'invalid JSON');").unwrap();
    assert!(matches!(
        migrate(&mut db),
        Err(agent_status_store::Error::Json(_))
    ));
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        9
    );
    assert!(db.prepare("SELECT * FROM usage_observations").is_err());
    db.pragma_update(None, "user_version", 11).unwrap();
    assert!(matches!(
        migrate(&mut db),
        Err(agent_status_store::Error::SchemaVersion(11))
    ));
}
