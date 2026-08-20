# M0 Codex contract gates

These live gates use the Codex-managed ChatGPT login. Arbiter never requests an
API key, and the probes explicitly remove `OPENAI_API_KEY` from child processes.
The fixed prompts are synthetic and contain no repository content.

The current Codex contract uses `$CODEX_HOME/arbiter.config.toml` for the named
`arbiter` profile. Codex 0.134.0 and later no longer load
`[profiles.arbiter]` from `config.toml`.

## Automated live run

1. Confirm `codex login status` reports ChatGPT authentication. API-key login is
   unsupported for M0.
2. Set `ARBITER_HOME` to a fresh temporary directory while leaving `CODEX_HOME`
   at the authenticated user location.
3. Run `arbiter init codex --yes`, then `arbiter doctor`.
4. Run the ignored contract suite serially:

   `cargo test -p arbiter-cli --test contract -- --ignored --test-threads=1`

5. Always run `arbiter uninstall --yes` with the same `ARBITER_HOME`. It stops
   the daemon, restores both Codex files from verified backups, and preserves
   the event database.

The suite checks direct Terra/Medium JSONL streaming, Codex-through-Arbiter
terminal behavior, usage extraction, metadata-only persistence, and cancellation
without a false completion. One Codex command is not assumed to equal one model
call because Codex is agentic.

## Evidence record

| Field | Result |
| --- | --- |
| Date | 2026-08-20 |
| Arbiter | 0.1.0 |
| Codex CLI | 0.148.0-alpha.21 |
| Authentication | ChatGPT login; no Arbiter-owned API key |
| Target | gpt-5.6-terra / medium |
| Wire contract | Responses streaming via `/v1/responses` |
| Provider retries | request 0; stream 0 |
| Direct live probe | PASS — exit 0, streamed JSONL, expected synthetic terminal behavior |
| Through-Arbiter live probe | PASS — exit 0, `/v1/responses`, streamed terminal behavior |
| Usage extraction | PASS — nonzero input/output usage in durable completion metadata |
| Cancellation | PASS — started attempt has no false completion after client termination |
| Privacy scan | PASS — synthetic marker absent from SQLite, WAL, and SHM |
| Restoration | PASS — original config hash restored, named profile removed, daemon stopped |

Do not record authorization values, account headers, prompts, responses, or
source code in this document.
