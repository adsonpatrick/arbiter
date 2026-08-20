use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arbiter_adapter_codex::validate_managed_profile;
use arbiter_core::health::DaemonHealth;
use arbiter_storage_sqlite::SqliteEventStore;

use super::{Paths, ServerMetadata, VERSION, read_config, read_json};

pub(crate) async fn run(paths: &Paths) -> anyhow::Result<()> {
    let config = read_config(paths)?;
    if !paths.database.is_file() {
        anyhow::bail!("Arbiter event database is missing");
    }
    let store = SqliteEventStore::open(&paths.database).await?;
    let integrity = store.integrity_check().await?;
    let now_unix_ms: u64 = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    let since_unix_ms = now_unix_ms.saturating_sub(24 * 60 * 60 * 1_000);
    let attempts = store.recent_attempt_counts(since_unix_ms).await?;
    store.close().await;
    let daemon_running = daemon_is_current(paths, &config).await;
    if !daemon_running && endpoint_responding(config.port).await {
        anyhow::bail!("port {} is serving an unrecognized process", config.port);
    }
    let provider_reachable = provider_reachable().await;
    let profile = if validate_managed_profile(&config.codex_config, config.port).is_ok() {
        "installed"
    } else {
        "invalid or missing"
    };

    println!("mode: PASSTHROUGH");
    println!("baseline: {} / medium", config.baseline.model);
    println!(
        "daemon: {}",
        if daemon_running { "healthy" } else { "stopped" }
    );
    println!(
        "provider reachability: {}",
        if provider_reachable {
            "chatgpt.com:443 reachable"
        } else {
            "chatgpt.com:443 unreachable"
        }
    );
    println!("storage: {integrity}");
    println!("codex profile: {profile}");
    println!(
        "recent attempts: {} (completed: {}, failed: {})",
        attempts.started, attempts.completed, attempts.failed
    );
    Ok(())
}

async fn provider_reachable() -> bool {
    tokio::time::timeout(
        Duration::from_millis(500),
        tokio::net::TcpStream::connect(("chatgpt.com", 443)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

pub(crate) async fn daemon_is_current(paths: &Paths, config: &super::LocalConfig) -> bool {
    let Ok(metadata) = read_json::<ServerMetadata>(&paths.server) else {
        return false;
    };
    metadata.pid > 0
        && metadata.port == config.port
        && metadata.version == VERSION
        && metadata.instance_id == config.instance_id
        && endpoint_health(config.port)
            .await
            .is_some_and(|health| health.identity == metadata)
}

pub(crate) async fn endpoint_health(port: u16) -> Option<DaemonHealth> {
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
    else {
        return None;
    };
    let response = client
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json().await.ok()
}

pub(crate) async fn endpoint_responding(port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_millis(300),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}
