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
use tokio::time::Instant;

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

    let (result, deadline) = tokio::select! {
        result = server.as_mut() => (result, Instant::now() + grace),
        () = &mut shutdown => {
            let deadline = Instant::now() + grace;
            let cleanup_reserve = Duration::from_secs(2).min(grace / 5);
            let serving_deadline = deadline - cleanup_reserve;
            state.begin_shutdown();
            let _ = server_shutdown_tx.send(());
            if let Some(result) =
                wait_until_deadline(serving_deadline, "graceful_http_drain", server.as_mut()).await
            {
                (result, deadline)
            } else {
                state.force_cancel();
                drop(server);
                (Ok(()), deadline)
            }
        }
    };

    let _ = wait_until_deadline(deadline, "active_streams", state.wait_for_idle()).await;
    let _ = wait_until_deadline(
        deadline,
        "terminal_persistence",
        state.wait_for_persistence(),
    )
    .await;
    let _ = wait_until_deadline(deadline, "sqlite_close", state.store.close()).await;
    result
}

async fn wait_until_deadline<F, T>(deadline: Instant, phase: &'static str, future: F) -> Option<T>
where
    F: Future<Output = T>,
{
    if let Ok(output) = tokio::time::timeout_at(deadline, future).await {
        Some(output)
    } else {
        tracing::warn!(shutdown_phase = phase, "shutdown deadline exhausted");
        None
    }
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
        identity: state.identity.clone(),
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

#[cfg(test)]
mod tests {
    use std::{future::pending, time::Duration};

    use tokio::time::Instant;

    use super::wait_until_deadline;

    #[tokio::test]
    async fn shutdown_phases_share_one_deadline() {
        let started = Instant::now();
        let deadline = started + Duration::from_millis(40);

        assert!(
            wait_until_deadline(deadline, "idle", pending::<()>())
                .await
                .is_none()
        );
        assert!(
            wait_until_deadline(deadline, "persistence", pending::<()>())
                .await
                .is_none()
        );
        assert!(
            wait_until_deadline(deadline, "close", pending::<()>())
                .await
                .is_none()
        );

        assert!(started.elapsed() < Duration::from_millis(90));
    }
}
