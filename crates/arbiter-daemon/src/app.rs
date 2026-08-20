use arbiter_core::{
    config::BaselineTarget,
    health::{ComponentHealth, ComponentName, DaemonHealth, HealthStatus},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::{AppState, proxy::responses};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/responses", post(responses))
        .route("/healthz", get(healthz))
        .route("/status", get(status))
        .with_state(state)
}

#[must_use]
pub const fn local_bind_address(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

/// Serves Arbiter on IPv4 loopback only.
///
/// # Errors
///
/// Returns an I/O error when the loopback socket cannot be bound or served.
pub async fn serve_local(state: AppState, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(local_bind_address(port)).await?;
    axum::serve(listener, build_router(state)).await
}

async fn healthz(State(state): State<AppState>) -> (StatusCode, Json<DaemonHealth>) {
    let storage_healthy = matches!(state.store.integrity_check().await.as_deref(), Ok("ok"));
    let status = if storage_healthy {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unhealthy
    };
    let http_status = if storage_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let health = DaemonHealth {
        status,
        components: vec![
            ComponentHealth {
                component: ComponentName::Daemon,
                status: HealthStatus::Healthy,
                detail: None,
            },
            ComponentHealth {
                component: ComponentName::Storage,
                status,
                detail: None,
            },
        ],
    };
    (http_status, Json(health))
}

async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let storage_integrity = state
        .store
        .integrity_check()
        .await
        .unwrap_or_else(|_| "unavailable".to_owned());
    Json(serde_json::json!({
        "mode": "passthrough",
        "baseline": BaselineTarget::m0(),
        "storage_integrity": storage_integrity,
    }))
}
