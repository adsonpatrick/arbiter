use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};

use arbiter_core::{
    config::BaselineTarget,
    events::{AttemptCompleted, AttemptFailed, AttemptStarted, ErrorClass, GovernorEvent},
    ids::{AttemptId, RequestId},
};
use arbiter_provider_codex::provider::{
    ProviderByteStream, ProviderChunk, ProviderError, ProviderResponse,
};
use arbiter_storage_sqlite::SqliteEventStore;
use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures_util::Stream;

use crate::{
    AppState,
    state::{RequestActivity, RuntimeState},
};

pub(crate) async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<serde_json::Value>,
) -> Response {
    let Some(activity) = state.start_request() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let context = AttemptContext::new();
    let started = GovernorEvent::attempt_started(AttemptStarted {
        request_id: context.request_id,
        attempt_id: context.attempt_id,
        attempt_index: 0,
        target: context.target.clone(),
        started_at_unix_ms: context.started_at_unix_ms,
    });

    if state.store.append(&started).await.is_err() {
        tracing::warn!(
            event_type = "attempt_start_failed",
            request_id = %context.request_id,
            attempt_id = %context.attempt_id,
            model = %context.target.model,
            reasoning_effort = "medium",
            http_status = StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            "Arbiter attempt event"
        );
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    tracing::info!(
        event_type = "attempt_started",
        request_id = %context.request_id,
        attempt_id = %context.attempt_id,
        model = %context.target.model,
        reasoning_effort = "medium",
        "Arbiter attempt event"
    );

    let upstream = match state.provider.forward(&headers, request).await {
        Ok(upstream) => upstream,
        Err(error) => {
            let error_class = match error {
                ProviderError::Upstream(_) => ErrorClass::ProviderConnect,
                _ => ErrorClass::ProviderSetup,
            };
            let failed = context.failed(error_class);
            let _ = state.store.append(&failed).await;
            log_terminal_event(&failed, Some(StatusCode::BAD_GATEWAY));
            return response_with_attempt(StatusCode::BAD_GATEWAY, context.attempt_id);
        }
    };

    proxy_response(upstream, state.store, state.runtime, context, activity)
}

fn response_with_attempt(status: StatusCode, attempt_id: AttemptId) -> Response {
    let mut response = status.into_response();
    response.headers_mut().insert(
        "x-arbiter-attempt-id",
        HeaderValue::from_str(&attempt_id.to_string())
            .expect("UUID is always a valid header value"),
    );
    response
}

fn proxy_response(
    upstream: ProviderResponse,
    store: SqliteEventStore,
    runtime: RuntimeState,
    context: AttemptContext,
    activity: RequestActivity,
) -> Response {
    let status = upstream.status;
    let mut response_headers = upstream.headers.clone();
    response_headers.insert(
        "x-arbiter-attempt-id",
        HeaderValue::from_str(&context.attempt_id.to_string())
            .expect("UUID is always a valid header value"),
    );
    let force_cancelled = Box::pin(runtime.cancellation_token().cancelled_owned());
    let stream = GovernedStream {
        upstream: upstream.bytes_stream(),
        store,
        runtime,
        context,
        terminal_recorded: false,
        _activity: activity,
        force_cancelled,
    };
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    response
}

#[derive(Clone)]
struct AttemptContext {
    request_id: RequestId,
    attempt_id: AttemptId,
    target: BaselineTarget,
    started_at_unix_ms: u64,
}

impl AttemptContext {
    fn new() -> Self {
        Self {
            request_id: RequestId::new(),
            attempt_id: AttemptId::new(),
            target: BaselineTarget::m0(),
            started_at_unix_ms: now_unix_ms(),
        }
    }

    fn completed(
        &self,
        metadata: arbiter_provider_codex::sse::TerminalResponseMetadata,
    ) -> GovernorEvent {
        let completed_at_unix_ms = now_unix_ms();
        GovernorEvent::attempt_completed(AttemptCompleted {
            request_id: self.request_id,
            attempt_id: self.attempt_id,
            attempt_index: 0,
            target: self.target.clone(),
            started_at_unix_ms: self.started_at_unix_ms,
            completed_at_unix_ms,
            duration_ms: completed_at_unix_ms.saturating_sub(self.started_at_unix_ms),
            usage: metadata.usage,
            provider_response_id: Some(metadata.response_id),
        })
    }

    fn failed(&self, error_class: ErrorClass) -> GovernorEvent {
        let failed_at_unix_ms = now_unix_ms();
        GovernorEvent::attempt_failed(AttemptFailed {
            request_id: self.request_id,
            attempt_id: self.attempt_id,
            attempt_index: 0,
            target: self.target.clone(),
            started_at_unix_ms: self.started_at_unix_ms,
            failed_at_unix_ms,
            duration_ms: failed_at_unix_ms.saturating_sub(self.started_at_unix_ms),
            error_class,
        })
    }
}

struct GovernedStream {
    upstream: ProviderByteStream,
    store: SqliteEventStore,
    runtime: RuntimeState,
    context: AttemptContext,
    terminal_recorded: bool,
    _activity: RequestActivity,
    force_cancelled: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl GovernedStream {
    fn record(&mut self, event: GovernorEvent) {
        self.terminal_recorded = true;
        let store = self.store.clone();
        self.runtime.spawn_persistence(async move {
            if store.append(&event).await.is_ok() {
                log_terminal_event(&event, None);
            }
        });
    }
}

fn log_terminal_event(event: &GovernorEvent, http_status: Option<StatusCode>) {
    match &event.kind {
        arbiter_core::events::GovernorEventKind::AttemptCompleted(completed) => {
            tracing::info!(
                event_type = "attempt_completed",
                request_id = %completed.request_id,
                attempt_id = %completed.attempt_id,
                model = %completed.target.model,
                reasoning_effort = "medium",
                duration_ms = completed.duration_ms,
                input_tokens = completed.usage.input_tokens,
                cached_input_tokens = completed.usage.cached_input_tokens,
                output_tokens = completed.usage.output_tokens,
                reasoning_tokens = completed.usage.reasoning_tokens,
                "Arbiter attempt event"
            );
        }
        arbiter_core::events::GovernorEventKind::AttemptFailed(failed) => {
            tracing::warn!(
                event_type = "attempt_failed",
                request_id = %failed.request_id,
                attempt_id = %failed.attempt_id,
                model = %failed.target.model,
                reasoning_effort = "medium",
                duration_ms = failed.duration_ms,
                error_class = ?failed.error_class,
                http_status = http_status.map(|status| status.as_u16()),
                "Arbiter attempt event"
            );
        }
        arbiter_core::events::GovernorEventKind::AttemptStarted(_) => {}
    }
}

impl Stream for GovernedStream {
    type Item = Result<Bytes, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.force_cancelled.as_mut().poll(context).is_ready() {
            if !self.terminal_recorded {
                let event = self.context.failed(ErrorClass::Cancelled);
                self.record(event);
            }
            return Poll::Ready(None);
        }
        match self.upstream.as_mut().poll_next(context) {
            Poll::Ready(Some(Ok(ProviderChunk {
                bytes,
                terminal_metadata,
            }))) => {
                if let Some(metadata) = terminal_metadata
                    && !self.terminal_recorded
                {
                    let event = self.context.completed(metadata);
                    self.record(event);
                }
                Poll::Ready(Some(Ok(bytes)))
            }
            Poll::Ready(Some(Err(error))) => {
                let event = self.context.failed(ErrorClass::StreamInterrupted);
                self.record(event);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                if !self.terminal_recorded {
                    let event = self.context.failed(ErrorClass::StreamInterrupted);
                    self.record(event);
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for GovernedStream {
    fn drop(&mut self) {
        if !self.terminal_recorded {
            let event = self.context.failed(ErrorClass::Cancelled);
            self.record(event);
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
