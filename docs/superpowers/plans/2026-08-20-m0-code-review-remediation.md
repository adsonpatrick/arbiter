# M0 Code Review Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct every validated M0 review finding while preserving Arbiter's transparent Codex proxy behavior, exact profile restoration, durable audit history, and bounded shutdown.

**Architecture:** Strengthen the existing modules at their current boundaries: the CLI owns daemon identity and managed files, the adapter owns reversible profile transactions, the store owns event invariants and recovery, and the daemon owns stream classification and lifecycle. Each task begins with a focused regression test, implements the smallest invariant-preserving change, and commits an independently reviewable slice.

**Tech Stack:** Rust 2024, Tokio, Axum, Reqwest, SQLx/SQLite, Serde, UUID, tempfile, sha2, Cargo test/clippy/fmt/deny.

**Spec:** `docs/superpowers/specs/2026-08-20-m0-code-review-remediation-design.md`

## Global Constraints

- Codex is managed directly through its existing ChatGPT session; no OpenAI API key is introduced or persisted.
- Forward request method, path, query, status, headers, body, and SSE bytes transparently; governance metadata must never alter the wire payload.
- An endpoint is current only when PID, port, Arbiter version, and a cryptographically random `instance_id` match local metadata exactly.
- The default listener port is selected by the OS; an explicit `--port` remains supported.
- A pre-existing named Codex profile is replaced only after an exact backup and is restored byte-for-byte, including prior existence and stricter Unix mode.
- Arbiter-owned directories are at most `0700` and files at most `0600` on Unix; Windows inherits the owner-restricted parent ACL and Arbiter must not broaden it.
- Each attempt has one `AttemptStarted` and at most one terminal event; existing legacy duplicate rows must not prevent migration.
- Terminal persistence is retried for a bounded period and incomplete attempts are reconciled to `AttemptFailed(StreamInterrupted)` before the daemon accepts traffic.
- Non-2xx provider responses are forwarded unchanged and recorded as `ProviderHttp`.
- SSE metadata buffering is bounded; inability to verify a terminal marker fails closed for audit classification while forwarding all bytes unchanged.
- Total shutdown work uses one ten-second deadline, reserving up to two seconds for persistence and database close.
- Do not add `unsafe` code or log credential-bearing header values.

---

## File Map

- `crates/arbiter-core/src/health.rs`: shared `DaemonIdentity` and identity-bearing health contract.
- `crates/arbiter-cli/src/commands/mod.rs`: config/metadata schema, random port and instance identity plumbing, private managed-file writes.
- `crates/arbiter-cli/src/commands/{init,start,status,doctor,uninstall}.rs`: lifecycle identity checks and unrecognized-listener behavior.
- `crates/arbiter-cli/tests/lifecycle.rs`: daemon spoofing, ephemeral default port, and lifecycle regressions.
- `crates/arbiter-adapter-codex/src/file_security.rs`: private-directory/file creation and original Unix mode capture/restore.
- `crates/arbiter-adapter-codex/src/{config_file,profile}.rs`: exact backup/replacement and compensated two-file transactions.
- `crates/arbiter-adapter-codex/tests/profile.rs`: existing-profile round trip and permission regressions.
- `crates/arbiter-storage-sqlite/migrations/0002_single_terminal.sql`: forward-only trigger enforcing one terminal event.
- `crates/arbiter-storage-sqlite/src/store.rs`: incomplete-attempt reconciliation, duplicate-terminal error mapping, and inline storage regressions.
- `crates/arbiter-daemon/src/{app,proxy,state}.rs`: startup recovery, reliable terminal persistence, response classification, one shutdown deadline.
- `crates/arbiter-provider-codex/src/{provider,sse}.rs`: sensitive header flags and bounded SSE metadata parser.
- `crates/arbiter-daemon/tests/proxy.rs`: end-to-end terminal, HTTP error, and transparent-stream regressions.
- `tests/contract/codex_cli_live.rs`: real Codex cancellation assertions, included by the CLI contract test target.
- `docs/m0/{contract-gates,operations,performance,release-checklist}.md`: updated security/lifecycle behavior and verification evidence.

