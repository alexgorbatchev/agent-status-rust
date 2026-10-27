use agent_common::types::*;
use agent_ingester::{
    classifier::{Config, SummaryJob},
    dispatch::Dispatcher,
    ingestion::Ingestion,
    notify::Publisher,
    owner::Owner,
    summarizer::{self, ProviderSpec, Summarizer},
};
use agent_status_store::read;
use axum::{Json, Router, extract::State, routing::post};
use genai::adapter::AdapterKind;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, Semaphore};

#[derive(Clone)]
struct Endpoint {
    entered: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<Value>>>,
    changed: Arc<Notify>,
    release: Arc<Semaphore>,
}
impl Default for Endpoint {
    fn default() -> Self {
        Self {
            entered: Default::default(),
            active: Default::default(),
            peak: Default::default(),
            bodies: Default::default(),
            changed: Default::default(),
            release: Arc::new(Semaphore::new(0)),
        }
    }
}
async fn complete(State(endpoint): State<Endpoint>, Json(body): Json<Value>) -> Json<Value> {
    endpoint.bodies.lock().await.push(body);
    endpoint.entered.fetch_add(1, Ordering::SeqCst);
    let active = endpoint.active.fetch_add(1, Ordering::SeqCst) + 1;
    endpoint.peak.fetch_max(active, Ordering::SeqCst);
    endpoint.changed.notify_one();
    endpoint.release.acquire().await.unwrap().forget();
    endpoint.active.fetch_sub(1, Ordering::SeqCst);
    Json(
        json!({"id":"local","choices":[{"message":{"role":"assistant","content":"{\"action\":\"Verifying native worker ordering\",\"phase\":\"testing\"}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}),
    )
}
struct Fixture {
    _dir: tempfile::TempDir,
    owner: Owner,
    publisher: Arc<Publisher>,
    dispatcher: Dispatcher,
    endpoint: Endpoint,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let owner = Owner::start(
            Ingestion::open(
                dir.path().join("data.db"),
                Config {
                    summarize: true,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
        .unwrap();
        let publisher = Arc::new(
            Publisher::bind(dir.path().join("notify.sock"))
                .await
                .unwrap(),
        );
        let endpoint = Endpoint::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(post(complete))
            .with_state(endpoint.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let summary = Arc::new(
            Summarizer::new(summarizer::Config {
                providers: vec![ProviderSpec {
                    name: "local".into(),
                    adapter: AdapterKind::OpenAI,
                    model: "local".into(),
                    api_key: "fixture".into(),
                    endpoint: Some(url),
                }],
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        let dispatcher = Dispatcher::new(owner.clone(), summary, publisher.clone());
        Self {
            _dir: dir,
            owner,
            publisher,
            dispatcher,
            endpoint,
            server,
        }
    }
    async fn job(&self, id: &str, seq: i64) -> SummaryJob {
        let frame = TelemetryFrame {
            version: "1.0".into(),
            seq,
            env_id: "env".into(),
            environment: EnvironmentInfo {
                id: "env".into(),
                platform: "linux".into(),
                ..Default::default()
            },
            agent_id: id.into(),
            session_id: id.into(),
            pid: 1,
            harness: HarnessType::Pi,
            event_type: "question_prompt".into(),
            timestamp: seq,
            ..Default::default()
        };
        self.owner
            .call(move |ingestion| {
                Ok(ingestion
                    .process_frames("env", &[frame], seq)?
                    .summaries
                    .pop()
                    .expect("forced turn summary"))
            })
            .await
            .unwrap()
    }
    async fn wait(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.endpoint.entered.load(Ordering::SeqCst) < count {
                self.endpoint.changed.notified().await;
            }
        })
        .await
        .unwrap();
    }
    async fn flush(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.dispatcher.flush())
            .await
            .unwrap();
    }
    async fn shutdown(self) {
        tokio::time::timeout(Duration::from_secs(2), self.dispatcher.shutdown())
            .await
            .unwrap();
        self.publisher.shutdown().await.unwrap();
        self.owner.shutdown().await.unwrap();
        self.server.abort();
    }
}
#[tokio::test]
async fn agent_workers_run_in_parallel_but_each_agent_serializes_and_publishes_fresh_results() {
    let fixture = Fixture::new().await;
    let first = fixture.job("first", 1).await;
    let second = fixture.job("second", 2).await;
    fixture.dispatcher.enqueue([first.clone(), first, second]);
    fixture.wait(2).await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.endpoint.peak.load(Ordering::SeqCst), 2);
    fixture.endpoint.release.add_permits(3);
    fixture.flush().await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 3);
    fixture
        .owner
        .call(|ingestion| {
            for id in ["first", "second"] {
                assert_eq!(
                    read::agent(ingestion.connection(), id, 0)?.activity.action,
                    "Verifying native worker ordering"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    fixture.shutdown().await;
}
#[tokio::test]
async fn latest_bounded_queue_drops_oldest_pending_context_and_rejects_stale_work() {
    let fixture = Fixture::new().await;
    let first = fixture.job("agent", 1).await;
    fixture.dispatcher.enqueue([first.clone()]);
    fixture.wait(1).await;
    let latest = fixture.job("agent", 2).await;
    let mut evicted = latest.clone();
    evicted.input.last_prompt = "evicted-oldest-context".into();
    let mut kept = latest.clone();
    kept.input.last_prompt = "kept-latest-context".into();
    fixture
        .dispatcher
        .enqueue(std::iter::once(evicted).chain(std::iter::repeat_n(kept, 10)));
    fixture.endpoint.release.add_permits(11);
    fixture.flush().await;
    let bodies = fixture.endpoint.bodies.lock().await;
    assert_eq!(bodies.len(), 11);
    assert!(
        !bodies
            .iter()
            .any(|body| body.to_string().contains("evicted-oldest-context"))
    );
    assert!(bodies[1].to_string().contains("kept-latest-context"));
    drop(bodies);
    fixture.dispatcher.enqueue([first]);
    fixture.flush().await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 11);
    let missing = SummaryJob {
        input: summarizer::SummaryInput {
            agent_id: "missing".into(),
            ..Default::default()
        },
        ..latest
    };
    fixture.dispatcher.enqueue([missing]);
    fixture.flush().await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 11);
    fixture.shutdown().await;
}
#[tokio::test]
async fn shutdown_cancels_pending_provider_requests_without_applying_or_accepting_more_jobs() {
    let fixture = Fixture::new().await;
    let job = fixture.job("agent", 1).await;
    fixture.dispatcher.enqueue([job.clone()]);
    fixture.wait(1).await;
    let flush = fixture.dispatcher.clone();
    let flushing = tokio::spawn(async move {
        flush.flush().await;
    });
    tokio::time::timeout(Duration::from_secs(2), fixture.dispatcher.shutdown())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), flushing)
        .await
        .unwrap()
        .unwrap();
    fixture.dispatcher.enqueue([job]);
    fixture.flush().await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 1);
    fixture
        .owner
        .call(|ingestion| {
            assert_ne!(
                read::agent(ingestion.connection(), "agent", 0)?
                    .activity
                    .action,
                "Verifying native worker ordering"
            );
            Ok(())
        })
        .await
        .unwrap();
    fixture.shutdown().await;
}
#[tokio::test]
async fn closed_database_worker_does_not_stall_the_queue() {
    let fixture = Fixture::new().await;
    let job = fixture.job("agent", 1).await;
    fixture.owner.shutdown().await.unwrap();
    fixture.dispatcher.enqueue([job]);
    fixture.flush().await;
    assert_eq!(fixture.endpoint.entered.load(Ordering::SeqCst), 0);
    fixture.shutdown().await;
}
