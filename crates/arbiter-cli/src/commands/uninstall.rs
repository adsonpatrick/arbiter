use std::time::Duration;

use anyhow::Context;
use arbiter_adapter_codex::{OriginalPermissions, atomic_replace_private, uninstall_profile};

use super::{
    Paths, ServerMetadata, read_config, read_json, read_receipt,
    status::{daemon_is_current, endpoint_responding},
};

pub(crate) async fn run(paths: &Paths, yes: bool) -> anyhow::Result<()> {
    println!("Plan:");
    println!("  stop the local Arbiter daemon if running");
    println!("  restore the verified pre-Arbiter Codex configuration");
    println!("  preserve event history at {}", paths.database.display());
    if !yes {
        println!("No changes applied. Re-run with --yes to apply this plan.");
        return Ok(());
    }

    let receipt = read_receipt(paths)?;
    let config = read_config(paths)?;
    require_safe_backup_paths(paths, &receipt)?;
    stop_daemon(paths, &config).await?;
    uninstall_profile(&paths.codex_config, &receipt).context("restore Codex configuration")?;
    std::fs::remove_file(&paths.receipt).context("remove installation receipt")?;
    if paths.config.exists() {
        std::fs::remove_file(&paths.config).context("remove local configuration")?;
    }
    println!("Arbiter uninstalled; Codex configuration restored.");
    println!(
        "Event history was preserved at {}",
        paths.database.display()
    );
    Ok(())
}

fn require_safe_backup_paths(
    paths: &Paths,
    receipt: &arbiter_adapter_codex::InstallReceipt,
) -> anyhow::Result<()> {
    let backup_root = paths.arbiter_home.join("backups").canonicalize()?;
    for managed in [&receipt.config, &receipt.profile] {
        let backup = managed.backup_path.canonicalize()?;
        let hash = managed.backup_hash_path.canonicalize()?;
        if !backup.starts_with(&backup_root)
            || !hash.starts_with(&backup_root)
            || managed.backup_hash_path != managed.backup_path.with_extension("toml.sha256")
        {
            anyhow::bail!("installation receipt points outside the Arbiter backup directory");
        }
    }
    Ok(())
}

async fn stop_daemon(paths: &Paths, config: &super::LocalConfig) -> anyhow::Result<()> {
    if !paths.server.exists() {
        if endpoint_responding(config.port).await {
            anyhow::bail!("port {} is serving an unrecognized process", config.port);
        }
        return Ok(());
    }
    let metadata: ServerMetadata = read_json(&paths.server)?;
    if !daemon_is_current(paths, config).await {
        if endpoint_responding(metadata.port).await {
            anyhow::bail!("port {} is serving an unrecognized process", metadata.port);
        }
        std::fs::remove_file(&paths.server).context("remove stale server metadata")?;
        return Ok(());
    }
    atomic_replace_private(&paths.stop, b"stop\n", &OriginalPermissions::default())
        .context("request daemon shutdown")?;
    for _ in 0..50 {
        if !paths.server.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if paths.server.exists() {
        anyhow::bail!("daemon did not stop; Codex configuration was not changed");
    }
    if paths.stop.exists() {
        std::fs::remove_file(&paths.stop).context("remove daemon stop marker")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use arbiter_adapter_codex::{InstallReceipt, ManagedFileReceipt, OriginalPermissions};
    use tempfile::tempdir;

    use super::{Paths, require_safe_backup_paths, stop_daemon};
    use crate::commands::default_config;

    #[test]
    fn rejects_a_receipt_whose_backup_escapes_arbiter_home() {
        let temporary = tempdir().unwrap();
        let arbiter_home = temporary.path().join(".arbiter");
        std::fs::create_dir_all(arbiter_home.join("backups")).unwrap();
        let outside = temporary.path().join("outside.toml");
        let outside_hash = temporary.path().join("outside.toml.sha256");
        std::fs::write(&outside, "outside").unwrap();
        std::fs::write(&outside_hash, "hash").unwrap();
        let paths = Paths {
            codex_config: temporary.path().join(".codex/config.toml"),
            config: arbiter_home.join("config.json"),
            database: arbiter_home.join("arbiter.db"),
            receipt: arbiter_home.join("install-receipt.json"),
            server: arbiter_home.join("server.json"),
            stop: arbiter_home.join("stop"),
            arbiter_home,
        };
        let outside_receipt = ManagedFileReceipt {
            path: paths.codex_config.clone(),
            backup_path: outside,
            backup_hash_path: outside_hash,
            original_sha256: "hash".to_owned(),
            installed_sha256: "hash".to_owned(),
            original_existed: true,
            original_permissions: OriginalPermissions::default(),
        };
        let receipt = InstallReceipt {
            config: outside_receipt.clone(),
            profile: outside_receipt,
        };

        assert!(require_safe_backup_paths(&paths, &receipt).is_err());
    }

    #[tokio::test]
    async fn rejects_an_occupied_configured_port_when_server_metadata_is_missing() {
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = default_config(
            &paths,
            listener.local_addr().unwrap().port(),
            "test-instance".to_owned(),
        );

        let error = stop_daemon(&paths, &config).await.unwrap_err();

        assert!(error.to_string().contains("unrecognized process"));
    }
}