### Task 1: Authenticated daemon identity and ephemeral default port

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/arbiter-cli/Cargo.toml`
- Modify: `crates/arbiter-core/src/health.rs`
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `crates/arbiter-daemon/src/main.rs`
- Modify: `crates/arbiter-daemon/src/state.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`
- Modify: `crates/arbiter-cli/src/commands/init.rs`
- Modify: `crates/arbiter-cli/src/commands/start.rs`
- Modify: `crates/arbiter-cli/src/commands/status.rs`
- Modify: `crates/arbiter-cli/src/commands/doctor.rs`
- Modify: `crates/arbiter-cli/src/commands/uninstall.rs`
- Test: `crates/arbiter-cli/tests/lifecycle.rs`

**Interfaces:**
- Produces: `DaemonIdentity { pid: u32, port: u16, version: String, instance_id: String }` and `DaemonHealth { status: HealthStatus, components: Vec<ComponentHealth>, identity: DaemonIdentity }`.
- Produces: `LocalConfig.instance_id: String`, `ServerMetadata.instance_id: String`, and `daemon_is_current(paths: &Paths) -> Result<bool>` using exact identity equality.
- Produces: CLI `Init.port: Option<u16>`; `None` reserves an OS-selected loopback port, while `Some(0)` remains invalid.

- [ ] **Step 1: Write failing lifecycle tests**

Add tests that initialize without `--port` and assert a nonzero configured port, and initialize with `--port <free-port>` and assert that exact port is retained. Then place a fake 2xx `/healthz` server on a recorded port and assert `start`, `status`, `doctor`, and `uninstall` all report an unrecognized process rather than treating it as Arbiter. Repeat with a parsed health response whose `instance_id` mismatches. Parse and compare the complete shape:

```rust
let metadata: serde_json::Value = serde_json::from_slice(&fs::read(server_path)?)?;
assert_eq!(metadata["port"], configured_port);
assert!(metadata["instance_id"].as_str().is_some_and(|v| !v.is_empty()));
assert_eq!(health["identity"], metadata);
```

- [ ] **Step 2: Run the focused tests and confirm red**

Run: `cargo test -p arbiter-cli --test lifecycle -- --nocapture`

Expected: FAIL because `init` requires the fixed default, metadata has no `instance_id`, and generic 2xx health is accepted.

- [ ] **Step 3: Implement the identity contract and port selection**

Enable UUID v4 and add the CLI dependency:

```toml
uuid = { version = "1.18", features = ["serde", "v4", "v7"] }
```

Use these shared types and exact comparison:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonIdentity {
    pub pid: u32,
    pub port: u16,
    pub version: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonHealth {
    pub status: HealthStatus,
    pub components: Vec<ComponentHealth>,
    pub identity: DaemonIdentity,
}
```

Generate `Uuid::new_v4().to_string()` during init, bind `127.0.0.1:0` to select the default port, persist both values, pass the identity into `AppState`, and require parsed `/healthz.identity == ServerMetadata`. Keep a separate TCP/listener occupancy probe so a healthy but mismatched listener causes an explicit unrecognized-process error instead of a second bind attempt.

- [ ] **Step 4: Run CLI, core, and daemon tests**

Run: `cargo test -p arbiter-core -p arbiter-daemon -p arbiter-cli`

Expected: PASS, including the generic-2xx and wrong-instance spoof regressions.

- [ ] **Step 5: Commit**

```powershell
git add Cargo.toml crates/arbiter-core crates/arbiter-daemon crates/arbiter-cli
git commit -m "fix: authenticate local daemon identity"
```

### Task 2: Private backups and exact pre-existing profile restoration

