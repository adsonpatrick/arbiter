use arbiter_adapter_codex::{
    CodexProfileError, install_profile, uninstall_profile, validate_managed_profile,
};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const ORIGINAL: &str = include_str!("../../../tests/fixtures/codex_config_existing_provider.toml");

#[test]
fn install_preserves_unrelated_toml_and_uninstall_restores_exact_backup() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();

    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 1_777_000_000_000)
        .expect("install profile");
    let installed = std::fs::read_to_string(&config_path).unwrap();
    let profile_path = config_path.parent().unwrap().join("arbiter.config.toml");
    let profile = std::fs::read_to_string(&profile_path).unwrap();

    assert!(installed.contains("# Keep this user comment."));
    assert!(installed.contains("profile = \"daily\""));
    assert!(installed.contains("[model_providers.company]"));
    assert!(installed.contains("env_key = \"COMPANY_TOKEN\""));
    assert!(installed.contains("[model_providers.arbiter]"));
    assert!(installed.contains("base_url = \"http://127.0.0.1:43123/v1\""));
    assert!(installed.contains("wire_api = \"responses\""));
    assert!(installed.contains("requires_openai_auth = true"));
    assert!(installed.contains("request_max_retries = 0"));
    assert!(installed.contains("stream_max_retries = 0"));
    assert!(!installed.contains("[profiles.arbiter]"));
    assert!(profile.contains("model = \"gpt-5.6-terra\""));
    assert!(profile.contains("model_provider = \"arbiter\""));
    assert!(profile.contains("model_reasoning_effort = \"medium\""));
    assert!(!profile.contains("env_key"));
    assert!(!profile.contains("experimental_bearer_token"));
    assert!(!installed.contains("OPENAI_API_KEY"));
    assert!(!installed.contains("experimental_bearer_token"));
    assert!(receipt.config.backup_path.exists());
    assert!(receipt.config.backup_hash_path.exists());
    assert!(receipt.profile.backup_path.exists());
    assert!(receipt.profile.backup_hash_path.exists());
    validate_managed_profile(&config_path, 43_123).expect("valid managed profile");

    uninstall_profile(&config_path, &receipt).expect("uninstall profile");

    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), ORIGINAL);
    assert!(!profile_path.exists());
}

#[test]
fn validation_rejects_credentials_or_contract_changes_in_managed_entries() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    install_profile(&config_path, &arbiter_home, 43_123, 6).unwrap();
    let changed = std::fs::read_to_string(&config_path).unwrap().replace(
        "stream_max_retries = 0",
        "stream_max_retries = 0\nenv_key = \"FORBIDDEN\"",
    );
    std::fs::write(&config_path, changed).unwrap();

    let error = validate_managed_profile(&config_path, 43_123).unwrap_err();

    assert!(matches!(error, CodexProfileError::ManagedEntryInvalid));
}

#[test]
fn install_supports_a_minimal_config_without_changing_its_default_profile() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "model = \"personal-model\"\n").unwrap();

    install_profile(&config_path, &arbiter_home, 9_999, 1).expect("install profile");

    let installed = std::fs::read_to_string(&config_path).unwrap();
    assert!(installed.contains("model = \"personal-model\""));
    assert!(installed.contains("[model_providers.arbiter]"));
    assert!(installed.contains("base_url = \"http://127.0.0.1:9999/v1\""));
    assert!(
        config_path
            .parent()
            .unwrap()
            .join("arbiter.config.toml")
            .exists()
    );
    assert!(!installed.starts_with("profile = \"arbiter\""));
}

#[test]
fn install_rejects_managed_entry_collisions_without_writing_anything() {
    let original = "[model_providers.arbiter]\nname = \"mine\"\n";
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, original).unwrap();

    let error = install_profile(&config_path, &arbiter_home, 43_123, 2).unwrap_err();

    assert!(matches!(error, CodexProfileError::ManagedEntryExists));
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), original);
    assert!(!arbiter_home.exists());
}

