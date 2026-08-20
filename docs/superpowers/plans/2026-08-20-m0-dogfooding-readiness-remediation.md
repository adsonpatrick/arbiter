# Arbiter M0 Dogfooding Readiness Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three post-M0 findings so Arbiter can move from canary use to daily NeuroVia dogfooding on a personal developer workstation without weakening M0 privacy, durability, restoration, or latency guarantees.

**Architecture:** Keep the existing Rust modular local monolith. Add an explicit 32 MiB request admission contract at the Axum boundary; separate public health from an authenticated localhost control identity surface; move daemon endpoint authority from installation config to listener-derived runtime state with compensating Codex-config/receipt transitions; and add deterministic Linux/Windows GitHub CI. The fixed `gpt-5.6-terra` / medium inference path and Codex-managed ChatGPT authentication remain unchanged.

**Tech Stack:** Rust 1.97.1, Tokio 1.x, Axum 0.8.x, Reqwest 0.12.x, SQLx 0.8.x + SQLite, Clap 4.x, Serde, `getrandom` 0.3, `base64` 0.22, GitHub Actions, cargo-deny 0.20.2.

**Spec:** `docs/superpowers/specs/2026-08-20-m0-dogfooding-readiness-remediation-design.md`

## Global Constraints

- Stay inside M0. Do not add Session/Turn/Step, routing, risk classification, budgets, evals, promotion, Supabase, LiteLLM, dashboards, or new harnesses/providers.
- The production target remains exactly `gpt-5.6-terra` with `medium` reasoning.
- Codex remains the authentication owner. Arbiter must not read Codex credential storage or own an OpenAI API key.
- Inference binds only to IPv4 loopback. Upstream remains the pinned first-party Codex Responses endpoint with redirects disabled and provider retries/fallback disabled.
- Request/response bodies, prompts, source code, tool content, credentials, cookies, repository paths, and control credentials must never enter logs, events, SQLite, WAL, SHM, or remote export.
- `AttemptStarted` remains durable before an upstream call. Pre-attempt HTTP rejection must create no attempt and make no upstream call.
- One physical provider invocation remains one immutable attempt, with at most one durable terminal.
- Response streaming/cancellation semantics remain byte-transparent.
- The request body contract is exactly 33,554,432 bytes (32 MiB): syntactically valid JSON at the exact boundary is accepted; one byte over is HTTP 413.
- `config.json` schema version 2 contains installation invariants only. Active port and `instance_id` live only in runtime `server.json` and authenticated control identity.
- `arbiter init codex --yes` installs an inactive provider endpoint `http://127.0.0.1:0/v1`; port zero is never used for inference.
- Every daemon start binds `127.0.0.1:0` first and publishes only the listener-assigned port.
- `GET /healthz` is public liveness/health only. Exact management identity is exposed only by `GET /control/identity` when `X-Arbiter-Control-Token` is valid.
- The control credential is exactly 32 OS-random bytes encoded as unpadded base64url, stored owner-only at `$ARBITER_HOME/control-token`.
- Missing/invalid control credentials return `404 Not Found` without identity data.
- Runtime endpoint publication/depublication must update Codex config and current installed receipt hash as a compensating transition. Immutable pre-Arbiter backup bytes/hashes never change.
- Uninstall retains exact restoration/conflict semantics and preserves `arbiter.db`.
- Public CI receives no OpenAI/ChatGPT credential. Live contract probes and the 1,000-pair performance benchmark remain manual release gates.
- Pre-remediation development installs are not migrated in place. Before installing this remediation build over a local M0 checkout, run the pre-remediation `arbiter uninstall --yes`; the remediated `init` always creates schema v2 state.

---

## File Structure Map

Files created:

- `crates/arbiter-core/src/control.rs` — redacted control-token value object and control header constant.
- `crates/arbiter-cli/src/commands/control.rs` — owner-only token generation/loading and authenticated control identity client.
- `crates/arbiter-cli/src/commands/endpoint.rs` — Codex endpoint publication/depublication plus receipt persistence/compensation.
- `crates/arbiter-daemon/src/control.rs` — control-header sensitivity middleware and `/control/identity` handler.
- `.github/workflows/ci.yml` — deterministic Linux/Windows CI gate.
- `docs/m0/neurovia-dogfooding.md` — post-remediation dogfooding procedure and residual-risk statement.

