use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use arbiter_adapter_codex::{
    ensure_private_dir, harden_private_file, install_profile, uninstall_profile,
};
use arbiter_storage_sqlite::SqliteEventStore;

use super::{InitTarget, Paths, default_config, write_json_atomic};

pub(crate) async fn run(
    paths: &Paths,
    target: InitTarget,
    yes: bool,
    requested_port: Option<u16>,
) -> anyhow::Result<()> {
    if requested_port == Some(0) {
        bail!("daemon port must be between 1 and 65535");
    }
    match target {
        InitTarget::Codex => {}
    }
    println!("Plan:");
    println!(
        "  create local PASSTHROUGH configuration at {}",
        paths.config.display()
    );
    println!(
        "  initialize SQLite history at {}",
        paths.database.display()
    );
    println!(
        "  install Codex profile 'arbiter' in {}",
        paths.codex_config.display()
    );
    if !yes {
        println!("No changes applied. Re-run with --yes to apply this plan.");
        return Ok(());
    }
    if paths.receipt.exists() {
        bail!("Arbiter is already initialized; uninstall it before initializing again");
    }

    let port = requested_port.map_or_else(reserve_ephemeral_port, Ok)?;
    let instance_id = uuid::Uuid::new_v4().to_string();

    ensure_private_dir(&paths.arbiter_home).context("create private Arbiter home")?;
    let config = default_config(paths, port, instance_id);
    write_json_atomic(&paths.config, &config).context("write local configuration")?;
    let store = SqliteEventStore::open(&paths.database)
        .await
        .context("initialize event database")?;
    store.close().await;
    harden_private_file(&paths.database).context("protect event database")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis()
        .try_into()
        .context("timestamp exceeds supported range")?;
    let receipt = install_profile(
        &paths.codex_config,
        &paths.arbiter_home,
        config.port,
        timestamp,
    )
    .context("install managed Codex profile")?;
    if let Err(error) = write_json_atomic(&paths.receipt, &receipt) {
        uninstall_profile(&paths.codex_config, &receipt)
            .context("roll back Codex profile after receipt write failure")?;
        return Err(error).context("persist installation receipt");
    }

    println!("Initialized Arbiter in PASSTHROUGH mode.");
    println!("Use Codex profile 'arbiter' to route through the local daemon.");
    Ok(())
}

fn reserve_ephemeral_port() -> anyhow::Result<u16> {
    let listener =
        std::net::TcpListener::bind("127.0.0.1:0").context("reserve an ephemeral daemon port")?;
    Ok(listener.local_addr()?.port())
}
