# Arbiter M0 Dogfooding Readiness Remediation Design

**Date:** 2026-08-20  
**Status:** Approved design, pending implementation  
**Target:** `adsonpatrick/arbiter` M0 (`0.1.0`)  
**Baseline commit at review:** `dfaa349f0a70a2dc7a471b443380404e4b609fbc`

## 1. Objective

Close the material findings identified by the post-M0 Engineering Guardrails,
security, and dependency-contract review before Arbiter becomes the default Codex
path for daily NeuroVia dogfooding.

This remediation remains strictly inside M0. It does not add adaptive routing,
phase/risk classification, evaluation, promotion, additional providers, remote
telemetry, or M1 runtime concepts.

The release goal is:

> **Arbiter M0 can be used as a daily Codex passthrough on a personal developer
> workstation with an explicit request-size contract, safer localhost lifecycle,
> reproducible CI evidence, and no regression to streaming, privacy, durability,
> restoration, or performance.**

The remediation closes three findings:

1. the Axum JSON request path inherits an implicit body-size limit that is smaller
   than the intended transparent-proxy contract;
2. the localhost daemon identity/lifecycle can be hardened further against stale
   or impersonated endpoints without claiming cryptographic server authentication
   that Codex does not currently provide;
3. release checks recorded manually in the M0 PR are not reproduced automatically
   by GitHub Actions.

## 2. Scope and non-goals

### 2.1 In scope

- explicit 32 MiB maximum request size for `POST /v1/responses`;
- deterministic behavior for accepted, malformed, and oversized request bodies;
- regression coverage above Axum's default body-size threshold;
- ephemeral daemon port allocation on every daemon start;
- profile publication only after listener and database lifecycle ownership exist;
- reduction of identity disclosure on the public health surface;
- a protected local control surface used by Arbiter CLI lifecycle commands;
- exact startup, shutdown, restart, crash-recovery, and uninstall semantics;
- GitHub Actions CI for deterministic Linux and Windows validation;
- release documentation separating CI, live-contract, and performance gates;
- final independent engineering and security review.

### 2.2 Explicitly out of scope

- M1 Session / Turn / Step concepts;
- adaptive routing;
- budget policy;
- evaluation, shadow execution, candidate policies, promotion, or rollback;
- Supabase or any remote sink;
- LiteLLM or non-Codex harnesses;
- local HTTPS with a private CA;
- OS trust-store management;
- Unix domain sockets or named pipes as the Codex inference transport;
- enterprise multi-user endpoint authentication;
- changing the fixed `gpt-5.6-terra` / medium M0 baseline;
- adding OpenAI or ChatGPT credentials to CI.

## 3. Architectural constraints retained from M0

This remediation must preserve the existing M0 invariants:

1. Arbiter is local-first and binds inference traffic only on IPv4 loopback.
2. Codex owns ChatGPT authentication and refresh.
3. Arbiter never reads Codex credential storage.
4. Authorization is forwarded only in memory to the pinned first-party Codex
   Responses upstream.
5. Upstream redirects remain disabled.
6. Request and response content are never persisted or logged.
7. One physical provider invocation maps to one immutable attempt.
8. `AttemptStarted` is durable before an upstream inference request is made.
9. At most one terminal event is durable for an attempt.
10. Provider failures do not trigger semantic retries, model escalation, or fallback.
11. Responses remain byte-streamed to Codex.
12. Uninstall restores the exact prior Codex configuration/profile state or refuses
    on a verified conflict.
13. Arbiter remains a transparent fixed-target passthrough, not a router.

## 4. Finding A — explicit request-size contract

### 4.1 Problem

The current `POST /v1/responses` handler uses Axum's `Json<Value>` extractor.
Axum applies a default body limit to extractors that consume the request body.
Because the M0 router does not override that limit, sufficiently large Codex
Responses requests can be rejected before reaching Arbiter's provider path.

This is incompatible with an unspecified "transparent" request contract: a request
that Codex and the upstream would otherwise accept can fail locally because of a
framework default that Arbiter never declared.

