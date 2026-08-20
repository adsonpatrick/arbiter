use std::path::PathBuf;

use anyhow::Context;
use arbiter_daemon::{AppState, serve_local};
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
    let provider = CodexUpstreamProvider::new().context("initialize Codex upstream provider")?;
    let store = SqliteEventStore::open(&args.database)
        .await
        .context("open Arbiter event database")?;

    serve_local(AppState::new(provider, store), args.port)
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
