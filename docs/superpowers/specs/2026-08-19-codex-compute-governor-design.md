# Arbiter — Design Specification

**Status:** Approved v0.2 — Adversarial Self-Review Applied  
**Date:** 2026-08-19  
**Scope:** Codex-first, OpenAI-first, local-first open-source compute governance  
**Document type:** Canonical architectural design specification

## 1. Purpose

Codex workloads contain operations with materially different compute requirements. Repository exploration, mechanical edits, root-cause analysis, implementation, verification, repair, architecture, and security analysis should not automatically consume the same model and reasoning effort.

Arbiter exists to allocate the least compute that can satisfy a verifiable engineering quality and safety floor.

**Core thesis:** spend frontier reasoning only when it changes the engineering outcome.

The optimization problem is to minimize engineering cost and latency subject to workload-specific quality constraints, security/risk constraints, and explicit budget ceilings.

The unit of optimization is `ComputeTarget = (model, reasoning_effort)`.

North-star technical metric:

`Verified Engineering Success / Engineering Cost`

**Production Engineering Cost** includes production inference, repair, required verification, and attributable production CI cost. **Learning Cost** (BENCH, Execution Shadow, evaluation-only verification) is accounted separately and may be reported as an amortized investment, but MUST NOT be silently folded into per-task production efficiency.

## 2. Product Positioning and Scope

Arbiter is initially:

- Codex-first;
- OpenAI-first;
- local-first;
- open source;
- CLI + local daemon;
- deterministic in the production hot path;
- evidence-driven in policy evolution.

The architecture is extensible to additional harnesses, providers, persistence sinks, and verifiers, but those extension points are not automatically public/stable APIs in v1.

### 2.1 Non-goals for v1

The v1 is not a generic LLM gateway, LiteLLM replacement, universal multi-provider router, Claude Code/OpenCode router, hosted control plane, dashboard product, IDE extension, generic MCP gateway, online-learning autonomous router, public plugin marketplace/stable plugin SDK, or authorization/sandbox system.

LiteLLM is an optional adapter, not a foundation.

## 3. Design Principles

1. **Engineering outcomes over model preference.** Routing quality is judged by verified engineering results.
2. **Quality floor before optimization.** Cost and latency are optimized only among candidates satisfying required constraints.
3. **Routing plane is not authorization plane.** Model selection cannot grant filesystem, shell, network, MCP, Git, sandbox, or approval privileges.
4. **Conservative uncertainty.** Missing or low-confidence context reduces permission to optimize aggressively.
5. **Fail safe.** Non-critical Arbiter failures degrade optimization, not governance. If no safe approved target can be resolved, fail closed.
6. **Evidence before promotion.** New targets and routes progress through controlled evaluation before production.
7. **Explainability.** Every production routing decision must be reconstructible.
8. **Minimal hot path.** No remote database, LLM classifier, embedding lookup, analytics service, or hosted control plane is required for routing.
9. **Local-first privacy.** Source code, prompts, model responses, secrets, and raw artifacts remain local by default.
10. **Human authority over production policy.** Candidate policies cannot self-promote in v1.

## 4. System Architecture

The v1 uses a modular monolith daemon with strict internal boundaries.

```text
Codex
  |
  v
Ingress / Protocol Plane
  |
  v
Governance Hot Path
  |-- SessionState
  |-- StepContext
  |-- PhaseDetector
  |-- RiskClassifier
  |-- RoutingPolicy
  |-- BudgetPolicy
  |-- EscalationPolicy
  |-- ModelRegistry
  |
  v
Inference / Egress Plane
  |-- OpenAIProvider
  `-- LiteLLMProvider (optional)
  |
  v
Codex

Async boundary
  |
  v
Evidence / Evaluation Plane
  |-- EventLog
  |-- EngineeringOutcome
  |-- Verifiers
  |-- ShadowEvaluator
  |-- EvaluationHarness
  |-- CandidatePolicyCompiler
  `-- PromotionPolicy
```

The cold path never blocks the hot path unless a durability-critical governance operation cannot be completed safely.

### 4.1 Core dependency rule

`core` cannot directly depend on Codex, OpenAI, LiteLLM, Supabase, or SQLite implementations. Vendor-specific implementations depend on core contracts. This boundary must be enforced by architectural tests.

### 4.2 Authority and provenance domains

Arbiter MUST NOT collapse all inputs into one linear trust score. Different inputs have authority over different domains.