Files modified:

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`
- `crates/arbiter-core/src/config.rs`, `health.rs`, `lib.rs`
- `crates/arbiter-provider-codex/src/provider.rs`
- `crates/arbiter-adapter-codex/src/profile.rs`, `lib.rs`
- `crates/arbiter-cli/Cargo.toml`
- `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`, `start.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`
- `crates/arbiter-cli/tests/lifecycle.rs`, `contract.rs`
- `crates/arbiter-daemon/src/lib.rs`, `app.rs`, `state.rs`, `main.rs`
- `crates/arbiter-daemon/tests/proxy.rs`, `privacy.rs`, `shutdown.rs`
- `docs/m0/contract-gates.md`, `operations.md`, `performance.md`, `release-checklist.md`

Do not create a second persistence layer, second HTTP server, or inference-side static secret header.

---

### Task 1: Make the 32 MiB request contract executable

**Files:**
- Modify: `crates/arbiter-core/src/config.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/tests/proxy.rs`

**Interfaces:**
- Produces: `pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;`
- The router and tests import this single constant; no second request-size literal is authoritative.

- [ ] **Step 1: Add the exact-body helper and failing tests**

Add this helper to `crates/arbiter-daemon/tests/proxy.rs`:

```rust
fn exact_json_body(total_bytes: usize) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"input\":\"";
    const SUFFIX: &[u8] = b"\"}";
    assert!(total_bytes >= PREFIX.len() + SUFFIX.len());

    let fill = total_bytes - PREFIX.len() - SUFFIX.len();
    let mut body = Vec::with_capacity(total_bytes);
    body.extend_from_slice(PREFIX);
    body.extend(std::iter::repeat_n(b'x', fill));
    body.extend_from_slice(SUFFIX);
    assert_eq!(body.len(), total_bytes);
    body
}
```

For the fake upstream used by these tests, disable its own Axum body limit so it cannot mask Arbiter behavior:

```rust
let upstream = Router::new()
    .route("/responses", post(capture_request))
    .layer(axum::extract::DefaultBodyLimit::disable());
```

Add the following four tests. Use the existing loopback `CodexUpstreamProvider::new_for_loopback_test`, `AppState`, and `SqliteEventStore` setup from this test file.

For a body larger than Axum's default:

```rust
let body = serde_json::json!({"input": "x".repeat(3 * 1024 * 1024)}).to_string();
let request = Request::post("/v1/responses")
    .header(header::AUTHORIZATION, "Bearer large-body-test")
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(body))
    .unwrap();
let response = app.oneshot(request).await.unwrap();
assert_eq!(response.status(), StatusCode::OK);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 1);
```

For the exact boundary:

```rust
let body = exact_json_body(arbiter_core::config::M0_MAX_REQUEST_BYTES);
assert_eq!(body.len(), 33_554_432);
let request = Request::post("/v1/responses")
    .header(header::AUTHORIZATION, "Bearer exact-limit-test")
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(body))
    .unwrap();
let response = app.oneshot(request).await.unwrap();
assert_eq!(response.status(), StatusCode::OK);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 1);
```

For one byte over:

```rust
let body = exact_json_body(arbiter_core::config::M0_MAX_REQUEST_BYTES + 1);
let request = Request::post("/v1/responses")
    .header(header::AUTHORIZATION, "Bearer oversized-test")
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(body))
    .unwrap();
let response = app.oneshot(request).await.unwrap();
assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 0);
assert_eq!(store.recent_attempt_counts(0).await.unwrap().started, 0);
```

For malformed JSON:

```rust
let request = Request::post("/v1/responses")
    .header(header::AUTHORIZATION, "Bearer malformed-test")
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from("{\"input\":"))
    .unwrap();
let response = app.oneshot(request).await.unwrap();
assert_eq!(response.status(), StatusCode::BAD_REQUEST);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 0);
assert_eq!(store.recent_attempt_counts(0).await.unwrap().started, 0);
```

The accepted-body upstream capture must assert that the normalized body still contains the entire `input`, fixed Terra model, medium effort, and any synthetic non-governed field supplied by the test.

- [ ] **Step 2: Run the focused tests before implementation**

```bash
cargo test -p arbiter-daemon --test proxy accepts_json_larger_than_axum_default_and_preserves_payload -- --exact
cargo test -p arbiter-daemon --test proxy accepts_exactly_m0_max_request_bytes -- --exact
cargo test -p arbiter-daemon --test proxy rejects_one_byte_over_m0_limit_before_attempt_or_upstream -- --exact
cargo test -p arbiter-daemon --test proxy malformed_json_is_rejected_before_attempt_or_upstream -- --exact
```

Expected before implementation: the accepted large-body tests fail under Axum's implicit default.

- [ ] **Step 3: Define the single M0 request-size constant**

Add to `arbiter-core/src/config.rs`:

```rust
pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
```

and test:

```rust
#[test]
fn m0_request_limit_is_exactly_32_mib() {
    assert_eq!(super::M0_MAX_REQUEST_BYTES, 33_554_432);
}
```

- [ ] **Step 4: Apply the explicit limit only to Responses**

In `arbiter-daemon/src/app.rs`:

```rust
Router::new()
    .route(
        "/v1/responses",
        post(responses).layer(DefaultBodyLimit::max(M0_MAX_REQUEST_BYTES)),
    )
