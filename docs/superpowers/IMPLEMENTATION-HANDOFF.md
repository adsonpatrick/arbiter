# Arbiter M0 Dogfooding Readiness — Implementation Handoff

This file is the entry point for implementing the approved M0 remediation in VSCode.

## Canonical documents

Read these in order before changing code:

1. `docs/superpowers/specs/2026-08-20-m0-dogfooding-readiness-remediation-design.md`
   - approved design/specification;
   - defines scope, threat model, accepted residual risk, 32 MiB request contract, control boundary, lifecycle semantics, CI requirements, and acceptance criteria.

2. `docs/superpowers/plans/2026-08-20-m0-dogfooding-readiness-remediation.md`
   - executable TDD implementation plan;
   - contains exact files, interfaces, tests, verification commands, commit boundaries, and final release gates.

The original M0 design remains relevant background:

- `docs/superpowers/specs/2026-08-19-codex-compute-governor-design.md`

Do not implement M1 while this remediation plan is open.

## Implementation order

Follow the plan strictly in task order. Each task must leave the workspace buildable and testable before moving to the next task.

High-level sequence:

1. explicit 32 MiB request contract;
2. authenticated CLI↔daemon control plane while preserving the current lifecycle;
3. compensating Codex endpoint + receipt transition;
4. schema-v2 installation state and bind-first ephemeral endpoint lifecycle;
5. Linux/Windows GitHub CI and pinned release tooling;
6. fresh live contracts, privacy checks, benchmark, Engineering Guardrails, Security review, and NeuroVia dogfooding release gate.

## Development workflow

Use an isolated branch/worktree for implementation. Apply TDD as written in the plan:

```text
failing test → verify failure → minimal implementation → verify pass → regression checks → commit
```

Do not weaken existing privacy, security, streaming, cancellation, durability, restoration, or latency assertions to make a task pass.

## Core invariants that must remain true

- fixed M0 target: `gpt-5.6-terra` / `medium`;
- Codex remains owner of ChatGPT authentication;
- Arbiter never reads Codex credential storage;
- request/response content, source code, prompts, tool data and credentials are never persisted or logged;
- `AttemptStarted` is durable before upstream inference;
- pre-attempt HTTP rejection creates no attempt and no upstream call;
- at most one durable terminal event per attempt;
- upstream remains pinned and redirects remain disabled;
- response streaming/cancellation semantics remain transparent;
- uninstall restores the exact pre-Arbiter Codex state or refuses on conflict;
- `arbiter.db` is preserved by uninstall;
- no adaptive routing, budgets, evals, promotion, Supabase, LiteLLM, dashboards or M1 concepts enter this remediation.

## Local verification baseline

Run the commands required by each task. Before considering the implementation complete, the final deterministic suite must include:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features --release
cargo deny check
git diff --check
```

The final release gate additionally requires:

- green GitHub Actions on Linux and Windows for the exact final SHA;
- manual authenticated Codex live contract tests;
- 1,000-pair proxy performance benchmark within existing M0 SLOs;
- privacy sentinel checks;
- fresh independent Engineering Guardrails verification;
- fresh Codex Security review with no blocking finding inside the approved M0 threat model.

## Dogfooding decision

Do not classify Arbiter as ready for daily NeuroVia use until the final release checklist states:

```text
PASS — M0 dogfooding-readiness remediation gate is closed.
GO FOR DAILY NEUROVIA DOGFOODING under the documented personal-workstation threat model.
```

This approval does not mean Arbiter is M1-complete or hardened for hostile multi-user enterprise hosts.
