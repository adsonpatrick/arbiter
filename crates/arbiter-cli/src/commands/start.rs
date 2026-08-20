use std::{
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use arbiter_adapter_codex::{
    OriginalPermissions, atomic_replace_private, validate_managed_profile,
};
use arbiter_daemon::{
    AppState, DEFAULT_SHUTDOWN_GRACE, serve_local_with_shutdown, shutdown_signal,
};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;

use super::{
    Paths, ServerMetadata, VERSION, read_config,
    status::{daemon_is_current, endpoint_responding},
    write_json_atomic,
};

pub(crate) async fn run(paths: &Paths, foreground: bool) -> anyhow::Result<()> {
    let config = read_config(paths)?;
    validate_managed_profile(&config.codex_config, config.port)
        .context("validate managed Codex profile")?;
    if foreground {
        return run_foreground(paths, &config).await;
    }
    if daemon_is_current(paths, &config).await {
        println!(
            "Arbiter daemon is already healthy on 127.0.0.1:{}.",
            config.port
        );
        return Ok(());
    }
    if endpoint_responding(config.port).await {
        bail!("port {} is serving an unrecognized process", config.port);
    }
    if paths.stop.exists() {
        std::fs::remove_file(&paths.stop).context("remove stale stop marker")?;
    }
    let executable = std::env::current_exe().context("resolve Arbiter executable")?;
    let mut command = std::process::Command::new(executable);
    command
        .args(["start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_child_window(&mut command);
    let mut child = command.spawn().context("start Arbiter daemon")?;
    for _ in 0..50 {
        if daemon_is_current(paths, &config).await {
            println!("Arbiter daemon started on 127.0.0.1:{}.", config.port);
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            bail!("Arbiter daemon exited before becoming healthy");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    atomic_replace_private(&paths.stop, b"stop\n", &OriginalPermissions::default())
        .context("request failed daemon startup shutdown")?;
    bail!("Arbiter daemon did not become healthy")
}

async fn run_foreground(paths: &Paths, config: &super::LocalConfig) -> anyhow::Result<()> {
    let provider = CodexUpstreamProvider::new().context("initialize Codex upstream provider")?;
    let store = SqliteEventStore::open(&paths.database)
        .await
        .context("open Arbiter event database")?;
    let recovered_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis()
        .try_into()
        .context("timestamp exceeds supported range")?;
    store
        .reconcile_incomplete_attempts(recovered_at_unix_ms)
        .await
        .context("reconcile interrupted Arbiter attempts")?;
    let metadata = ServerMetadata {
        pid: std::process::id(),
        port: config.port,
        version: VERSION.to_owned(),
        instance_id: config.instance_id.clone(),
    };
    write_json_atomic(&paths.server, &metadata).context("write server metadata")?;
    let stop_marker = paths.stop.clone();
    let shutdown = async move {
        tokio::select! {
            () = shutdown_signal() => {}
            () = wait_for_stop_marker(stop_marker) => {}
        }
    };
    let result = serve_local_with_shutdown(
        AppState::new_with_identity(provider, store, metadata.clone()),
        config.port,
        shutdown,
        DEFAULT_SHUTDOWN_GRACE,
    )
    .await;
    remove_server_metadata(paths)?;
    result.context("serve Arbiter daemon")
}

async fn wait_for_stop_marker(path: std::path::PathBuf) {
    loop {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn remove_server_metadata(paths: &Paths) -> anyhow::Result<()> {
    if paths.server.exists() {
        std::fs::remove_file(&paths.server).context("remove server metadata")?;
    }
    Ok(())
}

#[cfg(windows)]
fn hide_child_window(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_child_window(_command: &mut std::process::Command) {}
