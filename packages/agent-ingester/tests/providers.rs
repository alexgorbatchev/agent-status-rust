use agent_common::types::{ActivityState, AgentPhase};
use agent_ingester::summarizer::{
    Config, DisableSource, ProviderSpec, Summarizer, SummaryInput, sanitize_endpoint,
};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, Uri},
    response::IntoResponse,
    routing::post,
};
use genai::adapter::AdapterKind;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, Notify},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Endpoint {
    requests: Arc<Mutex<Vec<(Uri, HeaderMap, Value)>>>,
    entered: Arc<Notify>,
    delay: Duration,
    status: StatusCode,
    response: Value,
}
async fn serve(
    State(endpoint): State<Endpoint>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    endpoint
        .requests
        .lock()
        .await
        .push((uri, headers, serde_json::from_slice(&body).unwrap()));
    endpoint.entered.notify_one();
    tokio::time::sleep(endpoint.delay).await;
    (endpoint.status, axum::Json(endpoint.response))
}
async fn endpoint(
    adapter: AdapterKind,
    text: &str,
    status: StatusCode,
    delay: Duration,
) -> (String, Endpoint, JoinHandle<()>) {
    let response = match adapter {
        AdapterKind::Anthropic => {
            json!({"id":"local","type":"message","role":"assistant","model":"local-model","content":[{"type":"text","text":text}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}})
        }
        AdapterKind::Gemini => {
            json!({"candidates":[{"content":{"role":"model","parts":[{"text":text}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}})
        }
        _ => {
            json!({"id":"local","choices":[{"message":{"role":"assistant","content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
        }
    };
    let endpoint = Endpoint {
        requests: Default::default(),
        entered: Default::default(),
        delay,
        status,
        response,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(post(serve))
        .with_state(endpoint.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, endpoint, task)
}
fn spec(adapter: AdapterKind, url: String) -> ProviderSpec {
    ProviderSpec {
        name: format!("{adapter:?}"),
        adapter,
        model: "local-model".into(),
        api_key: "local-fixture-key".into(),
        endpoint: Some(url),
    }
}
fn input() -> SummaryInput {
    SummaryInput {
        activity: ActivityState::Busy,
        active_tool: Some("edit".into()),
        active_target: Some("source.rs".into()),
        last_prompt: "Repair parsing".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn native_providers_send_real_authenticated_requests_and_parse_results() {
    for adapter in [
        AdapterKind::OpenAI,
        AdapterKind::Gemini,
        AdapterKind::Anthropic,
    ] {
        let (url, server, task) = endpoint(
            adapter,
            r#"{"action":"Repairing parser"}"#,
            StatusCode::OK,
            Duration::ZERO,
        )
        .await;
        let summarizer = Summarizer::new(Config {
            providers: vec![spec(adapter, url)],
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(!summarizer.is_disabled());
        assert!(summarizer.init_errors().is_empty());
        assert_eq!(summarizer.active_providers()[0].model, "local-model");
        let result = summarizer
            .summarize(&CancellationToken::new(), &input())
            .await;
        assert!(!result.is_fallback, "{adapter:?}: {result:?}");
        assert_eq!(result.action, "Repairing parser");
        assert_eq!(result.phase, Some(AgentPhase::Editing));
        assert_eq!(result.active_target.as_deref(), Some("source.rs"));
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 1);
        let (uri, headers, body) = &requests[0];
        match adapter {
            AdapterKind::OpenAI => {
                assert_eq!(uri.path(), "/v1/chat/completions");
                assert_eq!(headers["authorization"], "Bearer local-fixture-key");
                assert_eq!(body["model"], "local-model");
                assert_eq!(body["temperature"], 0.1);
                assert_eq!(body["max_tokens"], 150);
                assert!(
                    body["messages"][1]["content"]
                        .as_str()
                        .unwrap()
                        .contains("Repair parsing")
                );
            }
            AdapterKind::Gemini => {
                assert_eq!(uri.path(), "/v1/models/local-model:generateContent");
                assert_eq!(headers["x-goog-api-key"], "local-fixture-key");
                assert_eq!(body["generationConfig"]["temperature"], 0.1);
                assert_eq!(body["generationConfig"]["maxOutputTokens"], 150);
            }
            AdapterKind::Anthropic => {
                assert_eq!(uri.path(), "/v1/messages");
                assert_eq!(headers["x-api-key"], "local-fixture-key");
                assert_eq!(body["temperature"], 0.1);
                assert_eq!(body["max_tokens"], 150);
            }
            _ => unreachable!(),
        }
        task.abort();
    }
}

#[tokio::test]
async fn failover_and_invalid_output_keep_deterministic_fallback() {
    for (status, text) in [
        (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        (StatusCode::OK, "invalid JSON"),
        (StatusCode::OK, ""),
    ] {
        let (first, first_server, first_task) =
            endpoint(AdapterKind::OpenAI, text, status, Duration::ZERO).await;
        let (second, second_server, second_task) = endpoint(
            AdapterKind::Anthropic,
            r#"{"action":"Testing migration","phase":"testing","activeTarget":"tests.rs"}"#,
            StatusCode::OK,
            Duration::ZERO,
        )
        .await;
        let summarizer = Summarizer::new(Config {
            providers: vec![
                spec(AdapterKind::OpenAI, first),
                spec(AdapterKind::Anthropic, second),
            ],
            ..Default::default()
        })
        .await
        .unwrap();
        let result = summarizer
            .summarize(&CancellationToken::new(), &input())
            .await;
        assert_eq!(result.action, "Testing migration");
        assert_eq!(result.phase, Some(AgentPhase::Testing));
        assert_eq!(result.active_target.as_deref(), Some("tests.rs"));
        assert_eq!(first_server.requests.lock().await.len(), 1);
        assert_eq!(second_server.requests.lock().await.len(), 1);
        first_task.abort();
        second_task.abort();
    }
    let (url, server, task) =
        endpoint(AdapterKind::OpenAI, "{}", StatusCode::OK, Duration::ZERO).await;
    let summarizer = Summarizer::new(Config {
        providers: vec![spec(AdapterKind::OpenAI, url)],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        summarizer
            .summarize(&CancellationToken::new(), &input())
            .await
            .is_fallback
    );
    assert_eq!(server.requests.lock().await.len(), 1);
    task.abort();
}

#[tokio::test]
async fn deadlines_and_cancellation_stop_real_pending_requests() {
    let (url, server, task) = endpoint(
        AdapterKind::OpenAI,
        r#"{"action":"Too late"}"#,
        StatusCode::OK,
        Duration::from_secs(30),
    )
    .await;
    let summarizer = Summarizer::new(Config {
        timeout: Duration::from_millis(30),
        providers: vec![spec(AdapterKind::OpenAI, url.clone())],
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(2),
            summarizer.summarize(&CancellationToken::new(), &input())
        )
        .await
        .unwrap()
        .is_fallback
    );
    assert_eq!(server.requests.lock().await.len(), 1);
    let summarizer = Arc::new(
        Summarizer::new(Config {
            providers: vec![spec(AdapterKind::OpenAI, url)],
            ..Default::default()
        })
        .await
        .unwrap(),
    );
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let worker = summarizer.clone();
    let work = tokio::spawn(async move { worker.summarize(&child, &input()).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.requests.lock().await.len() < 2 {
            server.entered.notified().await
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .unwrap()
            .unwrap()
            .is_fallback
    );
    assert!(summarizer.summarize(&cancel, &input()).await.is_fallback);
    assert_eq!(server.requests.lock().await.len(), 2);
    task.abort();
}

#[tokio::test]
async fn disable_sources_initialization_errors_and_event_time_debounce() {
    for disabled in [
        None,
        Some(DisableSource::Flag),
        Some(DisableSource::Environment),
    ] {
        let summarizer = Summarizer::new(Config {
            disabled,
            debounce: Duration::ZERO,
            timeout: Duration::ZERO,
            ..Default::default()
        })
        .await
        .unwrap();
        assert!(summarizer.is_disabled());
        assert_eq!(summarizer.disabled_source(), disabled);
        assert_eq!(summarizer.debounce_window(), Duration::from_secs(10));
        assert!(summarizer.allow_at("a", 0));
        assert!(!summarizer.allow_at("a", 2000));
        assert!(summarizer.allow_at("b", 2000));
        assert!(summarizer.allow_at("a", 10000));
        assert!(!summarizer.allow_at("a", 9000));
        assert!(summarizer.allow("fresh"));
        assert!(
            summarizer
                .summarize(&CancellationToken::new(), &input())
                .await
                .is_fallback
        );
    }
    let invalid = [
        "relative/path",
        "file:///secret",
        "http://[",
        "http://localhost/\nsecret",
    ];
    let summarizer = Summarizer::new(Config {
        providers: invalid
            .iter()
            .enumerate()
            .map(|(i, url)| {
                let mut s = spec(AdapterKind::OpenAI, (*url).into());
                s.name = i.to_string();
                s
            })
            .collect(),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(summarizer.init_errors().len(), 4);
    assert!(summarizer.is_disabled());
    for (raw, expected) in [
        (
            "https://user:secret@host:123/a?token=secret#fragment",
            "https://host:123/a",
        ),
        ("host:123/a?secret=1", "host:123/a"),
        ("https://host", "https://host"),
        ("https://host/", "https://host/"),
        ("", ""),
        ("https://[", ""),
        ("http://host/\n", ""),
    ] {
        assert_eq!(sanitize_endpoint(raw), expected);
    }
    let (url, server, task) = endpoint(
        AdapterKind::OpenAI,
        r#"{"action":"should not run"}"#,
        StatusCode::OK,
        Duration::ZERO,
    )
    .await;
    let summarizer = Summarizer::new(Config {
        providers: vec![spec(AdapterKind::OpenAI, url)],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut limited = input();
    limited.activity = ActivityState::RateLimited;
    assert!(
        summarizer
            .summarize(&CancellationToken::new(), &limited)
            .await
            .is_fallback
    );
    assert!(server.requests.lock().await.is_empty());
    task.abort();
}
