use arbiter_core::config::BaselineTarget;
use bytes::Bytes;
use futures_util::Stream;
use http::{HeaderMap, StatusCode};
use serde_json::Value;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use crate::sse::{SseMetadataParser, TerminalResponseMetadata};

pub const CODEX_UPSTREAM_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    #[error("failed to build the Codex upstream HTTP client")]
    ClientSetup(#[source] reqwest::Error),
    #[error("the localhost Codex request did not include authorization")]
    MissingAuthorization,
    #[error("the Responses request must be a JSON object")]
    InvalidRequest,
    #[error("Codex upstream request failed")]
    Upstream(#[source] reqwest::Error),
    #[cfg(feature = "test-support")]
    #[error("test upstream must use a loopback address")]
    NonLoopbackTestEndpoint,
}

pub struct ProviderChunk {
    pub bytes: Bytes,
    pub terminal_metadata: Option<TerminalResponseMetadata>,
}

impl std::fmt::Debug for ProviderChunk {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderChunk")
            .field("bytes_len", &self.bytes.len())
            .field("terminal_metadata", &self.terminal_metadata)
            .finish()
    }
}

pub type ProviderByteStream = std::pin::Pin<
    Box<dyn futures_util::Stream<Item = Result<ProviderChunk, ProviderError>> + Send>,
>;

struct ParsedByteStream {
    upstream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    parser: SseMetadataParser,
}

impl Stream for ParsedByteStream {
    type Item = Result<ProviderChunk, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.upstream.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(bytes))) => {
                let terminal_metadata = self.parser.push(&bytes);
                Poll::Ready(Some(Ok(ProviderChunk {
                    bytes,
                    terminal_metadata,
                })))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(ProviderError::Upstream(error)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct ProviderResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    stream: ProviderByteStream,
}

impl ProviderResponse {
    #[must_use]
    pub fn bytes_stream(self) -> ProviderByteStream {
        self.stream
    }
}

pub struct CodexUpstreamProvider {
    client: reqwest::Client,
    endpoint: String,
}

impl std::fmt::Debug for CodexUpstreamProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexUpstreamProvider")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl CodexUpstreamProvider {
    /// Creates a provider pinned to the first-party Codex Responses endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`ProviderError::ClientSetup`] when the redirect-disabled HTTP
    /// client cannot be constructed.
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_endpoint(CODEX_UPSTREAM_RESPONSES_URL)
    }

    fn with_endpoint(endpoint: &'static str) -> Result<Self, ProviderError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProviderError::ClientSetup)?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
        })
    }

    /// Creates a loopback-only provider for deterministic integration tests.
    ///
    /// # Errors
    ///
    /// Returns an error for non-loopback addresses or HTTP client setup failure.
    #[cfg(feature = "test-support")]
    pub fn new_for_loopback_test(address: std::net::SocketAddr) -> Result<Self, ProviderError> {
        if !address.ip().is_loopback() {
            return Err(ProviderError::NonLoopbackTestEndpoint);
        }
        let endpoint = format!("http://{address}/responses");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProviderError::ClientSetup)?;
        Ok(Self { client, endpoint })
    }

    /// Sends exactly one normalized Responses request and returns its byte stream.
    ///
    /// # Errors
    ///
    /// Returns an error when authorization is missing, the payload is not a JSON
    /// object, or the single upstream HTTP request cannot be established.
    pub async fn forward(
        &self,
        incoming_headers: &HeaderMap,
        request: Value,
    ) -> Result<ProviderResponse, ProviderError> {
        if !request.is_object() {
            return Err(ProviderError::InvalidRequest);
        }

        let request = normalize_request(request);
        let headers = forwarded_request_headers(incoming_headers)?;
        let response = self
            .client
            .post(&self.endpoint)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(ProviderError::Upstream)?;
        let status = response.status();
        let headers = safe_response_headers(response.headers());
        let stream = ParsedByteStream {
            upstream: Box::pin(response.bytes_stream()),
            parser: SseMetadataParser::default(),
        };

        Ok(ProviderResponse {
            status,
            headers,
            stream: Box::pin(stream),
        })
    }
}

/// Request headers required by the current Codex Responses contract.
///
/// `authorization`, `chatgpt-account-id`, and `x-oai-attestation` are secret or
/// account-bound values. They are copied directly into one outbound request and
/// are never retained by the provider. The remaining names carry content type,
/// client version/origin, session correlation, turn metadata, routing hints, or
/// Responses feature negotiation. No arbitrary caller header is forwarded.
const FORWARDED_REQUEST_HEADERS: &[&str] = &[
    "authorization",
    "accept",
    "content-type",
    "user-agent",
    "chatgpt-account-id",
    "openai-organization",
    "openai-project",
    "openai-beta",
    "originator",
    "version",
    "session-id",
    "thread-id",
    "x-client-request-id",
    "x-codex-installation-id",
    "x-codex-routing-hint",
    "x-codex-turn-state",
    "x-codex-turn-metadata",
    "x-codex-parent-thread-id",
    "x-codex-window-id",
    "x-openai-subagent",
    "x-openai-memgen-request",
    "x-responsesapi-include-timing-metrics",
    "x-oai-attestation",
];

