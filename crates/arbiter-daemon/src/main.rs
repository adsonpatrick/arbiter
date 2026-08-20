use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use arbiter_core::health::DaemonIdentity;
use arbiter_daemon::{
    AppState, DEFAULT_SHUTDOWN_GRACE, DaemonLease, init_logging, local_bind_address,
    serve_listener_with_shutdown, shutdown_signal,
};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;
use clap::Parser;

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    database: PathBuf,
    #[arg(long, default_value_t = 43_123)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    init_logging().context("initialize structured logging")?;
    let listener = tokio::net::TcpListener::bind(local_bind_address(args.port))
        .await
        .with_context(|| format!("acquire daemon listener on 127.0.0.1:{}", args.port))?;
    let _lease = DaemonLease::acquire(&args.database).context("acquire event database lease")?;
    let provider = CodexUpstreamProvider::new().context("initialize Codex upstream provider")?;
    let store = SqliteEventStore::open(&args.database)
        .await
        .context("open Arbiter event database")?;
    let recovered_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis()
        .try_into()
        .context("timestamp exceeds supported range")?;
    let recovered = store
        .reconcile_incomplete_attempts(recovered_at_unix_ms)
        .await
        .context("reconcile interrupted Arbiter attempts")?;
    if recovered > 0 {
        tracing::warn!(
            recovered_attempts = recovered,
            "reconciled interrupted attempts"
        );
    }

    serve_listener_with_shutdown(
        AppState::new_with_identity(
            provider,
            store,
            DaemonIdentity {
                pid: std::process::id(),
                port: args.port,
                version: env!("CARGO_PKG_VERSION").to_owned(),
                instance_id: uuid::Uuid::new_v4().to_string(),
            },
        ),
        listener,
        shutdown_signal(),
        DEFAULT_SHUTDOWN_GRACE,
    )
    .await
    .context("serve Arbiter daemon")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::Args;

    #[test]
    fn daemon_args_require_database_and_default_to_m0_port() {
        let args = Args::try_parse_from(["arbiter-daemon", "--database", "arbiter.db"])
            .expect("valid daemon args");

        assert_eq!(args.database, PathBuf::from("arbiter.db"));
        assert_eq!(args.port, 43_123);
    }
}
