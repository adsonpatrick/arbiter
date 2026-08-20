# Arbiter M0 — Transparent Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the M0 transparent local proxy so Codex can use Arbiter through the Responses API with streaming/cancellation preserved, durable local event capture, health/status/doctor commands, safe Codex configuration rollback, and measurable proxy overhead while production always uses the approved Terra/Medium baseline.

**Architecture:** M0 is a Rust Cargo workspace with a localhost-only Axum daemon, a Codex configuration adapter, a first-class OpenAI Responses passthrough provider, and SQLx/SQLite durable storage. The proxy accepts only the Responses-compatible surface required by Codex, forwards requests without semantic mutation, streams upstream bytes without full buffering, records privacy-minimized attempt metadata, and never performs adaptive routing in M0. LiteLLM is not in the M0 data path; a compatibility probe may be run only after direct OpenAI passthrough is green.

**Tech Stack:** Rust stable; Tokio 1.x; Axum 0.8.x; Reqwest 0.12.x with rustls + stream; SQLx 0.8.x with SQLite + migrations; Clap 4.x; Serde/serde_json; tracing/tracing-subscriber; uuid; sha2; tempfile for integration tests; purpose-built benchmark binary for local proxy overhead.

**Spec:** `docs/superpowers/specs/2026-08-19-codex-compute-governor-design.md`

## Global Constraints

- Codex-first, OpenAI-first, local-first.
- Fresh installs start in `PASSTHROUGH`; `init` MUST NOT silently enable optimized routing.
- M0 performs no adaptive model routing. All production requests resolve to the approved baseline `gpt-5.6-terra` with reasoning effort `medium`, subject to contract verification.
- Routing cannot expand filesystem, shell, network, MCP, Git, sandbox, or approval privileges.
- The daemon binds to `127.0.0.1` by default.
- Only `POST /v1/responses` is required for the Codex proxy data path in M0; all other inference routes are rejected unless an explicit contract test proves Codex requires them.
- Streaming MUST be forwarded incrementally; the proxy MUST NOT buffer the complete upstream response before returning it to Codex.
- One upstream provider invocation equals one immutable attempt record.
- Raw prompts, source code, model output, Authorization headers, cookies, API keys, and environment secrets MUST NOT be persisted by default.
- `SECRET` data is never exportable.
- SQLite is sufficient for M0 core operation; no Supabase dependency is permitted.
- OpenAI upstream retry is owned by Arbiter. Reqwest performs no automatic semantic retry; Codex custom-provider retries MUST be set to zero in the managed profile for M0 contract tests so retry amplification is measurable.
- Software release and policy state remain separate; M0 ships a fixed local baseline artifact, not an adaptive policy compiler.
- `OpenAIProvider` is first-class. `LiteLLMProvider` is not required to complete M0.
- M0 acceptance requires direct-vs-proxy benchmark evidence and safe uninstall/restoration of the user's previous Codex configuration.
- No dashboard, hosted control plane, Supabase sink, evaluation harness, BENCH, shadow execution, policy promotion, ML routing, or multi-harness support in M0.

## Contract Gate Findings Frozen for M0

1. Codex supports custom providers through `model_providers.<id>.base_url` plus `wire_api = "responses"`. The OpenAI Codex repository contains a strict Responses API proxy that forwards `POST /v1/responses`, providing an authoritative behavioral reference for M0.
2. Current contract verification identified `gpt-5.6-luna`, `gpt-5.6-terra`, and `gpt-5.6-sol`; the M0 baseline is `gpt-5.6-terra` + `medium`. Live contract probes remain authoritative at implementation time.
3. M0 must preserve the incoming Codex request body except for the fixed baseline model/effort normalization explicitly tested below.
4. Responses streaming exposes terminal completion information and usage. M0 may tee/parse SSE events for metadata, but bytes returned to Codex remain the upstream stream.
5. LiteLLM remains optional because the direct OpenAI path is the authoritative compatibility baseline.
6. SQLx supports SQLite pools, migrations, and explicit transactions. M0 uses it for durable append-only events and remains ready for M1 budget reservations without replacing persistence.