**Governance authority**, highest to lowest:

1. Constitutional invariants.
2. Emergency controls.
3. ApprovedPolicy and approved ModelRegistry.
4. UserConfig hard ceilings and explicit user overrides within those ceilings.
5. Routing preferences inferred from runtime evidence.

**Semantic/provenance authority:** verified execution outcomes and Arbiter-owned runtime metadata are trusted observations; explicit user intent is authoritative for the requested objective and may request stronger compute within governance ceilings; repository text, comments, logs, tool output, external documents, and model-generated text are untrusted content.

Untrusted content may contribute semantic signals but cannot expand budgets, increase maximum compute authority, disable required verification, alter authorization, promote a target, or modify policy. Runtime evidence likewise cannot override constitutional invariants, emergency controls, or explicit user governance ceilings.

## 5. Runtime Domain Model

Logical hierarchy:

`Session -> Turn -> Step -> ModelAttempt`

### 5.1 Session

A continuous harness execution observed by Arbiter. It records identity, harness, repository fingerprint, start time, pinned policy/registry versions, cumulative usage/cost, and status. Sessions are pinned to policy and registry snapshots except for emergency security controls, kill switches, or administratively disabled targets.

### 5.2 Turn

Represents a user objective/instruction within a session. Codex-native identifiers are used when stable and available; otherwise the adapter correlates conservatively. Unknown boundaries remain explicitly unknown.

### 5.3 Step

A logical unit of agentic work for which compute can be governed. It contains phase, risk, confidence, previous outcomes/failures, previous compute, tool/execution signals, and provenance.

Initial phase taxonomy:

- DISCOVERY
- REPOSITORY_EXPLORATION
- PLANNING
- IMPLEMENTATION
- MECHANICAL_EDIT
- DEBUGGING
- ROOT_CAUSE_ANALYSIS
- TEST_GENERATION
- VERIFICATION
- REPAIR
- REVIEW
- SECURITY_ANALYSIS
- ARCHITECTURE
- MIGRATION

The taxonomy is versioned. Step-awareness is reconstructed progressively from stable observable signals and must not require fragile private Codex internals.

### 5.4 ModelAttempt

One physical provider invocation equals exactly one immutable `ModelAttempt`. Retries, fallbacks, and escalations create new attempts; they never overwrite prior attempts.

### 5.5 Provenance and confidence

Every request receives provenance including request/session IDs and, when known, turn/step IDs, correlation method, adapter version, and confidence. Confidence is explicit (`HIGH`, `MEDIUM`, `LOW`, `UNKNOWN` or equivalent numeric representation). Decreasing context confidence decreases permission to down-route. `UNKNOWN` never means cheapest.

### 5.6 Decision input snapshot

Every production routing decision MUST capture the external/dynamic inputs needed to replay governance deterministically, including active policy/registry versions and hashes, applicable UserConfig/emergency-control version, provider-health/circuit-breaker state, PricingSnapshot ID, relevant budget state/reservations, and compatibility/capability snapshot.

Replay reconstructs the historical decision from captured inputs; it must not re-query current provider health, pricing, or catalog and pretend those values existed historically.

## 6. Compute Model, Registry, and Routing

### 6.1 Compute tiers

Policy operates on abstract requirements: `ECONOMICAL`, `BALANCED`, `DEEP`, `FRONTIER`.

A `ComputeRequirement` contains preferred/minimum/maximum tier, optimization objective, required capabilities, and confidence.

### 6.2 Compute target

A resolved target contains model ID, reasoning effort, provider, capabilities, lifecycle status, and registry version. Pricing is referenced through a versioned `PricingSnapshot` on routing/attempt records rather than being part of target identity. The optimization unit is `(model, reasoning_effort)`.

### 6.3 Model registry

`AVAILABLE != APPROVED`.

Each `(model, effort)` has an independent lifecycle:

`DISCOVERED -> REGISTERED -> BENCH -> SHADOW -> CANDIDATE -> APPROVED -> PRODUCTION`

Production targets may become `DEGRADED`, `SUSPENDED`, or `DEMOTED`. A newly discovered model never enters production automatically.

### 6.4 Baseline

The initial experimental baseline is an approved `GPT-5.6 Terra / Medium` target, subject to verification against the actual OpenAI/Codex contract before implementation. Baseline is versioned and can change only through explicit promotion. It is the conservative cold-start escape hatch when no candidate has sufficient workload-specific evidence.

### 6.5 Routing composition

