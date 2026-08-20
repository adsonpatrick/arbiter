# Arbiter M0 Dogfooding Readiness Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the post-M0 request-contract, localhost-control, and release-evidence findings so Arbiter can be used daily on NeuroVia under the approved personal-workstation threat model.

**Architecture:** Preserve the existing Rust M0 proxy and fixed Terra/Medium path. First make the 32 MiB request contract explicit. Then introduce an authenticated CLI↔daemon control plane while the current lifecycle still works, add a tested compensating primitive for changing the managed Codex endpoint, and only then migrate installation state to schema v2 with bind-first ephemeral endpoint publication/depublication. Finish with deterministic Linux/Windows CI and fresh live/performance/security release evidence.

**Tech Stack:** Rust 1.97.1; Tokio 1.x; Axum 0.8.x; Reqwest 0.12.x; SQLx 0.8.x + SQLite; Clap 4.x; Serde; `getrandom` 0.3; `base64` 0.22; GitHub Actions; cargo-deny 0.20.2.

**Spec:** `docs/superpowers/specs/2026-08-20-m0-dogfooding-readiness-remediation-design.md`

## Global Constraints

- Remain inside M0. Do not add routing, phase/risk classification, budgets, evals, promotion, Supabase, LiteLLM, dashboards, or another harness/provider.
- Production inference remains exactly `gpt-5.6-terra` with `medium` reasoning.
- Codex owns ChatGPT authentication; Arbiter does not read credential storage or own an OpenAI API key.
- Inference binds only to IPv4 loopback. Upstream stays pinned to the first-party Codex Responses endpoint with redirects and semantic retries disabled.
- Prompts, source code, request/response bodies, tool content, credentials, cookies, repository paths, and control credentials never enter logs, events, SQLite/WAL/SHM, or remote export.
- `AttemptStarted` remains durable before an upstream call. Requests rejected before the handler create no attempt and make no upstream call.
- At most one terminal event is durable per attempt; response streaming/cancellation semantics stay unchanged.
- Request admission is exactly 33,554,432 bytes. Exact-boundary valid JSON is accepted; one byte over is HTTP 413.
- The control credential is exactly 32 OS-random bytes encoded unpadded base64url and stored owner-only at `$ARBITER_HOME/control-token`.
- `GET /healthz` is public health only. `GET /control/identity` requires `X-Arbiter-Control-Token`; missing/wrong token returns 404 without identity.
- The control token is never forwarded to Codex/ChatGPT and is not server authentication for inference traffic.
- Final schema-v2 `config.json` contains installation invariants only; active port and `instance_id` exist only in `server.json` and authenticated control identity.
- Final init installs `http://127.0.0.1:0/v1`. Every daemon start binds `127.0.0.1:0` before publishing the listener-assigned port.
- Endpoint publication/depublication changes only Arbiter `base_url` plus the current installed receipt hash. Immutable original backups/hashes never change.
- Uninstall preserves exact restoration/conflict behavior and keeps `arbiter.db`.
- Public CI receives no OpenAI/ChatGPT credential. Live contract tests and the 1,000-pair performance benchmark remain manual release gates.

## File Structure Map

Create:

- `crates/arbiter-core/src/control.rs` — redacted `ControlToken` and header constant.
- `crates/arbiter-cli/src/commands/control.rs` — token generation/loading and authenticated identity client.
- `crates/arbiter-cli/src/commands/endpoint.rs` — compensating endpoint+receipt transition coordinator.
- `crates/arbiter-daemon/src/control.rs` — control-header sensitivity middleware and identity handler.
- `.github/workflows/ci.yml` — Linux/Windows deterministic CI.
- `docs/m0/neurovia-dogfooding.md` — daily-use runbook and residual risk.

