# Destiny Mutation Planning

## Objective

Implement WU-009 from `docs/mvp-implementation-plan.md`: shared, adapter-neutral semantic `move_item` planning with explicit intent authorization, deterministic direct or Vault-mediated effects, bounded expiry, revision binding, and one mandatory preflight policy path for every future adapter.

## Problem

The core already has a minimal `MovePlan` and pure binding policy, while WU-007 provides bounded search and freshness-aware query services. There is no application service that resolves an item and destination semantically, rejects ambiguity, creates a complete executable-internal step plan, or composes authorization and mandatory preflight under one shared contract. CLI and MCP therefore cannot safely converge on the same mutation behavior.

## Why

WU-009 is the safety boundary between semantic user intent and WU-010 remote execution. The plan must bind one concrete item, membership, current source, destination, and profile revision for a short period; describe expected effects without mutating remotely; and guarantee that explicit intent and a fresh preflight are evaluated through shared policy rather than adapter-specific ceremony.

## Scope

- Extend provider-neutral mutation contracts with validated semantic item/destination selectors, explicit-intent authorization, ordered transfer steps, and bounded sanitized planning results.
- Resolve an item by concrete instance ID or bounded semantic definition search; return missing/ambiguous outcomes instead of guessing.
- Resolve a destination as Vault or one uniquely matched character; reject unknown, missing, and ambiguous destinations.
- Produce a direct one-step plan when either endpoint is Vault and a two-step plan through Vault for character-to-character moves.
- Bind every plan to membership, concrete item, current source, destination, current profile revision, creation time, and bounded expiry.
- Share authorization and mandatory-preflight validation in the application mutation service so CLI and MCP cannot bypass them.
- Prove planning has no remote mutation side effect and that explicit MCP intent requires no preview token, confirmation value, approval ceremony, or second call.

## Constraints and decisions

