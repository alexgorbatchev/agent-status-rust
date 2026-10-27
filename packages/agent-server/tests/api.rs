use agent_common::types::*;
use agent_server::{
    api::{Api, ApiConfig},
    hub::Hub,
    store::ReadStore,
};
use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Fixture {
    _directory: tempfile::TempDir,
    writer: rusqlite::Connection,
    store: ReadStore,
    api: Api,
}

impl Fixture {
    async fn new(config: ApiConfig) -> Self {
        let directory = tempfile::tempdir_in(".tmp").unwrap();
        let path = directory.path().join("state.db");
        let mut writer = agent_status_store::open_writer(&path).unwrap();
        let agent = AgentState {
            id: "env:claude-code:session%20literal".into(),
            session_id: "session".into(),
            harness: HarnessType::ClaudeCode,
            environment: EnvironmentInfo {
                id: "env".into(),
                label: "Local".into(),
                hostname: "host".into(),
                platform: "linux".into(),
                user: "user".into(),
                last_heartbeat_at: i64::MAX,
                ..Default::default()
            },
            project: ProjectInfo {
                id: "project".into(),
                name: "Repository".into(),
                canonical_root: "/repo".into(),
            },
            workspace: WorkspaceInfo {
                branch: "main".into(),
                cwd: "/repo".into(),
                ..Default::default()
            },
            activity: AgentActivity {
                lifecycle: LifecycleState::Running,
                activity: ActivityState::Busy,
                title: "Working".into(),
                action: "Editing".into(),
                summary: Some("Native recap".into()),
                summary_updated_at: Some(123),
                updated_at: 123,
                first_started_at: 100,
                current_run: Some(RunInfo {
                    id: "run".into(),
                    pid: 42,
                    harness_version: "1.0".into(),
                    started_at: 100,
                    ended_at: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let tx = writer.transaction().unwrap();
        agent_status_store::write::save_agent(&tx, &agent).unwrap();
        for timestamp in 1..=3 {
            agent_status_store::write::record_event(
                &tx,
                &TelemetryFrame {
                    agent_id: agent.id.clone(),
                    harness: agent.harness.clone(),
                    event_type: "tool_call".into(),
                    timestamp,
                    subagent_id: (timestamp == 2).then(|| "child".into()),
                    data: EventData {
                        tool_name: Some("Skill".into()),
                        tool_input: Some(serde_json::Map::from_iter([(
                            "skill".into(),
                            "review".into(),
                        )])),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let token = CancellationToken::new();
        let store = ReadStore::open(&path, &token).await.unwrap();
        let api = Api::new(store.clone(), Hub::default(), config, token).unwrap();
        Self {
            _directory: directory,
            writer,
            store,
            api,
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: &str,
    ) -> Response<Body> {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(auth) = auth {
            request = request.header("authorization", auth);
        }
        self.api
            .router()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    async fn close(self) {
        self.api.shutdown().await;
        self.store.shutdown().await;
    }
}

async fn json_body(response: Response<Body>) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}

#[tokio::test]
async fn all_read_endpoints_retain_native_models_usage_and_encoded_identity() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    let response = fixture.request("GET", "/api/v1/status", None, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let value = json_body(response).await;
    assert_eq!(value["status"], "ok");
    assert_eq!(value["activeAgentCount"], 1);
    assert_eq!(
        json_body(fixture.request("GET", "/api/v1/projects", None, "").await).await[0]["project"]["name"],
        "Repository"
    );
    assert_eq!(
        json_body(
            fixture
                .request("GET", "/api/v1/environments", None, "")
                .await
        )
        .await[0]["id"],
        "env"
    );
    let agents = json_body(fixture.request("GET", "/api/v1/agents", None, "").await).await;
    assert_eq!(agents[0]["activity"]["summary"], "Native recap");
    assert_eq!(agents[0]["activity"]["summaryUpdatedAt"], 123);
    let base = "/api/v1/agents/env%3Aclaude-code%3Asession%2520literal";
    let agent = json_body(fixture.request("GET", base, None, "").await).await;
    assert_eq!(agent["id"], "env:claude-code:session%20literal");
    assert_eq!(agent["activity"]["action"], "Editing");
    let usage = json_body(
        fixture
            .request("GET", &format!("{base}/usage"), None, "")
            .await,
    )
    .await;
    assert_eq!(usage["skills"][0]["name"], "review");
    assert_eq!(usage["skills"][0]["count"], 3);
    assert_eq!(usage["coverage"]["commands"]["status"], "partial");
    let logs = json_body(
        fixture
            .request("GET", &format!("{base}/logs?limit=2"), None, "")
            .await,
    )
    .await;
    assert_eq!(logs.as_array().unwrap().len(), 2);
    assert_eq!(logs[0]["seq"], 3);
    let logs = json_body(
        fixture
            .request(
                "GET",
                &format!("{base}/logs?before=3&subagent=child"),
                None,
                "",
            )
            .await,
    )
    .await;
    assert_eq!(logs.as_array().unwrap().len(), 1);
    assert_eq!(logs[0]["seq"], 2);
    for query in [
        "limit=invalid&before=invalid",
        "limit=-1&before=-1",
        "limit=0",
        "limit=999999999999999999999999",
    ] {
        assert_eq!(
            json_body(
                fixture
                    .request("GET", &format!("{base}/logs?{query}"), None, "")
                    .await
            )
            .await
            .as_array()
            .unwrap()
            .len(),
            3
        );
    }
    fixture.close().await;
}

#[tokio::test]
async fn filters_are_independent_and_unknown_states_preserve_query_semantics() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    for query in [
        "project=project",
        "environment=env",
        "lifecycle=RUNNING",
        "activity=BUSY",
        "hidden=false",
        "hidden=unknown",
        "lifecycle=future",
        "project=project&environment=env&activity=BUSY&hidden=false",
    ] {
        let value = json_body(
            fixture
                .request("GET", &format!("/api/v1/agents?{query}"), None, "")
                .await,
        )
        .await;
        assert_eq!(value.as_array().unwrap().len(), 1, "{query}");
    }
    for query in [
        "project=absent",
        "environment=absent",
        "lifecycle=EXITED",
        "activity=future",
        "hidden=true",
    ] {
        let value = json_body(
            fixture
                .request("GET", &format!("/api/v1/agents?{query}"), None, "")
                .await,
        )
        .await;
        assert!(value.as_array().unwrap().is_empty(), "{query}");
    }
    fixture.close().await;
}

#[tokio::test]
async fn auth_header_precedence_query_fallback_and_upgrade_auth_run_before_handlers() {
    let fixture = Fixture::new(ApiConfig {
        auth_token: "configured".into(),
        ..Default::default()
    })
    .await;
    for (path, auth, status, message) in [
        ("/api/v1/status", None, 401, "missing bearer token"),
        ("/api/v1/status", Some("Bearer wrong"), 401, "invalid token"),
        ("/api/v1/status?token=configured", None, 200, ""),
        ("/api/v1/status?access_token=configured", None, 200, ""),
        (
            "/api/v1/status?token=configured",
            Some("Basic ignored"),
            200,
            "",
        ),
        (
            "/api/v1/status?token=configured",
            Some("Bearer "),
            401,
            "missing bearer token",
        ),
        (
            "/api/v1/status?token=wrong&access_token=configured",
            None,
            401,
            "invalid token",
        ),
        ("/api/v1/status", Some("bEaReR   configured  "), 200, ""),
        ("/ws/events", None, 401, "missing bearer token"),
        ("/api/v1/events", None, 401, "missing bearer token"),
        (
            "/api/v1/agents/a/live-tail",
            None,
            401,
            "missing bearer token",
        ),
    ] {
        let response = fixture.request("GET", path, auth, "").await;
        assert_eq!(response.status().as_u16(), status, "{path} {auth:?}");
        if status == 401 {
            assert_eq!(json_body(response).await["message"], message);
        }
    }
    fixture.close().await;
}

#[tokio::test]
async fn missing_agent_and_database_failures_have_route_specific_safe_errors() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    for suffix in ["", "/usage"] {
        let response = fixture
            .request("GET", &format!("/api/v1/agents/absent{suffix}"), None, "")
            .await;
        assert_eq!(response.status(), 404);
        assert_eq!(
            json_body(response).await,
            json!({"error":"agent not found"})
        );
    }
    assert_eq!(
        json_body(
            fixture
                .request("GET", "/api/v1/agents/absent/logs", None, "")
                .await
        )
        .await,
        json!([])
    );
    fixture
        .writer
        .execute("ALTER TABLE environments RENAME TO unavailable", [])
        .unwrap();
    for (path, message) in [
        ("/api/v1/status", "failed to get status"),
        ("/api/v1/projects", "failed to get projects"),
        ("/api/v1/environments", "failed to get environments"),
        ("/api/v1/agents", "failed to get agents"),
        ("/api/v1/agents/a", "failed to get agent"),
        ("/api/v1/agents/a/usage", "failed to get agent usage"),
    ] {
        let response = fixture.request("GET", path, None, "").await;
        assert_eq!(response.status(), 500, "{path}");
        assert_eq!(json_body(response).await, json!({"error":message}));
    }
    fixture.close().await;
}

#[tokio::test]
async fn patch_validation_accepts_real_mutations_and_rejects_wrong_types_before_proxy() {
    let fixture = Fixture::new(ApiConfig {
        ingester_url: "http://127.0.0.1:0".into(),
        ..Default::default()
    })
    .await;
    for body in ["{", "[]", "true", "{\"isHidden\":\"true\"}"] {
        let response = fixture
            .request("PATCH", "/api/v1/agents/a", None, body)
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(json_body(response).await["error"], "invalid request body");
    }
    for body in [
        "null",
        "{}",
        "{\"unknown\":true}",
        "{\"dismissCompletion\":false}",
        "{\"acknowledgeCompletion\":null}",
    ] {
        let response = fixture
            .request("PATCH", "/api/v1/agents/a", None, body)
            .await;
        assert_eq!(response.status(), 400);
        assert_eq!(
            json_body(response).await["error"],
            "no recognized patch fields"
        );
    }
    for body in [
        "{\"isHidden\":false}",
        "{\"dismissCompletion\":true}",
        "{\"acknowledgeCompletion\":true}",
    ] {
        assert_eq!(
            fixture
                .request("PATCH", "/api/v1/agents/a", None, body)
                .await
                .status(),
            502
        );
    }
    assert_eq!(
        fixture
            .request("POST", "/api/v1/agents/a/focus", None, "")
            .await
            .status(),
        502
    );
    fixture.close().await;
}

#[tokio::test]
async fn unreadable_patch_body_retains_the_json_error_contract() {
    let fixture = Fixture::new(ApiConfig::default()).await;
    let body = Body::from_stream(futures_util::stream::once(async {
        Err::<axum::body::Bytes, _>(std::io::Error::other("request interrupted"))
    }));
    let response = fixture
        .api
        .router()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/agents/a")
                .body(body)
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(
        json_body(response).await,
        json!({"error":"failed to read request body"})
    );
    fixture.close().await;
}

#[tokio::test]
async fn cors_matches_source_single_wildcards_and_credentialed_all_origin_is_valid() {
    for origins in [
        vec![String::from("http://LOCALHOST:3000")],
        vec!["http://*.example.test".into()],
        vec!["*".into()],
    ] {
        let origin = if origins[0].contains("example") {
            "http://ui.example.test"
        } else {
            "http://localhost:3000"
        };
        let fixture = Fixture::new(ApiConfig {
            cors_origins: origins,
            ..Default::default()
        })
        .await;
        let request = Request::builder()
            .method("OPTIONS")
            .uri("/api/v1/agents/a")
            .header("origin", origin)
            .header("access-control-request-method", "PATCH")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap();
        let response = fixture.api.router().oneshot(request).await.unwrap();
        assert!(response.status().is_success());
        assert_eq!(response.headers()["access-control-allow-origin"], origin);
        assert_eq!(
            response.headers()["access-control-allow-credentials"],
            "true"
        );
        assert_eq!(response.headers()["access-control-max-age"], "300");
        fixture.close().await;
    }
    let fixture = Fixture::new(ApiConfig::default()).await;
    let request = Request::builder()
        .uri("/api/v1/status")
        .header("origin", "http://denied.test")
        .body(Body::empty())
        .unwrap();
    let response = fixture.api.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    fixture.close().await;
}
