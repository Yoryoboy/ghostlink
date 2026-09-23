# Destiny Query Services

## Objective

Implement WU-007 from `docs/mvp-implementation-plan.md`: shared, adapter-neutral query services for `list_characters`, semantic `search_inventory`, and `inspect_item` over the durable profile projection and Manifest definitions, with explicit freshness, ambiguity, and definition-versus-instance behavior.

## Problem

WU-004 through WU-006 can acquire and persist Manifest definitions and a complete instanced-item profile snapshot, but no application query layer or repository search seam exists. The only available reads are exact definition-hash lookup and whole-snapshot loading, so CLI and MCP cannot yet offer bounded semantic inventory reads without duplicating business logic.

## Why

WU-007 establishes the shared read contract required by both WU-008 CLI reads and WU-011 MCP reads. It must keep provider payloads and credentials outside result types, preserve the distinction between item definitions and concrete instances, and never present stale inventory as current.

## Scope

- Add provider-neutral query inputs and bounded sanitized result records for characters, inventory search, and item inspection.
- Add a `SearchRepository` seam and equivalent deterministic fake and SQLite implementations.
- Search inventory definitions by normalized name/type fields and filter concrete instances by exact perk hash; perk-name search is excluded because no perk definition catalog exists.
- Compose the existing profile synchronization policy before query answers so missing/stale/invalid-source projections refresh or fail through typed errors.
- Return explicit missing, unique, and ambiguous semantic outcomes without selecting an instance by display name alone.
- Include definition hash, concrete instance ID, location, character association, state, perks, revision, checked time, source, and freshness where relevant.
- Prove the complete query journey offline over real SQLite persistence and deterministic profile/definition fixtures.

## Constraints and decisions

- Bungie remains authoritative; SQLite is a rebuildable search projection.
- `list_characters`, `search_inventory`, and `inspect_item` are shared application services. CLI formatting and MCP transport remain WU-008/WU-011.
- `ItemHash` and `ItemInstanceId` remain structurally distinct in every request and result.
- Search results are bounded and deterministic; raw provider payloads, credentials, authorization codes, headers, and unbounded definition records never cross the query boundary.
- Ambiguous display-name matches return bounded candidates; the service never guesses a concrete instance.
- Reads use caller-supplied `FreshnessPolicy` and current time. A missing, stale, or invalid-source profile projection must synchronize before answering; a fresh projection may be reused.
- Text matching covers normalized definition name/type/search tags supported by the stored definition projection. Perk filtering uses exact `PerkHash`; textual perk-name search is deferred until a perk-definition catalog exists.
- Search storage follows the architecture's `SearchRepository` boundary. DQS-003 will choose and verify the smallest durable SQLite search/index design compatible with the pinned bundled SQLite build; no unverified FTS capability is assumed.
- No live Bungie calls or credentials are required by automated checks.
- Mutation planning, transfer execution, CLI rendering, MCP schemas, ranking/optimization, build matching, Postmaster, and instance-less materials remain out of scope.
- Technical artifacts are English.
- TDD mode: not configured by project or session evidence; ordinary functional checks apply.
- Native RDD: disabled for this clone by prior explicit user decision; ordinary risk-gated verification applies.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 1,800–3,000 authored changed lines across contracts, fake parity, SQLite search, application orchestration, fixtures, tests, and documentation.
- Chain strategy: `feature-branch-chain`, continuing the existing linear WU chain from WU-006 plus maintenance closure `8a2250e`.
- Branch: `feature/destiny-query-services`.
- Engram mirror topic: `odd/destiny-query-services/tasks`.
- Review slices: contracts/fake; SQLite search; application services; offline journey; final closure.
- The forecast exceeds the 400-line delivery heuristic; each cohesive task is a separate work-unit commit and potential chained PR slice. Tests remain with behavior.

## Effective checks

