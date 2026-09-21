# Destiny SQLite Foundations

## Objective

Implement WU-005 from `docs/mvp-implementation-plan.md`: establish a reversible SQLite schema and migrations plus durable, transactional, secret-free persistence for normalized Manifest definitions, the single active MVP profile projection, sync/freshness metadata, and repository-generated revisions.

## Problem

The workspace has provider-neutral records, repository ports, freshness/revision policies, deterministic in-memory fakes, and a complete Manifest acquisition pipeline, but every projection disappears on process exit. There is no migration authority, database adapter, durable revision allocation, transactional replacement, or freshness/source metadata store.

## Why

Later profile synchronization, search/inspect, mutation preflight, and recovery require one rebuildable local projection whose replacement is atomic, whose revisions are deterministic, whose schema is migratable, and whose database can never become a credential or raw-payload store.

## Scope

- Add a synchronous SQLite adapter using current `rusqlite` with bundled SQLite.
- Add explicit, monotonic, reversible migration/version management.
- Evolve core repository inputs/metadata so Manifest and profile replacement can atomically persist fetched-at/source/revision/schema markers.
- Preserve the WU-004 provider-neutral Manifest boundary and in-memory fake semantics.
- Persist normalized definitions/tags and one active MVP profile projection (membership, characters, item instances, locations, flags, quantities, power).
- Allocate or retain repository revisions by complete structural equality, never collision-prone fingerprints.
- Guarantee transactional replacement, rollback preservation, bounded lock waits, and same-process/same-database serialization behavior.
- Prove schema and stored bytes contain no tokens, API keys, authorization codes, callback values, credential material, or raw provider payloads.

## Constraints and decisions

- SQLite is a rebuildable projection, never provider authority.
- User decision: defer plan/execution-record persistence to WU-009/010, when their final domain types and execution semantics exist. WU-005 will not create speculative schemas.
- `rusqlite` 0.40.2 with `bundled` is selected for the synchronous architecture. Official/current docs identify `Connection::transaction` rollback-on-drop semantics and the bundled feature as appropriate for applications controlling their own databases. Sources: <https://docs.rs/crate/rusqlite/0.40.2>, <https://github.com/rusqlite/rusqlite>, <https://docs.rs/rusqlite/latest/rusqlite/struct.Connection.html>.
- Revision retention uses complete normalized structural equality against the committed projection. No new hash dependency or collision risk is introduced.
- The current `InventoryRepository` contract represents one active MVP account; SQLite stores and atomically replaces that one active projection. Multi-account repository selection is post-MVP unless a later port explicitly adds it.
- Database path ownership remains caller-supplied through adapter constructors. Platform data-directory/config wiring belongs to the later composition/packaging slice; WU-005 will support explicit file paths and in-memory/test connections.
- One repository-owned `rusqlite::Connection` serializes ordinary use; file-backed concurrent writers rely on SQLite transactions, bounded busy timeout, foreign keys, and rollback guarantees. No async runtime or connection pool.
- No profile-provider mapping or sync-reason orchestration: WU-006 produces normalized input and consumes these repositories.
- No raw provider JSON, secret columns, SQL logging of values, credential fallback, CLI/MCP persistence wiring, or presentation behavior.
- Automated checks remain offline and credential-free. Technical artifacts are English.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 2,200–3,200 authored changed lines across contracts, migrations, adapters, tests, and documentation.
- Chain strategy: `feature-branch-chain`.
- Branch: `feature/destiny-sqlite-foundations`, chained from completed WU-004 at `e1f7688`.
- Engram mirror topic: `odd/destiny-sqlite-foundations/tasks`.
- TDD mode: not configured; ordinary functional checks apply.
- Native RDD: disabled for this clone by prior explicit user decision; unavailable native assessment uses high-risk independent fallback verification.

## Effective checks

- Baseline: fmt; workspace clippy all targets/features with `-D warnings`; workspace locked tests; all-features locked tests; CLI/MCP help.
- Focused: core contracts; app test-support fakes; adapters all-features; migration/schema tests; definition/profile round trips; structural revision retention/advance/overflow; rollback preservation; two-connection lock behavior; foreign keys; reopened file persistence; migration idempotence/version rejection; secret/schema/file-byte scans.
- Runtime harness: a temporary file-backed SQLite database migrated, populated through repositories, closed, reopened, read back, replaced, and verified. No network or credentials.

## Tasks

- [x] **DSF-001 — Establish evidence, decisions, and feature boundary**
  - Route: delegated repository mapping plus parent primary-source dependency research and one user scope decision; mapping trigger fired because understanding spans more than four files.
  - Outcome: created the chained branch/tracker, recorded rusqlite and schema decisions, deferred plans/executions, and defined six bounded slices, forecast, exclusions, and checks.
  - Check: tracker and Engram mirror readback passed; `git diff --check` passed; branch was clean after the work-unit commit.
  - Evidence: commit `224c797` (`chore: start Destiny SQLite foundations`).
  - Rollback: remove only this branch and tracker before source implementation.

