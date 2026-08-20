use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
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

fn serve_health_response(
    listener: TcpListener,
    body: String,
) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(300)))
                        .unwrap();
                    let mut request = [0_u8; 1_024];
                    if stream.read(&mut request).unwrap_or(0) > 0 {
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept health request: {error}"),
            }
        }
    });
    (stop, server)
}

fn daemon_health(port: u16) -> serde_json::Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect daemon health");
    write!(
        stream,
        "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("write daemon health request");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read daemon health");
    let body = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| &response[position + 4..])
        .expect("health response body");
    serde_json::from_slice(body).expect("parse daemon health")
}

fn assert_running_identity(arbiter_home: &Path, port: u16) {
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(arbiter_home.join("server.json")).unwrap()).unwrap();
    let fields = metadata.as_object().unwrap();
    assert_eq!(fields.len(), 4);
    assert!(fields.contains_key("pid"));
    assert_eq!(fields["port"], port);
    assert_eq!(fields["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        fields["instance_id"]
            .as_str()
            .is_some_and(|instance_id| !instance_id.is_empty())
    );
    assert_eq!(daemon_health(port)["identity"], metadata);
}

#[test]
fn init_without_port_persists_an_ephemeral_port_and_random_instance() {
    let temporary = tempdir().unwrap();
    let arbiter_home = temporary.path().join(".arbiter");
    let codex_home = temporary.path().join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();

    let init = arbiter(&arbiter_home, &codex_home, &["init", "codex", "--yes"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(arbiter_home.join("config.json")).unwrap()).unwrap();
    assert!(config["port"].as_u64().is_some_and(|port| port > 0));
    assert!(
        config["instance_id"]
            .as_str()
            .is_some_and(|instance_id| !instance_id.is_empty())
    );
}

#[test]
fn stale_metadata_plus_generic_health_is_not_accepted_as_arbiter() {
    let temporary = tempdir().unwrap();
    let arbiter_home = temporary.path().join(".arbiter");
    let codex_home = temporary.path().join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let init = arbiter(
        &arbiter_home,
        &codex_home,
        &["init", "codex", "--yes", "--port", &port.to_string()],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(arbiter_home.join("config.json")).unwrap()).unwrap();
    std::fs::write(
        arbiter_home.join("server.json"),
        serde_json::to_vec(&serde_json::json!({
            "pid": std::process::id(),
            "port": port,
            "version": env!("CARGO_PKG_VERSION"),
            "instance_id": config["instance_id"],
        }))
        .unwrap(),
    )
    .unwrap();
    let (stop, server) = serve_health_response(listener, "{}".to_owned());

    let start = arbiter(&arbiter_home, &codex_home, &["start"]);
    let status = arbiter(&arbiter_home, &codex_home, &["status"]);
    let doctor = arbiter(&arbiter_home, &codex_home, &["doctor"]);
    let uninstall = arbiter(&arbiter_home, &codex_home, &["uninstall", "--yes"]);
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    for output in [start, status, doctor, uninstall] {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("unrecognized process"));
    }
    assert!(arbiter_home.join("install-receipt.json").exists());
}

#[test]
fn a_health_identity_mismatch_is_not_accepted_as_arbiter() {
    let temporary = tempdir().unwrap();
    let arbiter_home = temporary.path().join(".arbiter");
    let codex_home = temporary.path().join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let init = arbiter(
        &arbiter_home,
        &codex_home,
        &["init", "codex", "--yes", "--port", &port.to_string()],
    );
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(arbiter_home.join("config.json")).unwrap()).unwrap();
    let metadata = serde_json::json!({
        "pid": std::process::id(),
        "port": port,
        "version": env!("CARGO_PKG_VERSION"),
        "instance_id": config["instance_id"],
    });
    std::fs::write(
        arbiter_home.join("server.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    let mismatched_health = serde_json::json!({
        "status": "healthy",
        "components": [],
        "identity": {
            "pid": metadata["pid"],
            "port": port,
            "version": metadata["version"],
            "instance_id": "wrong-instance",
        }
    })
    .to_string();
    let (stop, server) = serve_health_response(listener, mismatched_health);

    let start = arbiter(&arbiter_home, &codex_home, &["start"]);
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    assert!(!start.status.success());
    assert!(String::from_utf8_lossy(&start.stderr).contains("unrecognized process"));
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
    assert_running_identity(&arbiter_home, port.parse().unwrap());

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
