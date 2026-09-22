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

- User decision: WU-006 synchronizes instanced items only. Instance-less stackable materials and consumables are excluded; no synthetic IDs and no schema change is made to represent them.
- The architecture and WU-006 checks require persisted instance perk summaries, which WU-005 intentionally did not model. WU-006 adds a bounded reversible v2 projection migration for normalized perk references before provider normalization.
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
- Bungie perk reference semantics (`visible` and `isActive` are booleans): <https://bungie-net.github.io/multi/schema_Destiny-Perks-DestinyPerkReference.html>.
- Bungie item location enum (0 Unknown, 1 Inventory, 2 Vault, 3 Vendor, 4 Postmaster): <https://bungie-net.github.io/multi/schema_Destiny-ItemLocation.html>.
- Bungie Cross Save membership semantics: <https://bungie-net.github.io/multi/schema_User-CrossSaveUserMembership.html>.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 1,500–2,400 authored changed lines across contracts, adapters, orchestration, fixtures, tests, and documentation.
- Chain strategy: `feature-branch-chain`, continuing the existing WU chain from WU-005 closure `3aa8229`.
- Branch: `feature/destiny-profile-sync-foundations`.
- Engram mirror topic: `odd/destiny-profile-sync-foundations/tasks`.
- Review slices: membership/contracts; perk projection v2; normalization; transport; orchestration; final offline journey.
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

- [x] **DPS-002A — Add durable instance perk summaries**
  - Route: delegated multi-file core/SQLite writer. The writer correctly stopped before edits when exhaustive schema expectations were outside its initial surface; the parent explicitly added only `sqlite/tests.rs`, then the continued writer completed the cohesive v2 slice. Native assessment remained schema-incompatible/unavailable, so independent high-risk verification was required.
  - Outcome: added distinct `PerkHash` and ordered duplicate-tolerant `PerkSummary { perk_hash, active, visible }` values to every item; full structural identity now includes perk order and fields. Added a reversible STRICT v2 `item_perks` child table, atomic repository writes/reads, contiguous ordinal and corruption checks, stale removal, and fake/SQLite revision parity without provider text or raw payloads.
  - Check: core 23 passed; app test-support 67 passed; 60 focused SQLite tests passed; focused all-target/all-feature clippy with `-D warnings`, fmt, and diff checks passed. Migration v1→v2 preservation, v2→v1→v2 reopen, constraint/FK/cascade behavior, ordered/duplicate round trips, every perk mutation, stale removal, mid-write rollback, corruption, combined reopen, and byte/schema secrecy passed. Independent verifier reproduced all checks and found no blocker/high/medium findings; parent then corrected one documentation wording nit and reran all 60 focused SQLite tests. LSP emitted the same stale `!bool`/pattern diagnostics contradicted by rustc and clippy.
  - Review workload: 787 additions and 64 deletions across one indivisible domain/schema/repository contract plus its tests. The size exceeds the 400-line heuristic but keeps the reversible migration, persistence behavior, and verification together; DPS-003 provider normalization remains separate.
  - Evidence: commit `19cadc6` (`feat: persist instance perk summaries`).
  - Rollback: revert the v2 migration and perk fields/tests while preserving the v1 profile projection and DPS-002 contracts.

