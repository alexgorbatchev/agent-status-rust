use agent_common::types::{ServerEvent, TelemetryFrame};
use agent_server::hub::{CLIENT_BUFFER_SIZE, Hub};
use std::sync::Arc;

fn event(kind: &str, id: &str, seq: i64) -> ServerEvent {
    ServerEvent {
        kind: kind.into(),
        agent_id: id.into(),
        event: Some(TelemetryFrame {
            seq,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn state_overflow_evicts_and_drains_but_logs_remain_lossy() {
    let hub = Hub::default();
    let mut global = hub.subscribe_global();
    let mut tail = hub.subscribe_tail("a");
    let mut other = hub.subscribe_tail("b");
    for seq in 0..CLIENT_BUFFER_SIZE as i64 {
        hub.broadcast(event("log_append", "a", seq));
    }
    hub.broadcast(event("log_append", "a", 999));
    assert_eq!(hub.global_count(), 1);
    assert_eq!(hub.tail_count("a"), 1);
    assert!(other.try_recv().is_err());
    hub.broadcast(event("agent_upsert", "a", 1000));
    assert_eq!(hub.global_count(), 0);
    for seq in 0..CLIENT_BUFFER_SIZE as i64 {
        assert_eq!(
            global.recv().await.unwrap().event.as_ref().unwrap().seq,
            seq
        );
        assert_eq!(tail.recv().await.unwrap().seq, seq);
    }
    assert!(global.recv().await.is_none());
    hub.broadcast(event("log_append", "a", 1001));
    assert_eq!(tail.recv().await.unwrap().seq, 1001);
    drop(tail);
    drop(other);
    assert_eq!(hub.tail_count("a"), 0);
    assert_eq!(hub.tail_count("b"), 0);
}

#[tokio::test]
async fn every_state_kind_forces_resync_and_close_is_idempotent() {
    for kind in [
        "snapshot",
        "agent_upsert",
        "environment_upsert",
        "environment_stale",
        "future_state",
    ] {
        let hub = Hub::default();
        let mut global = hub.subscribe_global();
        for seq in 0..=CLIENT_BUFFER_SIZE as i64 {
            hub.broadcast(event(kind, "a", seq));
        }
        assert_eq!(hub.global_count(), 0, "{kind}");
        for _ in 0..CLIENT_BUFFER_SIZE {
            assert!(global.recv().await.is_some());
        }
        assert!(global.recv().await.is_none());
    }
    let hub = Hub::default();
    let mut global = hub.subscribe_global();
    let mut tail = hub.subscribe_tail("a");
    hub.broadcast(event("log_append", "a", 7));
    hub.close();
    hub.close();
    hub.broadcast(event("log_append", "a", 8));
    assert_eq!(global.recv().await.unwrap().event.as_ref().unwrap().seq, 7);
    assert!(global.recv().await.is_none());
    assert_eq!(tail.recv().await.unwrap().seq, 7);
    assert!(tail.recv().await.is_none());
    assert!(hub.subscribe_global().recv().await.is_none());
    assert!(hub.subscribe_tail("a").recv().await.is_none());
}

#[tokio::test]
async fn global_receives_incomplete_log_events_but_tails_require_matching_frame() {
    let hub = Hub::default();
    let mut global = hub.subscribe_global();
    let mut tail = hub.subscribe_tail("a");
    hub.broadcast(ServerEvent {
        kind: "log_append".into(),
        agent_id: "a".into(),
        ..Default::default()
    });
    hub.broadcast(event("agent_upsert", "a", 0));
    hub.broadcast(event("log_append", "", 0));
    for _ in 0..3 {
        assert!(global.recv().await.is_some());
    }
    assert!(tail.try_recv().is_err());
    drop(global);
    assert_eq!(hub.global_count(), 0);
    let clone = hub.clone();
    clone.broadcast(event("log_append", "a", 2));
    assert_eq!(tail.recv().await.unwrap().seq, 2);
}

#[tokio::test]
async fn fanout_shares_one_immutable_event_and_subscription_owns_registration() {
    let hub = Hub::default();
    let mut a = hub.subscribe_global();
    let mut b = hub.subscribe_global();
    hub.broadcast(event("snapshot", "a", 1));
    let first = a.recv().await.unwrap();
    let second = b.recv().await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(hub.global_count(), 2);
    drop(a);
    drop(b);
    assert_eq!(hub.global_count(), 0);
    assert_eq!(
        serde_json::to_value(&*first).unwrap()["agents"],
        serde_json::json!([])
    );
}