Modify:

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`
- `bench/proxy_overhead.rs`
- `crates/arbiter-core/src/config.rs`, `health.rs`, `lib.rs`
- `crates/arbiter-provider-codex/src/provider.rs`
- `crates/arbiter-adapter-codex/src/profile.rs`, `lib.rs`
- `crates/arbiter-cli/Cargo.toml`
- `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`, `start.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`
- `crates/arbiter-cli/tests/lifecycle.rs`, `contract.rs`
- `crates/arbiter-daemon/src/lib.rs`, `app.rs`, `state.rs`, `main.rs`
- `crates/arbiter-daemon/tests/proxy.rs`, `privacy.rs`, `shutdown.rs`
- `docs/m0/contract-gates.md`, `operations.md`, `performance.md`, `release-checklist.md`

Each task below must leave the workspace buildable and testable before the next task begins.

---

### Task 1: Enforce the explicit 32 MiB request contract

**Files:**
- Modify: `crates/arbiter-core/src/config.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/tests/proxy.rs`

**Interfaces:**
- Produces: `pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;`

- [ ] **Step 1: Add exact-size request helpers and failing tests**

Add:

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

The fake upstream used for these tests must disable its own Axum limit:

```rust
let upstream = Router::new()
    .route("/responses", post(capture_request))
    .layer(axum::extract::DefaultBodyLimit::disable());
