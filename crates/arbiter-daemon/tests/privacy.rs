use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

use arbiter_daemon::{AppState, build_router, json_subscriber};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use axum::{
    Router,
    body::Body,
    http::{Request, header},
    response::Response,
    routing::post,
};
use serde_json::json;
use tempfile::tempdir;
use tower::ServiceExt;
use tracing_subscriber::{EnvFilter, fmt::MakeWriter};

const BEARER: &str = "Bearer privacy-bearer-sentinel";
const COOKIE: &str = "privacy-cookie-sentinel";
const SOURCE: &str = "privacy-source-sentinel";
const RESPONSE_CONTENT: &str = "privacy-response-sentinel";

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn secrets_and_content_never_reach_json_logs_or_sqlite() {
    let logs = LogBuffer::default();
    let subscriber = json_subscriber(logs.clone(), EnvFilter::new("arbiter_daemon=info"));
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);
    let terminal = String::from_utf8_lossy(include_bytes!(
        "../../../tests/fixtures/response_completed.sse"
    ));
    let upstream_body = format!(
        "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{RESPONSE_CONTENT}\"}}\n\n{terminal}"
    );
    let upstream = Router::new().route(
        "/responses",
        post(move || {
            let body = upstream_body.clone();
            async move {
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(body))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider =
        CodexUpstreamProvider::new_for_loopback_test(listener.local_addr().unwrap()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    let temporary = tempdir().unwrap();
    let database = temporary.path().join("arbiter.db");
    let store = SqliteEventStore::open(&database).await.unwrap();
    let app = build_router(AppState::new(provider, store.clone()));
    let request = Request::post("/v1/responses")
        .header(header::AUTHORIZATION, BEARER)
        .header(header::COOKIE, COOKIE)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"input": SOURCE}).to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let attempt_id = response.headers()["x-arbiter-attempt-id"]
        .to_str()
        .unwrap()
        .to_owned();
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    for _ in 0..100 {
        if logs.contents().contains("attempt_completed") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    store.close().await;
    server.abort();

    let log_output = logs.contents();
    assert!(log_output.contains(&attempt_id));
    assert!(log_output.contains("gpt-5.6-terra"));
    assert!(log_output.contains("attempt_completed"));
    for forbidden in [BEARER, COOKIE, SOURCE, RESPONSE_CONTENT] {
        assert!(!log_output.contains(forbidden));
    }

    let mut persisted = Vec::new();
    for entry in std::fs::read_dir(temporary.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            persisted.extend(std::fs::read(path).unwrap());
        }
    }
    let persisted = String::from_utf8_lossy(&persisted);
    for forbidden in [BEARER, COOKIE, SOURCE, RESPONSE_CONTENT] {
        assert!(!persisted.contains(forbidden));
    }
}
