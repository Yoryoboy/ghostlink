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

- [x] **DMP-002 — Complete provider-neutral move contracts and pure policy**
  - Outcome: added validated exact/semantic item selectors, Vault/exact/semantic destination selectors, explicit/absent/ambiguous intent authorization, deterministic direct or two-step Vault-mediated effects, a caller-selected lifetime bounded to 300 seconds, overflow-safe expiry, and complete plan binding with distinct missing/ambiguous destination outcomes.
  - Check: focused mutation tests 16 passed; complete core tests 47 passed; focused and workspace all-feature clippy passed with `-D warnings`; workspace all-target/all-feature check, fmt, and diff check passed. Independent high-risk verification found no blocker/high/medium issue; one low documentation overstatement was corrected. LSP retained stale boolean/type findings contradicted by rustc tests and clippy.
  - Review workload: 893 additions and 97 deletions across the cohesive contracts, pure policy, and 16 tests; splitting tests from behavior would violate the work-unit rule.
  - Rollback: revert only mutation contract/policy changes and core exports.
  - Evidence: commit `a2566d3` (`feat: complete mutation planning contracts`).

- [x] **DMP-003 — Implement semantic planning service**
  - Outcome: added a shared `MutationService` that authorizes before side effects, runs ordinary freshness preparation, resolves exact or bounded semantic item selectors through one coherent definition-to-instance candidate set, resolves Vault/exact/semantic character destinations without guessing, spends an injected plan ID only after validation, and creates deterministic direct/Vault-mediated plans. The service owns no remote mutation port.
  - Check: covered with the combined DMP-003/004 focused suite and verification below.
  - Rollback: remove the application mutation planner and its exports/tests while preserving core contracts.
  - Evidence: commit `f1119be` (`feat: add semantic mutation planning service`).

- [x] **DMP-004 — Enforce shared explicit intent and mandatory preflight**
  - Outcome: `prepare_move` composes planning with a forced `ProfileSyncReason::Preflight`; preflight re-resolves the plan's stable item/source/membership/revision and independently binds the refreshed request item/destination selectors to that plan, preventing intent for one target from authorizing another. Every policy outcome maps to a sanitized typed error and no approval artifact exists.
  - Check: 47 focused mutation tests and 157 complete app test-support tests passed; app clippy passed with `-D warnings`; fmt and diff check passed. Independent high-risk verification found no blocker/high/medium issue; its low request/plan mismatch observation was promoted and corrected with six additional tests. LSP reported only stale pre-existing diagnostics contradicted by rustc/clippy.
  - Review workload: the new application module is approximately 2,000 lines including 47 exhaustive tests; DMP-003 and DMP-004 share one service and test harness, so the honest review slice remains one cohesive combined work-unit commit rather than separating tests or duplicating orchestration.
  - Rollback: revert the preflight/authorization orchestration together with the shared planner module, leaving core contracts unchanged.
  - Evidence: commit `f1119be` (`feat: add semantic mutation planning service`).

- [x] **DMP-005 — Prove the offline mutation-planning journey**
  - Outcome: added seven offline scenarios over the production `MutationService`/`ProfileSyncService`, real file-backed `SqliteStore`, real `BungieProfileGateway`, scripted deterministic profile transport, and injected IDs. They prove “The Last Word to Warlock,” direct and character-to-character Vault-mediated effects, one-call explicit intent, forced preflight, revision invalidation, fail-closed intent, read-only provider calls, and credential absence. The invalidated preflight returns before any effect can execute; structurally, no transfer seam exists in the composed service.
  - Check: focused journey 7 passed; workspace all-target/all-feature check and clippy passed with `-D warnings`; fmt and diff check passed. Independent verification confirmed the production stack, read-only endpoint allowlist, and no blocker/high/medium issue.
  - Rollback: remove only `crates/destiny-adapters/tests/mutation_planning_journey.rs`.
  - Evidence: pending journey work-unit commit.

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
