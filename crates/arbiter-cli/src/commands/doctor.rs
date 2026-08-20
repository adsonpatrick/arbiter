use std::{net::TcpListener, process::Command};

use anyhow::{Context, bail};
use arbiter_adapter_codex::validate_managed_profile;
use arbiter_storage_sqlite::SqliteEventStore;

use super::{Paths, read_config};

pub(crate) async fn run(paths: &Paths) -> anyhow::Result<()> {
    let config = read_config(paths).context("local PASSTHROUGH contract")?;
    println!("ok: local PASSTHROUGH contract");
    validate_managed_profile(&config.codex_config, config.port)
        .context("managed Codex provider/profile")?;
    println!("ok: managed Codex provider/profile uses Codex authentication");
    if !paths.database.is_file() {
        bail!("Arbiter event database is missing");
    }
    let store = SqliteEventStore::open(&paths.database)
        .await
        .context("SQLite integrity")?;
    store.close().await;
    println!("ok: SQLite integrity");

    if !super::status::daemon_is_current(paths, config.port).await {
        let listener = TcpListener::bind((config.bind_address.as_str(), config.port))
            .context("local daemon port is unavailable")?;
        drop(listener);
    }
    println!("ok: local-only binding and port");

    let output = Command::new("codex")
        .args(["login", "status"])
        .output()
        .context("Codex binary not found")?;
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("`codex login status` failed");
    }
    require_chatgpt_authentication(&report)?;
    println!("ok: Codex reports ChatGPT authentication");
    println!("ok: remote export disabled; Arbiter reads no credential storage");
    Ok(())
}

fn require_chatgpt_authentication(report: &str) -> anyhow::Result<()> {
    let lower = report.to_ascii_lowercase();
    if lower.contains("api key") {
        bail!("Codex API-key authentication is unsupported in Arbiter M0");
    }
    if !lower.contains("chatgpt") {
        bail!("Codex is not authenticated with ChatGPT");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::require_chatgpt_authentication;

    #[test]
    fn doctor_accepts_chatgpt_login_and_rejects_api_key_or_logged_out_states() {
        assert!(require_chatgpt_authentication("Logged in using ChatGPT").is_ok());
        assert!(
            require_chatgpt_authentication("Logged in using an API key")
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(require_chatgpt_authentication("Not logged in").is_err());
    }
}
