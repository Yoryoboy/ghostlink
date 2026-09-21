# Destiny Domain Foundations

## Objective

Implement WU-002 from `docs/mvp-implementation-plan.md`: establish provider-neutral domain records, opaque identifiers, freshness/revision semantics, behavioral ports, mutation-policy seams, deterministic clocks/IDs, and offline test doubles.

## Problem

WU-001 established the workspace and safe startup boundary, but application behavior still lacks stable domain contracts. Provider adapters, persistence, CLI, and MCP must not invent their own identifiers, records, freshness rules, or mutation semantics.

## Why

A provider-free core and behaviorally useful fakes create the anti-coupling seam required for later OAuth, Manifest, SQLite, query, and mutation units.

## Scope

- Add opaque IDs that prevent definition hashes, item instances, characters, memberships, revisions, and plans from being mixed.
- Add minimal domain records and open/preserved location values required by the MVP.
- Define freshness, revision binding, ambiguity, plan expiry, and mutation-policy inputs.
- Define behavioral ports without HTTP, JSON, SQL, CLI, or MCP concepts.
- Add deterministic clock/ID seams and application-owned fakes/fixtures for offline behavior tests.

## Constraints

- `destiny-core` remains provider-, transport-, serialization-, and persistence-free.
- `destiny-app` depends inward on core and owns scenario-oriented test doubles.
- Use open newtypes for provider values such as membership type rather than a closed enum unsupported by current evidence.
- First-class item locations are Vault and Character; unknown provider locations must be preserved without becoming business logic.
- Add `PlanId` now for expiry/revision binding; defer step IDs and provider-specific transfer sequencing to WU-009/010.
- No Bungie DTOs, endpoint assumptions, SQLite, CLI/MCP schemas, OAuth, credential storage, or mutation execution.
- Technical artifacts are written in English.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 500–750 authored changed lines across domain types, policies, ports, fakes, and tests.
- Chain strategy: `feature-branch-chain`, carried forward from the user's WU-001 delivery choice.
- Engram mirror topic: `odd/destiny-domain-foundations/tasks`.
- Branch: `feature/destiny-domain-foundations`, chained from completed WU-001.

## Effective checks

- TDD mode: not configured; use ordinary functional checks without claiming strict RED/GREEN evidence.
- Required commands:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace`
  - `cargo test --workspace --all-features`
- Structural checks: core remains free of provider/transport/storage dependencies; fakes are behavior-oriented; error displays remain sanitized.

## Tasks

- [x] **DDF-001 — Establish the feature boundary and domain decisions**
  - Route: delegated read-only mapping plus parent branch/tracker operations.
  - Outcome: recorded the bounded domain decisions, exclusions, dependency direction, delivery strategy, and five implementation tasks on a branch chained from completed WU-001.
  - Check: tracker readback passed and branch status was clean after commit.
  - Evidence: commit `076277a` (`chore: start Destiny domain foundations`).

- [x] **DDF-002 — Implement opaque IDs and foundational records**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation followed by a bounded identifier-formatting correction.
  - Outcome: added strongly separated IDs, Vault/Character/unknown locations, normalized character/item/definition records, snapshot metadata, and focused tests without core dependencies. A verification finding about raw identifier formatting was corrected by removing raw `Display` and using redacted `Debug` for string-backed IDs while preserving explicit raw access.
  - Check: fmt, clippy, workspace/all-features tests, and focused core tests passed; independent correction verification confirmed no raw formatting exposure and a clean worktree.
  - Evidence: implementation commit `431de4f` (`feat: add Destiny domain records`); correction commit `4db85c4` (`fix: redact Destiny domain identifiers`); re-verification passed.

- [x] **DDF-003 — Implement freshness, revisions, and plan validity**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation followed by a bounded invariant correction.
  - Outcome: added deterministic timestamps, dataset/source-aware freshness, revision allocation/retention/overflow, semantic resolution, validated expiring plans, and pure mutation binding across membership, revision, source, item, and destination. Independent verification identified seven invalid-state/misclassification issues plus missing current-source binding; all were corrected.
  - Check: fmt, clippy, workspace/all-features tests, focused core tests, and diff checks passed; correction re-verification passed all prior findings with 22 workspace and 15 focused core tests.
  - Evidence: implementation commit `dc78588` (`feat: add Destiny state policies`); correction commit `33553e9` (`fix: enforce Destiny state invariants`); re-verification passed.

- [x] **DDF-004 — Define ports, deterministic seams, and fakes**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation plus bounded fake-contract and test-coverage corrections.
  - Outcome: added provider-neutral ports, checked deterministic clock/plan-ID seams, structurally equal atomic inventory/definition repositories, queued profile/Manifest gateways, and an explicitly exposed but redacted credential boundary. The `test-support` feature remains offline. Verification findings about lossy snapshot fingerprints, unchecked exhaustion, missing contracts, and incomplete field coverage were corrected.
  - Check: fmt, clippy, workspace/all-features, core, and app test-support checks passed. Final re-verification confirmed independent revision-advance coverage for all normalized membership/definition/character/item fields and collection changes, with no production changes in the final test correction.
  - Evidence: implementation commit `19c6cfd`; contract correction `21cc670` (`fix: strengthen Destiny fake contracts`); exhaustive coverage commit `e9d98b4` (`test: cover Destiny snapshot revisions`); final re-verification passed.

- [ ] **DDF-005 — Verify and close WU-002**
  - Route: native review/assessment and delegated verification as required by active RDD policy.
  - Outcome: verify the full committed feature, correct any blockers, and close the work unit.
  - Check: baseline commands, structural dependency review, repository status, and review outcome.
  - Evidence: pending.

## Acceptance criteria

- Definition hashes and item instance IDs cannot be accidentally interchanged.
- Membership, character, bucket, revision, and plan identifiers are opaque and typed.
- Vault, character, and unknown locations preserve semantics without provider DTO leakage.
- Freshness, revision binding, ambiguity, and plan expiry are deterministic and tested.
- Core ports express behavior rather than HTTP, JSON, SQL, or transport details.
- Deterministic clocks/IDs and behaviorally useful fakes exercise application boundaries offline.
- No provider integration, persistence, presentation schema, credential, or mutation-execution behavior enters WU-002.
- All baseline commands pass offline.

## Progress

- WU-001 completed at `12783e2`.
- Read-only exploration mapped WU-002 contracts, dependencies, tests, and exclusions.
- The implementation decisions above resolve shape ambiguities conservatively without inventing provider behavior.
- DDF-001 established the chained feature boundary and recoverable implementation plan in commit `076277a`.
- DDF-002 established dependency-free core IDs and records in `431de4f`; independent verification found raw identifier formatting, corrected in `4db85c4`, and re-verification passed.
- DDF-003 established dependency-free state and mutation policy in `dc78588`; verification findings about invalid states, freshness, expiry, classification, and source binding were corrected in `33553e9`, and re-verification passed.
- DDF-004 established behavioral ports and offline fakes in `19c6cfd`; contract corrections landed in `21cc670`, exhaustive revision-field tests in `e9d98b4`, and final re-verification passed.

## Next step

Complete DDF-005 with full-feature verification and native review handling.
