use agent_debug::client::ServerClient;
use axum::{
    Router,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (address, task)
}

#[tokio::test]
async fn authenticated_requests_preserve_base_path_and_escape_agent_identity() {
    let hits = Arc::new(AtomicUsize::new(0));
    let captured = hits.clone();
    let router = Router::new()
        .route(
            "/base/api/v1/agents",
            get(
                move |headers: HeaderMap,
                      axum::extract::Query(query): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| async move {
                    assert_eq!(headers["accept"], "application/json");
                    assert_eq!(headers["authorization"], "Bearer local-fixture");
                    assert_eq!(query["lifecycle"], "RUNNING");
                    captured.fetch_add(1, Ordering::SeqCst);
                    "[]"
                },
            ),
        )
        .route(
            "/base/api/v1/agents/{id}/usage",
            get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    assert_eq!(id, "env:pi:session/with space");
                    axum::Json(agent_common::usage::Report::new(
                        id,
                        agent_common::types::HarnessType::Pi,
                    ))
                },
            ),
        );
    let (base, task) = server(router).await;
    let client = ServerClient::new(
        &format!("{base}/base"),
        "local-fixture",
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(
        client
            .agents(&CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        client
            .usage(&CancellationToken::new(), "env:pi:session/with space")
            .await
            .unwrap()
            .agent_id,
        "env:pi:session/with space"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn rejects_redirects_auth_failures_bad_json_and_mismatched_usage_safely() {
    for (status, body) in [
        (StatusCode::FOUND, "secret-response"),
        (StatusCode::UNAUTHORIZED, "secret-response"),
        (StatusCode::FORBIDDEN, "secret-response"),
        (StatusCode::BAD_GATEWAY, "secret-response"),
        (StatusCode::OK, "{secret-response"),
        (StatusCode::OK, "[] []"),
    ] {
        let (base, task) = server(Router::new().fallback(move || async move {
            (
                status,
                [("location", "http://sensitive:credential@invalid")],
                body,
            )
                .into_response()
        }))
        .await;
        let client = ServerClient::new(&base, "local-fixture", Duration::from_secs(1)).unwrap();
        let error = client
            .agents(&CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !error.contains("secret-response")
                && !error.contains("credential")
                && !error.contains("local-fixture")
        );
        assert!(error.starts_with("GET http://127.0.0.1:"));
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            assert!(error.contains("AGENT_STATUS_TOKEN"));
        }
        task.abort();
    }
    let (base, task) = server(Router::new().fallback(|| async {
        axum::Json(agent_common::usage::Report::new(
            "wrong",
            agent_common::types::HarnessType::Pi,
        ))
    }))
    .await;
    let client = ServerClient::new(&base, "", Duration::from_secs(1)).unwrap();
    assert!(
        client
            .usage(&CancellationToken::new(), "expected")
            .await
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    task.abort();
}

#[tokio::test]
async fn deadlines_cancellation_and_connection_failures_are_bounded() {
    let (base, task) = server(Router::new().fallback(|| async {
        tokio::time::sleep(Duration::from_secs(3)).await;
        "[]"
    }))
    .await;
    let client = ServerClient::new(&base, "", Duration::from_millis(25)).unwrap();
    assert!(
        client
            .agents(&CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    let client = ServerClient::new(&base, "", Duration::from_secs(3)).unwrap();
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        signal.cancel()
    });
    assert!(
        client
            .agents(&cancel)
            .await
            .unwrap_err()
            .to_string()
            .contains("cancelled")
    );
    task.abort();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(
        ServerClient::new(&address, "", Duration::from_secs(1))
            .unwrap()
            .agents(&CancellationToken::new())
            .await
            .is_err()
    );
}

#[test]
fn validates_configuration_without_echoing_credentials() {
    for url in [
        "file:///secret",
        "https://user:password@host",
        "https://host?",
        "https://host?q=x",
        "https://host/#secret",
        "bad",
    ] {
        assert!(ServerClient::new(url, "", Duration::from_secs(1)).is_err());
    }
    assert!(
        ServerClient::new(
            "http://localhost",
            "invalid\r\nheader",
            Duration::from_secs(1)
        )
        .is_err()
    );
}
