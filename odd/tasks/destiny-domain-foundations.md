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

- [ ] **DDF-002 — Implement opaque IDs and foundational records**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation.
  - Outcome: add strongly separated IDs, locations, characters, items, definitions, and snapshot metadata with focused tests.
  - Check: ID separation, definition-versus-instance semantics, unknown-location preservation, fmt/clippy/tests.
  - Evidence: pending.

- [ ] **DDF-003 — Implement freshness, revisions, and plan validity**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation.
  - Outcome: add dataset freshness states, repository-generated revision semantics, ambiguity outcomes, plan expiry, and revision-bound mutation inputs.
  - Check: boundary-time expiry, changed/unchanged revision behavior, stale/mismatch and ambiguity tests.
  - Evidence: pending.

- [ ] **DDF-004 — Define ports, deterministic seams, and fakes**
  - Route: delegated to `gentle-ai-worker`; multi-file implementation.
  - Outcome: add provider-neutral behavioral ports, deterministic clock/ID generators, and application-owned behavior fakes/fixtures.
  - Check: port/fake contract tests, deterministic behavior, no provider-shaped types or side effects.
  - Evidence: pending.

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

## Next step

Delegate DDF-002 as the first implementation slice.