#[test]
fn uninstall_refuses_to_overwrite_a_config_edited_after_install() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 3).unwrap();
    let edited = format!(
        "{}\n# edited after install\n",
        std::fs::read_to_string(&config_path).unwrap()
    );
    std::fs::write(&config_path, &edited).unwrap();

    let error = uninstall_profile(&config_path, &receipt).unwrap_err();

    assert!(matches!(error, CodexProfileError::ConfigurationConflict));
    assert_eq!(std::fs::read_to_string(config_path).unwrap(), edited);
}

#[test]
fn uninstall_refuses_to_overwrite_a_named_profile_edited_after_install() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let profile_path = config_path.parent().unwrap().join("arbiter.config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 8).unwrap();
    let edited = format!(
        "{}\n# edited after install\n",
        std::fs::read_to_string(&profile_path).unwrap()
    );
    std::fs::write(&profile_path, &edited).unwrap();

    let error = uninstall_profile(&config_path, &receipt).unwrap_err();

    assert!(matches!(error, CodexProfileError::ConfigurationConflict));
    assert_eq!(std::fs::read_to_string(profile_path).unwrap(), edited);
}

#[test]
fn uninstall_refuses_a_tampered_backup_without_touching_the_config() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 4).unwrap();
    let installed = std::fs::read(&config_path).unwrap();
    std::fs::write(&receipt.config.backup_path, "tampered").unwrap();

    let error = uninstall_profile(&config_path, &receipt).unwrap_err();

    assert!(matches!(error, CodexProfileError::BackupHashMismatch));
    assert_eq!(std::fs::read(config_path).unwrap(), installed);
}

#[test]
fn uninstall_removes_a_config_that_did_not_exist_before_install() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");

    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 5).unwrap();
    assert!(config_path.exists());
    assert!(
        config_path
            .parent()
            .unwrap()
            .join("arbiter.config.toml")
            .exists()
    );

    uninstall_profile(&config_path, &receipt).unwrap();

    assert!(!config_path.exists());
    assert!(
        !config_path
            .parent()
            .unwrap()
            .join("arbiter.config.toml")
            .exists()
    );
}

#[test]
fn install_replaces_and_exactly_restores_an_existing_named_profile_file() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let profile_path = config_path.parent().unwrap().join("arbiter.config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    let original_profile = b"# prior profile\nmodel = \"gpt-5\"\n";
    std::fs::write(&profile_path, original_profile).unwrap();

    #[cfg(unix)]
    std::fs::set_permissions(&profile_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 7).unwrap();

    assert!(receipt.profile.original_existed);
    assert_ne!(std::fs::read(&profile_path).unwrap(), original_profile);

    uninstall_profile(&config_path, &receipt).unwrap();

    assert_eq!(std::fs::read(&config_path).unwrap(), ORIGINAL.as_bytes());
    assert_eq!(std::fs::read(&profile_path).unwrap(), original_profile);
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&profile_path).unwrap().mode() & 0o777,
        0o400
    );
}

#[cfg(unix)]
#[test]
fn managed_directories_and_backups_are_owner_only_and_stricter_modes_survive() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let profile_path = config_path.parent().unwrap().join("arbiter.config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    std::fs::write(&profile_path, "model = \"private\"\n").unwrap();
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o400)).unwrap();
    std::fs::set_permissions(&profile_path, std::fs::Permissions::from_mode(0o400)).unwrap();

    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 9).unwrap();

    for directory in [&arbiter_home, &arbiter_home.join("backups")] {
        assert_eq!(std::fs::metadata(directory).unwrap().mode() & 0o077, 0);
    }
    for file in [
        &receipt.config.backup_path,
        &receipt.config.backup_hash_path,
        &receipt.profile.backup_path,
        &receipt.profile.backup_hash_path,
        &config_path,
        &profile_path,
    ] {
        assert_eq!(std::fs::metadata(file).unwrap().mode() & 0o177, 0);
    }

    uninstall_profile(&config_path, &receipt).unwrap();
    assert_eq!(
        std::fs::metadata(&config_path).unwrap().mode() & 0o777,
        0o400
    );
    assert_eq!(
        std::fs::metadata(&profile_path).unwrap().mode() & 0o777,
        0o400
    );
}
