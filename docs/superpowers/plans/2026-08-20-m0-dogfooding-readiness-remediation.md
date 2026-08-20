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

Files created by this remediation:

- `crates/arbiter-core/src/control.rs` — redacted control-token value object and control header constant.
- `crates/arbiter-cli/src/commands/control.rs` — owner-only token generation/loading and authenticated control identity client.
- `crates/arbiter-cli/src/commands/endpoint.rs` — one coordinator for Codex endpoint publication/depublication plus receipt persistence/compensation.
- `crates/arbiter-daemon/src/control.rs` — control-header sensitivity middleware and `/control/identity` handler.
- `.github/workflows/ci.yml` — deterministic Linux/Windows CI gate.
- `docs/m0/neurovia-dogfooding.md` — post-remediation personal-workstation dogfooding procedure and residual-risk statement.

Files modified by this remediation:

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`
- `crates/arbiter-core/src/config.rs`, `crates/arbiter-core/src/health.rs`, `crates/arbiter-core/src/lib.rs`
- `crates/arbiter-provider-codex/src/provider.rs`
- `crates/arbiter-adapter-codex/src/profile.rs`
- `crates/arbiter-cli/Cargo.toml`
- `crates/arbiter-cli/src/commands/mod.rs`, `init.rs`, `start.rs`, `status.rs`, `doctor.rs`, `uninstall.rs`
- `crates/arbiter-cli/tests/lifecycle.rs`, `crates/arbiter-cli/tests/contract.rs`
- `crates/arbiter-daemon/src/lib.rs`, `app.rs`, `state.rs`, `main.rs`
- `crates/arbiter-daemon/tests/proxy.rs`, `privacy.rs`, `shutdown.rs`
- `docs/m0/contract-gates.md`, `operations.md`, `performance.md`, `release-checklist.md`

Do not create a second persistence layer, a second HTTP server, or an inference-side static secret header.

---

### Task 1: Make the 32 MiB request contract executable

**Files:**
- Modify: `crates/arbiter-core/src/config.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/tests/proxy.rs`

**Interfaces:**
- Produces: `pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;`
- Consumes later: the router and request-contract tests import that constant; no other task defines a second size value.

- [ ] **Step 1: Add failing exact-boundary and rejection tests**

Add this helper near the proxy integration-test helpers in `crates/arbiter-daemon/tests/proxy.rs`:

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

Add three integration tests using the existing loopback fake-provider pattern and `SqliteEventStore`:

```rust
#[tokio::test]
async fn accepts_json_larger_than_axum_default_and_preserves_payload() { /* 3 MiB body */ }

#[tokio::test]
async fn accepts_exactly_m0_max_request_bytes() { /* exact_json_body(M0_MAX_REQUEST_BYTES) */ }

#[tokio::test]
async fn rejects_one_byte_over_m0_limit_before_attempt_or_upstream() { /* M0_MAX_REQUEST_BYTES + 1 */ }
```

For the oversized case assert all of:

```rust
assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
assert_eq!(upstream_calls.load(Ordering::SeqCst), 0);
assert_eq!(store.recent_attempt_counts(0).await.unwrap().started, 0);
```

Add a malformed JSON case with the same zero-attempt/zero-upstream assertions and HTTP 400.

- [ ] **Step 2: Run the focused tests and verify the framework-default failure**

Run:

```bash
cargo test -p arbiter-daemon --test proxy accepts_json_larger_than_axum_default_and_preserves_payload -- --exact
cargo test -p arbiter-daemon --test proxy accepts_exactly_m0_max_request_bytes -- --exact
cargo test -p arbiter-daemon --test proxy rejects_one_byte_over_m0_limit_before_attempt_or_upstream -- --exact
```

Expected before implementation: the >2 MiB accepted test fails with body rejection and the exact-boundary test cannot pass under the implicit Axum default.

- [ ] **Step 3: Define the single M0 request-size constant**

In `crates/arbiter-core/src/config.rs` add:

```rust
pub const M0_MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
```

Add a unit assertion:

```rust
#[test]
fn m0_request_limit_is_exactly_32_mib() {
    assert_eq!(super::M0_MAX_REQUEST_BYTES, 33_554_432);
}
```

- [ ] **Step 4: Apply the limit at the Axum route boundary**

In `crates/arbiter-daemon/src/app.rs`, import `axum::extract::DefaultBodyLimit` and `arbiter_core::config::M0_MAX_REQUEST_BYTES`. Apply the layer to the Responses route, not to health/control routes:

```rust
Router::new()
    .route(
        "/v1/responses",
        post(responses).layer(DefaultBodyLimit::max(M0_MAX_REQUEST_BYTES)),
    )
