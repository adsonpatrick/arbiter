# Arbiter M0 Code Review Remediation Design

**Date:** 2026-08-20  
**Status:** Approved design, pending implementation  
**Review range:** `16007feaf282b8de0599b1db6209da06943577fb..d79c4556fd78bd714b90eb584fadfda912e78d05`

## 1. Objective

Close every validated finding from the Superpowers review of the M0 transparent
proxy without expanding into M1. The corrected release must preserve Codex-managed
ChatGPT authentication, the fixed Terra/Medium baseline, transparent Responses
streaming, privacy-minimized local events, and exact safe uninstall.

The PR remains draft until the implementation, full verification, live gates,
performance benchmark, and a second independent Superpowers review pass.

## 2. Security boundary and threat model

Arbiter protects against stale daemon metadata, accidental port conflicts, and
unrelated local processes. A malicious process running as the same OS account is
outside the M0 threat model: it can already read that account's Codex configuration,
and HTTP loopback cannot fully isolate it without a Codex-supported authenticated
transport.

Each installation receives:

- a cryptographically random `instance_id`;
- an OS-selected ephemeral port by default, while explicit `--port` remains
  supported;
- server metadata containing PID, port, version, and `instance_id`;
- a health identity response whose PID, port, version, and instance ID must match
  the local receipt exactly.

`start`, `status`, `doctor`, and `uninstall` must reject a generic 2xx health
response or any identity mismatch as an unrecognized process. Arbiter-owned
configuration, receipts, backups, hashes, and metadata must be owner-only. On
platforms where Unix modes apply, directories use at most `0700` and sensitive
files at most `0600`. Replacement and restoration preserve any stricter original
permissions. Windows uses inherited owner-restricted ACLs and must not deliberately
broaden an existing ACL.

## 3. Codex profile lifecycle

A pre-existing `$CODEX_HOME/arbiter.config.toml` is supported after explicit
`init --yes`. Arbiter backs up its exact bytes, existence state, hash, and relevant
permissions, temporarily installs the managed named layer, and restores the prior
state on uninstall. A pre-existing `model_providers.arbiter` entry remains a hard
collision because merging or replacing that shared provider table would be
ambiguous.

Two-file install and uninstall use a staged protocol with compensating rollback:

1. validate both current states and all backups before changing either target;
2. prepare synced same-filesystem temporary replacements;
3. apply the first target, then the second;
4. if the second operation fails, restore the first target to its exact pre-operation
   state;
5. report a compound error if compensation itself fails, retaining enough evidence
   for manual recovery.

Tests inject failure into the second step of both install and uninstall. Success
requires either the complete new state or the complete previous state, never a
provider/profile mixture.

## 4. Attempt lifecycle and durable recovery

Exactly one terminal event is a storage invariant. A new migration adds a SQLite
trigger that rejects a second `attempt_completed` or `attempt_failed` row for an
attempt. A trigger is used instead of a unique-index migration so an existing
database containing historical duplicates is preserved rather than becoming
unmigratable.

The governed stream also enforces the invariant before storage:

- terminal metadata schedules completion once;
- a later transport error is still returned to the client but does not schedule
  failure after completion;
- cancellation or EOF schedules failure only when no terminal is known;
- a non-success upstream HTTP status schedules `ProviderHttp` once while its
  original status, safe headers, and body remain transparent.

Terminal writes are tracked and retried a small bounded number of times with
explicit privacy-safe error logging. The in-memory terminal flag prevents duplicate
scheduling, while the SQLite trigger prevents duplicate durable facts. A permanent
write failure intentionally leaves only `AttemptStarted` rather than claiming a
terminal fact that was not stored.

Daemon startup, before listening, runs transactional reconciliation. Every started
attempt with no terminal receives `AttemptFailed(StreamInterrupted)`. Read-only CLI
operations never run reconciliation because they may inspect a live daemon's active
attempts. Recovery is idempotent through the terminal trigger and transaction.

## 5. Streaming and memory bounds

Response bytes remain pass-through and are never buffered as a whole. The SSE
metadata parser has a fixed upper bound for its pending event buffer. If the bound
is exceeded, it clears pending content, permanently disables metadata extraction
for that response, and continues forwarding every byte unchanged. Without verified
terminal metadata, a success-status stream ends as `StreamInterrupted`; it must not
guess completion.

Authorization and account-bound copied `HeaderValue` instances are marked sensitive
before the outbound request is built. This is defense in depth against future Debug
or middleware logging and does not change their wire values.

## 6. Shutdown deadline

Shutdown uses one monotonic deadline, defaulting to ten seconds. Admission stops
immediately. Two seconds of that deadline are reserved for forced cancellation,
terminal persistence, and pool close; graceful HTTP drain may use only the time
before that reserve. The same remaining budget is shared by:

1. graceful HTTP drain;
2. forced cancellation of streams still active at the cleanup-reserve boundary;
3. tracked terminal persistence;
4. SQLite pool close.

No stage receives a fresh ten-second allowance. At the reserve boundary, remaining
streams are force-cancelled so their failure events can use the reserved time.
Cleanup attempts with no remaining budget are non-blocking and emit privacy-safe
diagnostics when incomplete. A forced stream cancellation records
`AttemptFailed(Cancelled)` when persistence is available.

## 7. Required regression evidence

Focused red-green-refactor tests cover:

- stale metadata and spoofed 2xx health identity;
- default random port and explicit port compatibility;
- completion followed by transport failure;
- storage rejection of a second terminal;
- terminal append failure followed by restart reconciliation;
- transparent 401, 429, and 500 classification as `ProviderHttp`;
- pre-existing named profile replacement and exact restoration;
- second-file install and uninstall failures with compensation;
- owner-only files/directories where platform APIs expose permissions;
- oversized SSE input with bounded parser memory and unchanged forwarding;
- sensitive header flags without changed header values;
- one process-wide shutdown deadline;
- live cancellation yielding exactly one started event and one cancelled failure.

Final verification must include:

- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
- debug and release workspace tests;
- `cargo check --workspace --all-targets --all-features`;
- `cargo deny check`;
- the ignored direct and through-Arbiter live contract suite;
- manual init/doctor/start/`m0-ok`/status/uninstall with exact restoration;
- the 1,000-pair release proxy benchmark and existing M0 SLO;
- a second independent Superpowers code review.

## 8. Scope boundary

This remediation does not add adaptive routing, phase/risk classification,
evaluation, promotion, Supabase/export, dashboards, LiteLLM, multi-harness support,
or a public provider SDK. It changes only security, durability, lifecycle,
classification, memory safety, tests, and evidence required to make the existing
M0 claims executable.
