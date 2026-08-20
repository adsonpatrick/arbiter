use std::{
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use arbiter_adapter_codex::{
    OriginalPermissions, atomic_replace_private, ensure_private_dir, validate_managed_profile,
};
use arbiter_daemon::{
    AppState, DEFAULT_SHUTDOWN_GRACE, DaemonLease, local_bind_address,
    serve_listener_with_shutdown, shutdown_signal,
};
use arbiter_provider_codex::provider::CodexUpstreamProvider;
use arbiter_storage_sqlite::SqliteEventStore;

use super::{
    Paths, ServerMetadata, VERSION, read_config, read_json,
    status::{daemon_is_current, endpoint_responding},
    write_json_atomic,
};

pub(crate) async fn run(paths: &Paths, foreground: bool) -> anyhow::Result<()> {
    let config = read_config(paths)?;
    validate_managed_profile(&config.codex_config, config.port)
        .context("validate managed Codex profile")?;
    if foreground {
        return run_foreground(paths, config).await;
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
        if let Ok(current) = read_config(paths) {
            if daemon_is_current(paths, &current).await {
                println!("Arbiter daemon started on 127.0.0.1:{}.", config.port);
                return Ok(());
            }
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

async fn run_foreground(paths: &Paths, mut config: super::LocalConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(local_bind_address(config.port))
        .await
        .with_context(|| format!("acquire daemon listener on 127.0.0.1:{}", config.port))?;
    let _lease = DaemonLease::acquire(&paths.database).context("acquire event database lease")?;
    ensure_private_dir(&paths.arbiter_home).context("protect Arbiter home")?;
    if config.instance_id.is_empty() {
        config.instance_id = uuid::Uuid::new_v4().to_string();
        write_json_atomic(&paths.config, &config).context("migrate local daemon identity")?;
    }
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
    let result = serve_listener_with_shutdown(
        AppState::new_with_identity(provider, store, metadata.clone()),
        listener,
        shutdown,
        DEFAULT_SHUTDOWN_GRACE,
    )
    .await;
    remove_server_metadata(paths, &metadata)?;
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

fn remove_server_metadata(paths: &Paths, owned_identity: &ServerMetadata) -> anyhow::Result<()> {
    if paths.server.exists()
        && read_json::<ServerMetadata>(&paths.server)
            .is_ok_and(|metadata| metadata == *owned_identity)
    {
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

#[cfg(test)]
mod tests {
    use arbiter_core::{
        config::BaselineTarget,
        events::{AttemptStarted, GovernorEvent},
        ids::{AttemptId, RequestId},
    };
    use arbiter_daemon::DaemonLease;
    use arbiter_storage_sqlite::SqliteEventStore;
    use tempfile::tempdir;

    use super::{Paths, ServerMetadata, remove_server_metadata, run_foreground};
    use crate::commands::default_config;

    #[tokio::test]
    async fn bind_failure_does_not_reconcile_attempts_owned_by_another_daemon() {
        let temporary = tempdir().unwrap();
        let arbiter_home = temporary.path().join(".arbiter");
        std::fs::create_dir_all(&arbiter_home).unwrap();
        let paths = Paths {
            codex_config: temporary.path().join(".codex/config.toml"),
            config: arbiter_home.join("config.json"),
            database: arbiter_home.join("arbiter.db"),
            receipt: arbiter_home.join("install-receipt.json"),
            server: arbiter_home.join("server.json"),
            stop: arbiter_home.join("stop"),
            arbiter_home,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let config = default_config(
            &paths,
            listener.local_addr().unwrap().port(),
            "test-instance".to_owned(),
        );
        let attempt_id = AttemptId::new();
        let store = SqliteEventStore::open(&paths.database).await.unwrap();
        store
            .append(&GovernorEvent::attempt_started(AttemptStarted {
                request_id: RequestId::new(),
                attempt_id,
                attempt_index: 0,
                target: BaselineTarget::m0(),
                started_at_unix_ms: 1,
            }))
            .await
            .unwrap();
        store.close().await;

        assert!(run_foreground(&paths, config).await.is_err());

        let store = SqliteEventStore::open(&paths.database).await.unwrap();
        assert_eq!(store.events_for_attempt(attempt_id).await.unwrap().len(), 1);
        assert!(!paths.server.exists());
    }

    #[tokio::test]
    async fn database_lease_failure_does_not_reconcile_live_attempts() {
        let temporary = tempdir().unwrap();
        let arbiter_home = temporary.path().join(".arbiter");
        std::fs::create_dir_all(&arbiter_home).unwrap();
        let paths = Paths {
            codex_config: temporary.path().join(".codex/config.toml"),
            config: arbiter_home.join("config.json"),
            database: arbiter_home.join("arbiter.db"),
            receipt: arbiter_home.join("install-receipt.json"),
            server: arbiter_home.join("server.json"),
            stop: arbiter_home.join("stop"),
            arbiter_home,
        };
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = default_config(&paths, port, "test-instance".to_owned());
        let attempt_id = AttemptId::new();
        let store = SqliteEventStore::open(&paths.database).await.unwrap();
        store
            .append(&GovernorEvent::attempt_started(AttemptStarted {
                request_id: RequestId::new(),
                attempt_id,
                attempt_index: 0,
                target: BaselineTarget::m0(),
                started_at_unix_ms: 1,
            }))
            .await
            .unwrap();
        store.close().await;
        let _lease = DaemonLease::acquire(&paths.database).unwrap();

        assert!(run_foreground(&paths, config).await.is_err());

        let store = SqliteEventStore::open(&paths.database).await.unwrap();
        assert_eq!(store.events_for_attempt(attempt_id).await.unwrap().len(), 1);
        assert!(!paths.server.exists());
    }

    #[test]
    fn cleanup_does_not_remove_metadata_owned_by_another_daemon() {
        let temporary = tempdir().unwrap();
        let arbiter_home = temporary.path().join(".arbiter");
        std::fs::create_dir_all(&arbiter_home).unwrap();
        let paths = Paths {
            codex_config: temporary.path().join(".codex/config.toml"),
            config: arbiter_home.join("config.json"),
            database: arbiter_home.join("arbiter.db"),
            receipt: arbiter_home.join("install-receipt.json"),
            server: arbiter_home.join("server.json"),
            stop: arbiter_home.join("stop"),
            arbiter_home,
        };
        let owned = ServerMetadata {
            pid: 1,
            port: 1,
            version: "test".to_owned(),
            instance_id: "owned".to_owned(),
        };
        let replacement = ServerMetadata {
            instance_id: "replacement".to_owned(),
            ..owned.clone()
        };
        crate::commands::write_json_atomic(&paths.server, &replacement).unwrap();

        remove_server_metadata(&paths, &owned).unwrap();

        assert!(paths.server.exists());
    }
}
