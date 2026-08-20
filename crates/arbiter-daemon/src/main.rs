use std::path::PathBuf;

use anyhow::Context;
use arbiter_core::health::DaemonIdentity;
use arbiter_daemon::{
    AppState, DEFAULT_SHUTDOWN_GRACE, init_logging, serve_local_with_shutdown, shutdown_signal,
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
    let provider = CodexUpstreamProvider::new().context("initialize Codex upstream provider")?;
    let store = SqliteEventStore::open(&args.database)
        .await
        .context("open Arbiter event database")?;

    serve_local_with_shutdown(
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
        args.port,
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
