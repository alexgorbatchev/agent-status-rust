use agent_observer::config::{Identity, Paths, read_machine_id, resolve_label};
use std::path::PathBuf;

#[test]
fn configured_paths_keep_observer_checkpoints_separate_from_harness_storage() {
    let paths = Paths::new(
        Some("/custom/config".into()),
        Some("/custom/cache".into()),
        Some("/home/test".into()),
    );
    assert_eq!(
        paths.telemetry_config(),
        Some(PathBuf::from("/custom/config/ai-registry/telemetry.json"))
    );
    assert_eq!(
        paths.observer_cache("observer-cursors.json"),
        PathBuf::from("/custom/cache/agent-status/observer-cursors.json")
    );
    let paths = Paths::new(None, None, Some("/home/test".into()));
    assert_eq!(
        paths.telemetry_config(),
        Some(PathBuf::from(
            "/home/test/.config/ai-registry/telemetry.json"
        ))
    );
    assert_eq!(
        paths.observer_cache("observer-sessions.json"),
        PathBuf::from("/home/test/.cache/agent-status/observer-sessions.json")
    );
    assert_eq!(Paths::new(None, None, None).telemetry_config(), None);
    assert!(
        Paths::capture()
            .observer_cache("observer-cursors.json")
            .ends_with("agent-status/observer-cursors.json")
    );
    assert!(
        Paths::new(None, None, None)
            .observer_cache("observer-sessions.json")
            .ends_with("agent-status/observer-sessions.json")
    );
}

#[test]
fn labels_follow_flag_environment_config_hostname_precedence() {
    std::fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let config = dir.path().join("telemetry.json");
    std::fs::write(
        &config,
        r#"{"environment":"file-env","env":"short","label":"label"}"#,
    )
    .unwrap();
    assert_eq!(
        resolve_label(" flag ", " env ", Some(&config), "host"),
        "flag"
    );
    assert_eq!(resolve_label(" ", " env ", Some(&config), "host"), "env");
    assert_eq!(resolve_label("", "", Some(&config), "host"), "file-env");
    std::fs::write(&config, r#"{"env":"short","label":"label"}"#).unwrap();
    assert_eq!(resolve_label("", "", Some(&config), "host"), "short");
    std::fs::write(&config, r#"{"label":"label"}"#).unwrap();
    assert_eq!(resolve_label("", "", Some(&config), "host"), "label");
    std::fs::write(&config, "invalid").unwrap();
    assert_eq!(resolve_label("", "", Some(&config), "host"), "host");
    assert_eq!(resolve_label("", "", None, ""), "unknown-host");
}

#[test]
fn identity_hashes_machine_and_user_and_builds_telemetry_metadata() {
    let identity = Identity {
        machine_id: "fixture-machine".into(),
        hostname: "host".into(),
        uid: "1001".into(),
        username: "tester".into(),
    };
    let id = identity.environment_id();
    assert_eq!(id.len(), 16);
    assert!(!id.contains("fixture"));
    assert_ne!(
        id,
        Identity {
            uid: "1002".into(),
            ..identity.clone()
        }
        .environment_id()
    );
    let environment = identity.environment("label", 42);
    assert_eq!(environment.id, id);
    assert_eq!(environment.label, "label");
    assert_eq!(environment.hostname, "host");
    assert_eq!(environment.user, "tester");
    assert_eq!(environment.last_heartbeat_at, 42);
    assert!(!environment.is_stale);
    let missing = Identity {
        machine_id: String::new(),
        ..identity.clone()
    };
    assert_eq!(
        missing.environment_id(),
        Identity {
            machine_id: "host".into(),
            ..identity.clone()
        }
        .environment_id()
    );
    let missing = Identity {
        machine_id: String::new(),
        hostname: String::new(),
        username: String::new(),
        ..identity
    };
    assert_eq!(missing.environment("label", 0).hostname, "unknown");
    assert_eq!(missing.environment("label", 0).user, "unknown");
    let native = Identity::capture();
    assert_eq!(
        native.environment_id(),
        Identity::capture().environment_id()
    );
    assert!(!native.uid.is_empty());
}

#[test]
fn machine_id_uses_first_readable_nonempty_candidate() {
    std::fs::create_dir_all(".tmp").unwrap();
    let dir = tempfile::tempdir_in(".tmp").unwrap();
    let missing = dir.path().join("missing");
    let empty = dir.path().join("empty");
    let valid = dir.path().join("valid");
    std::fs::write(&empty, " \n").unwrap();
    std::fs::write(&valid, " fixture-id\n").unwrap();
    assert_eq!(read_machine_id([&missing, &empty, &valid]), "fixture-id");
    assert_eq!(read_machine_id([&missing, &empty]), "");
}
