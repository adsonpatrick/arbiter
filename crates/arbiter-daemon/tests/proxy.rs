use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};

use arbiter_core::{
    events::{ErrorClass, GovernorEventKind},
    ids::AttemptId,
};
use arbiter_daemon::{AppState, build_router, local_bind_address};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
    response::Response,
    routing::post,
};
use bytes::Bytes;
use futures_util::{StreamExt, stream};
use serde_json::json;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

struct HangingUpstreamStream {
    sent_first: bool,
    dropped: Arc<AtomicBool>,
}

impl futures_util::Stream for HangingUpstreamStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.sent_first {
            Poll::Pending
        } else {
            self.sent_first = true;
            Poll::Ready(Some(Ok(Bytes::from_static(
                b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
            ))))
        }
    }
}

impl Drop for HangingUpstreamStream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[test]
fn daemon_bind_address_is_always_ipv4_loopback() {
    let address = local_bind_address(43123);

    assert_eq!(address.ip().to_string(), "127.0.0.1");
    assert_eq!(address.port(), 43123);
}

#[tokio::test]
async fn storage_failure_before_attempt_returns_503_without_calling_upstream() {
    let upstream_calls = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&upstream_calls);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Response::new(Body::empty()) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    store.close().await;
    let app = build_router(AppState::new(provider, store));
    let request = Request::post("/v1/responses")
        .header("authorization", "Bearer daemon-storage-failure")
        .header("content-type", "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(upstream_calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn exposes_only_m0_routes_and_rejects_other_inference_surfaces() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    drop(listener);
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store));

    let health = app
        .clone()
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = app
        .clone()
        .oneshot(Request::get("/status").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let rejected = app
        .clone()
        .oneshot(
            Request::post("/v1/chat/completions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let wrong_method = app
        .oneshot(Request::get("/v1/responses").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(rejected.status(), StatusCode::NOT_FOUND);
    assert_eq!(wrong_method.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn streams_before_terminal_and_persists_start_then_completion() {
    let release_terminal = Arc::new(tokio::sync::Notify::new());
    let upstream_release = Arc::clone(&release_terminal);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let release = Arc::clone(&upstream_release);
            async move {
                let first = stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
                    ))
                });
                let terminal = stream::once(async move {
                    release.notified().await;
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(include_bytes!(
                        "../../../tests/fixtures/response_completed.sse"
                    )))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(first.chain(terminal)))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer daemon-streaming")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let mut body = response.into_body().into_data_stream();
    let first = tokio::time::timeout(std::time::Duration::from_millis(250), body.next())
        .await
        .expect("first chunk must not wait for terminal")
        .expect("first chunk")
        .expect("first chunk success");
    assert!(String::from_utf8_lossy(&first).contains("response.created"));
    assert_eq!(store.events_for_attempt(attempt_id).await.unwrap().len(), 1);

    release_terminal.notify_one();
    while let Some(chunk) = body.next().await {
        chunk.expect("terminal stream chunk");
    }
    let events = wait_for_events(&store, attempt_id, 2).await;
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].kind,
        GovernorEventKind::AttemptStarted(_)
    ));
    let GovernorEventKind::AttemptCompleted(completed) = &events[1].kind else {
        panic!("second event must complete the attempt");
    };
    assert_eq!(completed.usage.input_tokens, 120);
    assert_eq!(completed.usage.cached_input_tokens, 32);
    assert_eq!(completed.usage.output_tokens, 24);
    assert_eq!(completed.usage.reasoning_tokens, 8);
    server.abort();
}

#[tokio::test]
async fn client_cancellation_drops_upstream_and_never_records_completion() {
    let upstream_dropped = Arc::new(AtomicBool::new(false));
    let drop_signal = Arc::clone(&upstream_dropped);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let dropped = Arc::clone(&drop_signal);
            async move {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(HangingUpstreamStream {
                        sent_first: false,
                        dropped,
                    }))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer daemon-cancel")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let mut body = response.into_body().into_data_stream();
    body.next()
        .await
        .expect("first chunk")
        .expect("stream success");
    drop(body);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !upstream_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("upstream body must be dropped");
    let events = wait_for_events(&store, attempt_id, 2).await;
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].kind,
        GovernorEventKind::AttemptStarted(_)
    ));
    let GovernorEventKind::AttemptFailed(failed) = &events[1].kind else {
        panic!("cancellation must fail rather than complete the attempt");
    };
    assert_eq!(
        failed.error_class,
        arbiter_core::events::ErrorClass::Cancelled
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, GovernorEventKind::AttemptCompleted(_)))
    );
    server.abort();
}