- Bungie remains authoritative; planning reads the synchronized projection and never performs a transfer.
- WU-009 introduces no transfer gateway or provider endpoint assumptions. WU-010 owns actual remote calls, partial failure, post-action refresh, and verification.
- Concrete `ItemInstanceId` selection is exact. Semantic text selection uses the existing normalized bounded definition search plus concrete instance search and returns ambiguity rather than ranking.
- Character destination matching is case-insensitive against normalized character class/name labels; exact character ID is also supported. Vault is an explicit destination, not a magic character name.
- A source or destination outside Vault/Character is unsupported. A no-op move is rejected.
- Character-to-character plans contain `source -> Vault` then `Vault -> destination`; Vault-to-character and character-to-Vault plans contain one direct step.
- Explicit user intent is the shared authorization fact for direct MCP planning. Absent or ambiguous intent is rejected; CLI may add confirmation UX outside this service.
- Preflight is mandatory and calls the existing profile sync service with `ProfileSyncReason::Preflight`, then re-resolves stable identities and applies the same binding policy. Planning itself uses an ordinary freshness gate.
- Plan lifetime is caller-configured only through a validated bounded policy; timestamp overflow fails with the existing typed error.
- Result and effect records are bounded and sanitized; they contain no credentials, raw provider payloads, approval artifacts, or adapter-specific values.
- Technical artifacts are English.
- TDD mode: not configured by project or session evidence; ordinary functional checks apply.
- Native RDD: inherited as disabled for this clone; ordinary risk-gated verification applies.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 1,200–2,000 authored changed lines across core contracts/tests, application orchestration/tests, and an offline integration journey.
- Branch: `feature/destiny-mutation-planning` from exact base `2b6c001`.
- Engram mirror topic: `odd/destiny-mutation-planning/tasks`.
- Review slices: contracts/pure policy; semantic planner; shared preflight authorization; offline journey; final closure.
- Every cohesive task closes with tests/docs in the same Conventional Commit. No push or PR is part of this work unit.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`; clippy across all targets/features with `-D warnings`; workspace tests with and without all features; CLI/MCP help; `git diff --check`.
- Focused: core mutation contracts; app mutation service with `test-support`; direct/Vault-mediated steps; semantic item/destination missing/ambiguity; expiry/revision/source/item/destination mismatch; explicit versus absent intent; mandatory preflight refresh and no planning-side transfer boundary.
- Runtime harness: compose deterministic gateway/repository/clock/ID seams, seed definitions and a profile snapshot, plan “The Last Word to Warlock,” force a changed preflight, and verify invalidation or a valid rebound policy outcome without any transfer adapter or live network.

## Tasks

- [x] **DMP-001 — Establish WU-009 boundary and mutation architecture decisions**
  - Outcome: created this tracker after reading the MVP plan, architecture, existing mutation/query/sync contracts, ports, fakes, and prior ODD conventions; fixed semantic selectors, direct/Vault step rules, explicit-intent semantics, preflight composition, delivery slices, checks, and rollback boundaries before source edits.
  - Check: tracker readback and `git diff --check`.
  - Rollback: remove only this tracker and its Engram mirror before source implementation.
  - Evidence: commit `4d3fbac` (`chore: start Destiny mutation planning`).

- [ ] **DMP-002 — Complete provider-neutral move contracts and pure policy**
  - Outcome: validated selectors, intent authorization, step/effect records, bounded lifetime policy, complete plan binding, and exhaustive pure tests.
  - Check: focused `destiny-core` mutation tests, fmt, clippy, and diff check.
  - Rollback: revert only mutation contract/policy changes and core exports.

- [ ] **DMP-003 — Implement semantic planning service**
  - Outcome: ordinary freshness preparation, exact or semantic item resolution, destination resolution, deterministic direct/Vault-mediated plans, and no remote mutation side effect.
  - Check: focused application planner tests plus core/app clippy and LSP.
  - Rollback: remove the application mutation planner and its exports/tests while preserving core contracts.

- [ ] **DMP-004 — Enforce shared explicit intent and mandatory preflight**
  - Outcome: one shared service accepts explicit intent, forces `Preflight` synchronization, re-resolves current stable state, and maps binding failures to typed sanitized errors without preview or approval artifacts.
  - Check: focused policy/preflight tests covering expiry, changed revision/source/item/destination, missing/ambiguous intent/selection, and exact sync reason/order.
  - Rollback: revert the preflight/authorization orchestration without changing the pure planner.

- [ ] **DMP-005 — Prove the offline mutation-planning journey**
  - Outcome: deterministic fixture journey proves “The Last Word to Warlock,” direct and Vault-mediated effects, planning non-mutation, explicit one-call intent, and preflight invalidation against production services/fakes.
  - Check: focused integration test plus workspace baseline commands and LSP diagnostics on changed paths.
  - Rollback: remove only the integration target and fixture additions.

- [ ] **DMP-006 — Verify and close WU-009**
  - Outcome: full-range verification, final tracker evidence, exact authored-line count, remaining implementation-time provider questions, and clean committed branch.
  - Check: all effective commands and runtime harness pass; no blocker/high/medium finding remains.
  - Rollback: reset the branch to exact base `2b6c00159ebc574d1e97d9ffc490244d04f1ca70` to remove only WU-009.

## Acceptance criteria

- One semantic request resolves to exactly one concrete item and one supported destination or returns a typed missing/ambiguity/unsupported outcome without guessing.
- A plan records membership, instance, source, destination, profile revision, creation/expiry, ordered steps, and expected bounded effects.
- Vault-to-character and character-to-Vault are direct; character-to-character uses Vault exactly once as an intermediary.
- Planning causes no remote mutation and introduces no transfer adapter.
- Explicit user intent authorizes the direct MCP use case; absent or ambiguous intent is rejected, with no preview token, confirmation value, approval ceremony, or second call.
- Shared preflight always forces a fresh profile sync and invalidates expired or changed revision/membership/item/source/destination assumptions.
- CLI and MCP can consume the same shared contracts; adapter-specific UX cannot weaken policy.
- Every completed task records observed checks, rollback boundary, and Conventional Commit identity.

## Open decisions and blockers

- No product decision blocks WU-009 under the architecture: semantic matching, Vault routing, explicit MCP intent, and preflight behavior are fixed above.
- Official/current Bungie lock, availability, endpoint, and transfer constraints remain an implementation-time WU-010 revalidation; WU-009 must not encode unverified provider behavior.
- The exact production plan TTL may be tuned later. WU-009 enforces a documented safe bound and deterministic injected duration rather than choosing a deployment default in an adapter.

## Progress

- WU-007 is closed at base `2b6c001`; query, profile sync, revision, repository, and deterministic fake seams are available.
- Existing `destiny-core::mutation` proves minimal expiry/revision/source/item/destination binding but lacks step/effect shapes, semantic selectors, explicit intent, and application orchestration.
- Source implementation has not started; this tracker and its Engram mirror are the first WU-009 artifacts.
