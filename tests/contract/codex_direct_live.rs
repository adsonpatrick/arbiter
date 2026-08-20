use std::process::Command;

use tempfile::tempdir;

#[test]
#[ignore = "requires a local Codex ChatGPT login and performs a live model request"]
fn terra_medium_streams_with_codex_managed_chatgpt_authentication() {
    require_chatgpt_login();
    let temporary = tempdir().expect("temporary working directory");
    let output = Command::new("codex")
        .args([
            "exec",
            "--ephemeral",
            "--ignore-user-config",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--json",
            "--model",
            "gpt-5.6-terra",
            "--config",
            "model_reasoning_effort=\"medium\"",
            "Reply with exactly: direct-ok",
        ])
        .current_dir(temporary.path())
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("run direct Codex probe");

    assert!(output.status.success(), "direct Codex probe failed");
    let stdout = String::from_utf8(output.stdout).expect("Codex JSONL is UTF-8");
    let events = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid Codex JSONL"))
        .collect::<Vec<_>>();
    assert!(
        events.len() > 1,
        "Codex did not emit a streamed event sequence"
    );
    assert!(
        stdout.contains("direct-ok"),
        "Codex terminal answer did not match"
    );
}

fn require_chatgpt_login() {
    let output = Command::new("codex")
        .args(["login", "status"])
        .env_remove("OPENAI_API_KEY")
        .output()
        .expect("run Codex login status");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_ascii_lowercase();
    assert!(output.status.success(), "Codex login status failed");
    assert!(
        report.contains("chatgpt"),
        "live probe requires ChatGPT login"
    );
    assert!(
        !report.contains("api key"),
        "API-key login is unsupported in M0"
    );
}
