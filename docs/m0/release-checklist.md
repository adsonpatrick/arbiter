# Arbiter M0 release checklist

Date: 2026-08-20  
Version: `0.1.0`  
Final evidence parent: `21c5471`

## Decision

**PASS — M0 transparent proxy release gate is closed.** No blocking correctness,
contract, privacy, restoration, dependency, or performance finding remains.

## Product and contract gates

- [x] Rust workspace builds on stable Rust `1.97.1` / Windows x86_64 MSVC.
- [x] Codex CLI `0.148.0-alpha.21` uses ChatGPT-managed authentication; Arbiter
      owns no API key and never reads credential storage.
- [x] Managed Codex 0.134+ named layer uses `arbiter.config.toml`; no legacy
      `[profiles.arbiter]` dependency remains.
- [x] Provider is pinned to the verified HTTPS Codex Responses upstream,
      disables redirects and semantic retries, and forwards authorization only
      in memory for the active request.
- [x] Production target is fixed to `gpt-5.6-terra` with medium reasoning.
- [x] One provider invocation creates one attempt at index 0; connection errors
      do not retry, route, or fall back to a stronger model.
- [x] Direct and through-Arbiter live contract tests pass explicitly.
- [x] Manual `m0-ok` smoke passes through `--profile arbiter`.

## Runtime gates

- [x] Daemon binds only to `127.0.0.1` and exposes `POST /v1/responses`,
      `GET /healthz`, and `GET /status`.
- [x] Request fields are preserved except fixed baseline normalization.
- [x] SSE reaches the client before terminal completion and usage is extracted
      without changing forwarded bytes.
- [x] Client cancellation and interrupted SSE cannot create false completion.
- [x] Pre-attempt storage failure returns 503 and makes zero upstream requests.
- [x] Provider setup/connect failures produce normalized gateway failure and a
      durable typed terminal event.
- [x] Shutdown stops admission, gives in-flight work a bounded 10-second grace,
      force-cancels remaining streams, drains terminal persistence, and closes
      SQLite.

## Storage, privacy, and lifecycle gates

- [x] SQLite event history is append-only, migration-backed, integrity-checked,
      WAL-enabled, and survives reopen.
- [x] WAL checkpoint maintenance runs after write idleness instead of in request
      commits; shutdown still closes/checkpoints the pool.
- [x] Prompt, response, source, repository path, authorization, cookie, account
      header, and environment sentinels are absent from logs and SQLite/WAL/SHM.
- [x] Structured telemetry contains only approved operational metadata.
- [x] `init` is plan-then-apply, preserves unrelated TOML, and does not change
      the default Codex profile.
- [x] `uninstall` verifies hashes, refuses post-install edits, restores exact
      prior existence/content state, stops the daemon, and preserves history.
- [x] Final live run reported `CONFIG_RESTORED=True`,
      `PROFILE_RESTORED=True`, and uninstall exit 0.

## Performance and engineering gates

- [x] Final release benchmark used 50 warmups and 1,000 paired direct/proxy
      samples with first-byte forwarding asserted for every response.
- [x] Added first-byte latency is p50 0.249 ms / p95 1.277 ms / p99 11.915 ms.
- [x] Added total latency is p50 0.320 ms / p95 1.749 ms / p99 15.393 ms.
- [x] Both distributions satisfy the 10 / 25 / 50 ms p50/p95/p99 M0 gate.
- [x] `cargo fmt`, Clippy with `-D warnings`, debug tests, release tests,
      workspace check, `cargo deny`, and diff validation pass.
- [x] Dependency audit reports advisories, bans, licenses, and sources `ok`;
      duplicate-version warnings are transitive and non-blocking.

## Scope audit

M0 contains no phase detector, risk classifier, candidate/adaptive routing,
evaluation or promotion engine, Supabase/export path, dashboard, LiteLLM path,
public provider SDK, or multi-harness implementation. Those remain explicitly
deferred beyond the transparent proxy milestone.

Supporting evidence: [contract gates](contract-gates.md),
[performance](performance.md), and [operations](operations.md).