### 4.2 Contract

M0 adopts an explicit request-size ceiling:

```text
0 bytes .. 32 MiB     accepted by the body-size gate
> 32 MiB              HTTP 413 Payload Too Large
malformed JSON        HTTP 400 Bad Request
```

The exact byte constant is:

```text
32 * 1024 * 1024 = 33,554,432 bytes
```

The value must be defined once in a stable M0-owned constant and reused by the
router and tests.

### 4.3 Runtime behavior

The route must apply an explicit Axum body limit compatible with Axum 0.8, such as
`DefaultBodyLimit::max(M0_MAX_REQUEST_BYTES)` or an equivalent layer with the same
observable behavior.

The request continues to materialize as `serde_json::Value`. M0 does **not** change
to incremental request-JSON parsing in this remediation because it must rewrite the
top-level `model` and `reasoning.effort` fields before forwarding.

Accepted bodies retain existing behavior:

1. body-size gate accepts the request;
2. JSON extraction succeeds;
3. admission opens;
4. Arbiter persists `AttemptStarted`;
5. request target is normalized to Terra/Medium;
6. exactly one upstream call is made;
7. upstream response streams back unchanged except for the already-allowlisted
   response headers and Arbiter attempt header;
8. terminal evidence is recorded as before.

Oversized or malformed bodies are rejected **before attempt creation**. They must
not:

- create `AttemptStarted`;
- allocate an `AttemptId`;
- invoke the provider;
- persist request content;
- produce misleading provider-failure telemetry.

HTTP-rejection telemetry may be added only if it contains approved operational
metadata and no request body or headers. Such telemetry is not required to close
this remediation.

### 4.4 Memory posture

The 32 MiB contract intentionally avoids an unlimited `Json<Value>` path. Arbiter
may temporarily use more memory than the wire body due to JSON parsing and object
representation, but the wire body itself has a hard bounded admission size.

Streaming the request body while selectively rewriting JSON is deferred. Response
streaming remains unchanged.

### 4.5 Required tests

Tests must prove:

- a request larger than 2 MiB but smaller than 32 MiB reaches a loopback fake
  upstream;
- its non-governed fields remain semantically intact after normalization;
- the model and reasoning effort are still normalized;
- a syntactically valid JSON request whose total HTTP body is exactly 32 MiB is accepted;
- a request one byte over the maximum receives `413`;
- an oversized request produces zero upstream calls;
- an oversized request produces zero attempt lifecycle rows;
- malformed JSON receives `400` and produces zero upstream calls/attempts;
- existing streaming and cancellation tests remain green.

## 5. Finding B — localhost lifecycle and control-boundary hardening

### 5.1 Threat model

The current M0 already protects against:

- stale daemon metadata;
- accidental port conflicts;
- unrelated processes that merely return a generic `2xx`;
- accidental credential persistence;
- redirect-based credential forwarding;
- broad filesystem access to Arbiter-owned state.

The post-review threat is narrower:

> A different local process might attempt to impersonate the expected HTTP
> endpoint after learning or racing a previously used port.

A process already executing as the same OS account remains outside the M0 isolation
boundary because it can generally read that account's Codex configuration and
credentials. The remediation improves multi-process and multi-user workstation
hardening but does not claim cryptographic endpoint authentication.

### 5.2 Rejected design: static secret request header as server authentication

Codex custom providers support additional static or environment-derived HTTP
headers. That capability is **not** sufficient to authenticate the Arbiter server
to Codex.

If a malicious process successfully owns the configured port, the first Codex
request would send both the provider's extra header and the Codex authorization
material to that process. Therefore a static `X-Arbiter-Token` on inference
requests must not be described as server authentication and is not part of the
solution.

### 5.3 Rejected design: local HTTPS with Arbiter-owned CA in M0

Local HTTPS with certificate pinning/trust management could materially strengthen
server authentication, but doing it correctly requires cross-platform certificate
generation, installation, trust-store lifecycle, revocation/rollback, and Codex
client compatibility.