- Baseline: `cargo fmt --all -- --check`; `cargo check --workspace --all-targets --all-features`; clippy across all targets with and without `--all-features` using `-D warnings`; workspace locked tests with and without all features; CLI/MCP help; `git diff --check`.
- Focused: core query contracts; app query services with `test-support`; SQLite migration/search contract tests; fake/SQLite parity; name/type/perk filtering; character listing; exact inspection; missing/ambiguous/stale behavior; definition-versus-instance identity; bounded sanitized results.
- Runtime harness: publish deterministic Manifest definitions and a complete profile snapshot into a real temporary `SqliteStore`, compose the real query service with the existing sync boundary and deterministic gateway/clock seams, then exercise list/search/inspect, forced stale refresh, ambiguity, reopen, and secret absence. No live network or credentials.

## Tasks

- [x] **DQS-001 — Establish WU-007 boundary and query architecture decisions**
  - Route: delegated read-only mapping because understanding spans more than four files; parent creates the one-file tracker.
  - Outcome: created the chained feature branch and this durable tracker with the existing/missing seam map, bounded search scope, exact-perk limitation, freshness/ambiguity rules, six task slices, checks, delivery forecast, and rollback boundaries.
  - Check: tracker and Engram mirror readback passed; `git diff --check` passed before the boundary commit.
  - Rollback: delete only this new branch and tracker before source implementation.
  - Evidence: commit `e6d78f0` (`chore: start Destiny query services`).

- [x] **DQS-002 — Add provider-neutral query contracts and deterministic fake parity**
  - Route: delegated multi-file writer; native assessment was unassessable because the new query module was intentionally untracked, so the required high-risk independent verifier reviewed the complete candidate and one bounded correction round.
  - Outcome: added validated bounded query contracts, explicit definition/instance identities, shared Unicode-aware normalization/matching rules, exact perk-hash filters, deterministic result metadata/shapes, and a `SearchRepository` port. Added an in-memory fake with whole-projection replacement, stable sort-before-limit semantics, exact lookup resolution, and contract tests. Caller-controlled limits and hash filters are bounded; `Any` remains distinct from fail-closed `AnyOf(empty)`.
  - Check: writer and independent verifier observed fmt/diff clean; core 35 passed; app test-support 91 passed; core and app focused clippy passed with `-D warnings`. Parent spot check reran the 64/65 hash-filter boundary test successfully. LSP reported three stale/false diagnostics in pre-existing fake code, contradicted by rustc tests and clippy.
  - Review workload: 1,339 additions and 9 deletions across the indivisible public contracts, shared matching rules, fake implementation, and behavioral tests. An honest contracts/fake split would still produce roughly 690- and 590-line slices, so DQS-002 retains one cohesive accepted over-budget work unit rather than separating tests from behavior.
  - Rollback: remove `query.rs`, its core exports/port, and the in-memory search fake/tests while preserving all WU-006 contracts.
  - Evidence: commit `053fe2a` (`feat: add query contracts and search fake`).

- [x] **DQS-003 — Implement durable SQLite search and contract parity**
  - Route: delegated multi-file writer; native assessment was unassessable because `search.rs` was intentionally untracked, so an independent high-risk verifier reviewed the complete candidate. The first v3 precomputed-index design had two medium corruption/migration-bound findings; one bounded correction removed the derived schema entirely and the verifier passed the simplified design.
  - Outcome: implemented `SearchRepository` directly over the existing v2 source-of-truth tables. Definition search streams hashes in deterministic order, reuses the canonical fail-closed definition reader, applies the shared Rust Unicode matcher, and stops at the validated limit. Instance search uses bounded parameterized SQL for exact hash/perk/character filters, deterministic ordering, complete reconstruction, and exact lookup. No schema migration or derived index can drift; the documented MVP N+1 scan is the future index seam.
  - Check: writer and independent verifier observed fmt/diff clean; 66 focused SQLite tests and 206 complete adapter tests passed; adapter all-target/all-feature clippy passed with `-D warnings`. Parent spot check reran the Unicode/name/type/tag fake-parity test successfully. LSP emitted stale slice-coercion/`None` diagnostics contradicted by rustc and clippy. No blocker/high/medium finding remains.
  - Review workload: 848 additions and 2 deletions in one implementation-and-tests slice. The 317-line implementation and 527-line tests cannot be separated without violating the tests-with-behavior rule, so this remains one accepted over-budget work unit.
  - Rollback: remove `sqlite/search.rs` and the `mod.rs` declaration/doc update; schema v2 and existing repositories remain byte-for-byte unaffected.
  - Evidence: commit `00e6bef` (`feat: add durable inventory search`).