`StepContext -> PhasePolicy -> RiskPolicy -> ConfidencePolicy -> BudgetPolicy -> ComputeRequirement`

Policies are monotonic with respect to safety. A later policy stage cannot remove an earlier risk/security floor.

### 6.6 Target resolution

Targets are filtered by required capabilities, lifecycle eligibility, risk constraints, quality evidence, provider health, budget, and policy restrictions. Only then are cost/latency objectives used. There is no permanent global model ranking; evidence is workload-specific and may be segmented by repository, framework, language, phase, and risk class.

### 6.7 Overrides

Explicit human overrides are temporary, auditable, budgeted, and constrained by hard ceilings and safety invariants. They do not mutate policy.

## 7. Escalation, De-escalation, Retry, and Fallback

Infrastructure fallback and cognitive escalation are separate state machines. Provider errors do not automatically justify a stronger model. Cognitive escalation requires evidence such as repeated verified failure, failed repair, insufficient progress, or verification failure. Model requests for escalation are hints only.

De-escalation is first-class. There is one retry authority; nested provider/SDK/LiteLLM retries must be disabled or tightly bounded. Fallback targets must continue satisfying the original ComputeRequirement floors.

## 8. Budgets and Cost Governance

Budgets exist at step, turn, session, daily, and monthly levels as appropriate. There is no separate runtime `Task` entity in v1.

`ProductionBudget != EvaluationBudget`.

Evaluation budget exhaustion pauses paid evaluation but does not affect production. Budget accounting uses atomic reserve/settle/release semantics. Hard safety floors cannot be lowered to fit budget; when required safety cannot fit inside a hard budget, Arbiter reports an explicit governed conflict rather than silently downgrading.

## 9. Engineering Outcomes and Verification

Outcome truth is typed rather than reduced to one score. Outcome types include BuildOutcome, TestOutcome, TypecheckOutcome, LintOutcome, SecurityOutcome, RegressionOutcome, DiffOutcome, RepairOutcome, CIOutcome, and HumanReviewOutcome.

Common statuses: `PASS`, `FAIL`, `PARTIAL`, `UNKNOWN`, `NOT_APPLICABLE`. `UNKNOWN != PASS`.

Model self-report never substitutes for a verifier.

### 9.1 Verifier contract

Verifiers can determine applicability, prepare an authorized check, execute it, normalize results, and emit an outcome. They observe and validate; they do not edit code, repair failures, choose routing, or expand permissions.

Verifier commands MUST come from an explicit approved project/configuration contract or another authorization source already accepted by the harness. A verifier MUST NOT execute a shell command synthesized ad hoc by a model/challenger merely because it appears in model output.

### 9.2 Quality floors

Quality floors are workload-specific and multicriteria. Hard constraints apply before economics. Security regressions cannot be compensated by lower cost or latency.

### 9.3 Anti-gaming

Verification detects suspicious self-modification such as deleting tests, lowering coverage/test count, disabling scanners, modifying verifier config, suppressing lint rules, or changing CI in a way that judges the same attempt. Such conflicts block automatic learning/promotion until independently resolved.

### 9.4 Engineering cost

Production Engineering Cost includes production inference, repair, required verification, and attributable production CI cost. Learning Cost includes BENCH, Execution Shadow, evaluation-only verification, and experimentation spend and is tracked separately.

## 10. Evidence, BENCH, Shadow, and Evaluation

### 10.1 BENCH

New ComputeTargets compete against the approved baseline on versioned workloads. Built-in, community, and local sources may be used. BENCH filters weak candidates but is not production authority.

### 10.2 Decision Shadow

Computes what a candidate policy would select without issuing additional inference. It can run broadly and functions as static analysis of candidate policy behavior.

### 10.3 Execution Shadow

Runs a challenger only when side effects can be isolated, verification is available, privacy policy permits it, and evaluation budget exists. It cannot mutate the production workspace or automatically inherit production credentials/permissions. If isolation cannot be guaranteed, the workload is ineligible.

### 10.4 Sampling

Execution Shadow sampling depends on evidence deficit, uncertainty, workload importance, repository coverage, model changes, and evaluation budget. Production has priority over evaluation.

### 10.5 Evaluation

Prefer paired comparisons from the same task, state, and verification contract. Results include `CANDIDATE_BETTER`, `BASELINE_BETTER`, `QUALITY_EQUIVALENT`, `INCONCLUSIVE`, and `INSUFFICIENT_EVIDENCE`. Record sample size, coverage, effect size where applicable, uncertainty/confidence, and provenance.