That complexity is disproportionate to this M0 dogfooding remediation and introduces
a new privileged configuration surface. It is deferred to a future enterprise
hardening track unless Codex gains a simpler authenticated local transport.

### 5.4 Selected design: bind-first ephemeral endpoint publication

Every daemon start receives a fresh ephemeral loopback port.

The lifecycle becomes:

```text
arbiter start
    |
    +-- acquire process/lifecycle intent
    |
    +-- bind 127.0.0.1:0
    |       |
    |       +-- OS assigns fresh port
    |
    +-- acquire exclusive database lease
    |
    +-- open/reconcile SQLite
    |
    +-- generate a fresh daemon instance identity
    |
    +-- atomically publish current endpoint into managed Codex profile
    |
    +-- write exact server metadata
    |
    +-- become READY and admit inference
```

The managed Codex provider must never point at a port that the new Arbiter process
has not already bound.

The currently persisted port stops being an installation-time authority. It becomes
runtime endpoint metadata.

### 5.5 Configuration model

Installation remains responsible for:

- creating Arbiter-owned state;
- backing up the original Codex files;
- installing/owning the managed Arbiter provider/profile contract;
- recording restoration receipts;
- generating the local control credential.

`config.json` advances to schema version 2 and contains installation invariants,
not active daemon identity:

- mode = `passthrough`;
- baseline = Terra/Medium;
- bind address = `127.0.0.1`;
- remote export = false;
- Codex configuration path.

The active `port` and daemon `instance_id` are removed from installation
configuration. They belong only to runtime `server.json` and the authenticated
control response. A fresh `instance_id` is generated on every successful daemon
start.

Immediately after `init`, the managed provider uses this explicit inactive endpoint:

```text
http://127.0.0.1:0/v1
```

Port zero is never used for inference. The implementation must include a contract
test proving that the Codex configuration accepts the URL syntax and that no
successful TCP connection can be made through the inactive endpoint on supported
platforms.

Starting the daemon becomes responsible for publishing the **current** provider
endpoint only after binding `127.0.0.1:0` and learning the actual listener port.

Because the current installation receipt verifies the whole installed Codex file,
runtime endpoint publication must update the managed config and its receipt as one
compensating state transition:

1. verify the current config/profile and receipt are mutually consistent;
2. stage the new Codex config containing only the new Arbiter `base_url`;
3. stage the receipt with the corresponding new installed hash;
4. persist the Codex config atomically;
5. persist the updated receipt atomically;
6. if step 5 fails, restore the exact previously installed Codex config;
7. if compensation fails, report a compound recovery error and do not mark the
   daemon READY.

The original pre-Arbiter backup/hash never changes. Therefore uninstall always has
the same restoration baseline even though Arbiter's managed `base_url` evolves at
runtime.

No runtime publication may change unrelated Codex keys or the managed model,
provider ID, retry settings, auth mode, or reasoning effort.

### 5.6 Public health surface

`GET /healthz` remains available for basic liveness and storage health, but it must
not expose the complete reusable daemon identity to arbitrary local callers.

Public health may expose:

- overall health;
- component health;
- version only if needed for diagnostics.

It must not expose, as a bundle:

- PID;
- active port;
- private/random instance identifier;
- local control secret.

### 5.7 Protected local control surface

CLI lifecycle commands need a stronger identity check than public liveness.

Arbiter therefore introduces a local control credential stored in an owner-only
Arbiter file. The control credential authenticates **Arbiter CLI to Arbiter daemon**
for management operations. It is not forwarded to Codex or ChatGPT.

The daemon exposes exactly one M0 management identity route:

```text
GET /control/identity
X-Arbiter-Control-Token: <credential>
```

The credential is a 32-byte cryptographically random value encoded as unpadded
base64url and stored at:

```text
$ARBITER_HOME/control-token
```

The control endpoint returns the exact daemon identity required by CLI lifecycle
validation:

- PID;
- active port;
- Arbiter version;
- instance ID.

Control requirements:

