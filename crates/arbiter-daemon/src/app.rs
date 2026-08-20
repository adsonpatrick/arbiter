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
use std::{
    future::{Future, IntoFuture},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use crate::{AppState, proxy::responses};

pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

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

/// Binds IPv4 loopback and serves with bounded graceful shutdown.
///
/// # Errors
///
/// Returns an I/O error when the listener cannot bind or the server fails.
pub async fn serve_local_with_shutdown<F>(
    state: AppState,
    port: u16,
    shutdown: F,
    grace: Duration,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(local_bind_address(port)).await?;
    serve_listener_with_shutdown(state, listener, shutdown, grace).await
}

/// Waits for the platform's process shutdown signal.
pub async fn shutdown_signal() {
    #[cfg(unix)]
    {
        if let Ok(mut terminate) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {
                result = tokio::signal::ctrl_c() => { let _ = result; }
                _ = terminate.recv() => {}
            }
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Serves until shutdown, then gives active streams a bounded grace period.
///
/// New work is rejected as soon as shutdown begins. Once the grace period
/// expires, remaining streams are dropped, their cancellation events are
/// drained, and the `SQLite` pool is closed.
///
/// # Errors
///
/// Returns an I/O error from the HTTP server.
pub async fn serve_listener_with_shutdown<F>(
    state: AppState,
    listener: tokio::net::TcpListener,
    shutdown: F,
    grace: Duration,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel();
    let server = axum::serve(listener, build_router(state.clone()))
        .with_graceful_shutdown(async move {
            let _ = server_shutdown_rx.await;
        })
        .into_future();
    let mut server = Box::pin(server);
    tokio::pin!(shutdown);

    let result = tokio::select! {
        result = server.as_mut() => result,
        () = &mut shutdown => {
            state.begin_shutdown();
            let _ = server_shutdown_tx.send(());
            if let Ok(result) = tokio::time::timeout(grace, server.as_mut()).await {
                result
            } else {
                state.force_cancel();
                drop(server);
                Ok(())
            }
        }
    };

    let _ = tokio::time::timeout(grace, state.wait_for_idle()).await;
    let _ = tokio::time::timeout(grace, state.wait_for_persistence()).await;
    let _ = tokio::time::timeout(grace, state.store.close()).await;
    result
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