## Planned Repository Structure

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
deny.toml

crates/
  arbiter-core/
    Cargo.toml
    src/
      lib.rs
      config.rs
      ids.rs
      events.rs
      health.rs

  arbiter-storage-sqlite/
    Cargo.toml
    migrations/
      0001_m0_events.sql
    src/
      lib.rs
      store.rs

  arbiter-provider-openai/
    Cargo.toml
    src/
      lib.rs
      provider.rs
      sse.rs

  arbiter-adapter-codex/
    Cargo.toml
    src/
      lib.rs
      config_file.rs
      profile.rs

  arbiter-daemon/
    Cargo.toml
    src/
      lib.rs
      app.rs
      proxy.rs
      state.rs
      main.rs

  arbiter-cli/
    Cargo.toml
    src/
      main.rs
      commands/
        mod.rs
        init.rs
        start.rs
        status.rs
        doctor.rs
        uninstall.rs

tests/
  fixtures/
    codex_config_minimal.toml
    codex_config_existing_provider.toml
    response_completed.sse
  integration/
    proxy_streaming.rs
    proxy_headers.rs
    proxy_cancellation.rs
    proxy_persistence.rs
    codex_config_roundtrip.rs
    daemon_health.rs
  contract/
    openai_live.rs
    codex_cli_live.rs
  security/
    secret_redaction.rs
    route_rejection.rs

bench/
  proxy_overhead.rs

docs/
  m0/
    contract-gates.md
    operations.md