## 11. Policy Compilation and Promotion

Production runtime reads only `ApprovedPolicy`. Evidence feeds a deterministic rule-based PolicyCompiler in v1 that emits immutable `CandidatePolicy` artifacts. Future ML/bandit/LLM compilers may generate candidates but never direct production authority.

### 11.1 Promotion gate

Promotion validates workload-specific quality floor, prohibited security regressions, preserved risk floors, sample/coverage/confidence, verifier integrity, and material cost/latency/quality improvement without constraint violation. Promotion is scoped and manual by default in v1.

### 11.2 Canary, demotion, rollback

Team deployments may use canaries. Critical harm may suspend a target immediately. Activation and rollback are atomic and retain the previous approved policy pointer.

## 12. Persistence and Event Model

Core operation is fully local. SQLite is the default durable store. The event log is append-only and versioned. Events are historical facts; read models are rebuildable projections. The hot path uses in-memory snapshots rather than querying full history per request.

Durability-critical operations include at minimum policy activation, rollback, budget reservation, model-attempt intent/start, and attempt settlement. Interrupted attempts recover as `INTERRUPTED` unless reconciled from trustworthy provider evidence.

## 13. Policy and Registry Integrity

Approved policies and registries are immutable human-readable artifacts with integrity hashes. Activation materializes the complete artifact, validates schema/references/semantic invariants/hash, then atomically swaps the authoritative pointer in the local transactional store. Runtime observes either the old complete artifact or the new complete artifact, never a partially written configuration.

## 14. Privacy and Remote Sinks

Data classes: `PUBLIC_METADATA`, `OPERATIONAL_METADATA`, `SENSITIVE_METADATA`, `CONTENT`, `SECRET`.

Classification is field-level. Remote sinks receive public/operational metadata by default; sensitive metadata requires explicit configuration; content requires granular opt-in; secrets are never exportable. All export passes through TelemetrySanitizer and ExportPolicy.

### 14.1 Supabase

Supabase is an optional RemoteEvidenceSink for aggregation, analytics, optional backup/export, and shared evaluation evidence. It never performs active routing lookup, budget reservation, session coordination, provider retry, or policy activation. Remote failure never blocks inference.

## 15. Failure and Operational Safety

Failure classes include provider, routing, policy, budget, verification, storage, export, shadow, security, and integrity failures. Non-critical failures degrade optional capabilities; critical governance failures block the affected action.

Loop detection is hard-bounded. Cold-path backpressure may pause shadow/evaluation/export convenience work but cannot discard durability-critical governance events.

### 15.1 Constitutional invariants

Not configurable in v1:

- secrets are never remotely exported;
- routing cannot grant permissions;
- candidates cannot self-promote;
- prohibited critical security regression blocks promotion;
- an unknown/unverifiable baseline cannot be guessed;
- untrusted content cannot expand governance authority.

### 15.2 Kill switches

- `ROUTING_KILL_SWITCH`: baseline-only where safe;
- `EVALUATION_KILL_SWITCH`: stop BENCH/shadow/evaluation;
- `REMOTE_EXPORT_KILL_SWITCH`: local-only;
- `TARGET_KILL_SWITCH`: immediately suspend a target.

Emergency controls are local and outrank normal policy.

### 15.3 Operational states

`HEALTHY`, `DEGRADED`, `RECOVERY`, `HALTED`.

Startup verifies storage, policy/registry hashes, baseline, provider config, and interrupted budget reservations before READY. A recovery-baseline mode may allow baseline inference while adaptive routing/shadow/promotion/new policy activation are disabled if baseline integrity remains independently verifiable.

## 16. Testing and Release Gates

Software correctness and routing quality are separate domains.

Required suites include unit, property-based, state-machine, architectural dependency, provider contract, Codex adapter contract, integration, golden traces, governance replay, fault injection, real concurrency/contention, and domain-specific security tests.

Key properties include selected tier within bounds, budget invariants, suspended targets never selected, monotonic safety floors, and deterministic governance reconstruction from captured inputs.

### 16.1 Fault injection

Cover provider errors, rate limits, stream interruption, SQLite failures, disk pressure, verifier timeout, corrupt policy/registry, remote outage, shadow failure, and process crash during attempts.

### 16.2 Eval suite