- [x] **DPS-003 — Normalize complete Bungie profile fixtures**
  - Route: delegated adapter writer. Three initial writer response emissions failed with a clean tree; continuing the final worker session completed the slice. Native assessment was unavailable, so independent high-risk verification ran through two correction rounds and a final pass.
  - Outcome: added a pure `decode_profile` boundary and complete synthetic fixture. It strictly validates the success envelope and required component wrappers, rejects private/disabled/partial/malformed/conflicting input, normalizes canonical bounded string-or-u64 identifiers, character enums with forward-compatible `Unknown(n)`, instanced-only Vault/character/unknown locations, Postmaster omission, equipment agreement, lock/quantity/power, and ordered duplicate perk summaries. Caller membership/version/time remain authoritative; output fixes `SourceMarker::Profile`; errors are category-only and retain no raw provider payload.
  - Check: 20 focused profile tests and 144 adapter tests passed; fmt, all-target/all-feature clippy with `-D warnings`, and diff checks passed. Independent review first caught enum-forward-compatibility and ID-form gaps, then official-source revalidation corrected the perk `visible` field from a mistaken numeric assumption to boolean. Final review found no blocker/high/medium issues. Deterministic mixed-length numeric ordering, canonical IDs, exact u32/u16 bounds, ComponentPrivacySetting, ItemLocation 0–4, missing components, duplicates, conflicts, secrecy, and fixture output are covered. The known pre-existing OAuth timeout test flaked once in the writer's first full run, then passed the immediate full rerun, isolated repeats, and every verifier/final run.
  - Review workload: approximately 1,485 authored lines, including about 758 in-module test lines and a 160-line synthetic fixture. This exceeds the heuristic but remains one pure decoder contract; HTTP transport and application orchestration are still separate DPS-004/005 slices.
  - Evidence: commit `9cba845` (`feat: normalize Bungie profile snapshots`).
  - Rollback: remove `profile.rs`, its fixture, and the two adapter re-export lines without affecting existing HTTP or repositories.

- [x] **DPS-004 — Implement bounded authenticated Bungie profile gateway**
  - Route: delegated adapter writer with independent read-only verification. The work was split into membership decoding, transport, and gateway wiring after repeated monolithic worker response failures, then reconciled as one work unit.
  - Outcome: added hardened current-user membership discovery and profile GET transport with exact allowlisted URLs/components, Bearer/API-key handling, redirect refusal, bounded bodies/timeouts, strict declared-versus-observed length checks, status mapping, injected-clock expiry enforcement, and sanitized errors. Cross Save selection uses the required-but-nullable `primaryMembershipId` and validates `crossSaveOverride`/`applicableMembershipTypes`; non-Cross-Save responses require exactly one unambiguous membership. The documented operation name remains `User.GetMembershipDataForCurrentUser`, while the exact endpoint path is `/Platform/User/GetMembershipsForCurrentUser/`.
  - Check: 50 focused gateway tests and all 194 adapter tests passed; fmt, all-target/all-feature clippy with `-D warnings`, and diff checks passed. No live calls were made. Independent verification found no blocker/high/medium findings after the official Cross Save and endpoint corrections.
  - Review workload: 2,574 additions across the gateway module, its in-module contract tests, and adapter exports. This exceeds the heuristic but keeps the mutually dependent membership selection, hardened request transport, secret-handling boundary, and gateway behavior together; application orchestration remains separate DPS-005.
  - Evidence: commit `6214bee` (`feat: add authenticated Bungie profile gateway`).
  - Rollback: remove `profile_gateway.rs` and its adapter module/re-exports while retaining pure profile normalization.