```

Do not disable the limit globally and do not move attempt creation ahead of JSON extraction.

- [ ] **Step 5: Run regressions**

```bash
cargo test -p arbiter-daemon --test proxy
cargo test -p arbiter-core config::tests
```

Expected: exact 32 MiB accepted, one byte over 413, malformed JSON 400, zero pre-attempt events/upstream calls.

- [ ] **Step 6: Commit**

```bash
git add crates/arbiter-core/src/config.rs crates/arbiter-daemon/src/app.rs crates/arbiter-daemon/tests/proxy.rs
git commit -m "fix: enforce explicit m0 request limit"
```

---

### Task 2: Separate public health from authenticated control identity

**Files:**
- Create: `crates/arbiter-core/src/control.rs`
- Modify: `crates/arbiter-core/src/lib.rs`
- Modify: `crates/arbiter-core/src/health.rs`
- Create: `crates/arbiter-daemon/src/control.rs`
- Modify: `crates/arbiter-daemon/src/lib.rs`
- Modify: `crates/arbiter-daemon/src/state.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/tests/proxy.rs`
- Modify: `crates/arbiter-provider-codex/src/provider.rs`

**Interfaces:**
- Produces: `CONTROL_HEADER_NAME`, `ControlToken`, `ControlTokenError`.
- Produces: `AppState::new(provider, store, control_token)` and `AppState::new_with_identity(provider, store, identity, control_token)`.
- Produces: `GET /control/identity` returning `DaemonIdentity` only for a valid token.
- Changes: `DaemonHealth` no longer contains `identity`.

- [ ] **Step 1: Write failing `ControlToken` tests**

Create `arbiter-core/src/control.rs` with these tests first:

```rust
#[test]
fn control_token_accepts_only_43_char_base64url_values() {
    let token = ControlToken::parse("A".repeat(43)).unwrap();
    assert_eq!(token.expose(), "A".repeat(43));
    assert!(ControlToken::parse("short".to_owned()).is_err());
    assert!(ControlToken::parse(format!("{}=", "A".repeat(42))).is_err());
    assert!(ControlToken::parse(format!("{}+", "A".repeat(42))).is_err());
}

#[test]
fn control_token_debug_is_redacted() {
    let token = ControlToken::parse("A".repeat(43)).unwrap();
    assert_eq!(format!("{token:?}"), "ControlToken([REDACTED])");
}
```

- [ ] **Step 2: Implement the complete redacted value object**

```rust
pub const CONTROL_HEADER_NAME: &str = "x-arbiter-control-token";

