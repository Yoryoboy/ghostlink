# Destiny Profile Sync Foundations

## Objective

Implement WU-006 from `docs/mvp-implementation-plan.md`: acquire the authenticated user's active Destiny profile through Bungie's documented APIs, normalize a complete instanced-item projection, and atomically publish characters and items with explicit sync reasons, freshness, source, and repository revision semantics.

## Problem

WU-003 provides authorization and secure credentials, WU-004 provides normalized Manifest definitions, and WU-005 provides an atomic durable inventory repository, but the application cannot discover the authenticated Destiny membership, fetch profile components, normalize provider data, or decide when a profile refresh is required.

## Why

Search, inspection, mutation preflight, and later post-action verification need one trustworthy local profile projection. Provider responses are component-scoped and may be private, malformed, or partial, so replacement must happen only after a complete normalized candidate has been validated.

## Scope

- Resolve the authenticated user's effective Destiny membership through Bungie's current-user membership endpoint, honoring Cross Save and rejecting ambiguous or absent active memberships.
- Fetch the documented profile components needed for characters, profile inventory, character inventories, equipment, item instances, and perk summaries.
- Normalize characters and only items carrying a provider `itemInstanceId`; intentionally omit instance-less materials and consumables.
- Map Vault, character inventory/equipment, unknown locations, equipped state, lock state, quantity, power, and bounded perk summaries without leaking raw provider payloads.
- Treat all required components as one complete candidate; privacy denial, missing/null components, malformed references, or component-level errors fail closed and preserve the prior repository projection.
- Add application synchronization for forced, stale, preflight, and post-action reasons using caller-supplied freshness policy and current time.
- Preserve repository-owned structural revision retention/advance and atomic replacement from WU-005.
- Prove the gateway, normalization, orchestration, and fixture journey offline without credentials or live network access.

## Constraints and decisions

- User decision: WU-006 synchronizes instanced items only. Instance-less stackable materials and consumables are excluded; no synthetic IDs and no WU-005 schema migration.
- Bungie remains provider authority; SQLite remains a rebuildable local projection.
- The OAuth token response membership ID identifies the Bungie.net account, not the Destiny profile. Membership discovery uses `User.GetMembershipDataForCurrentUser`, then profile acquisition uses `Destiny2.GetProfile` with a non-Bungie membership type and Destiny membership ID.
- Cross Save selection must be deterministic. Overridden memberships are ignored; exactly one effective membership is required. Zero or multiple effective memberships produce a sanitized validation error rather than guessing.
- Complete snapshot semantics are strict: Characters, ProfileInventories, CharacterInventories, CharacterEquipment, ItemInstances, and requested perk data must be present and internally consistent before publication. Privacy/missing/partial responses never replace a good snapshot.
- Freshness TTL remains caller policy through the existing `FreshnessPolicy`; WU-006 will not hard-code product TTL.
- Plans, execution records, mutation calls, Postmaster, world state, build logic, duplicate ranking, query UX, CLI/MCP wiring, and live-account testing remain out of scope. Plans/executions stay deferred to WU-009/010.
- No raw provider response, access token, API key, membership payload, authorization code, or callback value may enter domain snapshots, SQLite, fixtures, logs, errors, or snapshots.
- Technical artifacts are English. Automated checks are offline and credential-free.
- TDD mode: not configured by project/session evidence; ordinary functional checks apply.

## External evidence

- Bungie `User.GetMembershipDataForCurrentUser`: <https://bungie-net.github.io/multi/operation_get_User-GetMembershipDataForCurrentUser.html>.
- Bungie `Destiny2.GetProfile`: <https://bungie-net.github.io/multi/operation_get_Destiny2-GetProfile.html>.
- Bungie component enum and inventory semantics: <https://bungie-net.github.io/multi/schema_Destiny-DestinyComponentType.html>.
- Bungie profile response component contracts: <https://bungie-net.github.io/multi/schema_Destiny-Responses-DestinyProfileResponse.html>.
- Bungie item instance semantics: <https://bungie-net.github.io/multi/schema_Destiny-Entities-Items-DestinyItemInstanceComponent.html>.
- Bungie Cross Save membership semantics: <https://bungie-net.github.io/multi/schema_User-CrossSaveUserMembership.html>.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 1,500–2,400 authored changed lines across contracts, adapters, orchestration, fixtures, tests, and documentation.
- Chain strategy: `feature-branch-chain`, continuing the existing WU chain from WU-005 closure `3aa8229`.
- Branch: `feature/destiny-profile-sync-foundations`.
- Engram mirror topic: `odd/destiny-profile-sync-foundations/tasks`.
- Review slices: membership/contracts; normalization; transport; orchestration; final offline journey.
- Native RDD: disabled for this clone by prior explicit user decision; ordinary risk-gated verification applies.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; workspace check/clippy all targets/features with `-D warnings`; workspace locked tests; all-feature locked tests; CLI/MCP help; `git diff --check`.
- Focused: core contracts; app test-support and sync tests; adapter all-feature tests; membership and profile request contract tests; normalization fixtures; malformed/private/partial component failures; unchanged/changed revision behavior; repository rollback preservation.
- Runtime harness: offline scripted transports feed membership and profile fixtures through the application sync service into an in-memory or temporary SQLite repository, then reload normalized state. No live network or credentials.

## Tasks

- [x] **DPS-001 — Establish WU-006 boundary and authoritative API decisions**
  - Route: delegated repository mapping plus parent official API research; mapping trigger fired because the work spans more than four files.
  - Outcome: created the chained feature branch/tracker and recorded the instanced-only user decision, strict complete-snapshot policy, Cross Save selection, six task slices, checks, exclusions, and forecast.
  - Check: tracker and Engram mirror readback passed; `git diff --check` passed; branch contained only the intended new tracker before commit.
  - Evidence: commit `eeb5a78` (`chore: start Destiny profile sync foundations`).
  - Rollback: delete only this new branch and tracker before source implementation.

