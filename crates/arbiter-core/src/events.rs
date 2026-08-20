//! Privacy-minimized Arbiter event types.

use serde::{Deserialize, Serialize};

use crate::{
    config::BaselineTarget,
    ids::{AttemptId, EventId, RequestId},
};

pub const M0_EVENT_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    ProviderSetup,
    ProviderConnect,
    ProviderHttp,
    StreamInterrupted,
    Storage,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptStarted {
    pub request_id: RequestId,
    pub attempt_id: AttemptId,
    pub attempt_index: u32,
    pub target: BaselineTarget,
    pub started_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptCompleted {
    pub request_id: RequestId,
    pub attempt_id: AttemptId,
    pub attempt_index: u32,
    pub target: BaselineTarget,
    pub started_at_unix_ms: u64,
    pub completed_at_unix_ms: u64,
    pub duration_ms: u64,
    pub usage: TokenUsage,
    pub provider_response_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptFailed {
    pub request_id: RequestId,
    pub attempt_id: AttemptId,
    pub attempt_index: u32,
    pub target: BaselineTarget,
    pub started_at_unix_ms: u64,
    pub failed_at_unix_ms: u64,
    pub duration_ms: u64,
    pub error_class: ErrorClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_type", content = "payload", rename_all = "snake_case")]
pub enum GovernorEventKind {
    AttemptStarted(AttemptStarted),
    AttemptCompleted(AttemptCompleted),
    AttemptFailed(AttemptFailed),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovernorEvent {
    pub event_id: EventId,
    pub occurred_at_unix_ms: u64,
    pub schema_version: u16,
    #[serde(flatten)]
    pub kind: GovernorEventKind,
}

impl GovernorEvent {
    #[must_use]
    pub fn attempt_started(payload: AttemptStarted) -> Self {
        Self::new(
            payload.started_at_unix_ms,
            GovernorEventKind::AttemptStarted(payload),
        )
    }

    #[must_use]
    pub fn attempt_completed(payload: AttemptCompleted) -> Self {
        Self::new(
            payload.completed_at_unix_ms,
            GovernorEventKind::AttemptCompleted(payload),
        )
    }

    #[must_use]
    pub fn attempt_failed(payload: AttemptFailed) -> Self {
        Self::new(
            payload.failed_at_unix_ms,
            GovernorEventKind::AttemptFailed(payload),
        )
    }

    fn new(occurred_at_unix_ms: u64, kind: GovernorEventKind) -> Self {
        Self {
            event_id: EventId::new(),
            occurred_at_unix_ms,
            schema_version: M0_EVENT_SCHEMA_VERSION,
            kind,
        }
    }

    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        match self.kind {
            GovernorEventKind::AttemptStarted(_) => "attempt_started",
            GovernorEventKind::AttemptCompleted(_) => "attempt_completed",
            GovernorEventKind::AttemptFailed(_) => "attempt_failed",
        }
    }

    #[must_use]
    pub const fn request_id(&self) -> RequestId {
        match &self.kind {
            GovernorEventKind::AttemptStarted(payload) => payload.request_id,
            GovernorEventKind::AttemptCompleted(payload) => payload.request_id,
            GovernorEventKind::AttemptFailed(payload) => payload.request_id,
        }
    }

    #[must_use]
    pub const fn attempt_id(&self) -> AttemptId {
        match &self.kind {
            GovernorEventKind::AttemptStarted(payload) => payload.attempt_id,
            GovernorEventKind::AttemptCompleted(payload) => payload.attempt_id,
            GovernorEventKind::AttemptFailed(payload) => payload.attempt_id,
        }
    }

    #[must_use]
    pub const fn attempt_index(&self) -> u32 {
        match &self.kind {
            GovernorEventKind::AttemptStarted(payload) => payload.attempt_index,
            GovernorEventKind::AttemptCompleted(payload) => payload.attempt_index,
            GovernorEventKind::AttemptFailed(payload) => payload.attempt_index,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttemptCompleted, AttemptFailed, AttemptStarted, ErrorClass, GovernorEvent, TokenUsage,
    };
    use crate::{
        config::BaselineTarget,
        ids::{AttemptId, RequestId},
    };

    const FORBIDDEN: [&str; 4] = [
        "raw-prompt-sentinel",
        "source-code-sentinel",
        "Bearer secret-sentinel",
        "raw-response-sentinel",
    ];

    #[test]
    fn event_serialization_contains_metadata_but_no_content_or_credentials() {
        let request_id = RequestId::new();
        let attempt_id = AttemptId::new();
        let target = BaselineTarget::m0();
        let events = [
            GovernorEvent::attempt_started(AttemptStarted {
                request_id,
                attempt_id,
                attempt_index: 0,
                target: target.clone(),
                started_at_unix_ms: 1_000,
            }),
            GovernorEvent::attempt_completed(AttemptCompleted {
                request_id,
                attempt_id,
                attempt_index: 0,
                target: target.clone(),
                started_at_unix_ms: 1_000,
                completed_at_unix_ms: 1_025,
                duration_ms: 25,
                usage: TokenUsage {
                    input_tokens: 10,
                    cached_input_tokens: 2,
                    output_tokens: 5,
                    reasoning_tokens: 3,
                },
                provider_response_id: Some("resp_metadata_only".to_owned()),
            }),
            GovernorEvent::attempt_failed(AttemptFailed {
                request_id,
                attempt_id,
                attempt_index: 0,
                target,
                started_at_unix_ms: 1_000,
                failed_at_unix_ms: 1_010,
                duration_ms: 10,
                error_class: ErrorClass::ProviderConnect,
            }),
        ];

        for event in events {
            let json = serde_json::to_string(&event).expect("event must serialize");
            assert!(json.contains("gpt-5.6-terra"));
            for sentinel in FORBIDDEN {
                assert!(!json.contains(sentinel));
            }
            for forbidden_field in ["prompt", "source_code", "authorization", "response_content"] {
                assert!(!json.contains(forbidden_field));
            }
        }
    }
}
