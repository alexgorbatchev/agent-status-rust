use agent_common::types::*;
use agent_ingester::{
    classifier::Config,
    ingestion::Ingestion,
    reconciler::{ReconcileConfig, process_alive},
};
use agent_status_store::read;
use agent_watcher::scanner::{ProcessMeta, ProcessSnapshot};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[test]
fn native_process_snapshot_detects_exit_and_recycled_pid_without_rejecting_old_processes() {
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let pid = i64::from(child.id());
    let snapshot = ProcessSnapshot::capture(&CancellationToken::new()).unwrap();
    assert!(process_alive(&snapshot, pid, 0));
    let created = snapshot
        .process(i32::try_from(pid).unwrap())
        .unwrap()
        .create_time;
    assert!(process_alive(&snapshot, pid, created + 120_000));
    assert!(!process_alive(&snapshot, pid, created - 60_001));
    assert!(process_alive(&snapshot, pid, created - 60_000));
    child.kill().unwrap();
    child.wait().unwrap();
    let snapshot = ProcessSnapshot::capture(&CancellationToken::new()).unwrap();
    assert!(!process_alive(&snapshot, pid, 0));
    assert!(!process_alive(&snapshot, -1, 0));
    assert!(!process_alive(&snapshot, i64::MAX, 0));
}

#[test]
fn reconciliation_is_atomic_preserves_remote_agents_and_waits_for_abandoned_runs() {
    let mut ingestion = Ingestion::open(":memory:", Config::default()).unwrap();
    for (seq, id, pid, host) in [
        (1, "dead", 12, "local"),
        (2, "live", 13, "local"),
        (3, "remote", 14, "remote"),
        (4, "abandoned", 0, "local"),
        (5, "recent", 0, "local"),
    ] {
        let frame = TelemetryFrame {
            version: "1.0".into(),
            seq,
            env_id: host.into(),
            environment: EnvironmentInfo {
                id: host.into(),
                hostname: host.into(),
                platform: "linux".into(),
                ..Default::default()
            },
            agent_id: id.into(),
            session_id: id.into(),
            pid,
            harness: HarnessType::Pi,
            event_type: "turn_start".into(),
            timestamp: if id == "recent" { 400_000 } else { 1000 },
            ..Default::default()
        };
        ingestion.process_frames(host, &[frame], 400_000).unwrap();
    }
    let snapshot = ProcessSnapshot::from_processes([ProcessMeta {
        pid: 13,
        alive: true,
        create_time: 1000,
        ..Default::default()
    }]);
    let config = ReconcileConfig {
        hostname: "local".into(),
        abandoned_after: Duration::from_secs(300),
        ..Default::default()
    };
    ingestion.connection().execute_batch("CREATE TRIGGER reject_exit BEFORE UPDATE OF current_run_id ON agents WHEN OLD.id='dead' BEGIN SELECT RAISE(ABORT,'controlled failure'); END;").unwrap();
    assert!(ingestion.reconcile(&snapshot, &config, 400_001).is_err());
    assert_eq!(
        read::agent(ingestion.connection(), "abandoned", 400_001)
            .unwrap()
            .activity
            .lifecycle,
        LifecycleState::Running
    );
    ingestion
        .connection()
        .execute_batch("DROP TRIGGER reject_exit")
        .unwrap();
    let batch = ingestion.reconcile(&snapshot, &config, 400_001).unwrap();
    assert_eq!(batch.updates.len(), 2);
    assert!(
        batch
            .updates
            .iter()
            .all(|agent| agent.activity.lifecycle == LifecycleState::Exited
                && agent.activity.current_run.is_none())
    );
    for id in ["live", "remote", "recent"] {
        assert_eq!(
            read::agent(ingestion.connection(), id, 400_001)
                .unwrap()
                .activity
                .lifecycle,
            LifecycleState::Running
        );
    }
    assert!(
        ingestion
            .reconcile(&snapshot, &config, 400_002)
            .unwrap()
            .updates
            .is_empty()
    );
    let config = ReconcileConfig {
        environment: Some("remote".into()),
        ..config
    };
    assert_eq!(
        ingestion
            .reconcile(&snapshot, &config, 400_003)
            .unwrap()
            .updates[0]
            .id,
        "remote"
    );
}
