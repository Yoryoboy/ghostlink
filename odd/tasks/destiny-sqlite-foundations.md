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

- [x] **DSF-003 — Add SQLite dependency, connection policy, and reversible migrations**
  - Route: delegated adapter writer; first internal WU-005 review slice. Native assessment was unavailable and required independent high-risk verification. The initial independent review found a blocking canonical-decimal range hole; the writer added explicit maximum lengths, STRICT tables, integer storage guards, exact boundary tests, and an exact dependency requirement before independent re-verification passed.
  - Outcome: pinned `rusqlite =0.40.2` with bundled SQLite, added a single-connection synchronous `destiny-adapters::sqlite::SqliteStore`, foreign keys, a 5-second busy timeout, trusted-schema hardening, file WAL policy, in-memory policy, explicit reversible v1 migrations, future-version refusal, and six normalized secret-free STRICT tables for metadata, definitions/tags, active profile, characters, and items. No repository implementation was added.
  - Check: adapter check/clippy passed; 17 focused SQLite tests, 84 adapter tests, and 160 workspace tests passed; migration rollback/version atomicity, v1→v0→v1 reversibility, FK/CHECK enforcement, exact u64/u32 decimal bounds, integer storage, schema secrecy, reopen behavior, and one active rusqlite/libsqlite3-sys path were independently verified. One pre-existing OAuth timeout test failed once and then passed 6 isolated runs plus the final full-workspace rerun; it remains a non-blocking follow-up.
  - Review workload: 1,415 changed lines, including 894 test lines and 100 lockfile lines. This exceeds the 400-line heuristic but stays within one indivisible migration/schema contract slice; repository behavior remains separated into DSF-004/005.
  - Evidence: commit `f96ad55` (`feat: add SQLite schema migrations`).
  - Rollback: remove dependency/module/migration files and restore lock/manifests; WU-004 fake remains intact.

- [x] **DSF-004 — Implement transactional SQLite definition repository**
  - Route: delegated adapter writer plus independent high-risk verification because native assessment remained unavailable. Review identified a misleading committed scope guard and an order-identity divergence with the in-memory fake; both were corrected and independently rechecked before commit.
  - Outcome: implemented `DefinitionRepository` for `SqliteStore` with deterministic reads, canonical decimal parsing, pre-transaction validation, one `IMMEDIATE` transaction, complete structural set comparison, metadata-only freshness refresh, revision retain/advance/overflow, full catalog/tag replacement, rollback-on-drop, and exact file reopen persistence. Definition order is not set identity; tag order remains identity across both fake and SQLite repositories.
  - Check: core 16 passed; app test-support 61 passed; 13 focused definition tests; adapters 97 passed; workspace 174 passed; adapter all-target check/clippy passed; invalid input, mid-write constraint failure, overflow, corruption, stale-row removal, reopen, casing/tag determinism, error sanitization, and contract parity independently passed.
  - Review workload: 1,256 changed lines, including 736 focused test lines. The implementation remains one transactional repository contract and does not include profile persistence.
  - Evidence: commit `4f2e735` (`feat: persist Manifest definitions in SQLite`).
  - Rollback: remove only definition repository modules/tests and the narrow shared content-equality parity correction while preserving the v1 schema and profile boundary.

- [x] **DSF-005 — Implement transactional SQLite profile projection repository**
  - Route: two broad writer attempts failed without changes, so the slice was decomposed into sequential delegated core/fake semantics and SQLite repository writers, then independently verified under the unavailable-native high-risk fallback. Independent review found one defensive corrupt-extra-anchor gap; the writer fixed it and the verifier rechecked it.
  - Outcome: implemented the single-active-profile `InventoryRepository` with shared order-insensitive structural content identity, unique-ID/location-reference validation, canonical deterministic ordering, exact typed location and range reconstruction, all-or-none projection reads, one `IMMEDIATE` replacement transaction, revision retain/advance/overflow, metadata-only freshness refresh, complete stale-row removal, rollback preservation, file reopen durability, and Manifest dataset isolation. No profile provider mapping was added.
  - Check: core 16 passed; app test-support 67 passed; 17 focused inventory tests; adapters 114 passed; workspace 197 passed; adapter all-target check/clippy passed. Full field/location round trips, 19 content mutations, permutations, invalid source/duplicates/dangling refs, overflow, mid-write rollback, corrupt/partial state, exact-one profile anchor, stale removal, reopen, secrecy, and Manifest coexistence independently passed.
  - Review workload: 1,745 changed lines, including approximately 900 focused test lines. The slice contains one cohesive repository contract; provider profile normalization remains isolated to WU-006.
  - Evidence: commit `20a0b5c` (`feat: persist profile projections in SQLite`).
  - Rollback: remove only inventory repository modules/tests and shared profile content/validity semantics, leaving definition persistence and v1 schema intact.

- [ ] **DSF-006 — Verify concurrency, secrecy, persistence, and close WU-005** *(in progress)*
  - Route: native committed-range assessment plus delegated independent verification; unassessable remains high risk. Verification command execution is delegated.
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
- DSF-003 added the exact bundled SQLite dependency, hardened connection policy, reversible migration runner, normalized STRICT schema, and 17 focused tests in `f96ad55` after fixing an independent-review blocker in canonical decimal bounds.
- DSF-004 added atomic durable Manifest definition persistence in `4f2e735`, aligned complete-set order semantics with the in-memory fake, and proved rollback, overflow, corruption, and reopen behavior.
- DSF-005 added atomic single-active-profile persistence in `20a0b5c`, aligned unordered projection semantics with the fake, and proved all-or-none reads, exact typed reconstruction, rollback, corruption, coexistence, and reopen behavior.

## Verification evidence

- DSF-001 tracker readback and clean-branch checks passed.
- DSF-002 writer and independent verifier both observed clean formatting, diff checks, focused tests, focused clippy, and the offline adapter fixture. Native assessment was unavailable/schema-incompatible, so the candidate followed the required high-risk fallback path.
- DSF-003 writer and independent verifier observed clean formatting/check/clippy, 17 focused SQLite tests, 84 adapter tests, 160 workspace tests, reversible migrations, exact numeric bounds, schema secrecy, and dependency uniqueness. Independent review found and then cleared one blocking over-range decimal acceptance defect. Native assessment was unavailable, so the candidate followed the required high-risk fallback path.
- DSF-004 writer and independent verifier observed clean formatting/check/clippy, 13 focused definition repository tests, 97 adapter tests, 174 workspace tests, transaction atomicity, structural revision parity, corruption fail-closed behavior, and file reopen persistence. Two non-blocking review findings were corrected and rechecked before commit.
- DSF-005 sequential writers and the independent verifier observed clean formatting/check/clippy, 17 focused inventory tests, 114 adapter tests, 197 workspace tests, order-insensitive fake parity, transaction atomicity, corruption fail-closed behavior, Manifest coexistence, and file reopen persistence. One defensive extra-anchor finding was corrected and rechecked before commit.

## Next step

Run DSF-006 full committed-range verification, including two-connection lock/visibility behavior, database-byte secrecy, runtime close/reopen harness, dependency direction, binaries, and final review.