**Files:**
- Create: `crates/arbiter-adapter-codex/src/file_security.rs`
- Modify: `crates/arbiter-adapter-codex/src/lib.rs`
- Modify: `crates/arbiter-adapter-codex/src/config_file.rs`
- Modify: `crates/arbiter-adapter-codex/src/profile.rs`
- Modify: `crates/arbiter-cli/src/commands/mod.rs`
- Test: `crates/arbiter-adapter-codex/tests/profile.rs`

**Interfaces:**
- Produces: `OriginalPermissions { unix_mode: Option<u32> }` serialized as `ManagedFileReceipt.original_permissions`.
- Produces: public `ensure_private_dir(&Path)`, `write_new_private_synced(&Path, &[u8])`, and `atomic_replace_with_permissions(&Path, &[u8], &OriginalPermissions)` for the adapter and CLI to share.
- Consumes: Task 1's managed Arbiter paths and identity-bearing receipts.

- [ ] **Step 1: Replace the overwrite-rejection test with failing exact-roundtrip tests**

Create a pre-existing `arbiter.config.toml` containing the exact bytes `b"# prior profile\nmodel = \"gpt-5\"\n"` and a stricter Unix mode, install the managed profile, uninstall, and assert:

```rust
assert_eq!(fs::read(&profile_path)?, original_profile_bytes);
assert_eq!(receipt.profile.original_existed, true);
#[cfg(unix)]
assert_eq!(fs::metadata(&profile_path)?.permissions().mode() & 0o777, 0o400);
```

Also assert `.arbiter`, `backups`, backup payloads, backup hashes, receipts, config, and server metadata are never broader than `0700`/`0600` on Unix.
Keep the existing regression that a pre-existing `model_providers.arbiter` table is rejected before either target file changes.

- [ ] **Step 2: Run focused tests and confirm red**

Run: `cargo test -p arbiter-adapter-codex --test profile -- --nocapture`

Expected: FAIL because an existing named profile is rejected and new backup files use default modes.

- [ ] **Step 3: Implement private file primitives and exact restoration metadata**

Use `OpenOptionsExt::mode(0o600)` plus an explicit post-create permission clamp on Unix; clamp created directories to `0700`. Capture the source mode before replacement and restore the stricter recorded mode. On Windows, retain the parent directory's inherited DACL and never copy an ACL from a less-restricted temporary location.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginalPermissions {
    pub unix_mode: Option<u32>,
}

pub(crate) fn private_file_mode(original: &OriginalPermissions) -> u32 {
    original.unix_mode.unwrap_or(0o600) & 0o600
}
```

Remove the existing-profile rejection. Back up both config and profile before replacing either, record exact bytes/hash/existence/permissions, and delete the installed profile on uninstall only when `original_existed == false`.

- [ ] **Step 4: Run adapter and CLI tests**

Run: `cargo test -p arbiter-adapter-codex -p arbiter-cli`

Expected: PASS with byte-for-byte pre-existing profile restoration and private Unix modes.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-adapter-codex crates/arbiter-cli
git commit -m "fix: preserve and protect Codex profiles"
```

### Task 3: Compensated two-file profile transactions

**Files:**
- Modify: `crates/arbiter-adapter-codex/src/config_file.rs`
- Modify: `crates/arbiter-adapter-codex/src/profile.rs`
- Test: `crates/arbiter-adapter-codex/src/profile.rs`
- Test: `crates/arbiter-adapter-codex/tests/profile.rs`

**Interfaces:**
- Produces: private `FileOperations` trait with `replace` and `remove` methods, implemented by `RealFileOperations` and a test failure injector.
- Produces: `ProfileError::CompensationFailed { operation: Box<ProfileError>, compensation: Box<ProfileError> }`.
- Consumes: Task 2's exact backups and permission-aware restore primitive.

- [ ] **Step 1: Add failure-injection tests for the second mutation**

Use an in-module fake that fails on a configured mutation number. Assert a failed profile install restores the original config, and a failed profile restore during uninstall puts the installed config back:

```rust
let error = uninstall_profile_with_ops(&receipt, &FailOnMutation::new(2)).unwrap_err();
assert!(matches!(error, ProfileError::Io(_)));
assert_eq!(fs::read(&config_path)?, installed_config_bytes);
assert_eq!(fs::read(&profile_path)?, installed_profile_bytes);
```