- [ ] **DQS-004 — Orchestrate freshness-aware query services** *(in progress)*
  - Route: delegated multi-file writer; application service plus exports/tests.
  - Outcome: implement `list_characters`, `search_inventory`, and `inspect_item` over the sync/repository seams; refresh non-fresh projections, preserve typed authorization/provider/storage errors, return bounded ambiguity candidates, and expose sanitized freshness metadata.
  - Likely surfaces: new `crates/destiny-app/src/queries.rs`, `crates/destiny-app/src/lib.rs`, and focused service tests.
  - Check: fresh skip and stale/missing refresh; refresh failure preservation; list ordering; missing/unique/ambiguous search; exact inspection; definition/instance separation; app tests/clippy/fmt/diff; independent verification per native risk assessment.
  - Rollback: remove the query service and exports without changing sync, storage, or query contracts.
  - Evidence: pending.

- [ ] **DQS-005 — Prove the offline SQLite query journey**
  - Route: delegated integration-harness writer plus independent read-only verification.
  - Outcome: compose real `SqliteStore`, real query services, deterministic fixtures, and only the existing injected gateway/clock seams; prove durable list/search/inspect, stale-triggered refresh, ambiguity, reopen, bounded results, and secret absence.
  - Likely surfaces: a new focused integration test under `crates/destiny-adapters/tests/` and only narrowly required sanitized fixture additions.
  - Check: focused integration tests plus the complete effective check set.
  - Rollback: remove only the query journey test/fixture additions; production behavior remains intact.
  - Evidence: pending.

- [ ] **DQS-006 — Verify and close WU-007**
  - Route: delegated independent full-range verification; parent reconciles tracker, mirror, and closure.
  - Outcome: verify the entire WU-007 range, record every check and manual limitation, close the feature, and identify WU-008/WU-011 as the next adapter work.
  - Check: complete effective set; committed-range inspection; clean status; no direct provider or secret-bearing boundary in query outputs.
  - Rollback: reset this feature branch to `8a2250e` to remove only WU-007.
  - Evidence: pending.

## Acceptance criteria

- A caller can list normalized characters through an adapter-neutral service with revision, checked time, source, and freshness.
- Inventory search supports bounded deterministic name/type/tag matching and exact perk-hash filtering over concrete instanced items.
- Missing, unique, and ambiguous matches are explicit; no display-name query silently selects a concrete item instance.
- Item inspection requires a concrete instance identity and reports its Manifest definition separately from current instance state.
- Fresh reads may reuse the projection; missing, stale, or invalid-source reads synchronize before returning or fail with the existing typed sanitized error.
- Fake and SQLite search implementations obey the same contract and deterministic ordering/limits.
- Offline integration proves durable list/search/inspect, stale refresh, ambiguity, reopen, and absence of secret/raw-provider material.
- Every completed task records observed checks, rollback boundary, and work-unit commit identity.

## Progress

- WU-006 and the OAuth callback test-isolation maintenance fix are complete and pushed through `8a2250e`.
- Read-only mapping confirmed there is no current query module, `SearchRepository`, definition enumeration, inventory query API, or SQLite search index.
- Existing `ProfileSyncService`, `FreshnessPolicy`, `Resolution`, definition/profile repositories, deterministic fakes, and SQLite projection provide the required foundations.
- No product decision blocks implementation. Textual perk-name search is impossible with the current data model and remains explicitly outside WU-007; exact perk-hash filtering satisfies the available contract.

## Verification evidence

- DQS-001 repository mapping covered core domain/state/ports/errors, app sync/test-support, SQLite definitions/inventory/migrations, fixtures, architecture, implementation plan, and prior ODD trackers.
- DQS-001 is complete in `e6d78f0` with boundary evidence in `bd90b9c`.
- DQS-002 contracts and fake parity are complete in `053fe2a`; independent verification passed after bounding caller-controlled hash filters and completing the public exports, with no blocker/high/medium finding remaining.
- DQS-003 durable SQLite search is complete in `00e6bef`; the corrected source-table design passed independent verification with no blocker/high/medium finding.

## Next step

Delegate DQS-004 freshness-aware application query orchestration over the verified search and sync seams.