```

Do not disable the limit globally and do not move attempt creation ahead of JSON extraction.

- [ ] **Step 5: Run request-contract and existing proxy regressions**

Run:

```bash
cargo test -p arbiter-daemon --test proxy
cargo test -p arbiter-core config::tests
```

Expected: all pass; exact 32 MiB is accepted, one byte over is 413, malformed JSON is 400, and both pre-attempt rejections create zero events/upstream calls.

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

- [ ] **Step 1: Write failing domain tests for a redacted control token**

Create `crates/arbiter-core/src/control.rs` initially with tests specifying the contract:

```rust
#[test]
fn control_token_accepts_only_43_char_base64url_values() {
    let token = ControlToken::parse("A".repeat(43)).unwrap();
    assert_eq!(token.expose(), "A".repeat(43));
    assert!(ControlToken::parse("short".to_owned()).is_err());
    assert!(ControlToken::parse(format!("{}=", "A".repeat(42))).is_err());
}

#[test]
fn control_token_debug_is_redacted() {
    let token = ControlToken::parse("A".repeat(43)).unwrap();
    assert_eq!(format!("{token:?}"), "ControlToken([REDACTED])");
}
```

Use 43 characters because 32 bytes encoded with base64url without padding has length 43.

- [ ] **Step 2: Implement the minimal redacted value object**

Implement:

```rust
pub const CONTROL_HEADER_NAME: &str = "x-arbiter-control-token";

#[derive(Clone, PartialEq, Eq)]
pub struct ControlToken(String);

impl ControlToken {
    pub fn parse(value: String) -> Result<Self, ControlTokenError> { /* exact length + URL-safe alphabet */ }
    pub fn expose(&self) -> &str { &self.0 }
}
```

Allowed bytes are ASCII alphanumeric, `-`, and `_`. Implement a custom `Debug` that never prints the value. Export the module from `arbiter-core/src/lib.rs`.

- [ ] **Step 3: Add failing daemon route tests**

In `crates/arbiter-daemon/tests/proxy.rs`, update test AppState creation with a deterministic 43-character test token and add:

```rust
#[tokio::test]
async fn public_health_does_not_expose_daemon_identity() { /* no pid/port/instance_id */ }

