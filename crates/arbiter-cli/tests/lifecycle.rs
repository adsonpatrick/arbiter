use std::{
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
};

use tempfile::tempdir;

const ORIGINAL_CODEX_CONFIG: &str = "# user config\nprofile = \"daily\"\n";

fn arbiter(home: &Path, codex_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arbiter"))
        .args(args)
        .env("ARBITER_HOME", home)
        .env("CODEX_HOME", codex_home)
        .env("HOME", home.parent().unwrap())
        .env("USERPROFILE", home.parent().unwrap())
        .env("OPENAI_API_KEY", "must-never-be-persisted")
        .output()
        .expect("run arbiter")
}

fn start_arbiter(home: &Path, codex_home: &Path) -> std::process::ExitStatus {
    Command::new(env!("CARGO_BIN_EXE_arbiter"))
        .arg("start")
        .env("ARBITER_HOME", home)
        .env("CODEX_HOME", codex_home)
        .env("HOME", home.parent().unwrap())
        .env("USERPROFILE", home.parent().unwrap())
        .env("OPENAI_API_KEY", "must-never-be-persisted")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("start arbiter")
}

#[test]
fn init_status_uninstall_lifecycle_is_safe_and_stays_passthrough() {
    let temporary = tempdir().unwrap();
    let arbiter_home = temporary.path().join(".arbiter");
    let codex_home = temporary.path().join(".codex");
    let codex_config = codex_home.join("config.toml");
    std::fs::create_dir_all(&codex_home).unwrap();
    std::fs::write(&codex_config, ORIGINAL_CODEX_CONFIG).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);

    let plan = arbiter(
        &arbiter_home,
        &codex_home,
        &["init", "codex", "--port", &port],
    );
    assert!(
        plan.status.success(),
        "{}",
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(String::from_utf8_lossy(&plan.stdout).contains("Plan"));
    assert_eq!(
        std::fs::read_to_string(&codex_config).unwrap(),
        ORIGINAL_CODEX_CONFIG
    );
    assert!(!arbiter_home.exists());

    let init = arbiter(
        &arbiter_home,
        &codex_home,
        &["init", "codex", "--yes", "--port", &port],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(arbiter_home.join("config.json").exists());
    assert!(arbiter_home.join("arbiter.db").exists());
    assert!(arbiter_home.join("install-receipt.json").exists());
    let installed = std::fs::read_to_string(&codex_config).unwrap();
    let profile = std::fs::read_to_string(codex_home.join("arbiter.config.toml")).unwrap();
    assert!(installed.contains("[model_providers.arbiter]"));
    assert!(!installed.contains("[profiles.arbiter]"));
    assert!(installed.contains("requires_openai_auth = true"));
    assert!(profile.contains("model = \"gpt-5.6-terra\""));
    assert!(profile.contains("model_reasoning_effort = \"medium\""));
    assert!(installed.starts_with("# user config\nprofile = \"daily\""));
    for entry in std::fs::read_dir(&arbiter_home).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            let bytes = std::fs::read(entry.path()).unwrap();
            assert!(
                !bytes
                    .windows(b"must-never-be-persisted".len())
                    .any(|window| { window == b"must-never-be-persisted" })
            );
        }
    }

    let status = arbiter(&arbiter_home, &codex_home, &["status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(status.contains("PASSTHROUGH"));
    assert!(status.contains("gpt-5.6-terra"));
    assert!(status.contains("medium"));
    assert!(status.contains("storage: ok"));
    assert!(status.contains("codex profile: installed"));
    assert!(status.contains("recent attempts: 0"));
    assert!(status.contains("daemon: stopped"));

    let start = start_arbiter(&arbiter_home, &codex_home);
    assert!(start.success());
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(arbiter_home.join("server.json")).unwrap()).unwrap();
    let fields = metadata.as_object().unwrap();
    assert_eq!(fields.len(), 3);
    assert!(fields.contains_key("pid"));
    assert_eq!(fields["port"], port.parse::<u16>().unwrap());
    assert_eq!(fields["version"], env!("CARGO_PKG_VERSION"));

    let running_status = arbiter(&arbiter_home, &codex_home, &["status"]);
    assert!(running_status.status.success());
    assert!(String::from_utf8_lossy(&running_status.stdout).contains("daemon: healthy"));

    let uninstall = arbiter(&arbiter_home, &codex_home, &["uninstall", "--yes"]);
    assert!(
        uninstall.status.success(),
        "{}",
        String::from_utf8_lossy(&uninstall.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&codex_config).unwrap(),
        ORIGINAL_CODEX_CONFIG
    );
    assert!(arbiter_home.join("arbiter.db").exists());
    assert!(!arbiter_home.join("install-receipt.json").exists());
    assert!(!arbiter_home.join("server.json").exists());
    assert!(!codex_home.join("arbiter.config.toml").exists());
    assert!(String::from_utf8_lossy(&uninstall.stdout).contains("arbiter.db"));
}
