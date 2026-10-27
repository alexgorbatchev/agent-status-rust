use agent_server::{
    api::{Api, ApiConfig},
    hub::Hub,
    store::ReadStore,
};
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

async fn api(config: ApiConfig) -> (tempfile::TempDir, ReadStore, Api) {
    let directory = tempfile::tempdir_in(".tmp").unwrap();
    let database = directory.path().join("state.db");
    agent_status_store::open_writer(&database).unwrap();
    let token = CancellationToken::new();
    let store = ReadStore::open(&database, &token).await.unwrap();
    let api = Api::new(store.clone(), Hub::default(), config, token).unwrap();
    (directory, store, api)
}

async fn request_bytes(stream: &mut TcpStream) -> Vec<u8> {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>().unwrap();
        }
        bytes.extend_from_slice(line.as_bytes());
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    bytes.extend(body);
    bytes
}

#[tokio::test]
async fn real_proxy_preserves_method_encoded_segment_exact_body_and_response_status() {
    for (method, suffix, body, status, response_body) in [
        (
            "PATCH",
            "",
            " {\"isHidden\":false,\"dismissCompletion\":true,\"extra\":7} ",
            409,
            "{\"error\":\"conflict\"}",
        ),
        (
            "POST",
            "/focus",
            "",
            200,
            "{\"ok\":true,\"paneId\":\"w1:p1\"}",
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request_bytes(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let (_directory, store, api) = api(ApiConfig {
            ingester_url: format!("http://{address}"),
            auth_token: "configured".into(),
            ..Default::default()
        })
        .await;
        let response=api.router().oneshot(Request::builder().method(method).uri(format!("/api/v1/agents/env%3Aclaude-code%3Asession%2520literal{suffix}?token=configured")).body(Body::from(body)).unwrap()).await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["content-type"], "application/json");
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            response_body
        );
        let bytes = tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        let request = String::from_utf8(bytes).unwrap();
        assert!(request.starts_with(&format!("{method} /api/v1/internal/agents/env:claude-code:session%2520literal{suffix} HTTP/1.1\r\n")),"{request}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer configured\r\n")
        );
        assert!(request.ends_with(body));
        api.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn forwarding_without_configured_auth_retains_incoming_header_only() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let bytes = request_bytes(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        bytes
    });
    let (_directory, store, api) = api(ApiConfig {
        ingester_url: format!("http://{address}"),
        ..Default::default()
    })
    .await;
    let response = api
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents/a/focus?token=do-not-forward-query")
                .header("authorization", "Bearer incoming")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    let request = String::from_utf8(server.await.unwrap())
        .unwrap()
        .to_ascii_lowercase();
    assert!(request.contains("authorization: bearer incoming\r\n"));
    assert!(!request.contains("do-not-forward-query"));
    api.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn truncated_response_is_a_gateway_failure_before_partial_success_escapes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = request_bytes(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"ok\":",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let (_directory, store, api) = api(ApiConfig {
        ingester_url: format!("http://{address}"),
        ..Default::default()
    })
    .await;
    let response = api
        .router()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents/a/focus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let value: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(value["error"], "failed to read ingester response");
    server.await.unwrap();
    api.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn mutable_redirects_are_returned_without_replay_or_credential_forwarding() {
    let forbidden = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let forbidden_address = forbidden.local_addr().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = request_bytes(&mut stream).await;
        let response = format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{forbidden_address}/should-not-replay\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let (_directory, store, api) = api(ApiConfig {
        ingester_url: format!("http://{address}"),
        ..Default::default()
    })
    .await;
    let response = api
        .router()
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/agents/a")
                .header("authorization", "Bearer incoming")
                .body(Body::from("{\"isHidden\":true}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 307);
    assert!(!response.headers().contains_key("location"));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), forbidden.accept())
            .await
            .is_err()
    );
    server.await.unwrap();
    api.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn proxy_deadline_and_outer_rest_deadline_are_both_enforced() {
    for (proxy_timeout, rest_timeout, status) in [
        (Duration::from_millis(20), Duration::from_secs(2), 502),
        (Duration::from_secs(2), Duration::from_millis(20), 504),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = request_bytes(&mut stream).await;
            let _ = accepted.send(());
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
        });
        let (_directory, store, api) = api(ApiConfig {
            ingester_url: format!("http://{address}"),
            proxy_timeout,
            rest_timeout,
            ..Default::default()
        })
        .await;
        let response = api
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/agents/a/focus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        ready.await.unwrap();
        assert_eq!(response.status().as_u16(), status);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        api.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn patch_preserves_large_valid_json_without_an_unrequested_body_limit() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = request_bytes(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
        request
    }));
    let (_directory, store, api) = api(ApiConfig {
        ingester_url: format!("http://{address}"),
        ..Default::default()
    })
    .await;
    let body = format!(
        "{{\"isHidden\":false,\"extra\":\"{}\"}}",
        "a".repeat(2 * 1024 * 1024)
    );
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        api.router().oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/api/v1/agents/a")
                .body(Body::from(body.clone()))
                .unwrap(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 200);
    let request = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    assert!(request.ends_with(body.as_bytes()));
    api.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn invalid_targets_report_safe_errors_without_network_access() {
    for target in ["invalid target", "mailto:user@example.test"] {
        let (_directory, store, api) = api(ApiConfig {
            ingester_url: target.into(),
            ..Default::default()
        })
        .await;
        let response = api
            .router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/agents/a/focus")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 500);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "{\"error\":\"failed to create proxy request\"}"
        );
        api.shutdown().await;
        store.shutdown().await;
    }
}

#[tokio::test]
async fn service_cancellation_closes_mutation_transport_while_headers_or_body_are_pending() {
    for body_pending in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (accepted, ready) = tokio::sync::oneshot::channel();
        let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = request_bytes(&mut stream).await;
            if body_pending {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nx")
                    .await
                    .unwrap();
            }
            accepted.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
        }));
        let (_directory, store, api) = api(ApiConfig {
            ingester_url: format!("http://{address}"),
            ..Default::default()
        })
        .await;
        let router = api.router();
        let request = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
            router
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/agents/a/focus")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }));
        tokio::time::timeout(Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
        api.shutdown().await;
        let response = tokio::time::timeout(Duration::from_secs(3), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 502);
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        store.shutdown().await;
    }
}