#[derive(Debug, thiserror::Error)]
pub enum ControlTokenError {
    #[error("Arbiter control token is not a 32-byte unpadded base64url value")]
    InvalidFormat,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ControlToken(String);

impl ControlToken {
    pub fn parse(value: String) -> Result<Self, ControlTokenError> {
        let valid = value.len() == 43
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if valid {
            Ok(Self(value))
        } else {
            Err(ControlTokenError::InvalidFormat)
        }
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ControlToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControlToken([REDACTED])")
    }
}
```

Export `pub mod control;` from `arbiter-core/src/lib.rs`.

- [ ] **Step 3: Add failing public-health/control-route tests**

Use this deterministic test helper:

```rust
fn test_control_token() -> arbiter_core::control::ControlToken {
    arbiter_core::control::ControlToken::parse("A".repeat(43)).unwrap()
}
```

Build `AppState::new_with_identity` with a known `DaemonIdentity` and add assertions:

```rust
let health = app
    .clone()
    .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
    .await
    .unwrap();
let health_bytes = axum::body::to_bytes(health.into_body(), 64 * 1024).await.unwrap();
let health_json: serde_json::Value = serde_json::from_slice(&health_bytes).unwrap();
assert!(health_json.get("identity").is_none());
assert!(health_json.get("pid").is_none());
assert!(health_json.get("port").is_none());
assert!(health_json.get("instance_id").is_none());
```

For control identity:

```rust
let missing = app
    .clone()
    .oneshot(Request::get("/control/identity").body(Body::empty()).unwrap())
    .await
    .unwrap();
assert_eq!(missing.status(), StatusCode::NOT_FOUND);

let wrong = app
    .clone()
    .oneshot(
        Request::get("/control/identity")
            .header(CONTROL_HEADER_NAME, "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(wrong.status(), StatusCode::NOT_FOUND);

let valid = app
    .oneshot(
        Request::get("/control/identity")
            .header(CONTROL_HEADER_NAME, test_control_token().expose())
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
assert_eq!(valid.status(), StatusCode::OK);
```

Deserialize the valid body to `DaemonIdentity` and assert exact equality with the state identity.

- [ ] **Step 4: Implement control middleware and handler**

Create `arbiter-daemon/src/control.rs`:

```rust
pub(crate) async fn mark_control_header_sensitive(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(value) = request.headers_mut().get_mut(CONTROL_HEADER_NAME) {
        value.set_sensitive(true);
    }
    next.run(request).await
}

pub(crate) async fn identity(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let accepted = headers
        .get(CONTROL_HEADER_NAME)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == state.control_token.expose());
    if !accepted {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(state.identity.clone()).into_response()
}
```

Do not log presented or expected tokens.

- [ ] **Step 5: Remove identity from public health and wire the control route**

Change `DaemonHealth` to:

```rust
pub struct DaemonHealth {
    pub status: HealthStatus,
    pub components: Vec<ComponentHealth>,
}
```

Store `ControlToken` in `AppState`, update both constructors to require it, add `/control/identity`, and layer `middleware::from_fn(control::mark_control_header_sensitive)` around the router before future HTTP instrumentation.

- [ ] **Step 6: Prove the control header cannot reach upstream**

Extend the provider allowlist test:

```rust
headers.insert(
    arbiter_core::control::CONTROL_HEADER_NAME,
    HeaderValue::from_static("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
);
```

After capture:

```rust
assert!(!captured.headers.contains_key(arbiter_core::control::CONTROL_HEADER_NAME));
```

- [ ] **Step 7: Run focused tests**

```bash
cargo test -p arbiter-core control
cargo test -p arbiter-daemon --test proxy public_health_does_not_expose_daemon_identity -- --exact
cargo test -p arbiter-daemon --test proxy control_identity_requires_exact_control_token -- --exact
cargo test -p arbiter-provider-codex forwards_only_allowlisted_headers_and_streams_terminal_metadata
```

- [ ] **Step 8: Commit**

```bash
git add crates/arbiter-core crates/arbiter-daemon crates/arbiter-provider-codex/src/provider.rs
git commit -m "security: separate arbiter control identity"
```

---

### Task 3: Move installation state to schema v2 and generate the owner-only control credential

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Modify: `crates/arbiter-cli/Cargo.toml`
- Create: `crates/arbiter-cli/src/commands/control.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`

**Interfaces:**
- Produces: `LocalConfig` schema v2 without `port` or `instance_id`.
- Produces: `Paths.control_token` at `$ARBITER_HOME/control-token`.
- Produces: `control::generate_and_store(paths) -> anyhow::Result<ControlToken>` and `control::read(paths) -> anyhow::Result<ControlToken>`.
- Installation provider endpoint is exactly `http://127.0.0.1:0/v1`.

- [ ] **Step 1: Write failing schema-v2 lifecycle test**

Use the existing `arbiter(...)` test helper:

```rust
#[test]
fn init_creates_schema_v2_without_runtime_identity() {
    let temporary = tempdir().unwrap();
    let arbiter_home = temporary.path().join(".arbiter");
    let codex_home = temporary.path().join(".codex");
    std::fs::create_dir_all(&codex_home).unwrap();

    let init = arbiter(&arbiter_home, &codex_home, &["init", "codex", "--yes"]);
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));

    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(arbiter_home.join("config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config["schema_version"], 2);
    assert!(config.get("port").is_none());
    assert!(config.get("instance_id").is_none());

    let installed = std::fs::read_to_string(codex_home.join("config.toml")).unwrap();
    assert!(installed.contains("base_url = \"http://127.0.0.1:0/v1\""));

    let token = std::fs::read_to_string(arbiter_home.join("control-token")).unwrap();
    assert_eq!(token.len(), 43);
    assert!(token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'));
}
```

Also add:

```rust
#[test]
fn init_no_longer_accepts_a_persisted_port() {
    let output = arbiter(&arbiter_home, &codex_home, &["init", "codex", "--port", "43123"]);
    assert!(!output.status.success());
}
```

Create complete temp paths in that test exactly as in the previous lifecycle test.

- [ ] **Step 2: Add only required token dependencies**

Workspace:

```toml
base64 = "0.22"
getrandom = "0.3"
```

Add both as workspace dependencies in `arbiter-cli/Cargo.toml`; do not add a second random/secret abstraction.

- [ ] **Step 3: Implement exact token generation/loading**

In `commands/control.rs`:

```rust
use base64::Engine as _;

pub(crate) fn generate_and_store(paths: &Paths) -> anyhow::Result<ControlToken> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).context("generate Arbiter control credential")?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let token = ControlToken::parse(encoded)?;
    write_new_private_synced(&paths.control_token, token.expose().as_bytes())?;
    Ok(token)
}

pub(crate) fn read(paths: &Paths) -> anyhow::Result<ControlToken> {
    let bytes = std::fs::read(&paths.control_token)
        .context("read Arbiter control credential")?;
    let value = std::str::from_utf8(&bytes)
        .context("Arbiter control credential is not UTF-8")?
        .to_owned();
    ControlToken::parse(value).map_err(Into::into)
}
```

Do not trim the file; unexpected bytes must invalidate it.

- [ ] **Step 4: Convert `LocalConfig` and CLI surface to schema v2**

```rust
pub(crate) struct LocalConfig {
    pub schema_version: u16,
    pub mode: String,
    pub baseline: BaselineTarget,
    pub codex_config: PathBuf,
    pub bind_address: String,
    pub remote_export: bool,
}
```

`default_config(paths)` sets schema 2 and existing invariants. `read_config` rejects anything else. Remove `port` from `Command::Init` and the `init::run` signature.

Add:

```rust
control_token: arbiter_home.join("control-token"),
```

to `Paths`.

- [ ] **Step 5: Make init install inactive state and clean token on failure**

Call `install_profile(..., 0, timestamp)`. Generate the control credential using `generate_and_store` before the final receipt write. If profile installation or receipt persistence fails after token creation, remove `control-token` before returning; do not report initialization success.

- [ ] **Step 6: Prove port zero is inert and token is private**

Add:

```rust
assert!(std::net::TcpStream::connect(("127.0.0.1", 0)).is_err());
```

On Unix:

```rust
assert_eq!(
    std::fs::metadata(arbiter_home.join("control-token"))
        .unwrap()
        .permissions()
        .mode()
        & 0o177,
    0,
);
```

On Windows, extend the existing private-file ACL test path rather than introducing another PowerShell/ACL implementation.

- [ ] **Step 7: Run tests**

```bash
cargo test -p arbiter-cli --test lifecycle
cargo test -p arbiter-cli m0_cli_exposes_only_codex_as_an_init_target
```

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/arbiter-cli
git commit -m "feat: initialize m0 runtime identity separately"
```

---

### Task 4: Make Codex endpoint publication and receipt updates compensating

**Files:**
- Modify: `crates/arbiter-adapter-codex/src/profile.rs`, `lib.rs`
- Create: `crates/arbiter-cli/src/commands/endpoint.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`

**Interfaces:**
- Produces: `AppliedEndpointUpdate` with `updated_receipt()` and `rollback()`.
- Produces: `update_managed_endpoint(config_path, receipt, port) -> Result<AppliedEndpointUpdate, CodexProfileError>`.
- Produces: `endpoint::publish(paths, port) -> anyhow::Result<InstallReceipt>` as the only CLI path that mutates runtime `base_url` and current installed receipt hash.

- [ ] **Step 1: Write failing adapter tests**

Exercise:

```rust
let update = update_managed_endpoint(&config, &receipt, 45_678).unwrap();
assert!(std::fs::read_to_string(&config).unwrap().contains("http://127.0.0.1:45678/v1"));
assert_ne!(update.updated_receipt().config.installed_sha256, receipt.config.installed_sha256);
assert_eq!(update.updated_receipt().config.original_sha256, receipt.config.original_sha256);
assert_eq!(update.updated_receipt().profile, receipt.profile);
```

Capture the original file string before update and assert unrelated user keys plus all provider fields other than `base_url` are identical after parsing both documents.

Add a rollback test: apply update, call `rollback()`, then assert exact previous bytes and permissions.

- [ ] **Step 2: Implement `AppliedEndpointUpdate`**

```rust
pub struct AppliedEndpointUpdate {
    updated_receipt: InstallReceipt,
    previous_config: Vec<u8>,
    previous_permissions: OriginalPermissions,
    config_path: PathBuf,
}

impl AppliedEndpointUpdate {
    pub fn updated_receipt(&self) -> &InstallReceipt {
        &self.updated_receipt
    }

    pub fn rollback(self) -> Result<(), CodexProfileError> {
        atomic_replace_with_permissions(
            &self.config_path,
            &self.previous_config,
            &self.previous_permissions,
        )?;
        Ok(())
    }
}
```

Implement a custom redacted `Debug` or no `Debug`; never expose previous config bytes in diagnostics.

- [ ] **Step 3: Implement `update_managed_endpoint` conflict checks**

Before mutation:

1. current config hash must equal `receipt.config.installed_sha256`;
2. current profile hash must equal `receipt.profile.installed_sha256`;
3. parse current config;
4. verify fixed M0 name, `wire_api`, auth and retry fields and a loopback `base_url`;
5. change only `model_providers.arbiter.base_url` to `http://127.0.0.1:{port}/v1`;
6. atomically replace config with current private permissions;
7. clone receipt and update only `config.installed_sha256`.

Do not modify immutable original hashes/backups/permissions or profile receipt.

- [ ] **Step 4: Add the CLI coordinator with injectable receipt writer**

```rust
fn publish_with_writer<F>(
    paths: &Paths,
    port: u16,
    write_receipt: F,
) -> anyhow::Result<InstallReceipt>
where
    F: FnOnce(&Path, &InstallReceipt) -> anyhow::Result<()>,
```

Production `publish` passes `write_json_atomic`. On receipt-write failure:

```rust
let receipt_error = write_receipt(&paths.receipt, update.updated_receipt()).unwrap_err();
match update.rollback() {
    Ok(()) => Err(receipt_error).context("persist endpoint receipt"),
    Err(rollback_error) => anyhow::bail!(
        "endpoint receipt persistence failed and Codex config rollback failed: {receipt_error}; rollback: {rollback_error}"
    ),
}
```

The unit test injects `Err(std::io::Error::other("injected receipt failure").into())`, then asserts Codex config bytes and on-disk receipt are unchanged.

- [ ] **Step 5: Prove latest receipt still restores the immutable original**

Test sequence:

```text
original config/profile
→ install port 0
→ update endpoint 45678
→ persist/use updated receipt
→ update endpoint 0
→ persist/use updated receipt
→ uninstall with latest receipt
→ exact original bytes + existence + permissions
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p arbiter-adapter-codex profile
cargo test -p arbiter-cli endpoint
```

- [ ] **Step 7: Commit**

```bash
git add crates/arbiter-adapter-codex crates/arbiter-cli/src/commands/endpoint.rs crates/arbiter-cli/src/commands/mod.rs
git commit -m "fix: make runtime endpoint publication atomic"
```

---

### Task 5: Bind first, publish second, and depublish after admission closes

**Files:**
- Modify: `crates/arbiter-daemon/src/app.rs`, `lib.rs`, `main.rs`
- Modify: `crates/arbiter-daemon/tests/shutdown.rs`
- Modify: `crates/arbiter-cli/src/commands/start.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`

**Interfaces:**
- Produces: `serve_listener_with_shutdown_hook(state, listener, shutdown, grace, on_admission_closed)`.
- Keeps: `serve_listener_with_shutdown(...)` as a no-op-hook wrapper.
- `start --foreground` derives `metadata.port` from `listener.local_addr()` and generates fresh `instance_id` per start.

- [ ] **Step 1: Add failing listener-derived lifecycle tests**

Change lifecycle helpers to read `server.json` after start. For two separate starts around a clean stop, assert:

```rust
assert!(first.port > 0);
assert!(second.port > 0);
assert_ne!(first.instance_id, second.instance_id);
```

Do not require port inequality. Assert schema-v2 config contains neither port nor instance ID. Add a failure test using an injectable endpoint writer in `start` so publication failure after listener bind leaves no READY `server.json` and leaves/restores managed endpoint port zero.

- [ ] **Step 2: Add the bounded shutdown hook**

```rust
pub async fn serve_listener_with_shutdown_hook<F, H>(
    state: AppState,
    listener: tokio::net::TcpListener,
    shutdown: F,
    grace: Duration,
    on_admission_closed: H,
) -> std::io::Result<()>
where
    F: Future<Output = ()> + Send + 'static,
    H: Future<Output = ()> + Send + 'static,
```

On shutdown: compute the one existing deadline, call `state.begin_shutdown()`, await the hook within that same deadline, then signal Axum graceful shutdown and preserve current drain/force-cancel/persistence/SQLite-close behavior. On hook timeout log only `shutdown_phase = "endpoint_depublication"`.

Existing `serve_listener_with_shutdown` delegates with `std::future::ready(())`.

- [ ] **Step 3: Reorder foreground startup exactly**

```text
TcpListener::bind(local_bind_address(0))
→ listener.local_addr().port()
→ DaemonLease::acquire(database)
→ read/validate control token
→ open SQLite
→ reconcile incomplete attempts
→ generate fresh instance_id
→ endpoint::publish(paths, bound_port)
→ write server.json
→ construct AppState(identity + control token)
→ serve
```

No profile mutation before listener + DB ownership.

- [ ] **Step 4: Compensate startup failure after publication**

If `server.json` persistence or later server setup fails after active publication, call `endpoint::publish(paths, 0)` before returning. If compensation fails, preserve both error causes and never emit READY/success.

- [ ] **Step 5: Depublish on controlled shutdown**

Pass:

```rust
let on_admission_closed = async move {
    if endpoint::publish(&shutdown_paths, 0).is_err() {
        tracing::error!(
            event_type = "endpoint_depublication_failed",
            "Arbiter failed to depublish its managed endpoint during shutdown"
        );
    }
};
```

Do not log error text that can contain local paths. Remove `server.json` after serve returns only when it still equals the identity owned by this process.

- [ ] **Step 6: Update standalone daemon**

`arbiter-daemon` defaults `--port` to `0`, derives real identity port from the bound listener, adds required `--control-token-file`, reads/validates it, and passes the token to `AppState`. It does not publish Codex config.

- [ ] **Step 7: Run regressions**

```bash
cargo test -p arbiter-daemon --test shutdown
cargo test -p arbiter-cli --test lifecycle
```

- [ ] **Step 8: Commit**

```bash
git add crates/arbiter-daemon crates/arbiter-cli/src/commands/start.rs crates/arbiter-cli/tests/lifecycle.rs
git commit -m "fix: publish only owned daemon endpoints"
```

---

### Task 6: Move status, doctor, and uninstall ownership checks to the control channel

**Files:**
- Modify: `crates/arbiter-cli/src/commands/control.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`
- Modify: `crates/arbiter-daemon/tests/privacy.rs`

**Interfaces:**
- Produces: `control::identity_at(port, token) -> Option<DaemonIdentity>`.
- Produces: `control::current_identity(paths) -> anyhow::Result<Option<DaemonIdentity>>` requiring `server.json` plus exact authenticated response.
- Public `/healthz` is never used to establish ownership.

- [ ] **Step 1: Extend fake-server tests for control semantics**

Teach the lifecycle fake server to parse the request line and `X-Arbiter-Control-Token`. Add four cases:

1. generic `200 /healthz` cannot satisfy start/status/doctor/uninstall ownership;
2. `/control/identity` without expected token is rejected;
3. valid token with mismatched identity is rejected;
4. valid token with exact identity is accepted.

Retain refusal when stale metadata points to another responding process.

- [ ] **Step 2: Implement authenticated identity lookup**

`identity_at` builds a short-timeout Reqwest client, constructs a `HeaderValue` from `token.expose()`, calls `set_sensitive(true)`, and sends:

```text
GET http://127.0.0.1:{port}/control/identity
X-Arbiter-Control-Token: <token>
```

Return `None` on timeout, non-2xx, invalid JSON, or decode failure; no token appears in an error string.

`current_identity(paths)`:

```text
server.json absent → Ok(None)
server.json present → read metadata → read control token → query metadata.port
authenticated response == metadata → Ok(Some(metadata))
anything else → Ok(None)
```

- [ ] **Step 3: Rewrite `status` around runtime metadata**

Report healthy only for exact authenticated identity. Report stopped only when no runtime metadata exists and managed provider validates at port zero. If stale metadata port responds without valid control identity, return an unrecognized-process error. Validate managed profile against authenticated runtime port when healthy and port zero when stopped.

- [ ] **Step 4: Rewrite `doctor` with the same contract**

Validate schema-v2 config, private control token, SQLite integrity, profile active-port-or-zero state, loopback bind, ChatGPT authentication, and remote export disabled. Do not print token or full private identity.

- [ ] **Step 5: Rewrite uninstall stop logic**

If `server.json` is absent, restore from receipt without probing a historical config port. If present: exact control identity → stop owned daemon; failed identity + responding metadata port → refuse; failed identity + nonresponding port → remove stale metadata and continue exact restoration. On successful uninstall remove control token/config/receipt/stop/server files while preserving `arbiter.db` and backups.

- [ ] **Step 6: Add control-secret privacy regression**

Read the generated token, perform init/start/status/doctor/synthetic proxy/uninstall, and assert exact token bytes are absent from Codex config/profile, SQLite/WAL/SHM, captured daemon logs, config/receipt/server metadata. The token is allowed only in `$ARBITER_HOME/control-token` while installed and that file must be gone after uninstall.

- [ ] **Step 7: Run deterministic workspace checks**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

- [ ] **Step 8: Commit**

```bash
git add crates/arbiter-cli crates/arbiter-daemon/tests/privacy.rs
git commit -m "security: authenticate arbiter lifecycle control"
```

---

### Task 7: Add reproducible Linux/Windows CI and pin release tooling

**Files:**
- Modify: `rust-toolchain.toml`
- Create: `.github/workflows/ci.yml`
- Modify only if syntax compatibility requires it: `deny.toml`

**Interfaces:**
- Produces required CI jobs `linux` and `windows`.
- CI uses only `contents: read`; no secrets.

- [ ] **Step 1: Pin repository execution toolchain**

```toml
[toolchain]
channel = "1.97.1"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

Keep workspace `rust-version = "1.85"` unchanged as package MSRV intent.

- [ ] **Step 2: Create `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  pull_request:
    branches: [main]
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install pinned Rust toolchain
        run: |
          rustup toolchain install 1.97.1 --profile minimal --component rustfmt --component clippy
          rustup default 1.97.1
      - name: Install cargo-deny
        run: cargo install cargo-deny --version 0.20.2 --locked
      - name: Format
        run: cargo fmt --all -- --check
      - name: Check
        run: cargo check --workspace --all-targets --all-features
      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features -- -D warnings
      - name: Test
        run: cargo test --workspace --all-targets --all-features
      - name: Dependency policy
        run: cargo deny check

  windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install pinned Rust toolchain
        shell: pwsh
        run: |
          rustup toolchain install 1.97.1 --profile minimal
          rustup default 1.97.1
      - name: Check
        run: cargo check --workspace --all-targets --all-features
      - name: Test
        run: cargo test --workspace --all-targets --all-features
```

Do not add caching, artifacts, OpenAI/ChatGPT secrets, or write permissions.

- [ ] **Step 3: Validate cargo-deny 0.20.2 locally**

```bash
cargo install cargo-deny --version 0.20.2 --locked
cargo deny check
```

If `deny.toml` syntax needs adjustment for 0.20.2, preserve the existing advisory/ban/license/source policy semantics; never weaken policy to get green output.

- [ ] **Step 4: Inspect workflow security**

```bash
git diff --check
rg -n "OPENAI_API_KEY|secrets\.|contents: write|pull-requests: write|actions/upload-artifact" .github/workflows/ci.yml
```

Expected: no matches for secret/write/artifact configuration.

- [ ] **Step 5: Require fresh GitHub evidence**

Push the implementation branch and require both `linux` and `windows` jobs green. Local tests do not substitute for this gate.

- [ ] **Step 6: Commit**

```bash
git add rust-toolchain.toml .github/workflows/ci.yml
git add deny.toml 2>/dev/null || true
git commit -m "ci: verify arbiter m0 on linux and windows"
```

Only stage `deny.toml` if it actually changed.

---

### Task 8: Re-run live, privacy, performance, and independent review gates

**Files:**
- Modify: `docs/m0/contract-gates.md`, `operations.md`, `performance.md`, `release-checklist.md`
- Create: `docs/m0/neurovia-dogfooding.md`

**Interfaces:**
- Produces `GO FOR DAILY NEUROVIA DOGFOODING` only if every gate has fresh evidence.
- Does not change runtime behavior.

- [ ] **Step 1: Run complete deterministic release suite**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
cargo deny check
git diff --check
```

Record exact implementation commit SHA and actual debug/release test counts in `release-checklist.md`; do not reuse prior `dfaa349...` evidence.

- [ ] **Step 2: Run ignored live Codex contract suite**

```bash
codex login status
cargo test -p arbiter-cli --test contract -- --ignored --test-threads=1
```

ChatGPT-managed authentication is required and `OPENAI_API_KEY` must be removed from the child environment. The suite must prove direct and through-Arbiter streaming, usage extraction, cancellation, metadata-only persistence, and exact restoration with the new bind-first lifecycle.

- [ ] **Step 3: Run a NeuroVia-shaped manual smoke without changing default Codex profile**

From the NeuroVia checkout:

```bash
arbiter init codex --yes
arbiter doctor
arbiter start
codex exec --profile arbiter "Reply with exactly: arbiter-dogfood-ok"
arbiter status
arbiter uninstall --yes
```

Expected response: exactly `arbiter-dogfood-ok`; running status healthy; uninstall restores pre-run Codex files exactly.

- [ ] **Step 4: Re-run privacy sentinels**

Use synthetic prompt/source/auth/control sentinels and verify none appears in `arbiter.db`, WAL, SHM, config, receipt, server metadata, Codex config/profile, or captured structured logs. The control token is allowed only in `control-token` while installed and is removed by uninstall.

- [ ] **Step 5: Re-run the 1,000-pair benchmark**

Use the existing release benchmark with 50 warmups and 1,000 paired direct/proxy samples. Record first-byte and total added latency p50/p95/p99 in `performance.md`. Pass only if:

```text
p50 < 10 ms
p95 < 25 ms
p99 < 50 ms
```

- [ ] **Step 6: Update operations documentation**

Document exactly:

```text
init → inactive port 0
start → bind ephemeral port → publish owned endpoint
/control/identity → CLI-only authenticated management identity
healthz → public health without private identity
shutdown → close admission → depublish to port 0 → bounded cleanup
uninstall → exact original restoration + preserve arbiter.db
```

State explicitly that localhost inference is not cryptographically server-pinned and hostile multi-user enterprise hardening is deferred.

- [ ] **Step 7: Add `docs/m0/neurovia-dogfooding.md`**

```markdown
# NeuroVia Dogfooding — Arbiter M0

- Use only on a personal developer workstation under the approved M0 threat model.
- Invoke Codex explicitly with `--profile arbiter`; do not replace the default Codex profile.
- M0 always uses `gpt-5.6-terra` / medium; it does not yet optimize model choice.
- Run `arbiter doctor` before the first work session after an Arbiter upgrade.
- If status reports an unrecognized process or invalid profile, stop dogfooding and diagnose before sending repository context.
- Use `arbiter uninstall --yes` as the rollback path; event history remains local.
```

Include exact init/start/status/uninstall commands and accepted localhost residual risk.

- [ ] **Step 8: Run independent Engineering Guardrails and Security reviews**

Run a fresh Codex Engineering Guardrails verification against the final implementation commit and a fresh Codex Security standard repository scan. Required outcome:

```text
no blocking correctness finding
no blocking security finding within the approved M0 threat model
```

Any validated blocker reopens remediation.

- [ ] **Step 9: Verify CI on the exact final SHA**

Both GitHub Actions jobs must be green on the final commit after all code/documentation changes.

- [ ] **Step 10: Close the release checklist and commit evidence**

Set decision text to:

```text
PASS — M0 dogfooding-readiness remediation gate is closed.
GO FOR DAILY NEUROVIA DOGFOODING under the documented personal-workstation threat model.
```

Keep explicit text that this is neither M1 nor enterprise hostile-host hardening.

```bash
git add docs/m0
git commit -m "docs: close m0 dogfooding readiness gate"
```

---

## Final Verification Matrix

| Acceptance criterion | Primary evidence |
| --- | --- |
| >2 MiB through 32 MiB accepted | `arbiter-daemon/tests/proxy.rs` |
| >32 MiB is 413 with no attempt/upstream | `arbiter-daemon/tests/proxy.rs` |
| Named 32 MiB contract | `M0_MAX_REQUEST_BYTES` + operations docs |
| Bind-before-publish | lifecycle integration test + `start.rs` review |
| No stale port authority in config | schema-v2 lifecycle test |
| Public health hides reusable identity | daemon control/health tests |
| CLI ownership uses control credential | lifecycle fake-server tests |
| Control secret owner-only/private | lifecycle + platform permission tests |
| Control secret never forwarded/persisted/logged | provider + privacy tests |
| Recovery after listener + DB ownership | existing + updated recovery tests |
| Exact uninstall after changing runtime endpoint | adapter transition + lifecycle tests |
| Linux CI gate | GitHub Actions `linux` job |
| Windows ACL CI gate | GitHub Actions `windows` job |
| No CI credentials/write permissions | workflow review |
| Live direct/through-Arbiter contract | ignored live contract suite |
| Performance SLO | 1,000-pair benchmark evidence |
| Independent engineering review | final Guardrails report |
| Independent security review | final Security report |
| Residual localhost risk explicit | spec + operations + dogfooding runbook |
| Daily NeuroVia use decision | final release checklist |

## Execution Notes

- Execute in an isolated worktree created at implementation time with `superpowers:using-git-worktrees`.
- Prefer `superpowers:subagent-driven-development`: one fresh implementer/reviewer cycle per task.
- Use TDD in the written order; do not batch all tests at the end.
- Do not start M1 while this plan is open.
- If Codex changes its custom-provider contract during execution, stop that task and re-run live contract/Context7 verification before altering the approved architecture.
- If a task requires weakening an existing safety/privacy assertion to pass, treat that as a finding rather than changing the assertion.