- [x] **DPS-005 — Orchestrate offline profile synchronization**
  - Route: delegated app writer plus independent read-only verifier. The parent applied one follow-up fix before freezing: the delegated module's test wrapper imported `super::*` unconditionally, which failed `cargo clippy --workspace --all-targets -- -D warnings` in the default-feature build because `test-support` is not a default feature; the wrapper is now `#[cfg(all(test, feature = "test-support"))]` and imports its own dependencies.
  - Outcome: added `ProfileSyncService<G, R>` in `crates/destiny-app/src/sync.rs` with the caller-supplied `(reason, policy, now)` signature. It reads the committed projection, evaluates the core `FreshnessInput` for `Dataset::ProfileInventory`, and skips with `SkippedFresh` only for an `Ordinary` request over a `Fresh` projection, without calling the gateway or mutating the repository. Every other combination refreshes in a strict order: resolve the effective membership, load one complete candidate, verify the candidate's membership identifier and type against the resolved membership (rejecting with `ProviderValidation` before any publication), then commit through `replace_current` and return `Refreshed` with repository metadata. The service never sets `checked_at`/`source`/`definition_version`, never computes a revision, clones no projection on success, and propagates gateway and repository errors unchanged and sanitized.
  - Check: 14 focused sync tests, 81 app all-feature tests, and 298 workspace all-feature tests passed; fmt, default-feature and all-feature all-target clippy with `-D warnings`, `git diff --check`, and the CLI/MCP help checks passed. Independent verification reconciled every test against the real `FreshnessInput::status()`, `InventorySnapshotInput::content_eq`, and `InMemoryInventoryRepository` semantics rather than test comments, confirmed scope was exactly the two candidate paths, and found no blocker/high/medium findings; two actionable lows were noted and need no change (the sanitization test covers only the service-generated error path, which is sufficient because the service never wraps propagated errors; the two mismatch tests do not re-assert `requested_memberships`, which is moot there). The known pre-existing OAuth timeout flake did not occur during verification.
  - Review workload: 642 additions in one new module (about 160 production lines including docs plus 14 behavioral tests) and two additive `lib.rs` lines. Within the heuristic for this slice.
  - Evidence: commit `528ddc8` (`feat: orchestrate offline profile sync`).
  - Rollback: remove `crates/destiny-app/src/sync.rs` and the two additive `lib.rs` lines; the gateway, normalization, and repository contracts are unaffected.

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
- Pre-normalization review found that the explicit perks-summary requirement needed a bounded reversible v2 projection migration; DPS-002A completed that domain/storage prerequisite without expanding instance-less item scope.
- Official Bungie documentation confirms required membership discovery, Profile component requests, Cross Save metadata, and item-instance semantics.
- DPS-001 established the feature boundary, user-selected instanced-only scope, strict completeness policy, API evidence, delivery strategy, and six implementation slices.
- DPS-002 added provider-neutral membership/sync value contracts and deterministic fake support; independent high-risk verification passed.
- DPS-003 added and fully verified the strict pure Bungie profile decoder and representative complete fixture after official-source correction of perk visibility.

## Verification evidence

- DPS-001 tracker and Engram mirror readback passed; `git diff --check` passed.
- DPS-002 writer verification: core 22 passed; app test-support 67 passed; fmt/clippy/diff checks passed. Independent verifier reproduced all checks and found no blocker/high issue; parent spot check reran core 22 successfully.
- DPS-002A writer and independent verifier observed core 23, app 67, and 60 focused SQLite tests passing with clean fmt/clippy/diff checks; parent reran the 60 SQLite tests after the sole documentation correction.
- DPS-003 final evidence: 20 focused profile tests and 144 adapter tests passed with clean fmt/clippy/diff checks; independent final review found no blocker/high/medium findings; parent spot check reran all 20 focused tests.
- DPS-004 final evidence: 50 focused gateway tests and 194 adapter tests passed with clean fmt/clippy/diff checks; independent verification found no blocker/high/medium findings after the official Cross Save selection and membership endpoint corrections.
- DPS-005 final evidence: 14 focused sync tests, 81 app all-feature tests, and 298 workspace all-feature tests passed with clean fmt, default-feature and all-feature clippy, diff, and CLI/MCP help checks; independent verification reconciled every test against real core and fake semantics, confirmed the two-path scope, and found no blocker/high/medium findings. The known pre-existing OAuth timeout flake did not occur.

## Manual gates and follow-ups

- Live authenticated membership/profile acquisition remains deferred to the live integration checklist.
- Instance-less materials and consumables require a future explicit domain/schema evolution if product scope expands.
- Windows/macOS SQLite locking and sidecar semantics remain WU-005 platform gates and are not reopened here.

## Next step

Delegate DPS-006 offline end-to-end journey verification and WU-006 closure.
