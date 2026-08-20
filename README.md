# Arbiter

[![CI](https://github.com/adsonpatrick/arbiter/actions/workflows/ci.yml/badge.svg)](https://github.com/adsonpatrick/arbiter/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Arbiter is a local compute governor for Codex. The M0 release is a transparent
localhost proxy that preserves Codex streaming while recording only the
operational metadata needed for later governance.

Arbiter does not own an OpenAI API key. Codex keeps responsibility for ChatGPT
login and token refresh, and Arbiter forwards authentication only in memory for
the active request.

## M0 behavior

- Routes the named Codex profile through a loopback-only Responses proxy.
- Fixes the current baseline to `gpt-5.6-terra` with medium reasoning.
- Streams upstream bytes without waiting for completion.
- Persists append-only attempt metadata in SQLite, never prompts or responses.
- Restores the exact previous Codex configuration during safe uninstall.
- Supports Windows and Unix permission models.

Adaptive routing, evaluation and promotion, dashboards, remote persistence,
and additional coding harnesses are intentionally deferred.

## Workspace

| Package | Responsibility |
| --- | --- |
| `arbiter-core` | Domain configuration, IDs, and governor events |
| `arbiter-cli` | Install, lifecycle, diagnostics, status, and uninstall commands |
| `arbiter-daemon` | Loopback HTTP server, streaming proxy, shutdown, and recovery |
| `arbiter-provider-codex` | Fixed Codex Responses upstream adapter and SSE metadata parsing |
| `arbiter-adapter-codex` | Safe Codex profile installation and restoration |
| `arbiter-storage-sqlite` | Durable append-only metadata storage and migrations |
| `arbiter-m0-bench` | Deterministic local proxy-overhead benchmark |

## Prerequisites

- Stable Rust with Cargo, rustfmt, and Clippy.
- A recent Codex CLI authenticated through ChatGPT.
- No `OPENAI_API_KEY` is required or supported by Arbiter M0.

Confirm the Codex login and build the CLI:

```powershell
codex login status
cargo build --release -p arbiter-cli
```

## Quick start

Preview installation before applying it:

```powershell
.\target\release\arbiter.exe init codex
.\target\release\arbiter.exe init codex --yes
.\target\release\arbiter.exe doctor
.\target\release\arbiter.exe start
```

On Unix, use `./target/release/arbiter` in place of
`.\target\release\arbiter.exe`.

Arbiter installs a named profile and does not change the default Codex profile:

```powershell
codex exec --profile arbiter "Reply with exactly: m0-ok"
.\target\release\arbiter.exe status
```

Remove the integration safely when finished:

```powershell
.\target\release\arbiter.exe uninstall
.\target\release\arbiter.exe uninstall --yes
```

Uninstall restores verified Codex configuration backups and preserves the
local event history.

## Development checks

```powershell
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
cargo deny check
```

The two live contract tests are ignored by default because they require a local
Codex ChatGPT login. See the contract-gate document before running them.

## Security and privacy

The daemon binds only to `127.0.0.1`, accepts only the strict M0 inference
surface, validates daemon identity, disables provider redirects and retries,
and never persists authorization, account headers, prompts, responses, source
code, environment values, or repository paths.

See [M0 operations](docs/m0/operations.md) for the complete lifecycle and
recovery guidance.

## Documentation

- [Architecture design](docs/superpowers/specs/2026-08-19-codex-compute-governor-design.md)
- [M0 implementation plan](docs/superpowers/plans/2026-08-19-arbiter-m0-transparent-proxy.md)
- [Operations](docs/m0/operations.md)
- [Contract gates](docs/m0/contract-gates.md)
- [Performance evidence](docs/m0/performance.md)
- [Release checklist](docs/m0/release-checklist.md)

## License

Licensed under the [Apache License 2.0](LICENSE).