- [x] **DSF-002 — Add durable sync metadata and revision contracts/fakes**
  - Route: delegated multi-file writer over core/app test-support surfaces; writer trigger fired because the slice touched multiple non-trivial files. Native assessment was unavailable/schema-incompatible and therefore required high-risk independent verification.
  - Outcome: added provider-neutral committed dataset metadata, explicit definition replacement input, repository metadata reads, explicit Manifest publication time/source/revision, and fake repository structural revision semantics. Revision identity includes every normalized content field but excludes freshness time after fixed source validation, so a successful refresh can update `checked_at` without creating a false content revision. Invalid source and overflow preserve prior committed state.
  - Check: `cargo fmt --all -- --check`; core 16 passed; app test-support 60 passed; adapters 67 passed; focused all-target clippy with `-D warnings`; `git diff --check`; offline fixture pipeline; independent contract/atomicity/SQLite-implementability review passed with no candidate-caused high findings.
  - Evidence: commit `82b22ab` (`feat: add durable sync metadata contracts`).
  - Rollback: revert only WU-005 metadata/revision contract and fake changes.

- [ ] **DSF-003 — Add SQLite dependency, connection policy, and reversible migrations** *(in progress)*
  - Route: delegated adapter writer; first internal WU-005 review slice; writer trigger fires across manifest, lockfile, module, and migration/test files.
  - Outcome: pin `rusqlite` 0.40.2 bundled, add `destiny-adapters::sqlite`, explicit schema migrations, schema-version validation, foreign keys, bounded busy timeout, file/in-memory constructors, and normalized secret-free tables for definitions, tags, sync metadata, active profile, characters, and items.
  - Check: migration from empty DB, repeated open, version mismatch/future-version refusal, foreign-key enforcement, rollback on failed migration, schema inspection, and dependency-direction checks.
  - Rollback: remove dependency/module/migration files and restore lock/manifests; WU-004 fake remains intact.

- [ ] **DSF-004 — Implement transactional SQLite definition repository**
  - Route: delegated bounded adapter writer.
  - Outcome: implement `DefinitionRepository` with complete-set transactional replacement, deterministic lookup/version/metadata, structural unchanged detection, revision retain/advance, tag rows, failure rollback, and reopen persistence.
  - Check: contract parity with fake; unchanged/changed replacement; duplicate/empty/mixed-version rejection; rollback; reopened file round trip; no stale rows; secret sentinels absent.
  - Rollback: remove only definition tables/repository/migration portion and tests while preserving profile schema boundary where independently valid.

- [ ] **DSF-005 — Implement transactional SQLite profile projection repository**
  - Route: delegated bounded adapter writer; second internal WU-005 review slice.
  - Outcome: implement the single-active-account `InventoryRepository`, normalized character/item/location mapping, complete structural revision retention/advance, atomic replacement, freshness/source metadata, and reopened reads without provider mapping.
  - Check: round trips for every normalized field/location, membership replacement, equal/changed revisions, overflow, FK/invariant rejection, rollback preservation, and no orphan rows.
  - Rollback: remove only profile tables/repository/migration portion and tests, leaving definition persistence intact.

- [ ] **DSF-006 — Verify concurrency, secrecy, persistence, and close WU-005**
  - Route: native committed-range assessment plus delegated independent verification; unassessable remains high risk.
  - Outcome: validate full migration/repository range, two-connection locking/single-flight behavior, temporary file close/reopen harness, schema/file-byte secret exclusion, dependency direction, rollback boundaries, manual gates, and closure.
  - Check: all baseline/focused commands, temporary file runtime harness, dependency tree, full diff/status, and independent correctness/security review.
  - Rollback: reset this feature branch to `e1f7688` to remove only WU-005.

## Acceptance criteria

- A new or existing supported database migrates deterministically to the exact expected schema; unsupported future schema versions fail closed.
- Definition and active-profile replacements are atomic: every row and metadata marker changes together or the previous committed state remains fully readable.
- Unchanged complete normalized input retains its revision; any normalized field change advances exactly once; overflow fails without mutation.
- Freshness metadata includes dataset, source, fetched-at, revision, definition version, and schema version and rejects inconsistent/future inputs according to core policy.
- Reopening a file-backed database restores definitions, profile projections, metadata, and revisions exactly.
- Concurrent replacement attempts serialize or fail with a sanitized bounded storage error; no partial rows become visible.
- Schema and database contents contain no credential/token/API-key/auth-code/callback/raw-payload fields or sentinel values.
- WU-004 in-memory fakes remain available and contract-equivalent; WU-006 can supply normalized profile input without SQLite/provider coupling.
- Plans/execution records remain explicitly deferred to WU-009/010.

## Progress

- WU-004 completed at `e1f7688`; this branch starts from that clean boundary.
- Repository mapping identified existing normalized domain types, freshness/revision policies, repository ports, and deterministic fakes but no SQLite dependency/module, no durable sync metadata contract, and no database-path configuration.
- User selected deferral of plan/execution persistence to WU-009/010.
- Current official research supports `rusqlite` 0.40.2 with bundled SQLite for this synchronous application-owned database.
- DSF-001 established and committed the feature boundary in `224c797`.
- DSF-002 added durable provider-neutral metadata and collision-free revision semantics in `82b22ab`; repeated successful refreshes now update freshness without manufacturing content revisions.

## Verification evidence

- DSF-001 tracker readback and clean-branch checks passed.
- DSF-002 writer and independent verifier both observed clean formatting, diff checks, focused tests, focused clippy, and the offline adapter fixture. Native assessment was unavailable/schema-incompatible, so the candidate followed the required high-risk fallback path.

## Next step

Delegate DSF-003 for the pinned SQLite dependency, connection policy, migrations, schema inspection, and rollback/version tests.