Add a case where mutation 2 and its compensation fail and assert `CompensationFailed` retains both errors.

- [ ] **Step 2: Run unit and roundtrip tests and confirm red**

Run: `cargo test -p arbiter-adapter-codex profile -- --nocapture`

Expected: FAIL because uninstall currently leaves a mixed state when the second restore fails.

- [ ] **Step 3: Implement staged validation and compensation**

Before any mutation, verify installed hashes and both backups. Capture current installed bytes and modes, and prepare synced same-directory temporary replacements for both targets. Apply config restoration, then profile restoration. If the second operation fails, atomically restore the installed config snapshot; return the original error if compensation succeeds and the compound variant if it fails. Use the same protocol during installation.

```rust
match ops.replace(&profile.path, &profile_original, &profile.original_permissions) {
    Ok(()) => Ok(()),
    Err(operation) => match ops.replace(&config.path, &installed_config, &installed_config_permissions) {
        Ok(()) => Err(operation),
        Err(compensation) => Err(ProfileError::CompensationFailed {
            operation: Box::new(operation),
            compensation: Box::new(compensation),
        }),
    },
}
```

- [ ] **Step 4: Run adapter tests**

Run: `cargo test -p arbiter-adapter-codex`

Expected: PASS for normal round trips and every injected failure point.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-adapter-codex
git commit -m "fix: make Codex profile restoration atomic"
```

### Task 4: Database-enforced terminal uniqueness and startup recovery

**Files:**
- Create: `crates/arbiter-storage-sqlite/migrations/0002_single_terminal.sql`
- Modify: `crates/arbiter-storage-sqlite/src/store.rs`
- Modify: `crates/arbiter-daemon/src/main.rs`
- Modify: `crates/arbiter-cli/src/commands/start.rs`
- Test: `crates/arbiter-storage-sqlite/src/store.rs`

**Interfaces:**
- Produces: `SqliteEventStore::reconcile_incomplete_attempts(&self, recovered_at_ms: u64) -> Result<u64, StoreError>`.
- Produces: `StoreError::DuplicateTerminal { attempt_id: String }` for trigger violations.
- Consumes: `GovernorEvent::AttemptFailed` with `ErrorClass::StreamInterrupted`.

- [ ] **Step 1: Add failing store invariants**

Insert a start plus completion, then assert inserting a failure for the same attempt returns `DuplicateTerminal`. Insert a start without a terminal, reconcile, and assert exactly one failure appears; reconcile again and assert zero changes. For legacy coverage, create a temporary database by executing `include_str!("../migrations/0001_m0_events.sql")`, insert duplicate historical terminals, then execute `include_str!("../migrations/0002_single_terminal.sql")`; assert the migration succeeds while a subsequent duplicate is blocked.

```rust
assert_eq!(store.reconcile_incomplete_attempts(now).await?, 1);
assert_eq!(store.reconcile_incomplete_attempts(now + 1).await?, 0);
let events = store.events_for_attempt(attempt_id).await?;
assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
```

- [ ] **Step 2: Run store tests and confirm red**

Run: `cargo test -p arbiter-storage-sqlite -- --nocapture`

Expected: FAIL because duplicate terminals are accepted and reconciliation does not exist.

- [ ] **Step 3: Add the trigger and transactional reconciliation**

Use a trigger, not a unique index, so historical duplicate rows do not fail migration:

```sql
CREATE TRIGGER governor_events_single_terminal
BEFORE INSERT ON governor_events
WHEN NEW.event_type IN ('attempt_completed', 'attempt_failed')
 AND EXISTS (
   SELECT 1 FROM governor_events
   WHERE attempt_id = NEW.attempt_id
     AND event_type IN ('attempt_completed', 'attempt_failed')
 )
BEGIN
  SELECT RAISE(ABORT, 'arbiter: duplicate terminal event');