```

Add four tests named exactly:

```text
accepts_json_larger_than_axum_default_and_preserves_payload
accepts_exactly_m0_max_request_bytes
rejects_one_byte_over_m0_limit_before_attempt_or_upstream
malformed_json_is_rejected_before_attempt_or_upstream
```

For the >2 MiB accepted case send:

```rust
let body = serde_json::json!({
    "input": "x".repeat(3 * 1024 * 1024),
    "future_field": {"preserve": true}
})
.to_string();
```

Assert response 200, exactly one upstream call, complete input length preserved, `future_field.preserve == true`, model `gpt-5.6-terra`, and reasoning effort `medium` at the fake upstream.

For exact boundary:

```rust
let body = exact_json_body(arbiter_core::config::M0_MAX_REQUEST_BYTES);
assert_eq!(body.len(), 33_554_432);
```

Send it as authenticated JSON and assert response 200 and one upstream call.

For one byte over:

```rust
let body = exact_json_body(arbiter_core::config::M0_MAX_REQUEST_BYTES + 1);
```

Assert:

```rust
assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 0);
assert_eq!(store.recent_attempt_counts(0).await.unwrap().started, 0);
```

For malformed JSON send `{"input":` and assert HTTP 400 plus the same zero-upstream/zero-attempt conditions.

- [ ] **Step 2: Run focused tests before implementation**

```bash
cargo test -p arbiter-daemon --test proxy accepts_json_larger_than_axum_default_and_preserves_payload -- --exact
cargo test -p arbiter-daemon --test proxy accepts_exactly_m0_max_request_bytes -- --exact
cargo test -p arbiter-daemon --test proxy rejects_one_byte_over_m0_limit_before_attempt_or_upstream -- --exact
cargo test -p arbiter-daemon --test proxy malformed_json_is_rejected_before_attempt_or_upstream -- --exact
```

Expected before the fix: the accepted large-body cases fail under Axum's implicit default limit.

- [ ] **Step 3: Add the single domain constant**

```rust
pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
```

Add:

```rust
#[test]
fn m0_request_limit_is_exactly_32_mib() {
    assert_eq!(super::M0_MAX_REQUEST_BYTES, 33_554_432);
}
```

- [ ] **Step 4: Apply the limit only to `/v1/responses`**

```rust
.route(
    "/v1/responses",
    post(responses).layer(DefaultBodyLimit::max(M0_MAX_REQUEST_BYTES)),
)
```

Do not disable the limit globally and do not move attempt creation ahead of JSON extraction.

- [ ] **Step 5: Verify Task 1**

```bash
cargo test -p arbiter-daemon --test proxy
cargo test -p arbiter-core config::tests
cargo check --workspace --all-targets --all-features
```

- [ ] **Step 6: Commit**

```bash
git add crates/arbiter-core/src/config.rs crates/arbiter-daemon/src/app.rs crates/arbiter-daemon/tests/proxy.rs
git commit -m "fix: enforce explicit m0 request limit"
```

---

### Task 2: Add the authenticated control plane while preserving the current lifecycle

**Files:**
- Modify: `Cargo.toml`, `Cargo.lock`
- Modify: `crates/arbiter-cli/Cargo.toml`
- Create: `crates/arbiter-core/src/control.rs`
- Modify: `crates/arbiter-core/src/lib.rs`, `health.rs`
- Create: `crates/arbiter-daemon/src/control.rs`
- Modify: `crates/arbiter-daemon/src/lib.rs`, `state.rs`, `app.rs`, `main.rs`
- Modify: `crates/arbiter-provider-codex/src/provider.rs`
- Create: `crates/arbiter-cli/src/commands/control.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`
- Modify: `crates/arbiter-daemon/tests/proxy.rs`, `privacy.rs`, `shutdown.rs`
- Modify: `bench/proxy_overhead.rs`

**Interfaces:**
- Produces `CONTROL_HEADER_NAME`, `ControlToken`, `ControlTokenError`.
- Produces `$ARBITER_HOME/control-token` without yet changing schema-v1 port/instance lifecycle.
- Produces `control::identity_at(port, token)` and `control::current_identity(paths, config)`.
- Public health loses private identity; all existing CLI ownership checks switch to authenticated control before Task 2 ends.

- [ ] **Step 1: Add token dependencies and failing domain tests**

Workspace dependencies:

```toml
base64 = "0.22"
getrandom = "0.3"
```

Add both to `arbiter-cli/Cargo.toml`.

Create `arbiter-core/src/control.rs` tests:

```rust
#[test]
fn control_token_requires_unpadded_base64url_shape() {
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

- [ ] **Step 2: Implement the redacted token type**

```rust
pub const CONTROL_HEADER_NAME: &str = "x-arbiter-control-token";

#[derive(Debug, thiserror::Error)]
pub enum ControlTokenError {
    #[error("Arbiter control token is not an unpadded 32-byte base64url credential")]
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
        valid.then_some(Self(value)).ok_or(ControlTokenError::InvalidFormat)
    }

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

Export `pub mod control;`.

- [ ] **Step 3: Generate/store the token during current M0 init**

Add `Paths.control_token = arbiter_home.join("control-token")`.

Create `commands/control.rs`:

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

Keep current schema-v1 `port`/`instance_id` for this task. Generate the token during `init --yes`; on later init failure remove the newly created token before returning.

Add lifecycle assertions: file length 43; base64url alphabet; Unix mode has no group/other bits. Windows reuses existing private-file ACL primitives.

- [ ] **Step 4: Add control identity route and remove identity from public health**

Create daemon middleware:

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
```

Control handler:

```rust
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

Change `DaemonHealth` to only `status` + `components`; add `/control/identity`; apply sensitivity middleware. Store `ControlToken` in `AppState` and require it in constructors.

Update every constructor call in:

```text
arbiter-daemon/src/main.rs
arbiter-daemon/src/app.rs tests
arbiter-daemon/tests/proxy.rs
arbiter-daemon/tests/privacy.rs
arbiter-daemon/tests/shutdown.rs
arbiter-cli/src/commands/start.rs
bench/proxy_overhead.rs
```

Tests use deterministic `ControlToken::parse("A".repeat(43))`.

- [ ] **Step 5: Add public/control route tests**

Public health assertions:

```rust
assert!(health_json.get("identity").is_none());
assert!(health_json.get("pid").is_none());
assert!(health_json.get("port").is_none());
assert!(health_json.get("instance_id").is_none());
```

Control route assertions:

```text
missing header → 404
wrong 43-character token → 404
correct token → 200 + exact DaemonIdentity
```

- [ ] **Step 6: Move CLI ownership checks to authenticated control immediately**

Implement:

```rust
pub(crate) async fn identity_at(port: u16, token: &ControlToken) -> Option<DaemonIdentity>
```

Build a 300 ms Reqwest client. Construct the header value from `token.expose()`, mark it sensitive, and GET `/control/identity`. Return `None` on timeout, non-success, invalid JSON, or decode failure.

Implement current-lifecycle helper:

```rust
pub(crate) async fn current_identity(
    paths: &Paths,
    config: &LocalConfig,
) -> anyhow::Result<Option<DaemonIdentity>>
```

For Task 2, it reads `server.json`, verifies metadata `port == config.port` and `instance_id == config.instance_id`, reads control token, calls `identity_at`, and returns identity only for exact equality with metadata.

Rewrite `status`, `doctor`, `start` “already running” detection, and `uninstall` stop ownership to use this helper instead of `/healthz` identity. `/healthz` remains only liveness/storage health.

- [ ] **Step 7: Prove control token is not forwarded or persisted**

Provider allowlist test inserts `CONTROL_HEADER_NAME` and asserts fake upstream does not receive it.

Privacy/lifecycle test reads the real generated token, starts Arbiter, exercises status/doctor, then asserts its bytes are absent from Codex config/profile, SQLite/WAL/SHM, logs, config/receipt/server metadata. The only permitted occurrence is `control-token`.

- [ ] **Step 8: Update standalone daemon and verify Task 2**

Add required `--control-token-file` to `arbiter-daemon`; read/validate it and pass token into AppState. Keep current standalone port default until Task 4.

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

All existing current-lifecycle tests must be green at the end of Task 2.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml Cargo.lock bench crates/arbiter-core crates/arbiter-provider-codex crates/arbiter-cli crates/arbiter-daemon
git commit -m "security: add authenticated arbiter control plane"
```

---

### Task 3: Add a compensating managed-endpoint transition primitive

**Files:**
- Modify: `crates/arbiter-adapter-codex/src/profile.rs`, `lib.rs`
- Create: `crates/arbiter-cli/src/commands/endpoint.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`

**Interfaces:**
- Produces `AppliedEndpointUpdate`.
- Produces `update_managed_endpoint(config_path, receipt, port)`.
- Produces `endpoint::publish(paths, port)`; Task 3 tests it without yet changing daemon lifecycle.

- [ ] **Step 1: Write adapter tests for endpoint-only mutation**

After normal `install_profile`, call:

```rust
let update = update_managed_endpoint(&config, &receipt, 45_678).unwrap();
```

Assert:

```rust
assert!(std::fs::read_to_string(&config).unwrap().contains("http://127.0.0.1:45678/v1"));
assert_ne!(update.updated_receipt().config.installed_sha256, receipt.config.installed_sha256);
assert_eq!(update.updated_receipt().config.original_sha256, receipt.config.original_sha256);
assert_eq!(update.updated_receipt().profile, receipt.profile);
```

Parse before/after TOML and assert every user key and every Arbiter provider field except `base_url` is unchanged.

- [ ] **Step 2: Implement rollback-capable update object**

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

Do not derive `Debug` over previous config bytes.

- [ ] **Step 3: Implement `update_managed_endpoint` checks**

Before writing:

1. current config hash equals current receipt installed hash;
2. current profile hash equals profile receipt installed hash;
3. managed provider still has fixed name, wire API, auth and zero retry settings;
4. current base URL is loopback Arbiter `/v1` shape;
5. only `base_url` changes to `http://127.0.0.1:{port}/v1`;
6. config replacement is atomic and permission-preserving;
7. returned receipt clone updates only `config.installed_sha256`.

Original backup/hash/existence/permissions never change.

- [ ] **Step 4: Implement the receipt coordinator and failure injection**

```rust
fn publish_with_writer<F>(
    paths: &Paths,
    port: u16,
    write_receipt: F,
) -> anyhow::Result<InstallReceipt>
where
    F: FnOnce(&Path, &InstallReceipt) -> anyhow::Result<()>,
```

Production `publish` uses `write_json_atomic`. A unit test writer always returns `std::io::Error::other("injected receipt failure")`. On that error, call `update.rollback()`; assert exact Codex config restoration and unchanged on-disk receipt. If rollback fails, return an error containing both high-level causes.

- [ ] **Step 5: Prove exact uninstall after multiple endpoint changes**

Test:

```text
original → install current port → publish 45678 → publish 0 → publish 41234 → uninstall latest receipt → exact original bytes/existence/permissions
```

- [ ] **Step 6: Verify Task 3**

```bash
cargo test -p arbiter-adapter-codex
cargo test -p arbiter-cli endpoint
cargo check --workspace --all-targets --all-features
```

- [ ] **Step 7: Commit**

```bash
git add crates/arbiter-adapter-codex crates/arbiter-cli/src/commands/endpoint.rs crates/arbiter-cli/src/commands/mod.rs
git commit -m "fix: make managed endpoint updates compensating"
```

---

### Task 4: Migrate to schema v2 and bind-first ephemeral runtime endpoints

**Files:**
- Modify: `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`, `start.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`, `control.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`, `contract.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`, `lib.rs`, `main.rs`
- Modify: `crates/arbiter-daemon/tests/shutdown.rs`, `privacy.rs`
- Modify: `bench/proxy_overhead.rs` only if constructor/start helper signatures require it

**Interfaces:**
- Final `LocalConfig` is schema 2 with no port/instance ID.
- Final init has no `--port` option and installs endpoint port zero.
- Runtime `server.json` is the only active port/instance metadata.
- Startup binds first, then publishes through Task 3 coordinator.
- Shutdown closes admission, republishes port zero, then completes bounded cleanup.

- [ ] **Step 1: Write schema-v2/inactive-init tests**

After `arbiter init codex --yes` assert:

```rust
assert_eq!(config["schema_version"], 2);
assert!(config.get("port").is_none());
assert!(config.get("instance_id").is_none());
assert!(installed.contains("base_url = \"http://127.0.0.1:0/v1\""));
assert!(std::net::TcpStream::connect(("127.0.0.1", 0)).is_err());
```

Add a Clap test proving `arbiter init codex --port 43123` fails because `--port` no longer exists.

- [ ] **Step 2: Convert installation config to schema v2**

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

`default_config(paths)` writes schema 2. `read_config` accepts only schema 2. Remove init port argument and install profile with port 0. Keep existing receipt/backups and Task 2 control token.

- [ ] **Step 3: Write bind-first runtime tests**

Start Arbiter and read `server.json`; assert port > 0. Stop, start again, and assert second `instance_id` differs. Do not require port inequality.

Before READY, inspect managed Codex config and require it to match the listener-derived `server.json.port`. No test may derive runtime port from `config.json`.

Add startup failure injection after listener bind but before READY: injected endpoint publication failure must leave no new `server.json` and managed endpoint at/in its previous safe state.

- [ ] **Step 4: Reorder foreground start exactly**

```text
TcpListener::bind(127.0.0.1:0)
→ bound_port = listener.local_addr().port()
→ acquire exclusive DB lease
→ read control token
→ open SQLite
→ reconcile incomplete attempts
→ generate fresh instance_id
→ endpoint::publish(paths, bound_port)
→ write server.json(identity)
→ build AppState(identity, token)
→ serve
```

No profile publication before listener + DB ownership.

If `server.json` write or later startup setup fails after publication, call `endpoint::publish(paths, 0)` before returning. A failed compensation reports both causes and never reports READY.

- [ ] **Step 5: Change CLI identity resolution to runtime metadata**

Change Task 2 helper to:

```rust
pub(crate) async fn current_identity(paths: &Paths) -> anyhow::Result<Option<DaemonIdentity>>
```

It reads `server.json`, reads control token, calls `identity_at(metadata.port, token)`, and accepts only exact identity equality. No `LocalConfig.port` or `instance_id` remains.

`status` rules:

```text
exact control identity → healthy; validate managed profile at metadata.port
no server metadata → stopped; validate managed profile at port 0
metadata port responds without exact control identity → unrecognized-process error
```

`doctor` uses the same active-port-or-zero profile validation.

- [ ] **Step 6: Add shutdown depublication hook**

Add:

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

On shutdown: establish the one existing deadline, call `state.begin_shutdown()`, await the hook under that same deadline, signal Axum graceful shutdown, then retain current drain/force-cancel/terminal-persistence/SQLite-close sequence. Existing `serve_listener_with_shutdown` delegates with `std::future::ready(())`.

CLI foreground passes a hook that calls `endpoint::publish(paths, 0)`. If it fails, log only:

```rust
tracing::error!(
    event_type = "endpoint_depublication_failed",
    "Arbiter failed to depublish its managed endpoint during shutdown"
);
```

Do not log path-bearing error text.

- [ ] **Step 7: Rewrite uninstall for runtime metadata**

No `server.json`: restore directly from receipt; do not probe a historical port.

`server.json` exists:

```text
exact authenticated identity → write private stop marker → wait for owned server metadata removal
identity fails + metadata port responds → refuse as unrecognized
identity fails + metadata port does not respond → remove stale metadata → continue receipt restoration
```

Successful uninstall removes `control-token`, config, receipt, stop/server metadata; preserves DB and immutable backups.

- [ ] **Step 8: Update standalone daemon to ephemeral default**

Change `arbiter-daemon --port` default to `0`; derive identity port from `listener.local_addr()`. Keep required `--control-token-file`. Standalone daemon still does not publish Codex config.

- [ ] **Step 9: Add crash/restart/privacy regressions**

Prove:

- reconciliation occurs only after listener + DB lease ownership;
- a crash leaves stale endpoint metadata but restart binds a new listener before republishing;
- public health remains identity-free;
- control token remains absent from config/receipt/server/SQLite/WAL/SHM/logs;
- exact uninstall still restores original Codex files after active→inactive endpoint transitions.

- [ ] **Step 10: Verify Task 4**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
```

- [ ] **Step 11: Commit**

```bash
git add bench crates/arbiter-cli crates/arbiter-daemon
git commit -m "fix: publish only listener-owned arbiter endpoints"
```

---

### Task 5: Add deterministic Linux/Windows CI and pin release tooling

**Files:**
- Modify: `rust-toolchain.toml`
- Create: `.github/workflows/ci.yml`
- Modify only if cargo-deny syntax compatibility requires it: `deny.toml`

**Interfaces:**
- CI jobs are named exactly `linux` and `windows`.
- Workflow permissions are `contents: read`; no secrets or artifacts.

- [ ] **Step 1: Pin repository execution toolchain**

```toml
[toolchain]
channel = "1.97.1"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

Keep workspace `rust-version = "1.85"` unchanged.

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

- [ ] **Step 3: Validate dependency policy without weakening it**

```bash
cargo install cargo-deny --version 0.20.2 --locked
cargo deny check
```

If `deny.toml` needs syntax-only adaptation, retain equivalent advisories/bans/licenses/sources policy.

- [ ] **Step 4: Inspect workflow security**

```bash
git diff --check
rg -n "OPENAI_API_KEY|secrets\.|contents: write|pull-requests: write|actions/upload-artifact" .github/workflows/ci.yml
```

Expected: no matches.

- [ ] **Step 5: Push implementation branch and require both CI jobs green**

A local green suite is not a substitute for GitHub `linux` + `windows` success on the implementation SHA.

- [ ] **Step 6: Commit**

```bash
git add rust-toolchain.toml .github/workflows/ci.yml
git add deny.toml 2>/dev/null || true
git commit -m "ci: verify arbiter m0 on linux and windows"
```

Only stage `deny.toml` if changed.

---

### Task 6: Re-run live, privacy, performance, and independent release gates

**Files:**
- Modify: `docs/m0/contract-gates.md`, `operations.md`, `performance.md`, `release-checklist.md`
- Create: `docs/m0/neurovia-dogfooding.md`

**Interfaces:**
- Produces release decision `GO FOR DAILY NEUROVIA DOGFOODING` only after fresh evidence.

- [ ] **Step 1: Run deterministic release suite on final implementation head**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
cargo deny check
git diff --check
```

Record exact commit SHA and actual debug/release test counts; do not reuse prior `dfaa349...` evidence.

- [ ] **Step 2: Run ignored live Codex contract suite**

```bash
codex login status
cargo test -p arbiter-cli --test contract -- --ignored --test-threads=1
```

Require ChatGPT-managed authentication with `OPENAI_API_KEY` removed. Verify direct and through-Arbiter streaming, usage extraction, cancellation, metadata-only persistence, new endpoint lifecycle, and exact restoration.

- [ ] **Step 3: Run NeuroVia-shaped smoke without changing default Codex profile**

From NeuroVia:

```bash
arbiter init codex --yes
arbiter doctor
arbiter start
codex exec --profile arbiter "Reply with exactly: arbiter-dogfood-ok"
arbiter status
arbiter uninstall --yes
```

Expected response exactly `arbiter-dogfood-ok`; status healthy; exact Codex restoration after uninstall.

- [ ] **Step 4: Re-run privacy sentinels**

Synthetic prompt/source/auth/control sentinels must be absent from DB/WAL/SHM, config, receipt, server metadata, Codex config/profile, and captured logs. Control credential may exist only in `control-token` before uninstall and must be removed afterward.

- [ ] **Step 5: Re-run 50-warmup/1,000-pair benchmark**

Record first-byte and total added p50/p95/p99. Pass only if:

```text
p50 < 10 ms
p95 < 25 ms
p99 < 50 ms
```

- [ ] **Step 6: Update operations and contract evidence**

Document exact lifecycle:

```text
init → inactive port 0
start → bind ephemeral port → publish owned endpoint
/control/identity → CLI-authenticated management identity
healthz → public identity-free health
shutdown → stop admission → depublish port 0 → bounded cleanup
uninstall → exact original restoration + preserve arbiter.db
```

State explicitly that inference localhost is not cryptographically server-pinned and hostile multi-user enterprise hardening remains deferred.

- [ ] **Step 7: Create `docs/m0/neurovia-dogfooding.md`**

```markdown
# NeuroVia Dogfooding — Arbiter M0

- Use only on a personal developer workstation under the approved M0 threat model.
- Invoke Codex explicitly with `--profile arbiter`; do not replace the default Codex profile.
- M0 always uses `gpt-5.6-terra` / medium; it does not yet optimize model choice.
- Run `arbiter doctor` before the first work session after an Arbiter upgrade.
- If status reports an unrecognized process or invalid profile, stop dogfooding before sending repository context.
- Use `arbiter uninstall --yes` as rollback; event history remains local.
```

Include init/start/status/uninstall commands and accepted localhost residual risk.

- [ ] **Step 8: Run fresh independent reviews**

Run Codex Engineering Guardrails verification on the final implementation commit and Codex Security standard repository scan. Required result:

```text
no blocking correctness finding
no blocking security finding within the approved M0 threat model
```

Any blocker reopens remediation.

- [ ] **Step 9: Require GitHub CI green on exact final SHA**

Both `linux` and `windows` must pass after final code/document changes.

- [ ] **Step 10: Close release checklist**

Set:

```text
PASS — M0 dogfooding-readiness remediation gate is closed.
GO FOR DAILY NEUROVIA DOGFOODING under the documented personal-workstation threat model.
```

Also state that this is neither M1 nor enterprise hostile-host hardening.

```bash
git add docs/m0
git commit -m "docs: close m0 dogfooding readiness gate"
```

---

## Final Verification Matrix

| Acceptance criterion | Evidence |
| --- | --- |
| >2 MiB through 32 MiB accepted | daemon proxy tests |
| >32 MiB 413 with zero attempt/upstream | daemon proxy tests |
| Named 32 MiB contract | `M0_MAX_REQUEST_BYTES` + operations docs |
| Public health hides private identity | daemon health/control tests |
| CLI ownership uses control token + exact identity | lifecycle control tests |
| Control secret owner-only | Unix/Windows private-file tests |
| Control secret never forwarded/persisted/logged | provider + privacy tests |
| Config schema v2 has no active port/instance | lifecycle tests |
| Bind-before-publish | start lifecycle test/review |
| Shutdown depublication | shutdown/lifecycle tests |
| Recovery only after listener + DB ownership | recovery tests |
| Exact uninstall after endpoint changes | adapter + lifecycle tests |
| Linux deterministic gate | GitHub `linux` job |
| Windows/ACL gate | GitHub `windows` job |
| No CI credentials/write permissions | workflow inspection |
| Live direct/through-Arbiter contract | ignored live suite |
| Performance SLO | 1,000-pair benchmark |
| Independent engineering review | final Guardrails report |
| Independent security review | final Security report |
| Residual localhost risk explicit | spec + operations + dogfooding runbook |
| Daily NeuroVia decision | final release checklist |

## Execution Notes

- Create an isolated worktree at execution time with `superpowers:using-git-worktrees`.
- Prefer `superpowers:subagent-driven-development`: fresh implementer/reviewer cycle per task.
- Use TDD in the written order and commit after each task.
- Do not start M1 while this remediation is open.
- If Codex changes its custom-provider contract, stop the affected task and re-run live/Context7 contract verification before changing the approved architecture.
- Never weaken a privacy/security regression merely to make a task pass; treat that as a finding.