fn forwarded_request_headers(incoming: &HeaderMap) -> Result<HeaderMap, ProviderError> {
    if !incoming.contains_key(http::header::AUTHORIZATION) {
        return Err(ProviderError::MissingAuthorization);
    }

    let mut forwarded = HeaderMap::new();
    for name in FORWARDED_REQUEST_HEADERS {
        if let Some(value) = incoming.get(*name) {
            forwarded.insert(http::HeaderName::from_static(name), value.clone());
        }
    }
    Ok(forwarded)
}

const FORWARDED_RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "cache-control",
    "request-id",
    "x-request-id",
    "openai-processing-ms",
    "x-codex-turn-state",
    "x-reasoning-included",
];

fn safe_response_headers(upstream: &HeaderMap) -> HeaderMap {
    let mut safe = HeaderMap::new();
    for name in FORWARDED_RESPONSE_HEADERS {
        if let Some(value) = upstream.get(*name) {
            safe.insert(http::HeaderName::from_static(name), value.clone());
        }
    }
    safe
}

fn normalize_request(mut request: Value) -> Value {
    let Some(object) = request.as_object_mut() else {
        return request;
    };
    let target = BaselineTarget::m0();
    object.insert("model".to_owned(), Value::String(target.model));

    let reasoning = object
        .entry("reasoning")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !reasoning.is_object() {
        *reasoning = Value::Object(serde_json::Map::new());
    }
    reasoning
        .as_object_mut()
        .expect("object was just ensured")
        .insert(
            "effort".to_owned(),
            serde_json::to_value(target.reasoning_effort)
                .expect("ReasoningEffort serialization is infallible"),
        );

    request
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, body::Body, extract::State, response::Response, routing::post};
    use futures_util::StreamExt;
    use http::{HeaderMap, HeaderValue, StatusCode, header};
    use serde_json::json;

    use super::{CodexUpstreamProvider, normalize_request};

    #[derive(Debug)]
    struct CapturedRequest {
        headers: HeaderMap,
        body: serde_json::Value,
    }

    async fn capture_request(
        State(captured): State<Arc<Mutex<Option<CapturedRequest>>>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> Response {
        *captured.lock().expect("capture mutex") = Some(CapturedRequest { headers, body });
        Response::builder()
            .status(200)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header("x-request-id", "request-metadata")
            .header(header::SET_COOKIE, "must-not-reach-codex=secret")
            .body(Body::from(
                &include_bytes!("../../../tests/fixtures/response_completed.sse")[..],
            ))
            .expect("fake response")
    }

    async fn fake_upstream() -> (
        &'static str,
        Arc<Mutex<Option<CapturedRequest>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/responses", post(capture_request))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake upstream");
        let endpoint = format!("http://{}/responses", listener.local_addr().unwrap());
        let endpoint = Box::leak(endpoint.into_boxed_str());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("fake upstream");
        });
        (endpoint, captured, server)
    }

    fn authorized_headers(secret: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static(secret));
        headers
    }

    #[test]
    fn normalizes_only_the_m0_target_and_preserves_unknown_fields() {
        let request = json!({
            "model": "caller-model",
            "reasoning": {
                "effort": "low",
                "summary": "auto",
                "future_reasoning_field": true
            },
            "input": [{"role": "user", "content": "source-sentinel"}],
            "future_top_level_field": {"nested": 42}
        });

        let normalized = normalize_request(request);

        assert_eq!(normalized["model"], "gpt-5.6-terra");
        assert_eq!(normalized["reasoning"]["effort"], "medium");
        assert_eq!(normalized["reasoning"]["summary"], "auto");
        assert_eq!(normalized["reasoning"]["future_reasoning_field"], true);
        assert_eq!(normalized["input"][0]["content"], "source-sentinel");
        assert_eq!(normalized["future_top_level_field"]["nested"], 42);
    }

    #[tokio::test]
    async fn forwards_only_allowlisted_headers_and_streams_terminal_metadata() {
        let (endpoint, captured, server) = fake_upstream().await;
        let provider = CodexUpstreamProvider::with_endpoint(endpoint).expect("provider");
        let secret = "Bearer task4-secret-sentinel";
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static(secret));
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_static("account-sentinel"),
        );
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("session-id", HeaderValue::from_static("session-123"));
        headers.insert(header::COOKIE, HeaderValue::from_static("cookie-secret"));
        headers.insert(
            "proxy-authorization",
            HeaderValue::from_static("proxy-secret"),
        );
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::HOST, HeaderValue::from_static("attacker.invalid"));
        headers.insert("x-not-allowlisted", HeaderValue::from_static("blocked"));

        let response = provider
            .forward(
                &headers,
                json!({
                    "model": "caller-model",
                    "reasoning": {"effort": "low", "summary": "auto"},
                    "input": "source-sentinel",
                    "future_field": 42
                }),
            )
            .await
            .expect("forward response");
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.headers[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(response.headers["x-request-id"], "request-metadata");
        assert!(!response.headers.contains_key(header::SET_COOKIE));

        let chunks = response.bytes_stream().collect::<Vec<_>>().await;
        let forwarded_body = chunks
            .iter()
            .filter_map(|chunk| chunk.as_ref().ok())
            .flat_map(|chunk| chunk.bytes.iter().copied())
            .collect::<Vec<_>>();
        assert_eq!(
            forwarded_body,
            include_bytes!("../../../tests/fixtures/response_completed.sse")
        );
        let terminal = chunks
            .iter()
            .filter_map(|chunk| chunk.as_ref().ok())
            .find_map(|chunk| chunk.terminal_metadata.as_ref())
            .expect("terminal metadata");
        assert_eq!(terminal.response_id, "resp_arbiter_m0");
        for chunk in &chunks {
            let chunk = chunk.as_ref().expect("successful upstream chunk");
            assert!(!format!("{chunk:?}").contains("arbiter-ok"));
        }

        let captured = captured
            .lock()
            .expect("capture mutex")
            .take()
            .expect("captured request");
        assert_eq!(captured.headers[header::AUTHORIZATION], secret);
        assert_eq!(captured.headers["chatgpt-account-id"], "account-sentinel");
        assert_eq!(captured.headers["originator"], "codex_cli_rs");
        assert_eq!(captured.headers["session-id"], "session-123");
        assert!(!captured.headers.contains_key(header::COOKIE));
        assert!(!captured.headers.contains_key("proxy-authorization"));
        assert!(!captured.headers.contains_key(header::CONNECTION));
        assert_ne!(captured.headers[header::HOST], "attacker.invalid");
        assert!(!captured.headers.contains_key("x-not-allowlisted"));
        assert_eq!(captured.body["model"], "gpt-5.6-terra");
        assert_eq!(captured.body["reasoning"]["effort"], "medium");
        assert_eq!(captured.body["reasoning"]["summary"], "auto");
        assert_eq!(captured.body["future_field"], 42);
        assert!(!format!("{provider:?}").contains(secret));
        server.abort();
    }

    #[tokio::test]
    async fn production_provider_is_pinned_and_rejects_invalid_local_requests_before_network() {
        let provider = CodexUpstreamProvider::new().unwrap();
        assert!(format!("{provider:?}").contains(super::CODEX_UPSTREAM_RESPONSES_URL));

        let missing_auth = provider.forward(&HeaderMap::new(), json!({})).await;
        assert!(matches!(
            missing_auth,
            Err(super::ProviderError::MissingAuthorization)
        ));

        let invalid_body = provider
            .forward(
                &authorized_headers("Bearer invalid-body-secret"),
                serde_json::Value::Null,
            )
            .await;
        assert!(matches!(
            invalid_body,
            Err(super::ProviderError::InvalidRequest)
        ));
    }

    #[tokio::test]
    async fn does_not_follow_redirects_or_forward_authorization_to_their_target() {
        async fn redirect() -> Response {
            Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(header::LOCATION, "/credential-sink")
                .body(Body::empty())
                .unwrap()
        }

        async fn credential_sink(State(hits): State<Arc<AtomicUsize>>) -> Response {
            hits.fetch_add(1, Ordering::SeqCst);
            Response::new(Body::empty())
        }

        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/responses", post(redirect))
            .route("/credential-sink", post(credential_sink))
            .with_state(Arc::clone(&hits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Box::leak(
            format!("http://{}/responses", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = CodexUpstreamProvider::with_endpoint(endpoint).unwrap();

        let response = provider
            .forward(
                &authorized_headers("Bearer redirect-secret"),
                json!({"input": "hello"}),
            )
            .await
            .unwrap();

        assert_eq!(response.status, StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn connection_errors_do_not_expose_authorization_and_are_not_retried() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Box::leak(
            format!("http://{}/responses", listener.local_addr().unwrap()).into_boxed_str(),
        );
        drop(listener);
        let provider = CodexUpstreamProvider::with_endpoint(endpoint).unwrap();
        let secret = "Bearer connection-error-secret";

        let Err(error) = provider
            .forward(&authorized_headers(secret), json!({"input": "hello"}))
            .await
        else {
            panic!("closed port must fail");
        };

        assert!(!format!("{error}").contains(secret));
        assert!(!format!("{error:?}").contains(secret));
    }

    #[tokio::test]
    async fn performs_one_upstream_call_for_an_http_failure() {
        async fn fail(State(hits): State<Arc<AtomicUsize>>) -> Response {
            hits.fetch_add(1, Ordering::SeqCst);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .unwrap()
        }

        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/responses", post(fail))
            .with_state(Arc::clone(&hits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Box::leak(
            format!("http://{}/responses", listener.local_addr().unwrap()).into_boxed_str(),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let provider = CodexUpstreamProvider::with_endpoint(endpoint).unwrap();

        let response = provider
            .forward(
                &authorized_headers("Bearer one-call-secret"),
                json!({"input": "hello"}),
            )
            .await
            .unwrap();

        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