END;
```

In one transaction, select starts without terminals, deserialize their correlation fields, append `AttemptFailed(StreamInterrupted)`, and commit. Invoke reconciliation immediately after opening the store in both daemon entry points and before either calls a serving function; do not invoke it from CLI read-only status or history commands.

- [ ] **Step 4: Run store and daemon tests**

Run: `cargo test -p arbiter-storage-sqlite -p arbiter-daemon`

Expected: PASS, with recovery idempotent and complete attempts unchanged.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-storage-sqlite crates/arbiter-daemon
git commit -m "fix: enforce durable attempt terminals"
```

### Task 5: Reliable stream terminal recording and provider HTTP classification

**Files:**
- Modify: `crates/arbiter-daemon/src/proxy.rs`
- Modify: `crates/arbiter-daemon/src/state.rs`
- Modify: `crates/arbiter-provider-codex/src/provider.rs`
- Test: `crates/arbiter-daemon/src/proxy.rs`
- Test: `crates/arbiter-daemon/tests/proxy.rs`

**Interfaces:**
- Produces: `RuntimeState::spawn_terminal_persistence(&self, store: SqliteEventStore, event: GovernorEvent)` with three bounded attempts and explicit tracing on final failure.
- Produces: `GovernedStream::record_terminal` that flips `terminal_recorded` only after scheduling one terminal and ignores all later terminal causes.
- Produces: deterministic unit coverage by constructing the existing public `ProviderByteStream` alias from `futures_util::stream::iter`, without exposing a production-only test constructor.
- Consumes: Task 4's database trigger/reconciliation and `ProviderResponse.status`.

- [ ] **Step 1: Add failing completed-then-error and non-2xx tests**

Construct a test stream that emits a valid `response.completed` SSE event and then an I/O error. Await persistence and assert exactly one terminal, `AttemptCompleted`. Add 401, 429, and 500 upstream cases that preserve status/body and record `AttemptFailed(ProviderHttp)` with the status.

```rust
assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
assert_eq!(body, original_body);
assert_eq!(terminal.error_class(), Some(ErrorClass::ProviderHttp));
```

Add a persistence-failure case by closing the pool after `AttemptStarted`, allowing the terminal, reopening the database, running reconciliation, and asserting one recovered failure.

- [ ] **Step 2: Run focused tests and confirm red**

Run: `cargo test -p arbiter-daemon proxy -- --nocapture`

Expected: FAIL because completion can be followed by failure, non-2xx is classified as interruption, and append failures are discarded.

- [ ] **Step 3: Implement one-shot terminal scheduling and bounded retry**

Guard every terminal path, including stream errors:

```rust
fn record_terminal(&mut self, event: GovernorEvent) {
    if self.terminal_recorded {
        return;
    }
    self.terminal_recorded = true;
    self.runtime.spawn_terminal_persistence(self.store.clone(), event);
}
```

Retry terminal appends three times with short capped backoff, treat `DuplicateTerminal` as success, and log attempt/correlation IDs but never headers. If `ProviderResponse.status` is not successful, schedule `ProviderHttp` before forwarding its unchanged headers and byte stream; later EOF/error paths see the guard and cannot overwrite it.

- [ ] **Step 4: Run daemon and transparent contract tests**

Run: `cargo test -p arbiter-daemon -p arbiter-provider-codex`