#[tokio::test]
async fn control_identity_requires_exact_control_token() { /* 404 missing/wrong; 200 exact */ }
```

The successful response body must equal the expected `DaemonIdentity`. The public `/healthz` body must not contain keys `pid`, `port`, `instance_id`, or `identity`.

- [ ] **Step 4: Implement control middleware and handler**

Create `crates/arbiter-daemon/src/control.rs` with:

```rust
pub(crate) async fn mark_control_header_sensitive(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response
```

If `CONTROL_HEADER_NAME` exists, call `HeaderValue::set_sensitive(true)` before `next.run(request).await`.

Add:

```rust
pub(crate) async fn identity(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response
```

Return `404` for missing/wrong tokens and `Json(state.identity.clone())` for an exact token. Do not log either presented or expected credentials.

- [ ] **Step 5: Remove identity from public health and wire the control route**

Change `DaemonHealth` to:

```rust
pub struct DaemonHealth {
    pub status: HealthStatus,
    pub components: Vec<ComponentHealth>,
}
```

Store `ControlToken` in `AppState`. Change both constructors to require it. In `build_router`, add:

```rust
.route("/control/identity", get(control::identity))
```

and apply the sensitivity middleware before any future HTTP instrumentation.

- [ ] **Step 6: Prove the control header can never reach ChatGPT**

Extend `forwards_only_allowlisted_headers_and_streams_terminal_metadata` in `crates/arbiter-provider-codex/src/provider.rs`:

```rust
headers.insert(
    arbiter_core::control::CONTROL_HEADER_NAME,
    HeaderValue::from_static("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
);
```

and assert the captured upstream headers do not contain it.

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo test -p arbiter-core control
cargo test -p arbiter-daemon --test proxy public_health_does_not_expose_daemon_identity -- --exact
cargo test -p arbiter-daemon --test proxy control_identity_requires_exact_control_token -- --exact
cargo test -p arbiter-provider-codex forwards_only_allowlisted_headers_and_streams_terminal_metadata
```

Expected: all pass and no test/debug output contains the control value.

- [ ] **Step 8: Commit**

```bash
git add crates/arbiter-core crates/arbiter-daemon crates/arbiter-provider-codex/src/provider.rs
git commit -m "security: separate arbiter control identity"
```

---

### Task 3: Move installation state to schema v2 and generate the owner-only control credential

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/arbiter-cli/Cargo.toml`
- Create: `crates/arbiter-cli/src/commands/control.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`
- Modify: `crates/arbiter-cli/src/commands/init.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`

**Interfaces:**
- Produces: `LocalConfig` schema v2 without `port` or `instance_id`.
- Produces: `Paths.control_token` at `$ARBITER_HOME/control-token`.
- Produces: `control::generate_and_store(paths) -> anyhow::Result<ControlToken>` and `control::read(paths) -> anyhow::Result<ControlToken>`.
- Installation provider endpoint is exactly `http://127.0.0.1:0/v1`.

- [ ] **Step 1: Write failing lifecycle tests for schema v2**

Replace the obsolete installation-port assertion with:

```rust
#[test]
fn init_creates_schema_v2_without_runtime_identity() {
    /* run arbiter init codex --yes */
    assert_eq!(config["schema_version"], 2);
    assert!(config.get("port").is_none());
    assert!(config.get("instance_id").is_none());
    assert!(installed_codex_config.contains("base_url = \"http://127.0.0.1:0/v1\""));
}
```

Add a test that `arbiter init codex --port 43123` is rejected by Clap because `--port` no longer exists.

Add a test that `$ARBITER_HOME/control-token` exists after `--yes`, contains exactly 43 base64url characters, and is owner-only on Unix. Extend existing Windows ACL coverage through the same private-file primitive rather than adding a second ACL implementation.

- [ ] **Step 2: Add only the dependencies needed for the specified token format**

In workspace dependencies:

```toml
base64 = "0.22"
getrandom = "0.3"
```

Add both to `arbiter-cli/Cargo.toml`. Do not add `rand`, `secrecy`, or a second crypto abstraction.

- [ ] **Step 3: Implement OS-random token generation**

In `commands/control.rs` implement generation exactly as:

```rust
let mut bytes = [0_u8; 32];
getrandom::fill(&mut bytes).context("generate Arbiter control credential")?;
let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
let token = ControlToken::parse(encoded)?;
write_new_private_synced(&paths.control_token, token.expose().as_bytes())?;
```

`read(paths)` reads UTF-8 without trimming hidden extra bytes and validates through `ControlToken::parse`.

- [ ] **Step 4: Convert CLI config to schema v2**

Change `LocalConfig` to contain only:

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

Change `default_config(paths)` accordingly and require `schema_version == 2` in `read_config`.

Remove `--port` from `Command::Init` and from `init::run`.

- [ ] **Step 5: Make init install only inactive state**

Call existing `install_profile(..., 0, timestamp)` so the initial managed provider is `127.0.0.1:0`. Generate the control token before writing the final installation receipt. If any later install step fails, remove the newly created control-token file during rollback; never leave a half-installed credential as evidence of a successful install.

Add `control_token: arbiter_home.join("control-token")` to `Paths`.

- [ ] **Step 6: Prove port zero is inert**

In lifecycle tests parse the installed `base_url`, assert it ends in `127.0.0.1:0/v1`, and assert `TcpStream::connect(("127.0.0.1", 0))` does not succeed. This is a syntax/configuration contract test, not a provider live call.

- [ ] **Step 7: Run CLI lifecycle tests**

Run:

```bash
cargo test -p arbiter-cli --test lifecycle
cargo test -p arbiter-cli m0_cli_exposes_only_codex_as_an_init_target
```

Expected: schema-v2 tests pass; tests that encoded the old persisted installation port are rewritten to the new listener-derived model, not silently deleted without replacement coverage.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/arbiter-cli
git commit -m "feat: initialize m0 runtime identity separately"
```

---

### Task 4: Make Codex endpoint publication and receipt updates compensating

**Files:**
- Modify: `crates/arbiter-adapter-codex/src/profile.rs`
- Modify: `crates/arbiter-adapter-codex/src/lib.rs`
- Create: `crates/arbiter-cli/src/commands/endpoint.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`

**Interfaces:**
- Produces: `AppliedEndpointUpdate` with `updated_receipt()` and `rollback()`.
- Produces: `update_managed_endpoint(config_path, receipt, port) -> Result<AppliedEndpointUpdate, CodexProfileError>`.
- Produces: `endpoint::publish(paths, port) -> anyhow::Result<InstallReceipt>` as the only CLI path that mutates the runtime `base_url` and installed receipt hash.

- [ ] **Step 1: Write failing adapter tests for endpoint-only mutation**

Add tests in `profile.rs` proving all of the following:

```rust
let update = update_managed_endpoint(&config, &receipt, 45_678).unwrap();
assert!(std::fs::read_to_string(&config).unwrap().contains("http://127.0.0.1:45678/v1"));
assert_ne!(update.updated_receipt().config.installed_sha256, receipt.config.installed_sha256);
assert_eq!(update.updated_receipt().config.original_sha256, receipt.config.original_sha256);
assert_eq!(update.updated_receipt().profile, receipt.profile);
```

Also assert unrelated original Codex keys and all Arbiter provider fields other than `base_url` are unchanged.

Add a rollback test that applies an endpoint update then calls `rollback()` and verifies exact previous bytes and permissions.

- [ ] **Step 2: Implement `AppliedEndpointUpdate`**

Keep rollback state private to the adapter:

```rust
pub struct AppliedEndpointUpdate {
    updated_receipt: InstallReceipt,
    previous_config: Vec<u8>,
    previous_permissions: OriginalPermissions,
    config_path: PathBuf,
}
```

Expose only:

```rust
pub fn updated_receipt(&self) -> &InstallReceipt;
pub fn rollback(self) -> Result<(), CodexProfileError>;
```

`Debug` must not print file contents.

- [ ] **Step 3: Implement `update_managed_endpoint` with conflict checking**

Before mutation:

1. verify current config hash equals `receipt.config.installed_sha256`;
2. verify the managed profile still equals `receipt.profile.installed_sha256`;
3. parse current config;
4. verify the managed provider has fixed M0 name, wire API, auth, retry settings, and loopback base URL shape;
5. change only `model_providers.arbiter.base_url` to `http://127.0.0.1:{port}/v1`;
6. atomically replace the config with current private permissions;
7. clone the receipt and update only `config.installed_sha256`.

Do not modify `original_sha256`, backup paths, original permissions, or profile receipt.

- [ ] **Step 4: Add the CLI coordinator and inject receipt-write failure in tests**

Create `commands/endpoint.rs` with an internal generic writer so compensation is testable:

```rust
fn publish_with_writer<F>(paths: &Paths, port: u16, write_receipt: F) -> anyhow::Result<InstallReceipt>
where
    F: FnOnce(&Path, &InstallReceipt) -> anyhow::Result<()>,
```

Production `publish(paths, port)` passes a closure using `write_json_atomic`.

If receipt persistence fails:

```rust
match update.rollback() {
    Ok(()) => return Err(receipt_error).context("persist endpoint receipt"),
    Err(rollback_error) => anyhow::bail!(
        "endpoint receipt persistence failed and Codex config rollback failed: {receipt_error}; rollback: {rollback_error}"
    ),
}
```

The test writer deliberately returns `std::io::Error::other("injected receipt failure")`; assert the Codex config is byte-for-byte restored and the on-disk receipt remains the previous one.

- [ ] **Step 5: Prove updated receipts still uninstall exactly**

Add an adapter test sequence:

```text
original config/profile
→ install port 0
→ update endpoint 45678
→ persist/use updated receipt
→ update endpoint 0
→ uninstall with latest receipt
→ exact original bytes + existence + permissions
```

- [ ] **Step 6: Run focused adapter/CLI tests**

Run:

```bash
cargo test -p arbiter-adapter-codex profile
cargo test -p arbiter-cli endpoint
```

Expected: endpoint-only mutation, compensation, conflict refusal, and exact uninstall all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/arbiter-adapter-codex crates/arbiter-cli/src/commands/endpoint.rs crates/arbiter-cli/src/commands/mod.rs
git commit -m "fix: make runtime endpoint publication atomic"
```

---

### Task 5: Bind first, publish second, and depublish while admission is closed

**Files:**
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/src/lib.rs`
- Modify: `crates/arbiter-daemon/src/main.rs`
- Modify: `crates/arbiter-daemon/tests/shutdown.rs`
- Modify: `crates/arbiter-cli/src/commands/start.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`

**Interfaces:**
- Produces: `serve_listener_with_shutdown_hook(state, listener, shutdown, grace, on_admission_closed)`.
- Keeps: existing `serve_listener_with_shutdown(...)` as a no-op-hook wrapper for current tests/consumers.
- `start --foreground` derives `metadata.port` from `listener.local_addr()` and generates a fresh `instance_id` per start.

- [ ] **Step 1: Write failing lifecycle tests for listener-derived runtime identity**

Rewrite lifecycle helpers so they read `server.json` after start instead of expecting the init-time port. Add tests that prove:

```rust
assert!(first.port > 0);
assert!(second.port > 0);
assert_ne!(first.instance_id, second.instance_id);
```

Do not assert mathematical port inequality because an OS may eventually reuse an ephemeral port. Instead inspect the initialized config and assert there is no persisted port authority, and assert each foreground start binds port `0` before deriving the actual `server.json` port.

Add a failure-path test where publication is injected to fail after listener bind; assert no READY metadata is published and the managed provider remains/restores to inactive port zero.

- [ ] **Step 2: Add a shutdown hook after admission closes**

Implement:

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

On shutdown signal:

1. compute the single existing process-wide deadline;
2. call `state.begin_shutdown()` first;
3. await `on_admission_closed` within that same deadline;
4. signal Axum graceful shutdown;
5. retain existing drain/force-cancel/persistence/SQLite-close behavior.

If the hook exhausts the deadline, log only `shutdown_phase = "endpoint_depublication"`; do not log path contents or credentials.

Keep `serve_listener_with_shutdown` delegating to this function with `std::future::ready(())`.

- [ ] **Step 3: Make foreground start bind `127.0.0.1:0` before publication**

In `start::run_foreground` perform this exact order:

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
→ construct AppState with exact identity + control token
→ serve
```

No profile mutation may occur before listener + DB ownership.

- [ ] **Step 4: Compensate startup failure after endpoint publication**

If `server.json` persistence or AppState/server setup fails after publishing the active endpoint, call `endpoint::publish(paths, 0)` before returning the error. If that compensation also fails, report both failures and never print a READY/success message.

- [ ] **Step 5: Depublish on every controlled shutdown**

Pass an `on_admission_closed` future that runs:

```rust
if let Err(error) = endpoint::publish(&shutdown_paths, 0) {
    tracing::error!(
        event_type = "endpoint_depublication_failed",
        "Arbiter failed to depublish its managed endpoint during shutdown"
    );
}
```

Do not format `error` into structured logs if it can contain filesystem paths. CLI stderr may report the high-level failure separately when appropriate.

After the server finishes, remove `server.json` only if identity still matches the process that owned it.

- [ ] **Step 6: Update standalone `arbiter-daemon` semantics**

Change standalone daemon default `--port` to `0`, derive the real identity port from `listener.local_addr()`, and add a required `--control-token-file` argument. Read/validate the token before serving. The standalone binary does not publish Codex config; document it as a low-level operator/test path.

- [ ] **Step 7: Run lifecycle and shutdown regressions**

Run:

```bash
cargo test -p arbiter-daemon --test shutdown
cargo test -p arbiter-cli --test lifecycle
```

Expected: shutdown still honors one bounded deadline, admission closes before depublication, listener-derived runtime identity is used, and failed startup never leaves a newly published unowned endpoint.

- [ ] **Step 8: Commit**

```bash
git add crates/arbiter-daemon crates/arbiter-cli/src/commands/start.rs crates/arbiter-cli/tests/lifecycle.rs
git commit -m "fix: publish only owned daemon endpoints"
```

---

### Task 6: Move status, doctor, and uninstall ownership checks to the control channel

**Files:**
- Modify: `crates/arbiter-cli/src/commands/control.rs`
- Modify: `crates/arbiter-cli/src/commands/status.rs`
- Modify: `crates/arbiter-cli/src/commands/doctor.rs`
- Modify: `crates/arbiter-cli/src/commands/uninstall.rs`
- Modify: `crates/arbiter-cli/tests/lifecycle.rs`
- Modify: `crates/arbiter-daemon/tests/privacy.rs`

**Interfaces:**
- Produces: `control::identity_at(port, token) -> Option<DaemonIdentity>`.
- Produces: `control::current_identity(paths) -> anyhow::Result<Option<DaemonIdentity>>`, which requires both `server.json` and a matching authenticated control response.
- Public `/healthz` is never used to establish ownership.

- [ ] **Step 1: Add failing control-client lifecycle tests**

Extend the fake local HTTP server used by lifecycle tests so it can distinguish `/healthz` from `/control/identity` and inspect the control header. Add cases:

- generic `200 /healthz` cannot satisfy `status`, `doctor`, `start`, or `uninstall` ownership;
- `/control/identity` without the expected token returns/acts as 404 and is rejected;
- wrong identity with a valid token is rejected;
- exact authenticated identity is accepted.

Retain the existing “unrecognized process” refusal behavior when a stale metadata port is occupied by another process.

- [ ] **Step 2: Implement authenticated identity lookup**

In `commands/control.rs`, build a Reqwest client with the existing short local timeout and send:

```text
GET http://127.0.0.1:{port}/control/identity
X-Arbiter-Control-Token: <token>
```

Construct the `HeaderValue` separately and call `set_sensitive(true)` before inserting it. Return `None` on non-success, invalid JSON, timeout, or identity mismatch; never include the token in error strings.

`current_identity(paths)` must:

1. return `Ok(None)` if `server.json` is absent;
2. read `ServerMetadata`;
3. read the owner-only control token;
4. query `metadata.port`;
5. return the identity only when the authenticated response equals metadata exactly.

- [ ] **Step 3: Rewrite `status` around runtime metadata, not config port**

`status` still reports PASSTHROUGH, Terra/Medium, storage integrity, profile state, provider reachability, and attempt counts. Daemon state becomes:

- `healthy` only for exact authenticated control identity;
- `stopped` when no server metadata and managed endpoint is inactive port zero;
- error/unrecognized when stale metadata points to a responding but unauthenticated process.

When healthy, validate the managed Codex profile against the authenticated identity port. When stopped, validate against port zero.

- [ ] **Step 4: Rewrite `doctor` with the same ownership contract**

`doctor` must validate:

- schema-v2 local PASSTHROUGH config;
- owner-only control token exists and parses;
- SQLite integrity;
- managed profile equals active authenticated port when running or inactive port zero when stopped;
- loopback bind remains enforced;
- Codex reports ChatGPT authentication;
- remote export remains disabled.

It must not print the control token or full private identity.

- [ ] **Step 5: Rewrite uninstall stop logic**

If `server.json` is absent, uninstall does not probe a historical configured port because config no longer has one. It proceeds to exact receipt-based restoration.

If `server.json` exists:

- authenticate exact control identity;
- if exact, create the existing private stop marker and wait for owned metadata removal;
- if identity fails and the metadata port is responding, refuse as unrecognized;
- if identity fails and the metadata port is not responding, treat metadata as stale, remove only that stale metadata, and continue exact receipt restoration.

After successful restoration remove `control-token`, `receipt`, `config`, stale stop/server metadata, but preserve `arbiter.db` and immutable backups.

- [ ] **Step 6: Add control-secret privacy regression**

In `crates/arbiter-daemon/tests/privacy.rs` and lifecycle tests:

1. read the generated token from `$ARBITER_HOME/control-token`;
2. perform init/start/status/doctor/one synthetic proxy call/uninstall;
3. scan Codex config/profile, SQLite, WAL, SHM, captured daemon logs, config/receipt/server metadata;
4. assert the exact token bytes occur only in the designated `control-token` file while installed and nowhere after successful uninstall.

Do not scan the token file and then claim “no occurrence”; explicitly exclude that single authorized storage location.

- [ ] **Step 7: Run full deterministic workspace tests locally**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Expected: all deterministic tests pass; live contract tests remain ignored by default.

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
- Modify if required by the newer audit tool: `deny.toml`

**Interfaces:**
- Produces two required CI job names: `linux` and `windows`.
- CI requires only `contents: read` and no secrets.

- [ ] **Step 1: Pin the Rust toolchain used by release evidence**

Change `rust-toolchain.toml` to:

```toml
[toolchain]
channel = "1.97.1"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

Keep `[workspace.package].rust-version = "1.85"` unchanged; it describes package MSRV intent, while the repository execution toolchain is pinned for reproducible CI/release evidence.

- [ ] **Step 2: Create the least-privilege workflow**

Create `.github/workflows/ci.yml`:

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

Do not add caching, artifact upload, OpenAI secrets, ChatGPT state, or write permissions in this remediation.

- [ ] **Step 3: Verify cargo-deny 0.20.2 against current policy locally**

Run:

```bash
cargo install cargo-deny --version 0.20.2 --locked
cargo deny check
```

If `deny.toml` syntax has changed, make only syntax-equivalent changes needed for 0.20.2; do not weaken advisories, bans, licenses, or sources policy to obtain green output.

- [ ] **Step 4: Validate workflow syntax and no-secret policy by inspection**

Check:

```bash
git diff --check
rg -n "OPENAI_API_KEY|CHATGPT|secrets\.|contents: write|pull-requests: write|actions/upload-artifact" .github/workflows/ci.yml
```

Expected: `rg` finds no secret or write/artifact configuration; only the workflow’s ordinary text should remain.

- [ ] **Step 5: Push implementation branch and require fresh CI evidence**

On the implementation PR, wait for both `linux` and `windows` jobs to complete successfully. A local green test suite is not a substitute for this step.

- [ ] **Step 6: Commit**

```bash
git add rust-toolchain.toml .github/workflows/ci.yml deny.toml
git commit -m "ci: verify arbiter m0 on linux and windows"
```

If `deny.toml` did not change, omit it from `git add`.

---

### Task 8: Re-run live, privacy, performance, and independent review gates

**Files:**
- Modify: `docs/m0/contract-gates.md`
- Modify: `docs/m0/operations.md`
- Modify: `docs/m0/performance.md`
- Modify: `docs/m0/release-checklist.md`
- Create: `docs/m0/neurovia-dogfooding.md`

**Interfaces:**
- Produces the release decision `GO FOR DAILY NEUROVIA DOGFOODING` only if every gate below has fresh evidence.
- Does not change runtime behavior.

- [ ] **Step 1: Run the complete deterministic release suite on the implementation head**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
cargo deny check
git diff --check
```

Record the exact commit SHA and test counts in `docs/m0/release-checklist.md`. Do not reuse the prior `dfaa349...` evidence.

- [ ] **Step 2: Run the ignored live Codex contract suite with ChatGPT-managed auth**

Confirm first:

```bash
codex login status
```

The report must identify ChatGPT authentication. Then run with `OPENAI_API_KEY` removed from the child environment:

```bash
cargo test -p arbiter-cli --test contract -- --ignored --test-threads=1
```

The suite must prove direct and through-Arbiter Responses streaming, usage extraction, cancellation, metadata-only persistence, and exact restoration using the new bind-first profile lifecycle.

- [ ] **Step 3: Run a manual NeuroVia-shaped smoke without making Arbiter default**

From a NeuroVia checkout:

```bash
arbiter init codex --yes
arbiter doctor
arbiter start
codex exec --profile arbiter "Reply with exactly: arbiter-dogfood-ok"
arbiter status
arbiter uninstall --yes
```

Verify the output is exactly `arbiter-dogfood-ok`, status reports a healthy daemon during the run, and uninstall restores the pre-run Codex files exactly. Do not persist repository content into Arbiter evidence.

- [ ] **Step 4: Re-run privacy sentinel checks**

Use synthetic sentinels for prompt/source/auth/control values. Verify no sentinel exists in:

```text
arbiter.db
arbiter.db-wal
arbiter.db-shm
config.json
install-receipt.json
server.json
Codex config/profile
captured structured logs
```

The only allowed location for the control credential before uninstall is `$ARBITER_HOME/control-token`; after uninstall it must be removed.

- [ ] **Step 5: Re-run the 1,000-pair release benchmark**

Use the existing benchmark binary/procedure with 50 warmups and 1,000 paired direct/proxy samples. Record first-byte and total added latency p50/p95/p99 in `docs/m0/performance.md`.

Release passes only if added local latency remains:

```text
p50 < 10 ms
p95 < 25 ms
p99 < 50 ms
```

Do not relax the gate because of the remediation.

- [ ] **Step 6: Update operational documentation with the new lifecycle**

`docs/m0/operations.md` must show:

```text
init → inactive port 0
start → bind ephemeral port → publish owned endpoint
/control/identity → CLI-only authenticated management identity
healthz → public health without private identity
shutdown → close admission → depublish to port 0 → bounded cleanup
uninstall → exact original restoration + preserve arbiter.db
```

Document that the localhost inference endpoint is not cryptographically server-pinned and that hostile multi-user enterprise hardening is deferred.

- [ ] **Step 7: Add NeuroVia dogfooding runbook**

Create `docs/m0/neurovia-dogfooding.md` with these operating rules:

```markdown
# NeuroVia Dogfooding — Arbiter M0

- Use only on a personal developer workstation under the approved M0 threat model.
- Invoke Codex explicitly with `--profile arbiter`; do not replace the default Codex profile.
- M0 always uses `gpt-5.6-terra` / medium; it does not yet optimize model choice.
- Run `arbiter doctor` before the first work session after an Arbiter upgrade.
- If status reports an unrecognized process or invalid profile, stop dogfooding and diagnose before sending repository context.
- Use `arbiter uninstall --yes` as the rollback path; event history remains local.
```

Include the exact init/start/status/uninstall commands and the accepted localhost residual risk.

- [ ] **Step 8: Run independent engineering and security reviews**

Run a fresh Codex Engineering Guardrails verification against the final implementation commit and a fresh Codex Security standard scan scoped to the repository. Required outcome:

```text
no blocking correctness finding
no blocking security finding within the approved M0 threat model
```

Any validated blocker reopens the remediation; do not mark the release checklist PASS around it.

- [ ] **Step 9: Verify GitHub CI on the exact final commit**

Confirm both GitHub Actions jobs are green on the final commit after all code/doc changes. If documentation-only changes follow a green run, the final push must still produce fresh CI status for that SHA.

- [ ] **Step 10: Close the release checklist and commit evidence**

Update `docs/m0/release-checklist.md` decision from the old M0 release wording to:

```text
PASS — M0 dogfooding-readiness remediation gate is closed.
GO FOR DAILY NEUROVIA DOGFOODING under the documented personal-workstation threat model.
```

Keep an explicit sentence that this is not M1 and not enterprise hostile-host hardening.

Commit:

```bash
git add docs/m0
git commit -m "docs: close m0 dogfooding readiness gate"
```

---

## Final Verification Matrix

Before merge/release, map fresh evidence to every approved acceptance criterion:

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
| Recovery after listener + DB ownership | existing + updated start recovery tests |
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
- Use TDD in the order written; do not batch all tests at the end.
- Do not start M1 work while this plan is open.
- If implementation discovers that Codex changed its custom-provider contract, stop that task and re-run the live contract/Context7 verification before altering the approved architecture.
- If a task requires weakening an existing safety/privacy assertion to pass, treat that as a finding rather than changing the assertion.