1. the credential is generated during `arbiter init codex --yes` using an OS-backed
   cryptographically secure random source;
2. the credential file is created owner-only using the same cross-platform private
   file primitives as other Arbiter secrets;
3. the daemon reads the credential from Arbiter-owned state at startup and does not
   copy it into public health state, server metadata, SQLite, or Codex configuration;
4. the header value is marked sensitive before any structured HTTP instrumentation
   can observe it;
5. the credential never appears in logs;
6. the credential is never forwarded upstream;
7. missing or incorrect credentials return `404 Not Found`, the same result used
   for an unavailable control resource, and include no daemon identity;
8. CLI uses `GET /control/identity` for `start`, `status`, `doctor`, and `uninstall`
   identity validation;
9. public `/healthz` alone is insufficient to establish daemon ownership.

This protects management decisions from a generic local listener while avoiding the
false claim that Codex inference requests themselves have cryptographic server
authentication.

### 5.8 Shutdown, crash, restart, and uninstall

#### Normal shutdown

- stop admission;
- while the listener is still owned, atomically republish the managed provider to
  the inactive `127.0.0.1:0` endpoint and update the current installed hash in the
  receipt using the same compensating mutation protocol;
- complete existing bounded shutdown behavior;
- persist/cancel attempts according to existing rules;
- remove active server metadata only if identity ownership still matches;
- retain the immutable original backups/restoration hashes.

If endpoint depublication fails, shutdown must emit a privacy-safe high-severity
diagnostic and continue bounded process cleanup; it must never forge a successful
depublication record. The residual stale-endpoint risk is then visible to
`doctor`/`status` and remains within the explicitly accepted local-host residual.

No command may treat a stale profile port as proof of a running daemon.

#### Restart

A restart must:

- obtain a different OS-selected ephemeral port in normal operation;
- bind it before profile publication;
- update the provider endpoint and current receipt hash through the compensating
  publication transition;
- preserve unrelated Codex configuration;
- keep the same exact immutable pre-Arbiter uninstall restoration baseline.

Tests must not require mathematical uniqueness of ephemeral ports across all OS
schedules, but must demonstrate that startup requests port `0` and does not reuse a
persisted fixed port as authority.

#### Crash

After an unclean crash:

- the previous listener disappears;
- startup acquires a new listener and database lease;
- incomplete attempts are reconciled only after ownership is acquired;
- the new endpoint is published atomically;
- stale server metadata cannot authorize management of another process.

#### Uninstall

Uninstall must:

- authenticate/validate any currently running Arbiter control identity;
- stop only the owned daemon;
- restore the exact pre-Arbiter Codex files from verified backups;
- preserve event history;
- refuse to overwrite user-modified managed files where existing conflict rules
  require refusal.

### 5.9 Residual risk

The remediation explicitly records this residual:

> Codex currently reaches custom model providers over ordinary HTTP(S) base URLs and
> does not provide Arbiter with a first-class local server-pinning mechanism,
> Unix-domain-socket provider transport, named-pipe transport, or an Arbiter-specific
> mutual-authentication handshake.

Therefore M0 does not claim cryptographic protection against every hostile local
process capable of observing and racing the active inference port.

For personal workstation dogfooding this residual is accepted.

A future enterprise-hardening milestone must revisit this boundary before Arbiter is
marketed as strongly isolated on hostile multi-user hosts.

## 6. Finding C — reproducible GitHub CI

### 6.1 Problem

The M0 PR contains strong recorded verification evidence, including formatting,
Clippy, workspace tests, `cargo deny`, live Codex contract probes, and a performance
benchmark. The repository, however, does not currently reproduce its deterministic
checks automatically for every pull request and main-branch change.

Engineering Guardrails cannot treat historical/manual evidence as equivalent to a
fresh automated release signal.

### 6.2 CI workflow

Add one GitHub Actions workflow:

```text
.github/workflows/ci.yml
```

Triggers:

- pull requests targeting `main`;
- pushes to `main`.

No OpenAI or ChatGPT secret is configured.

