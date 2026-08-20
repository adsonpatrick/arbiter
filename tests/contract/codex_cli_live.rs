use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use arbiter_core::events::GovernorEventKind;
use arbiter_storage_sqlite::SqliteEventStore;
use tempfile::tempdir;

#[tokio::test]
#[ignore = "requires arbiter init/start plus a local Codex ChatGPT login and live requests"]
async fn codex_profile_streams_through_arbiter_and_cancellation_never_completes() {
    let arbiter_home =
        PathBuf::from(env::var_os("ARBITER_HOME").expect("ARBITER_HOME is required"));
    let database = arbiter_home.join("arbiter.db");
    assert!(database.is_file(), "run `arbiter init codex --yes` first");
    let doctor = arbiter(&arbiter_home, &["doctor"]);
    assert!(doctor.status.success(), "Arbiter doctor failed");
    assert!(
        arbiter_start(&arbiter_home).success(),
        "Arbiter start failed"
    );

    let before = recent_counts(&database).await;
    let temporary = tempdir().expect("temporary working directory");
    let output = codex_command(temporary.path(), "Reply with exactly: arbiter-ok")
        .output()
        .expect("run Codex-through-Arbiter probe");
    assert!(
        output.status.success(),
        "Codex-through-Arbiter probe failed"
    );
    let stdout = String::from_utf8(output.stdout).expect("Codex JSONL is UTF-8");
    let streamed_events = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid Codex JSONL"))
        .collect::<Vec<_>>();
    assert!(
        streamed_events.len() > 1,
        "Codex did not emit a streamed event sequence"
    );
    assert!(
        stdout.contains("arbiter-ok"),
        "Codex terminal answer did not match"
    );

    let store = SqliteEventStore::open(&database)
        .await
        .expect("open event store");
    let after = store
        .recent_attempt_counts(0)
        .await
        .expect("attempt counts");
    assert!(after.started > before.started);
    assert!(after.completed > before.completed);
    let events = store
        .latest_attempt_events()
        .await
        .expect("latest attempt events");
    assert!(events.iter().any(|event| {
        matches!(
            &event.kind,
            GovernorEventKind::AttemptCompleted(completed)
                if completed.usage.input_tokens > 0 && completed.usage.output_tokens > 0
        )
    }));
    store.close().await;
    assert_private_storage(&database, b"arbiter-ok");

    let before_cancel = recent_counts(&database).await;
    let mut child = codex_command(
        temporary.path(),
        "Without tools, count upward one integer per line for as long as possible.",
    )
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .expect("start cancellation probe");
    let mut observed_start = false;
    for _ in 0..200 {
        if recent_counts(&database).await.started > before_cancel.started {
            observed_start = true;
            break;
        }
        if child.try_wait().expect("poll Codex").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(observed_start, "cancellation probe never reached Arbiter");
    child.kill().expect("terminate cancellation probe");
    let _ = child.wait();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let store = SqliteEventStore::open(&database)
        .await
        .expect("reopen event store");
    let cancelled = store
        .latest_attempt_events()
        .await
        .expect("cancelled attempt events");
    store.close().await;
    assert!(
        !cancelled
            .iter()
            .any(|event| { matches!(event.kind, GovernorEventKind::AttemptCompleted(_)) })
    );
}

fn arbiter(arbiter_home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arbiter"))
        .args(args)
        .env("ARBITER_HOME", arbiter_home)
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("run Arbiter")
}

fn arbiter_start(arbiter_home: &Path) -> std::process::ExitStatus {
    Command::new(env!("CARGO_BIN_EXE_arbiter"))
        .arg("start")
        .env("ARBITER_HOME", arbiter_home)
        .env_remove("OPENAI_API_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("start Arbiter")
}

fn codex_command(working_directory: &Path, prompt: &str) -> Command {
    let mut command = Command::new("codex");
    command
        .args([
            "exec",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--json",
            "--profile",
            "arbiter",
            prompt,
        ])
        .current_dir(working_directory)
        .env_remove("OPENAI_API_KEY");
    command
}

async fn recent_counts(path: &Path) -> arbiter_storage_sqlite::RecentAttemptCounts {
    let store = SqliteEventStore::open(path)
        .await
        .expect("open event store");
    let counts = store
        .recent_attempt_counts(0)
        .await
        .expect("attempt counts");
    store.close().await;
    counts
}

fn assert_private_storage(database: &Path, forbidden: &[u8]) {
    for path in [
        database.to_owned(),
        database.with_extension("db-wal"),
        database.with_extension("db-shm"),
    ] {
        if let Ok(bytes) = fs::read(path) {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden)
            );
        }
    }
}
