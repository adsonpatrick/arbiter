use arbiter_adapter_codex::{CodexProfileError, install_profile, uninstall_profile};
use tempfile::tempdir;

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
    assert!(installed.contains("[profiles.arbiter]"));
    assert!(installed.contains("model = \"gpt-5.6-terra\""));
    assert!(installed.contains("model_provider = \"arbiter\""));
    assert!(installed.contains("model_reasoning_effort = \"medium\""));
    assert!(!installed.contains("OPENAI_API_KEY"));
    assert!(!installed.contains("experimental_bearer_token"));
    assert!(receipt.backup_path.exists());
    assert!(receipt.backup_hash_path.exists());

    uninstall_profile(&config_path, &receipt).expect("uninstall profile");

    assert_eq!(std::fs::read_to_string(&config_path).unwrap(), ORIGINAL);
}

#[test]
fn install_supports_a_minimal_config_without_changing_its_default_profile() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "model = \"personal-model\"\n").unwrap();

    install_profile(&config_path, &arbiter_home, 9_999, 1).expect("install profile");

    let installed = std::fs::read_to_string(config_path).unwrap();
    assert!(installed.contains("model = \"personal-model\""));
    assert!(installed.contains("[model_providers.arbiter]"));
    assert!(installed.contains("base_url = \"http://127.0.0.1:9999/v1\""));
    assert!(installed.contains("[profiles.arbiter]"));
    assert!(!installed.starts_with("profile = \"arbiter\""));
}

#[test]
fn install_rejects_managed_entry_collisions_without_writing_anything() {
    for original in [
        "[model_providers.arbiter]\nname = \"mine\"\n",
        "[profiles.arbiter]\nmodel = \"mine\"\n",
    ] {
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
fn uninstall_refuses_a_tampered_backup_without_touching_the_config() {
    let temporary = tempdir().unwrap();
    let config_path = temporary.path().join("codex/config.toml");
    let arbiter_home = temporary.path().join("arbiter");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, ORIGINAL).unwrap();
    let receipt = install_profile(&config_path, &arbiter_home, 43_123, 4).unwrap();
    let installed = std::fs::read(&config_path).unwrap();
    std::fs::write(&receipt.backup_path, "tampered").unwrap();

    let error = uninstall_profile(&config_path, &receipt).unwrap_err();

    assert!(matches!(error, CodexProfileError::BackupHashMismatch));
    assert_eq!(std::fs::read(config_path).unwrap(), installed);
}