```

---

### Task 1: Scaffold the Rust workspace and enforce dependency boundaries

**Files:** root Cargo/toolchain/deny files and all six initial crates.

**Produces:** a Cargo workspace where `arbiter-core` has zero dependencies on adapter/provider/storage crates and all vendor-specific dependencies point inward.

- [ ] Create the workspace manifest with Rust 2024 edition, Apache-2.0 license, and workspace dependencies for anyhow, async-trait, axum, bytes, clap, futures-util, http, reqwest, serde, serde_json, sha2, sqlx, thiserror, tokio, toml_edit, tower, tracing, tracing-subscriber, and uuid.
- [ ] Keep `arbiter-core` limited to domain dependencies (`serde`, `thiserror`, `uuid`).
- [ ] Expose only `config`, `events`, `health`, and `ids` from core.
- [ ] Run `cargo metadata --no-deps` and `cargo check --workspace`.
- [ ] Add dependency checks with `cargo deny check` and `cargo tree -p arbiter-core`; core must not depend on Axum, Reqwest, SQLx, TomlEdit, OpenAI-specific code, or Codex-specific code.
- [ ] Commit: `chore: scaffold arbiter m0 workspace`.

### Task 2: Define M0 core domain contracts and privacy-minimized events

**Files:** `crates/arbiter-core/src/{ids,config,events,health}.rs`.

**Produces:** typed request/attempt/event IDs, `BaselineTarget`, `ReasoningEffort`, `GovernorEvent` equivalent Arbiter event types, and daemon/component health types.

- [ ] Write failing tests proving the M0 baseline resolves to `gpt-5.6-terra` + `medium` and generated IDs are distinct.
- [ ] Implement UUIDv7-backed typed IDs.
- [ ] Implement `ReasoningEffort::{None,Low,Medium,High,XHigh,Max}` and `BaselineTarget::m0()`.
- [ ] Define immutable payloads for AttemptStarted, AttemptCompleted, and AttemptFailed with IDs, target, timings, token usage, provider response ID, and typed error class only.
- [ ] Add serialization privacy tests proving no request/response content, Authorization, source sentinel, or secret sentinel appears in event JSON.
- [ ] Run `cargo test -p arbiter-core`.
- [ ] Commit: `feat: define m0 arbiter domain contracts`.

### Task 3: Implement SQLite durable append-only event storage

**Files:** `crates/arbiter-storage-sqlite/migrations/0001_m0_events.sql`, `src/store.rs`, `src/lib.rs`.

**Produces:** `SqliteEventStore::open`, append, attempt-event query, and integrity-check operations.

- [ ] Create `governor_events`/Arbiter event table with immutable event ID PK, timestamp, type, optional request/attempt IDs, schema version, payload JSON, and attempt index.
- [ ] Write a failing append/read test using a temporary SQLite file.
- [ ] Open SQLite with `create_if_missing`, WAL journal mode, foreign keys, and SQLx migrations.
- [ ] Implement append using plain `INSERT`; duplicate event IDs must fail instead of updating history.
- [ ] Implement `PRAGMA integrity_check` startup validation and require result `ok`.
- [ ] Test duplicate IDs and database reopen/persistence.
- [ ] Commit: `feat: add durable sqlite event store`.

### Task 4: Implement the direct OpenAI Responses streaming provider

**Files:** `crates/arbiter-provider-openai/src/{provider,sse,lib}.rs`, `tests/fixtures/response_completed.sse`.

**Produces:** `OpenAIProvider::new`, one-call Responses forwarding, streamed upstream response, and parsed terminal usage metadata. Provider performs no semantic retries internally.

- [ ] Create a terminal Responses SSE fixture and a failing parser test for response ID, input/cached/output/reasoning token fields.
- [ ] Implement an incremental SSE metadata parser tolerant of arbitrary byte chunk boundaries; test every split point of the fixture.
- [ ] Build Reqwest client with redirects disabled and no application retry loop.
- [ ] Override `Host`/`Authorization`; never forward incoming Authorization, Cookie, or Proxy-Authorization; preserve safe non-hop-by-hop Codex headers.
- [ ] Normalize only `model = gpt-5.6-terra` and `reasoning.effort = medium`; preserve all other request fields semantically, including unknown fields.
- [ ] Expose upstream `bytes_stream()` without whole-response buffering. Metadata extraction must be tee-like and must not delay downstream chunks for database writes.
- [ ] Run provider tests.
- [ ] Commit: `feat: add streaming openai responses provider`.

### Task 5: Implement the localhost daemon and strict Responses proxy

**Files:** `crates/arbiter-daemon/src/{state,app,proxy,main}.rs` plus streaming/cancellation/persistence/route-rejection integration tests.

**Produces:** `POST /v1/responses`, `GET /healthz`, `GET /status`, localhost binding, strict route rejection.

- [ ] Test that non-required inference routes are rejected.
- [ ] Build Axum router for the three M0 routes and bind `127.0.0.1` only.
- [ ] Persist AttemptStarted before invoking upstream. If this durability-critical append fails, return 503 and issue zero upstream requests.
- [ ] Forward upstream status/safe headers and stream with `Body::from_stream`.
- [ ] Append AttemptCompleted only after terminal completion metadata; append AttemptFailed for provider setup/connect errors and interrupted streams.
- [ ] Prove first chunk reaches client before fake upstream sends terminal chunk.
- [ ] Prove client cancellation drops the upstream body/stream and cannot create a false completion event.
- [ ] Prove storage order is start then exactly one terminal result.
- [ ] Commit: `feat: proxy codex responses through arbiter daemon`.

### Task 6: Implement privacy-safe structured logging and secret regression tests

**Files:** daemon logging config, `tests/security/secret_redaction.rs`, `docs/m0/operations.md`.

**Allowed logs:** IDs, HTTP status, model ID, effort, duration, token counts, typed error class.

**Forbidden logs/storage:** request body, response content, Authorization, Cookie, environment variables, repository paths.

- [ ] Send sentinel API key/bearer/cookie/source strings through the test proxy and assert none appears in captured logs or SQLite.
- [ ] Configure JSON tracing with `RUST_LOG`; do not add request-body tracing middleware.
- [ ] Document exactly what M0 stores and state explicitly that content persistence is unsupported in M0.
- [ ] Commit: `security: enforce privacy-safe m0 telemetry`.

### Task 7: Implement safe Codex profile installation, restoration, and uninstall

**Files:** `crates/arbiter-adapter-codex/src/{config_file,profile}.rs`, TOML fixtures, roundtrip integration test.

**Managed names:** provider `arbiter`, profile `arbiter`.

- [ ] Test config containing unrelated providers/comments/settings: install Arbiter, preserve unrelated TOML, uninstall, restore from verified backup.
- [ ] Use `toml_edit` to add a Responses provider pointing to `http://127.0.0.1:PORT/v1` with `request_max_retries = 0` and `stream_max_retries = 0` plus an Arbiter profile with Terra/Medium.
- [ ] Do not change the user's default Codex profile automatically.
- [ ] Write backup to `~/.arbiter/backups/codex-config-<timestamp>.toml`, close/fsync it, then update config through temp file + same-filesystem atomic rename.
- [ ] Store/verify backup hash. If config changed after Arbiter install, report conflict rather than silently overwriting newer user changes.
- [ ] Run adapter and roundtrip tests.
- [ ] Commit: `feat: manage codex arbiter profile safely`.