Expected: PASS, with exactly one terminal in all response paths.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-daemon crates/arbiter-provider-codex
git commit -m "fix: classify and persist stream terminals"
```

### Task 6: Bounded SSE metadata and sensitive credential headers

**Files:**
- Modify: `crates/arbiter-provider-codex/src/sse.rs`
- Modify: `crates/arbiter-provider-codex/src/provider.rs`
- Test: `crates/arbiter-provider-codex/src/sse.rs`
- Test: `crates/arbiter-provider-codex/src/provider.rs`
- Test: `crates/arbiter-daemon/tests/proxy.rs`

**Interfaces:**
- Produces: `SseMetadataParser { pending, extraction_disabled }` with `MAX_PENDING_EVENT_BYTES: usize = 1_048_576`.
- Produces: `SseMetadataParser::metadata_available(&self) -> bool` so EOF classification fails closed after overflow.
- Consumes: Task 5's guarded terminal classification.

- [ ] **Step 1: Add failing memory-bound and sensitivity tests**

Push an unterminated payload larger than the limit, assert extraction is disabled and retained bytes are at or below the cap, then verify later input is still ignored for metadata. Build forwarded headers and assert credentials are sensitive without changing values:

```rust
assert!(headers[AUTHORIZATION].is_sensitive());
assert!(headers["chatgpt-account-id"].is_sensitive());
assert_eq!(headers[AUTHORIZATION], "Bearer session-token");
```

Add a transparent-proxy case proving the oversized event bytes received downstream are byte-identical and the attempt ends failed because completion could not be verified.

- [ ] **Step 2: Run provider tests and confirm red**

Run: `cargo test -p arbiter-provider-codex -- --nocapture`

Expected: FAIL because `pending` grows without a cap and copied headers are not marked sensitive.

- [ ] **Step 3: Implement fail-closed metadata extraction**

Before extending `pending`, check `pending.len().saturating_add(chunk.len())`. On overflow, clear the duplicate buffer, set `extraction_disabled = true`, and return no metadata; forwarding still uses the original chunk. Mark Authorization and account-bound copied `HeaderValue`s sensitive after cloning and before building the Reqwest request.

```rust
if self.pending.len().saturating_add(chunk.len()) > MAX_PENDING_EVENT_BYTES {
    self.pending.clear();
    self.extraction_disabled = true;
    return None;
}
```

- [ ] **Step 4: Run provider and transparent proxy tests**

Run: `cargo test -p arbiter-provider-codex; cargo test -p arbiter-daemon --test proxy`

Expected: PASS with bounded duplicate buffering, unchanged wire bytes, and redacted Debug behavior.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-provider-codex crates/arbiter-daemon/tests/proxy.rs
git commit -m "fix: bound Codex SSE metadata parsing"
```

### Task 7: One shutdown deadline and exact live cancellation evidence

**Files:**
- Modify: `crates/arbiter-daemon/src/app.rs`
- Modify: `tests/contract/codex_cli_live.rs`
- Test: `crates/arbiter-daemon/tests/shutdown.rs`

**Interfaces:**
- Produces: `shutdown_with_deadline(..., grace: Duration)` using one `tokio::time::Instant` deadline.
- Consumes: Task 5's tracked terminal persistence and Task 4's unique terminal invariant.

- [ ] **Step 1: Add failing aggregate-time and cancellation assertions**

Use deliberately stalled server, idle, persistence, and close futures; measure the whole shutdown and assert it stays within one grace plus scheduler tolerance rather than four grace periods. Tighten the ignored live cancellation probe to poll durable history and require exactly one start plus one failed terminal classified `Cancelled`:

```rust
assert_eq!(events.len(), 2);
assert!(matches!(events[0], GovernorEvent::AttemptStarted { .. }));
assert!(matches!(events[1], GovernorEvent::AttemptFailed {
    error_class: ErrorClass::Cancelled,
    ..
}));
```

- [ ] **Step 2: Run focused daemon test and confirm red**

Run: `cargo test -p arbiter-daemon shutdown -- --nocapture`

Expected: FAIL because each shutdown phase currently receives the full grace duration.

- [ ] **Step 3: Implement a shared monotonic deadline**

Compute `deadline = Instant::now() + grace` once. Reserve `min(2 seconds, grace / 5)` for persistence and close, stop graceful serving by `deadline - reserve`, force-cancel remaining streams at that boundary, and pass `deadline` to every remaining `timeout_at`. Trigger forced cancellation only once, always attempt close within the remaining budget, and emit a privacy-safe diagnostic naming only the incomplete phase when no budget remains.

