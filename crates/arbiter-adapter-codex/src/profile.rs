use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::{
    config_file::sha256,
    file_security::{
        OriginalPermissions, atomic_replace_private, atomic_replace_with_permissions,
        ensure_private_dir, write_new_private_synced,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedFileReceipt {
    pub path: PathBuf,
    pub backup_path: PathBuf,
    pub backup_hash_path: PathBuf,
    pub original_sha256: String,
    pub installed_sha256: String,
    pub original_existed: bool,
    #[serde(default)]
    pub original_permissions: OriginalPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReceipt {
    pub config: ManagedFileReceipt,
    pub profile: ManagedFileReceipt,
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
    #[error("Codex Arbiter provider or profile is missing or does not match M0")]
    ManagedEntryInvalid,
    #[error("Codex installation receipt does not match the managed paths")]
    ManagedPathMismatch,
}

/// Installs the managed Arbiter provider and named profile without changing defaults.
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
    let profile_path = managed_profile_path(config_path)?;
    let (original, original_existed) = read_optional(config_path)?;
    let (profile_original, profile_original_existed) = read_optional(&profile_path)?;
    let original_permissions = OriginalPermissions::capture(config_path, original_existed)?;
    let profile_original_permissions =
        OriginalPermissions::capture(&profile_path, profile_original_existed)?;

    let mut document = std::str::from_utf8(&original)?.parse::<DocumentMut>()?;
    if managed_entry_exists(&document, "model_providers") {
        return Err(CodexProfileError::ManagedEntryExists);
    }
    insert_managed_table(&mut document, "model_providers", provider_table(port));
    let installed = document.to_string().into_bytes();
    let profile_installed = profile_document().to_string().into_bytes();

    let backups = arbiter_home.join("backups");
    ensure_private_dir(arbiter_home)?;
    ensure_private_dir(&backups)?;
    let config_receipt = backup_resource(
        config_path,
        &backups,
        "codex-config",
        timestamp_unix_ms,
        &original,
        &installed,
        original_existed,
        original_permissions.clone(),
    )?;
    let profile_receipt = backup_resource(
        &profile_path,
        &backups,
        "codex-arbiter-profile",
        timestamp_unix_ms,
        &profile_original,
        &profile_installed,
        profile_original_existed,
        profile_original_permissions.clone(),
    )?;

    atomic_replace_private(config_path, &installed, &original_permissions)?;
    if let Err(error) = atomic_replace_private(
        &profile_path,
        &profile_installed,
        &profile_original_permissions,
    ) {
        restore_bytes(
            config_path,
            &original,
            original_existed,
            &original_permissions,
        )?;
        return Err(error.into());
    }

    Ok(InstallReceipt {
        config: config_receipt,
        profile: profile_receipt,
    })
}

/// Restores the exact verified pre-Arbiter configuration and profile state.
///
/// # Errors
///
/// Returns a conflict if either installed file changed, or an integrity error
/// if either backup no longer matches its recorded hash.
pub fn uninstall_profile(
    config_path: &Path,
    receipt: &InstallReceipt,
) -> Result<(), CodexProfileError> {
    let expected_profile_path = managed_profile_path(config_path)?;
    if receipt.config.path != config_path || receipt.profile.path != expected_profile_path {
        return Err(CodexProfileError::ManagedPathMismatch);
    }
    if !installed_resource_matches(&receipt.config)?
        || !installed_resource_matches(&receipt.profile)?
    {
        return Err(CodexProfileError::ConfigurationConflict);
    }

    let config_backup = verified_backup(&receipt.config)?;
    let profile_backup = verified_backup(&receipt.profile)?;
    restore_bytes(
        &receipt.config.path,
        &config_backup,
        receipt.config.original_existed,
        &receipt.config.original_permissions,
    )?;
    restore_bytes(
        &receipt.profile.path,
        &profile_backup,
        receipt.profile.original_existed,
        &receipt.profile.original_permissions,
    )
}

/// Verifies that the managed provider and named profile match the M0 contract.
///
/// # Errors
///
/// Returns an error when either file cannot be read or parsed, or when a
/// managed field is absent or has changed.
pub fn validate_managed_profile(config_path: &Path, port: u16) -> Result<(), CodexProfileError> {
    let source = fs::read(config_path)?;
    let document = std::str::from_utf8(&source)?.parse::<DocumentMut>()?;
    let profile_source = fs::read(managed_profile_path(config_path)?)?;
    let profile = std::str::from_utf8(&profile_source)?.parse::<DocumentMut>()?;
    let provider = document["model_providers"]["arbiter"]
        .as_table()
        .ok_or(CodexProfileError::ManagedEntryInvalid)?;
    if table_matches(provider, &provider_table(port)) && profile_matches(&profile) {
        Ok(())
    } else {
        Err(CodexProfileError::ManagedEntryInvalid)
    }
}

fn read_optional(path: &Path) -> Result<(Vec<u8>, bool), CodexProfileError> {
    match fs::read(path) {
        Ok(bytes) => Ok((bytes, true)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok((Vec::new(), false)),
        Err(error) => Err(error.into()),
    }
}

fn managed_profile_path(config_path: &Path) -> Result<PathBuf, CodexProfileError> {
    config_path
        .parent()
        .map(|parent| parent.join("arbiter.config.toml"))
        .ok_or(CodexProfileError::MissingParent)
}

#[allow(clippy::too_many_arguments)]
fn backup_resource(
    path: &Path,
    backup_root: &Path,
    name: &str,
    timestamp_unix_ms: u64,
    original: &[u8],
    installed: &[u8],
    original_existed: bool,
    original_permissions: OriginalPermissions,
) -> Result<ManagedFileReceipt, CodexProfileError> {
    let original_sha256 = sha256(original);
    let backup_path = backup_root.join(format!("{name}-{timestamp_unix_ms}.toml"));
    let backup_hash_path = backup_path.with_extension("toml.sha256");
    write_new_private_synced(&backup_path, original)?;
    write_new_private_synced(&backup_hash_path, format!("{original_sha256}\n").as_bytes())?;
    Ok(ManagedFileReceipt {
        path: path.to_owned(),
        backup_path,
        backup_hash_path,
        original_sha256,
        installed_sha256: sha256(installed),
        original_existed,
        original_permissions,
    })
}

fn installed_resource_matches(receipt: &ManagedFileReceipt) -> Result<bool, CodexProfileError> {
    let (current, exists) = read_optional(&receipt.path)?;
    Ok(exists && sha256(&current) == receipt.installed_sha256)
}

fn verified_backup(receipt: &ManagedFileReceipt) -> Result<Vec<u8>, CodexProfileError> {
    let backup = fs::read(&receipt.backup_path)?;
    let sidecar = fs::read_to_string(&receipt.backup_hash_path)?;
    if sha256(&backup) == receipt.original_sha256 && sidecar.trim() == receipt.original_sha256 {
        Ok(backup)
    } else {
        Err(CodexProfileError::BackupHashMismatch)
    }
}

fn restore_bytes(
    path: &Path,
    bytes: &[u8],
    existed: bool,
    permissions: &OriginalPermissions,
) -> Result<(), CodexProfileError> {
    if existed {
        atomic_replace_with_permissions(path, bytes, permissions).map_err(Into::into)
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
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

fn table_matches(actual: &Table, expected: &Table) -> bool {
    expected.iter().all(|(key, expected_value)| {
        let actual_value = actual.get(key);
        if let Some(expected_string) = expected_value.as_str() {
            actual_value.and_then(Item::as_str) == Some(expected_string)
        } else if let Some(expected_bool) = expected_value.as_bool() {
            actual_value.and_then(Item::as_bool) == Some(expected_bool)
        } else if let Some(expected_integer) = expected_value.as_integer() {
            actual_value.and_then(Item::as_integer) == Some(expected_integer)
        } else {
            false
        }
    }) && !actual.contains_key("env_key")
        && !actual.contains_key("experimental_bearer_token")
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

fn profile_document() -> DocumentMut {
    let mut document = DocumentMut::new();
    document["model"] = value("gpt-5.6-terra");
    document["model_provider"] = value("arbiter");
    document["model_reasoning_effort"] = value("medium");
    document
}

fn profile_matches(profile: &DocumentMut) -> bool {
    profile["model"].as_str() == Some("gpt-5.6-terra")
        && profile["model_provider"].as_str() == Some("arbiter")
        && profile["model_reasoning_effort"].as_str() == Some("medium")
}