- [x] **DPS-002 — Add provider-neutral membership and sync contracts/fakes**
  - Route: delegated multi-file writer. Two writer result emissions failed without changes; continuing the second writer session completed the bounded slice. Native assessment remained schema-incompatible/unavailable, so the candidate followed the required high-risk independent verification path.
  - Outcome: added validated/redacted `DestinyMembership`, split the existing `ProfileGateway` into explicit effective-membership resolution and complete inventory acquisition, added ordinary/forced/preflight/post-action reasons plus repository-metadata outcomes, and extended the deterministic fake with independent queues, counters, and requested-membership recording. No provider transport, JSON, credentials, persistence, or orchestration was added.
  - Check: fmt passed; core 22 passed; app test-support 67 passed; focused all-target/all-feature clippy with `-D warnings` passed; `git diff --check` passed; parent core-test spot check passed. Independent verification passed with no blocker/high findings and one accepted architectural medium: `state` now references `SnapshotMetadata` from `domain`, creating a legal intra-crate back-edge. DPS-005 must enforce that only ordinary sync can skip and that acquired membership identity matches the request before publication. LSP emitted pre-existing/stale type/name diagnostics contradicted by rustc and clippy.
  - Evidence: commit `202f5df` (`feat: add profile sync contracts`).
  - Rollback: remove only WU-006 membership/sync contracts, re-exports, and fake changes while preserving WU-005 repository APIs.

- [ ] **DPS-003 — Normalize complete Bungie profile fixtures**
  - Route: delegated adapter writer.
  - Outcome: decode required components and normalize characters plus instanced items, locations, equipment, locks, quantities, power, and bounded perk summaries; reject malformed/private/partial candidates.
  - Check: fixture-driven adapter tests and stable sanitized snapshots covering every required semantic and failure mode.
  - Rollback: remove profile decoder/fixtures without affecting existing HTTP or repositories.

- [ ] **DPS-004 — Implement bounded authenticated Bungie profile gateway**
  - Route: delegated adapter writer.
  - Outcome: add hardened current-user membership discovery and profile GET transport with exact URLs/components, Bearer/API-key handling, redirect refusal, bounded bodies/timeouts, status mapping, Cross Save selection, and sanitized errors.
  - Check: scripted request-contract tests plus adapter check/clippy/tests; no live calls.
  - Rollback: remove only the WU-006 gateway/transport integration while retaining pure normalization.

- [ ] **DPS-005 — Orchestrate offline profile synchronization**
  - Route: delegated app writer.
  - Outcome: implement forced/stale/preflight/post-action decisions, skip fresh ordinary sync, acquire one complete candidate, atomically replace through `InventoryRepository`, and return sanitized outcome/revision metadata.
  - Check: fake-driven reason/idempotency/change/error tests plus focused app/core verification.
  - Rollback: remove only `destiny-app::sync` and its tests/fakes.

- [ ] **DPS-006 — Verify the offline end-to-end journey and close WU-006**
  - Route: delegated verification harness plus independent risk-gated verifier.
  - Outcome: prove scripted membership/profile acquisition → normalization → SQLite publication/reopen, idempotent refresh, revision advance, strict partial failure preservation, and secrecy; close the tracker.
  - Check: full effective check set, dependency/source scans, fixture secrecy scan, independent full-range review.
  - Rollback: reset this feature branch to `3aa8229` to remove only WU-006.

## Acceptance criteria

- The authenticated user's effective Destiny membership is resolved deterministically and Cross Save overrides never select an obscured profile.
- The gateway requests the exact required components with bounded authenticated HTTP behavior and exposes no credential material.
- Only provider-instanced items enter the projection; instance-less items are intentionally and observably omitted without synthetic identity.
- Vault, character inventory/equipment, unknown locations, equipped/locked state, quantity, power, and perk summaries normalize deterministically.
- Missing/private/malformed/partial required components fail closed and preserve the prior complete projection.
- Forced, stale, preflight, and post-action reasons refresh; an ordinary fresh request skips; unchanged normalized content retains its revision and changed content advances once.
- The complete offline fixture journey persists and reloads through SQLite without raw provider payloads or secrets.
- All automated checks remain offline and credential-free; live Bungie validation stays a later manual gate.

## Progress

- WU-005 closed cleanly at `3aa8229` with durable atomic repositories and 201 passing workspace tests.
- Read-only mapping identified existing repository/freshness contracts but no profile adapter, membership resolution, Bearer gateway, sync service, or fixtures.
- The user selected instanced items only for WU-006.
- Official Bungie documentation confirms required membership discovery, Profile component requests, Cross Save metadata, and item-instance semantics.
- DPS-001 established the feature boundary, user-selected instanced-only scope, strict completeness policy, API evidence, delivery strategy, and six implementation slices.
- DPS-002 added provider-neutral membership/sync value contracts and deterministic fake support; independent high-risk verification passed.

## Verification evidence

- DPS-001 tracker and Engram mirror readback passed; `git diff --check` passed.
- DPS-002 writer verification: core 22 passed; app test-support 67 passed; fmt/clippy/diff checks passed. Independent verifier reproduced all checks and found no blocker/high issue; parent spot check reran core 22 successfully.

## Manual gates and follow-ups

- Live authenticated membership/profile acquisition remains deferred to the live integration checklist.
- Instance-less materials and consumables require a future explicit domain/schema evolution if product scope expands.
- Windows/macOS SQLite locking and sidecar semantics remain WU-005 platform gates and are not reopened here.

## Next step

Delegate DPS-003 profile normalization and fixtures.