Built-in workloads cover repository exploration, mechanical edits, test generation, bug diagnosis, root-cause analysis, implementation, verification, repair, review, and security-sensitive work. Public, held-out, local, and community cases reduce benchmark overfitting.

### 16.3 Observability

Observe routing latency, TTFT, total request time, stream interruptions, attempts, retries, escalations/de-escalations/fallbacks; inference/verification/repair/evaluation costs; verified success/repair/regression/security outcomes; and Governor effectiveness including savings vs baseline, latency delta, quality delta, wrong-downroute rate, unknown-context rate, and avoidable frontier usage.

### 16.4 Performance SLO candidates

Initial targets to validate rather than advertise as guarantees:

- Governor-added local hot-path latency, including required local governance persistence and excluding upstream provider latency: p50 < 10 ms, p95 < 25 ms, p99 < 50 ms;
- TTFT degradation versus direct passthrough < 5%;
- cold-path failure does not block production unless governance-critical.

### 16.5 Release gates

A release is blocked by correctness/property/replay/architecture/migration failures, prohibited security findings/boundary violations, or unjustified material hot-path regressions. Software release and ApprovedPolicy release are independent.

## 17. Public Interfaces and Configuration

Stable v1 surface: CLI, localhost daemon, and human-readable configuration/policy/registry artifacts. Internal StepContext, detectors, policy compiler, evaluation internals, and projections remain evolvable.

### 17.1 Modes

- `PASSTHROUGH`: proxy approved baseline without routing interference.
- `OBSERVE`: baseline production plus classification/evidence.
- `SHADOW`: production remains approved while candidate decisions/eligible sampled execution shadows are evaluated.
- `ROUTING`: production uses ApprovedPolicy only.

Fresh installs start in `PASSTHROUGH`. Transition to another mode is explicit. `init` MUST NOT silently enable optimized routing.

### 17.2 CLI capabilities

V1 must support capabilities equivalent to init, start/stop/restart/status, mode, observe, shadow, bench, eval, candidates, explain, promote, rollback, budget, doctor, and uninstall.

Safe uninstall restores the previous Codex configuration.

### 17.3 Configuration separation

`UserConfig != ApprovedPolicy != ModelRegistry`.

Precedence:

`Constitutional Invariants > Emergency Controls > ApprovedPolicy > UserConfig constraints > Session override > Routing preference`

## 18. Adapters and Extension Boundary

Official v1 harness: Codex. Official first-class provider: OpenAI. LiteLLM is optional subject to contract/latency verification. SQLite is core persistence. Supabase is optional. No public Harness/Provider Plugin SDK is promised until multiple implementations prove the abstractions.

## 19. MVP and Delivery Strategy

### M0 — Transparent Proxy

Deliver daemon, Codex integration, OpenAIProvider, streaming/cancellation, usage accounting, SQLite, basic events, status/doctor, baseline, kill switch, and direct-vs-proxy benchmark.

Success: Codex behaves normally and measured overhead is acceptable.

### M1 — Observe

Add Session/Turn/Step, phase/risk classification, confidence/provenance, Routing Decision Records, basic EngineeringOutcomes, budgets, and explain. Production remains baseline.

### M2 — Evaluate

Add BENCH, Decision Shadow, generic verifiers, EvaluationHarness, safely isolated Execution Shadow, TargetEvidenceProfiles, deterministic CandidatePolicyCompiler, and candidates.

### M3 — Govern

Add manual promotion, ApprovedPolicy routing, escalation/de-escalation, constrained fallback, atomic rollback, continuous evaluation, and demotion.

M0+M1 form the technical MVP; M0-M3 form the complete product MVP.

## 20. Explicitly Deferred

Dashboard, hosted SaaS/control plane, Claude Code, OpenCode, generic provider marketplace, ML routing, automatic promotion, plugin marketplace, team/org management, community policy registry, remote policy distribution, GitOps, dedicated IDE extension, and web/mobile UI do not block v1.

## 21. Repository Shape

```text
/core
  /domain
  /policy
  /runtime

/adapters
  /codex
  /openai
  /litellm
  /sqlite
  /supabase

/verification
/evaluation
/cli

/tests
  /contracts
  /integration
  /property
  /fault-injection
  /evals
```

Exact language, package boundaries, libraries, SQLite strategy, concurrency primitives, sandbox/worktree implementation, and CLI framework are implementation-plan decisions unless required by a verified external contract.

## 22. Versioning