```rust
let deadline = Instant::now() + grace;
let cleanup_reserve = Duration::from_secs(2).min(grace / 5);
let serving_deadline = deadline - cleanup_reserve;
let _ = timeout_at(serving_deadline, server).await;
state.force_cancel();
let _ = timeout_at(deadline, runtime.wait_for_persistence()).await;
let _ = timeout_at(deadline, store.close()).await;
```

- [ ] **Step 4: Run daemon tests; run live probe only when prerequisites exist**

Run: `cargo test -p arbiter-daemon`

Run when a real logged-in Codex CLI is available: `cargo test -p arbiter-cli --test contract -- --ignored --nocapture`

Expected: daemon tests PASS; live cancellation persists exactly one `Cancelled` terminal. If the external Codex prerequisite is absent, record SKIPPED with the precise prerequisite rather than weakening the assertion.

- [ ] **Step 5: Commit**

```powershell
git add crates/arbiter-daemon tests/contract/codex_cli_live.rs
git commit -m "fix: bound shutdown and cancellation audit"
```

### Task 8: Documentation, complete verification, and review closure

**Files:**
- Modify: `docs/m0/contract-gates.md`
- Modify: `docs/m0/operations.md`
- Modify: `docs/m0/performance.md`
- Modify: `docs/m0/release-checklist.md`
- Modify only if a verification or review regression requires it: files from Tasks 1-7

**Interfaces:**
- Consumes: all preceding task contracts.
- Produces: auditable release evidence and a clean draft PR containing every remediation.

- [ ] **Step 1: Update operator documentation**

Document the random default port, exact daemon identity check, existing-profile backup/restore behavior, owner-only managed files, incomplete-attempt startup recovery, `ProviderHttp`, SSE metadata cap, and single ten-second shutdown budget. Add exact commands and results to release evidence; never include access tokens, account IDs, or backup contents.

- [ ] **Step 2: Run formatting and static gates**

Run:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-targets --all-features
cargo deny check
```

Expected: every command exits 0 with no warnings.

- [ ] **Step 3: Run all automated tests in debug and release**

Run:

```powershell
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
```

Expected: all non-ignored tests PASS in both profiles.

- [ ] **Step 4: Run contracts, smoke lifecycle, and persistence benchmark**

Run `cargo test -p arbiter-daemon --test proxy -- --nocapture` and `cargo test -p arbiter-cli --test lifecycle -- --nocapture`. When a real logged-in Codex CLI is available, run both ignored direct and through-Arbiter live suites with `cargo test -p arbiter-cli --test contract -- --ignored --nocapture`. Perform `init -> start -> m0-ok -> status -> doctor -> stop -> uninstall` in an isolated temporary Codex home, and rerun the 1,000 start/terminal pair release benchmark. Expected: wire equality, exact single terminals, exact file restoration, no orphan daemon, SQLite integrity `ok`, and benchmark within the approved M0 threshold. If live prerequisites are absent, record the exact skipped prerequisite in `docs/m0/contract-gates.md`.

- [ ] **Step 5: Request a second Superpowers code review and resolve every valid finding**

Invoke `superpowers:requesting-code-review` against the fixed base `b369d94`. For each finding, use `superpowers:receiving-code-review`, reproduce it, add a failing regression test, implement the smallest correction, and rerun the affected gates. The closure condition is zero unresolved critical/important/minor findings within M0 scope.

- [ ] **Step 6: Commit evidence and any review follow-ups**

```powershell
git add docs/m0/contract-gates.md docs/m0/operations.md docs/m0/performance.md docs/m0/release-checklist.md
git commit -m "docs: record m0 remediation evidence"
```

- [ ] **Step 7: Verify the final diff and update the draft PR**

Run `git status --short`, `git diff --check b369d94..HEAD`, and `git log --oneline b369d94..HEAD`. Expected: clean worktree, no whitespace errors, and focused remediation commits. Push `feature/m0-transparent-proxy`, then update PR #1 with the corrected invariants, exact test results, live-test status, benchmark result, and second-review outcome.