### Task 8: Implement the M0 CLI (`init`, `start`, `status`, `doctor`, `uninstall`)

**Files:** `crates/arbiter-cli/src/main.rs` and command modules; daemon health integration test.

- [ ] Define Clap commands. `InitTarget` contains only Codex in M0.
- [ ] Implement `arbiter init codex` as plan-then-apply; noninteractive mutation requires `--yes`.
- [ ] Initialize Arbiter home, DB/migrations, local config, Codex backup, managed provider/profile. Remain PASSTHROUGH.
- [ ] Implement `start`, resolving API credential without persistence, starting local daemon, and writing server metadata containing PID/port/version only.
- [ ] Implement `status`: daemon health, PASSTHROUGH mode, baseline, provider reachability, storage integrity, Codex profile state, recent attempt counts.
- [ ] Implement `doctor`: config parse, DB integrity, port, API credential presence without printing it, Terra/Medium contract config, Codex binary, managed profile/provider, no remote export, local binding only. Required failures return nonzero.
- [ ] Implement `uninstall`: stop daemon, safely restore Codex config, preserve Arbiter DB by default and print its path for manual deletion.
- [ ] Test full `init -> status -> uninstall` with temporary HOME.
- [ ] Commit: `feat: add m0 arbiter cli lifecycle`.

### Task 9: Run live OpenAI and Codex contract probes

**Files:** `tests/contract/{openai_live,codex_cli_live}.rs`, `docs/m0/contract-gates.md`.

Contract tests are ignored by default and require explicit environment configuration.

- [ ] Direct OpenAI live test: Terra/Medium, streaming enabled; assert HTTP success, streamed event(s), terminal completion, parseable model/usage.
- [ ] Governor/Arbiter passthrough live test: same semantic request through `/v1/responses`; assert equivalent terminal shape and usage extraction.
- [ ] Live Codex probe using temporary profile: `codex exec -p arbiter "Reply with exactly: arbiter-ok"`; assert exit 0, expected terminal user output, valid attempt records, and no content persisted.
- [ ] Do not assert one Codex command equals one model call; Codex is agentic.
- [ ] Probe cancellation by terminating a streaming Codex request and prove no false AttemptCompleted is recorded.
- [ ] Record tested Codex/Arbiter versions, target/effort, Responses behavior, cancellation, paths/headers, retry settings, date and PASS/FAIL without secrets/prompts/source.
- [ ] M0 is not contract-complete until direct OpenAI and Codex-through-Arbiter probes pass. Material mismatch requires plan/spec amendment, not compatibility hacks.
- [ ] Commit: `test: verify codex and openai m0 contracts`.

### Task 10: Benchmark proxy overhead and enforce M0 performance evidence

**Files:** `bench/proxy_overhead.rs`, `docs/m0/performance.md`.

- [ ] Build deterministic local fake upstream with configurable first-byte delay/chunk interval.
- [ ] Benchmark at least 1,000 warmed requests direct-to-fake versus client-to-Arbiter-to-fake and record delta p50/p95/p99.
- [ ] Measure first-byte forwarding and prove terminal chunk is not required before first downstream chunk.
- [ ] Run a small separate live TTFT sample; treat it as diagnostic because network/provider variance dominates.
- [ ] Document hardware/OS, release build, sample sizes, direct/proxied distributions, Arbiter-added delta, live TTFT, and SLO result.
- [ ] Block M1 if local Arbiter-added latency materially exceeds p50 10 ms / p95 25 ms / p99 50 ms; profile before proceeding.
- [ ] Commit: `perf: benchmark transparent proxy overhead`.