#[tokio::test]
async fn client_cancellation_before_upstream_headers_records_cancelled() {
    let upstream_entered = Arc::new(tokio::sync::Notify::new());
    let entered = Arc::clone(&upstream_entered);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let entered = Arc::clone(&entered);
            async move {
                entered.notify_one();
                std::future::pending::<Response>().await
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer pre-headers-cancellation")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let request_task = tokio::spawn(async move { app.oneshot(request).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        upstream_entered.notified(),
    )
    .await
    .expect("request must reach the upstream before cancellation");
    let started = store.latest_attempt_events().await.unwrap();
    assert_eq!(started.len(), 1);
    let attempt_id = started[0].attempt_id();

    request_task.abort();
    let _ = request_task.await;

    let events = wait_for_events(&store, attempt_id, 2).await;
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[1].kind,
        GovernorEventKind::AttemptFailed(failed)
            if failed.error_class == ErrorClass::Cancelled
    ));
    server.abort();
}

#[tokio::test]
async fn provider_setup_failure_records_started_then_failed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    drop(listener);
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let events = store.events_for_attempt(attempt_id).await.unwrap();
    assert_eq!(events.len(), 2);
    let GovernorEventKind::AttemptFailed(failed) = &events[1].kind else {
        panic!("provider setup failure must terminate the attempt");
    };
    assert_eq!(
        failed.error_class,
        arbiter_core::events::ErrorClass::ProviderSetup
    );
}

#[tokio::test]
async fn premature_upstream_eof_records_stream_interrupted_not_completed() {
    let upstream = Router::new().route(
        "/responses",
        post(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from(
                    "event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
                ))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer daemon-interrupted")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let mut body = response.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        chunk.expect("first chunk remains available");
    }

    let events = wait_for_events(&store, attempt_id, 2).await;
    assert_eq!(events.len(), 2);
    let GovernorEventKind::AttemptFailed(failed) = &events[1].kind else {
        panic!("premature EOF must fail the attempt");
    };
    assert_eq!(
        failed.error_class,
        arbiter_core::events::ErrorClass::StreamInterrupted
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, GovernorEventKind::AttemptCompleted(_)))
    );
    server.abort();
}

#[tokio::test]
async fn upstream_connect_failure_records_provider_connect() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    drop(listener);
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer daemon-connect")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let events = store.events_for_attempt(attempt_id).await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "a connect failure must not retry or fall back"
    );
    let GovernorEventKind::AttemptStarted(started) = &events[0].kind else {
        panic!("first event must start the only attempt");
    };
    assert_eq!(started.attempt_index, 0);
    assert_eq!(started.target, arbiter_core::config::BaselineTarget::m0());
    let GovernorEventKind::AttemptFailed(failed) = &events[1].kind else {
        panic!("connect error must fail the attempt");
    };
    assert_eq!(
        failed.error_class,
        arbiter_core::events::ErrorClass::ProviderConnect
    );
    assert_eq!(failed.attempt_index, 0);
    assert_eq!(failed.target, arbiter_core::config::BaselineTarget::m0());
}

#[tokio::test]
async fn duplicate_terminal_chunks_create_exactly_one_terminal_event() {
    let upstream = Router::new().route(
        "/responses",
        post(|| async {
            let first = stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(include_bytes!(
                    "../../../tests/fixtures/response_completed.sse"
                )))
            });
            let second = stream::once(async {
                tokio::task::yield_now().await;
                Ok::<_, std::convert::Infallible>(Bytes::from_static(include_bytes!(
                    "../../../tests/fixtures/response_completed.sse"
                )))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(first.chain(second)))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer duplicate-terminal")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let mut body = response.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        chunk.expect("duplicate terminal bytes remain transparent");
    }
    wait_for_events(&store, attempt_id, 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    assert_eq!(store.events_for_attempt(attempt_id).await.unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn completed_metadata_followed_by_transport_error_keeps_one_completion() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0_u8; 4_096];
        let _ = socket.read(&mut request).await.unwrap();
        let completed = include_bytes!("../../../tests/fixtures/response_completed.sse");
        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        socket.write_all(headers).await.unwrap();
        socket
            .write_all(format!("{:X}\r\n", completed.len()).as_bytes())
            .await
            .unwrap();
        socket.write_all(completed).await.unwrap();
        socket.write_all(b"\r\nZZ\r\n").await.unwrap();
        socket.flush().await.unwrap();
    });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer completed-then-error")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let chunks = response
        .into_body()
        .into_data_stream()
        .collect::<Vec<_>>()
        .await;
    assert!(chunks.iter().any(Result::is_ok));
    assert!(chunks.iter().any(Result::is_err));
    let events = wait_for_events(&store, attempt_id, 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;

    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[1].kind,
        GovernorEventKind::AttemptCompleted(_)
    ));
    assert_eq!(store.events_for_attempt(attempt_id).await.unwrap().len(), 2);
    server.abort();
}

