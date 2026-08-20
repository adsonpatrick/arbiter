# Arbiter M0 Operations

## Prerequisites and build

M0 supports Codex ChatGPT login only:

```powershell
codex login status
cargo build --release -p arbiter-cli
```

The login report must identify ChatGPT, not an API key. The built executable is
`target/release/arbiter.exe`.

## Lifecycle

Preview and apply installation, validate it, and start the local daemon:

```powershell
arbiter init codex
arbiter init codex --yes
arbiter doctor
arbiter start
```

`init` does not change the user's default Codex profile. Invoke Arbiter
explicitly:

```powershell
codex exec --profile arbiter "Reply with exactly: m0-ok"
arbiter status
```

`status` reports the fixed Terra/Medium baseline, daemon health, provider TCP
reachability, storage integrity, profile state, and recent attempt counts.

Remove the integration with a preview followed by confirmation:

```powershell
arbiter uninstall
arbiter uninstall --yes
```

Uninstall stops the daemon, verifies backup hashes, restores the exact prior
Codex configuration, and preserves `arbiter.db`. It refuses to overwrite either
Codex file if it changed after installation; resolve that conflict manually
instead of deleting the receipt or backups.

## Authentication boundary

Arbiter M0 requires an existing Codex ChatGPT login. Codex owns login and token refresh. Arbiter does not read Codex credential storage and does not accept an Arbiter-owned API key.

For each active localhost request, Arbiter forwards the incoming authorization only to `https://chatgpt.com/backend-api/codex/responses`. Redirects are disabled. Authorization, account/workspace headers, cookies, and proxy authorization are never written to logs, events, SQLite, diagnostics, or remote sinks.

## Durable data

M0 stores only append-only attempt metadata:

- event, request, and attempt IDs;
- event timestamp, schema version, type, and attempt index;
- fixed model and reasoning effort;
- start/completion/failure timestamps and duration;
- input, cached-input, output, and reasoning token counts;
- upstream response ID when a terminal completion provides one;
- typed failure class.

M0 does not support persistence of prompts, source code, request bodies, response bodies, tool content, authorization, cookies, environment values, or repository paths. SQLite and its WAL/SHM companions must be treated as operational metadata, but they contain none of those content or secret fields by design.

## Structured logs

The daemon writes JSON logs to stderr. `RUST_LOG` controls filtering; when it is absent or invalid, the default is `arbiter_daemon=info`.

Allowed fields are event type, request/attempt IDs, model, reasoning effort, HTTP status, duration, typed error class, and token counts. Request/response bodies and incoming headers are never attached to spans or events.

Example:

```powershell
$env:RUST_LOG = 'arbiter_daemon=info'
arbiter-daemon --database C:\path\to\arbiter.db --port 43123
```

The daemon binds to `127.0.0.1` regardless of host configuration. M0 exposes only `POST /v1/responses`, `GET /healthz`, and `GET /status`.

Codex may make a non-fatal discovery request to `/v1/models`; M0 intentionally
returns 404 because model listing is outside its strict inference surface. The
managed profile already pins `gpt-5.6-terra` with medium reasoning.

## Shutdown and recovery

Ctrl+C and SIGTERM (on Unix), as well as the uninstall stop marker, immediately
stop admission. Active streams receive up to 10 seconds to finish. A stream
still running after the grace period is cancelled, recorded as failed, and all
pending terminal persistence is drained before the SQLite pool closes.

Default data is under `%USERPROFILE%\.arbiter` on Windows or `$HOME/.arbiter`
elsewhere. `ARBITER_HOME` can select another directory. Codex files are under
`$CODEX_HOME`, or the user's `.codex` directory when that variable is unset.

If startup fails, run `arbiter doctor`, then inspect `arbiter status`. Do not
manually copy authorization data into Arbiter configuration. For an abandoned
installation, use `arbiter uninstall --yes`; verified backups under the Arbiter
home are the authoritative rollback source.