### 6.3 Linux job

The Linux job must run, with the repository's locked dependency graph/toolchain:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo deny check
```

If `cargo-deny` installation is required, the workflow must pin an explicit tool
version rather than fetching an unbounded latest release.

### 6.4 Windows job

The Windows job must run:

```powershell
cargo check --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features
```

This job is required because Arbiter contains platform-specific ACL and file
lifecycle behavior that Linux cannot validate.

### 6.5 CI security

Workflow permissions must follow least privilege.

Default intent:

```yaml
permissions:
  contents: read
```

The workflow must not:

- receive repository write credentials beyond the default read token requirement;
- store ChatGPT login state;
- use an OpenAI API key;
- use arbitrary third-party actions without pinning/version review;
- upload Arbiter databases, Codex configs, or local credentials as artifacts.

### 6.6 Release gates remain separate

M0 release evidence is split into three named classes.

#### CI Gate — automatic, deterministic

Required on every PR/main change:

- fmt;
- check;
- Clippy;
- non-live tests;
- dependency policy/audit;
- Windows platform tests.

#### Live Contract Gate — manual before release

Still requires an authenticated developer environment:

- direct Codex contract probe;
- Codex-through-Arbiter probe;
- cancellation behavior;
- usage extraction;
- privacy sentinel checks;
- exact configuration restoration.

These tests remain ignored by default and must never be enabled in public CI using
a persisted ChatGPT credential.

#### Performance Gate — manual before release

Run the existing release benchmark with its current statistically meaningful sample
count and compare against the M0 local-overhead SLO.

The benchmark is not required on every PR to avoid noisy CI and unnecessary runtime.

## 7. Data and privacy impact

The remediation must not expand data collection.

Allowed durable inference metadata remains the existing M0 event set:

- event/request/attempt identifiers;
- timestamps and durations;
- attempt index;
- fixed compute target;
- token usage;
- upstream response ID;
- typed failure class.

The following remain prohibited from logs and storage:

- prompts;
- source code;
- request bodies;
- response bodies;
- tool content;
- authorization values;
- account/workspace secrets;
- cookies;
- environment secrets;
- repository paths.

New control credentials are classified as `SECRET` and must never enter the event
schema.

Request-size rejections must not store the rejected body.

## 8. Error behavior

The remediation must preserve explicit classification boundaries.

| Condition | HTTP / CLI behavior | Attempt created? | Upstream call? |
| --- | --- | ---: | ---: |
| request > 32 MiB | HTTP 413 | No | No |
| malformed JSON | HTTP 400 | No | No |
| shutting down | HTTP 503 | No | No |
| durable start write fails | HTTP 503 | Start write attempted, no durable attempt | No |
| missing/invalid Codex auth header | HTTP 502 under existing provider contract | Yes | No successful call |
| upstream connect failure | HTTP 502 | Yes | One attempted call |
| upstream 4xx/5xx | transparent upstream status | Yes | Exactly one |
| stream interruption | stream error + durable typed failure | Yes | Exactly one |
| control credential invalid | non-revealing control denial | N/A | N/A |
| stale profile endpoint while stopped | CLI reports daemon stopped/unrecognized | N/A | N/A |

No new retry layer is introduced.

## 9. Testing strategy

Implementation follows red-green-refactor.

### 9.1 Request contract tests

Add focused tests for:

- >2 MiB accepted;
- 32 MiB boundary;
- >32 MiB rejected;
- malformed JSON rejected;
- zero provider calls before body/JSON acceptance;
- zero events for pre-attempt HTTP rejection.

Use loopback fake providers and synthetic data only.

### 9.2 Lifecycle/control tests

Add tests for:

- start binds `127.0.0.1:0`;
- endpoint publication occurs only after listener ownership;
- failed bind/profile publication cannot leave a profile pointing at an unowned
  endpoint;
- public health omits private daemon identity;
- control identity requires the owner-only credential;
- wrong/missing control credential does not reveal identity;
- `status`, `doctor`, and `uninstall` recognize only the exact authenticated daemon;
- crash recovery obtains ownership before reconciliation;
- restart republishes a listener-derived endpoint;
- uninstall still restores exact original bytes/existence/permissions;
- control secret never appears in logs, SQLite, WAL, SHM, or Codex config.

### 9.3 Existing regression suites

All existing M0 tests remain required, especially:

- provider allowlist/redirect security;
- SSE chunk-boundary parsing;
- cancellation;
- exactly-one terminal;
- SQLite recovery;
- Windows ACL behavior;
- two-file compensating rollback;
- bounded shutdown.

### 9.4 CI validation

The workflow itself is part of the deliverable. A green GitHub Actions run on the
implementation PR is required evidence.

### 9.5 Live validation

After deterministic CI passes:

1. authenticate Codex via ChatGPT login;
2. run direct contract suite;
3. run through-Arbiter contract suite;
4. run a manual `codex exec --profile arbiter` smoke;
5. verify exact uninstall restoration;
6. confirm content/credential sentinels remain absent from persistent state.

### 9.6 Performance validation

Re-run the existing paired direct/proxy release benchmark.

The remediation passes only if it remains within the established M0 added-latency
gates:

```text
p50 < 10 ms
p95 < 25 ms
p99 < 50 ms
```

No relaxation is granted merely because the control lifecycle changed.

## 10. Acceptance criteria

This remediation is complete only when all criteria below have fresh evidence.

1. A request larger than 2 MiB and no larger than 32 MiB reaches the upstream and
   preserves all non-governed fields.
2. A request larger than 32 MiB receives `413 Payload Too Large`.
3. Oversized and malformed requests create zero attempts and zero upstream calls.
4. The 32 MiB limit is a named, documented M0 contract rather than a framework
   default.
5. Daemon startup binds an OS-selected ephemeral loopback port before publishing
   the managed provider endpoint.
6. A stale persisted/profile port is never treated as current daemon authority.
7. The public health endpoint does not disclose the complete reusable daemon
   identity.
8. CLI lifecycle ownership uses a protected local control credential and exact
   daemon identity.
9. The control credential is owner-only, never logged, never persisted as event
   data, never written to Codex config, and never forwarded upstream.
10. Restart/crash recovery cannot reconcile attempts before listener + database
    ownership are obtained.
11. Existing safe profile backup, compensation, conflict detection, and exact
    uninstall restoration remain intact.
12. Linux GitHub Actions run fmt, check, Clippy `-D warnings`, workspace tests, and
    `cargo deny`.
13. Windows GitHub Actions run workspace check and tests, including platform-specific
    ACL coverage.
14. CI uses no OpenAI/ChatGPT credential and follows least-privilege workflow
    permissions.
15. Live direct and through-Arbiter Codex contract tests still pass manually.
16. The release proxy benchmark still satisfies the existing p50/p95/p99 M0 gates.
17. Privacy regression tests show no prompt/source/credential/control-secret content
    in logs or SQLite/WAL/SHM.
18. A fresh independent Engineering Guardrails review has no blocking finding.
19. A fresh independent security review has no blocking finding within the approved
    M0 threat model.
20. The documented residual localhost inference-endpoint risk remains explicit and
    is not misrepresented as cryptographically solved.
21. NeuroVia dogfooding documentation can classify the remediated M0 as suitable for
    daily personal-workstation use, while enterprise hostile-host hardening remains
    deferred.

## 11. Release decision after remediation

When all acceptance criteria pass, Arbiter M0 may move from:

```text
PARTIAL / GO FOR CANARY
```

to:

```text
GO FOR DAILY NEUROVIA DOGFOODING
```

This does not mean M1 is implemented and does not mean Arbiter is enterprise-stable.

The M0 still provides one fixed target:

```text
gpt-5.6-terra / medium
```

Its purpose during NeuroVia dogfooding is to validate real Codex compatibility,
latency, token accounting, failure behavior, cancellation, durability, and operator
lifecycle before M1 begins using that evidence for higher-level governance.
