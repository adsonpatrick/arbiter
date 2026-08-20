use std::{
    fs,
    path::{Path, PathBuf},
};

use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::config_file::{atomic_replace, sha256, write_new_synced};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReceipt {
    pub backup_path: PathBuf,
    pub backup_hash_path: PathBuf,
    pub original_sha256: String,
    pub installed_sha256: String,
}

#[derive(Debug, Error)]
pub enum CodexProfileError {
    #[error("Codex configuration I/O failed")]
    Io(#[from] std::io::Error),
    #[error("Codex configuration is not valid TOML")]
    Parse(#[from] toml_edit::TomlError),
    #[error("Codex configuration is not valid UTF-8")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Codex configuration path has no parent directory")]
    MissingParent,
    #[error("Codex configuration already contains an Arbiter-managed entry")]
    ManagedEntryExists,
    #[error("Codex configuration changed after Arbiter installation")]
    ConfigurationConflict,
    #[error("Codex configuration backup failed hash verification")]
    BackupHashMismatch,
}

/// Installs the managed Arbiter provider and profile without changing the default profile.
///
/// # Errors
///
/// Returns an error for invalid TOML, existing managed entries, or any backup/write failure.
pub fn install_profile(
    config_path: &Path,
    arbiter_home: &Path,
    port: u16,
    timestamp_unix_ms: u64,
) -> Result<InstallReceipt, CodexProfileError> {
    let original = fs::read(config_path)?;
    let original_text = std::str::from_utf8(&original)?;
    let mut document = original_text.parse::<DocumentMut>()?;
    if managed_entry_exists(&document, "model_providers")
        || managed_entry_exists(&document, "profiles")
    {
        return Err(CodexProfileError::ManagedEntryExists);
    }

    insert_managed_table(&mut document, "model_providers", provider_table(port));
    insert_managed_table(&mut document, "profiles", profile_table());
    let installed = document.to_string().into_bytes();
    let original_sha256 = sha256(&original);
    let installed_sha256 = sha256(&installed);
    let backups = arbiter_home.join("backups");
    fs::create_dir_all(&backups)?;
    let backup_path = backups.join(format!("codex-config-{timestamp_unix_ms}.toml"));
    let backup_hash_path = backup_path.with_extension("toml.sha256");
    write_new_synced(&backup_path, &original)?;
    write_new_synced(&backup_hash_path, format!("{original_sha256}\n").as_bytes())?;
    atomic_replace(config_path, &installed)?;

    Ok(InstallReceipt {
        backup_path,
        backup_hash_path,
        original_sha256,
        installed_sha256,
    })
}

/// Restores the exact verified pre-Arbiter configuration.
///
/// # Errors
///
/// Returns a conflict if the installed configuration changed, or an integrity
/// error if the backup no longer matches its recorded hash.
pub fn uninstall_profile(
    config_path: &Path,
    receipt: &InstallReceipt,
) -> Result<(), CodexProfileError> {
    let current = fs::read(config_path)?;
    if sha256(&current) != receipt.installed_sha256 {
        return Err(CodexProfileError::ConfigurationConflict);
    }

    let backup = fs::read(&receipt.backup_path)?;
    let sidecar = fs::read_to_string(&receipt.backup_hash_path)?;
    if sha256(&backup) != receipt.original_sha256 || sidecar.trim() != receipt.original_sha256 {
        return Err(CodexProfileError::BackupHashMismatch);
    }

    atomic_replace(config_path, &backup)
}

fn managed_entry_exists(document: &DocumentMut, section: &str) -> bool {
    document
        .get(section)
        .and_then(Item::as_table)
        .is_some_and(|table| table.contains_key("arbiter"))
}

fn insert_managed_table(document: &mut DocumentMut, section: &str, managed: Table) {
    if !document.contains_key(section) {
        document[section] = Item::Table(Table::new());
    }
    document[section]["arbiter"] = Item::Table(managed);
}

fn provider_table(port: u16) -> Table {
    let mut table = Table::new();
    table["name"] = value("Arbiter");
    table["base_url"] = value(format!("http://127.0.0.1:{port}/v1"));
    table["wire_api"] = value("responses");
    table["requires_openai_auth"] = value(true);
    table["request_max_retries"] = value(0);
    table["stream_max_retries"] = value(0);
    table
}

fn profile_table() -> Table {
    let mut table = Table::new();
    table["model"] = value("gpt-5.6-terra");
    table["model_provider"] = value("arbiter");
    table["model_reasoning_effort"] = value("medium");
    table
}