Version Governor software (SemVer), PolicyVersion, RegistryVersion, StorageSchemaVersion, EventSchemaVersion, PolicySchemaVersion, and Codex compatibility metadata independently.

## 23. Acceptance Criteria for the Architecture

1. Codex can run through transparent passthrough with streaming/cancellation preserved.
2. Routing does not expand agent authorization.
3. Unknown context cannot trigger aggressive down-routing.
4. Every provider invocation is represented as an immutable attempt.
5. Risk floors cannot be silently weakened by budgets, fallbacks, or candidate policies.
6. Production decisions use only approved targets and ApprovedPolicy.
7. A candidate cannot promote itself.
8. Quality evaluation is based on typed verified outcomes, not model self-report.
9. Defined security blockers cannot be traded for savings.
10. Production/evaluation budgets are independently enforceable.
11. Retry amplification is bounded and auditable.
12. Execution Shadow cannot mutate production workspace.
13. Core routing functions without Supabase or another remote service.
14. Remote telemetry cannot export secrets and does not export content by default.
15. Policy activation/rollback are atomic and integrity-checked.
16. Governance decisions can be reconstructed from versioned state/evidence.
17. Cold-path failure cannot silently change production routing behavior.
18. Software releases cannot silently replace ApprovedPolicy.
19. Safe uninstall restores direct Codex operation.
20. M3 measurably improves Production Engineering Cost versus approved baseline without violating quality/safety floors.
21. Governance replay uses captured historical inputs, not current provider health/pricing/catalog.
22. Verifier execution cannot introduce a second shell-authorization path.
23. Fresh install remains PASSTHROUGH until explicit mode change.
24. Atomic policy activation exposes either old or new complete artifact, never partial state.

## 24. Architectural Invariants

- Compute is governed by engineering state, risk, history, budget, and verified outcomes.
- Policy selects abstract compute requirements; registry resolves approved concrete targets.
- `(model, reasoning_effort)` is the optimization unit.
- No permanent global model ranking exists.
- Context uncertainty is explicit and conservative.
- Infrastructure fallback and cognitive escalation are independent.
- De-escalation is first-class.
- Outcome truth is typed and provenance-backed.
- `UNKNOWN != PASS`.
- Verifiers verify; they do not repair or gain authorization.
- Learning produces candidates, never direct production mutation.
- Policy evolution is evidence-backed, scoped, explicit, atomic, and reversible.
- Event history is append-only; read models are projections.
- Local operation is complete; remote services are optional.
- Privacy defaults are content-minimizing.
- Safety invariants outrank configurable policy.
- Production always has priority over evaluation.
- Arbiter optimizes verified engineering outcomes, not token spend in isolation.

## 25. Delivery Decomposition and Pre-Plan Contract Gates

This master design is broader than a single safe implementation plan. Implementation MUST be decomposed into milestone plans aligned with M0, M1, M2, and M3.

Before M0 implementation, freshly verify:

1. Codex custom-provider and Responses wire semantics for transparent proxying, streaming, cancellation, metadata correlation, and model/effort selection.
2. Current OpenAI model IDs, supported reasoning efforts, usage fields, pricing sources, and Responses behavior.
3. Whether LiteLLM preserves required Responses/streaming/retry semantics with acceptable overhead; failure does not block OpenAIProvider.
4. SQLite durability/concurrency characteristics for atomic budget reservation and active-pointer swaps.
5. Safe isolation before M2 enables side-effecting Execution Shadow.

A failed contract probe narrows/amends the milestone rather than being hidden by adapter assumptions.

## 26. Self-Review Findings Applied

Adversarial self-review corrected: undefined runtime Task usage; conflation of production and learning cost; oversimplified trust hierarchy; replay missing dynamic provider/pricing/config inputs; pricing as target identity; verifier command authorization ambiguity; unrealizable atomic-activation semantics; coarse event privacy classification; ambiguous fresh-install mode; ambiguous performance-SLO scope; master design too broad for one implementation plan; and external contract assumptions requiring fresh verification.

No unresolved contradiction remains intentionally in this approved draft. Future material changes require explicit amendments.

## 27. Product Definition

**Technical description:** An evidence-driven compute governor for coding agents that dynamically allocates model reasoning while enforcing quality, security, and budget constraints.

**Initial wedge:** Codex-first. OpenAI-first. Local-first.

**Core distinction:** a traditional router maps a prompt to a model. Arbiter closes the loop from engineering state to compute requirement, approved target, verified outcome, evidence, evaluation, and governed policy evolution.
