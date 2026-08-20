# Arbiter M0 Operations

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