### Task 11: Add failure-injection and shutdown/recovery behavior required by M0

**Files:** storage/upstream/shutdown integration tests and daemon lifecycle code.

- [ ] Force event-store append failure before attempt; assert upstream receives zero requests and daemon returns 503.
- [ ] Refuse upstream connection; assert AttemptStarted then AttemptFailed, normalized gateway error, and no stronger-model fallback.
- [ ] Emit one valid SSE chunk then interrupt upstream; assert first chunk reached client and attempt terminates failed/interrupted, not completed.
- [ ] Implement SIGINT/SIGTERM graceful shutdown: stop accepting, bounded grace for in-flight requests, persist observable terminal state, close SQLite pool.
- [ ] Test that a second request is rejected once shutdown begins and the in-flight request follows grace semantics.
- [ ] Commit: `test: harden m0 failure and shutdown behavior`.

### Task 12: Final M0 verification, documentation, and release gate

**Files:** `docs/m0/{operations,contract-gates,performance,release-checklist}.md`.

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] Run `cargo test --workspace` and `cargo test --workspace --release`.
- [ ] Run `cargo deny check`.
- [ ] Run ignored live OpenAI and Codex contract suites explicitly and record evidence.
- [ ] Run the proxy benchmark and require M0 SLO evidence or open a blocking performance finding.
- [ ] Manual smoke: `arbiter init codex`, `arbiter doctor`, `arbiter start`, `codex exec -p arbiter "Reply with exactly: m0-ok"`, `arbiter status`, `arbiter uninstall`; verify direct Codex config restoration.
- [ ] Complete release checklist for Responses compatibility, streaming, cancellation, one invocation/one attempt, SQLite durability, content/secret privacy, localhost binding, safe install/uninstall, provider failures, performance evidence, and tested versions.
- [ ] Self-review scope: no PhaseDetector, RiskClassifier, candidate routing, evaluation, promotion, Supabase, dashboard, or multi-harness implementation in M0.
- [ ] Commit: `docs: close m0 transparent proxy release gate`.

## Plan Self-Review

### Spec coverage

M0 requirements are covered by Tasks 1–12: daemon/Responses proxy, OpenAIProvider, streaming/cancellation, local durable events, status/doctor, fixed baseline, privacy, safe Codex config restoration, contract probes, performance benchmark, failure injection/shutdown, and release evidence.

Explicitly deferred:

- M1: Session/Turn/Step inference, PhaseDetector, RiskClassifier, full budget governance, explain.
- M2: BENCH, verifiers, shadow, EvaluationHarness, CandidatePolicyCompiler.
- M3: promotion, adaptive ApprovedPolicy routing, cognitive escalation/de-escalation, demotion.
- Optional Supabase and LiteLLM integrations.

### Placeholder scan

The plan contains no unresolved placeholders or generic implementation instructions. Any executor discovering a missing external contract must stop that task and amend the plan/spec rather than inventing behavior.

### Type/interface consistency

- Core defines IDs, baseline, effort, and event payloads.
- SQLite consumes core events only.
- OpenAI provider consumes baseline and emits streamed response/usage metadata.
- Daemon composes provider + store.
- Codex adapter edits Codex configuration only; it does not proxy traffic.
- CLI orchestrates daemon/config lifecycle; it does not implement provider logic.
- Live tests exercise the same interfaces as production M0.

## Execution Handoff

Implementation must run in an isolated worktree and use TDD task-by-task.

**Option 1 — Subagent-Driven (recommended):** use `superpowers:subagent-driven-development`; dispatch a fresh implementation agent per task and perform requirement/code-quality review at each gate.

**Option 2 — Inline Execution:** use `superpowers:executing-plans`; execute tasks sequentially in batches with review checkpoints.

Do not start M1 until Task 12 closes the M0 release gate.
