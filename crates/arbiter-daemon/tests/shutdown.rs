use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use arbiter_core::events::{ErrorClass, GovernorEventKind};
use arbiter_daemon::{AppState, serve_listener_with_shutdown};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use axum::{Router, body::Body, http::header, response::Response, routing::post};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::json;
use tempfile::tempdir;

struct HangingStream {
    sent_first: bool,
    dropped: Arc<AtomicBool>,
}

impl futures_util::Stream for HangingStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.sent_first {
            Poll::Pending
        } else {
            self.sent_first = true;
            Poll::Ready(Some(Ok(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n",
            ))))
        }
    }
}

impl Drop for HangingStream {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shutdown_rejects_new_work_and_bounds_a_hanging_attempt() {
    let upstream_dropped = Arc::new(AtomicBool::new(false));
    let drop_observer = Arc::clone(&upstream_dropped);
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let dropped = Arc::clone(&drop_observer);
            async move {
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(HangingStream {
                        sent_first: false,
                        dropped,
                    }))
                    .unwrap()
            }
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider =
        CodexUpstreamProvider::new_for_loopback_test(upstream_listener.local_addr().unwrap())
            .unwrap();
    let upstream_task =
        tokio::spawn(async move { axum::serve(upstream_listener, upstream).await.unwrap() });

    let temporary = tempdir().unwrap();
    let database = temporary.path().join("arbiter.db");
    let store = SqliteEventStore::open(&database).await.unwrap();
    let state = AppState::new(provider, store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let daemon = tokio::spawn(serve_listener_with_shutdown(
        state,
        listener,
        async move {
            let _ = shutdown_rx.await;
        },
        Duration::from_millis(500),
    ));

    let client = reqwest::Client::new();
    let first = client
        .post(format!("http://{address}/v1/responses"))
        .header(header::AUTHORIZATION, "Bearer shutdown-test")
        .json(&json!({"input": "hang"}))
        .send()
        .await
        .unwrap();
    let mut first_body = first.bytes_stream();
    let first_chunk = tokio::time::timeout(Duration::from_millis(250), first_body.next())
        .await
        .expect("first chunk before shutdown")
        .expect("first chunk exists")
        .expect("first chunk succeeds");
    assert!(!first_chunk.is_empty());

    shutdown_tx.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = reqwest::Client::new()
        .post(format!("http://{address}/v1/responses"))
        .header(header::AUTHORIZATION, "Bearer rejected-after-shutdown")
        .json(&json!({"input": "reject"}))
        .send()
        .await;
    assert!(second.is_err() || second.unwrap().status().is_server_error());

    tokio::time::timeout(Duration::from_secs(2), daemon)
        .await
        .expect("bounded shutdown")
        .expect("daemon task")
        .expect("daemon shutdown");
    assert!(upstream_dropped.load(Ordering::SeqCst));

    drop(first_body);
    let reopened = SqliteEventStore::open(&database).await.unwrap();
    let events = reopened.latest_attempt_events().await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0].kind,
        GovernorEventKind::AttemptStarted(_)
    ));
    assert!(matches!(
        events[1].kind,
        GovernorEventKind::AttemptFailed(ref failed)
            if failed.error_class == ErrorClass::Cancelled
    ));

    upstream_task.abort();
}