#[tokio::test]
async fn provider_http_failures_are_transparent_and_classified() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let expected_body = format!("provider failure {}", status.as_u16());
        let upstream_body = expected_body.clone();
        let upstream = Router::new().route(
            "/responses",
            post(move || {
                let body = upstream_body.clone();
                async move {
                    Response::builder()
                        .status(status)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
            .expect("test provider");
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let temporary = tempdir().unwrap();
        let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
            .await
            .unwrap();
        let app = build_router(AppState::new(provider, store.clone()));
        let request = Request::post("/v1/responses")
            .header(header::AUTHORIZATION, "Bearer provider-http")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"input": "hello"}).to_string()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        let attempt_id = AttemptId::from(
            response.headers()["x-arbiter-attempt-id"]
                .to_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, expected_body);
        let events = wait_for_events(&store, attempt_id, 2).await;
        let GovernorEventKind::AttemptFailed(failed) = &events[1].kind else {
            panic!("provider HTTP response must fail the attempt");
        };
        assert_eq!(failed.error_class, ErrorClass::ProviderHttp);
        server.abort();
    }
}

#[tokio::test]
async fn permanent_terminal_write_failure_is_recovered_on_restart() {
    let release_terminal = Arc::new(tokio::sync::Notify::new());
    let upstream_release = Arc::clone(&release_terminal);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let release = Arc::clone(&upstream_release);
            async move {
                let first = stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n",
                    ))
                });
                let terminal = stream::once(async move {
                    release.notified().await;
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(include_bytes!(
                        "../../../tests/fixtures/response_completed.sse"
                    )))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(first.chain(terminal)))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("arbiter.db");
    let store = SqliteEventStore::open(&database).await.unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer recovery")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let mut body = response.into_body().into_data_stream();
    body.next().await.unwrap().unwrap();
    store.close().await;
    release_terminal.notify_one();
    while body.next().await.is_some() {}
    tokio::time::sleep(std::time::Duration::from_millis(125)).await;

    let recovered = SqliteEventStore::open(&database).await.unwrap();
    assert_eq!(
        recovered
            .reconcile_incomplete_attempts(10_000)
            .await
            .unwrap(),
        1
    );
    let events = recovered.events_for_attempt(attempt_id).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[1].kind,
        GovernorEventKind::AttemptFailed(failed)
            if failed.error_class == ErrorClass::StreamInterrupted
    ));
    server.abort();
}

#[tokio::test]
async fn oversized_sse_is_forwarded_exactly_and_fails_when_completion_is_unverifiable() {
    let oversized = Bytes::from(vec![b'x'; 1_048_577]);
    let completed = Bytes::from_static(include_bytes!(
        "../../../tests/fixtures/response_completed.sse"
    ));
    let mut expected = Vec::with_capacity(oversized.len() + completed.len());
    expected.extend_from_slice(&oversized);
    expected.extend_from_slice(&completed);
    let upstream_oversized = oversized.clone();
    let upstream_completed = completed.clone();
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let chunks = stream::iter([
                Ok::<_, std::convert::Infallible>(upstream_oversized.clone()),
                Ok(upstream_completed.clone()),
            ]);
            async move {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap())
        .expect("test provider");
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let store = SqliteEventStore::open(temporary.path().join("arbiter.db"))
        .await
        .unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, "Bearer oversized-sse")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": "hello"}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = AttemptId::from(
        response.headers()["x-arbiter-attempt-id"]
            .to_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), expected);
    let events = wait_for_events(&store, attempt_id, 2).await;
    assert!(matches!(
        &events[1].kind,
        GovernorEventKind::AttemptFailed(failed)
            if failed.error_class == ErrorClass::StreamInterrupted
    ));
    server.abort();
}

async fn wait_for_events(
    store: &SqliteEventStore,
    attempt_id: AttemptId,
    expected: usize,
) -> Vec<arbiter_core::events::GovernorEvent> {
    for _ in 0..100 {
        let events = store.events_for_attempt(attempt_id).await.unwrap();
        if events.len() >= expected {
            return events;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    store.events_for_attempt(attempt_id).await.unwrap()
}
