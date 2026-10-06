# Full Postmaster Support — IN PROGRESS

Expose and recover Postmaster gear, materials and consumables without inventing
instance identifiers or claiming verification from an API acknowledgement alone.

## Scope and authorization

- User approved Feature B order: Postmaster → saved loadout apply → equip_items.
- Postmaster includes non-instanced stacks; do not silently reduce to gear-only.
- Subsequent loadout work must verify equipped items and exact indexed sockets.
- Worktree: `../ghostlink-inventory`; branch `feature/ghostlink-postmaster` from
  Feature A at `4adc624`. Feature A remains on `feature/inventory-actions-core`.
- No publishing, PR creation, merge or live account mutations are authorized here.

## Design and evidence

- Preserve genuine `ItemInstanceId` semantics. Non-instanced inventory has a
  separate stack observation model with container/owner, actual bucket, item
  hash and quantity. Snapshot-local row identity is not a Bungie action target.
- Preserve multiplicity: same-hash stacks must not be collapsed or keyed uniquely
  by owner/hash. Projection equality must be order-independent but sensitive to
  quantities and duplicate counts.
- Classify character Postmaster explicitly, not as carried equipment. Existing
  move/equip/lock must not accidentally admit the new location.
- SQLite migrations preserve committed instances, child tables and metadata;
  old character rows cannot be reclassified without a fresh profile acquisition.
- Fresh preflight, one-shot pull, forced post-action observation, honest partial
  or uncertain evidence. No automatic retries or rollback claims.
- Stack destinations may merge; compare aggregate quantities. Missing destination
  coverage, stale data or contradictory deltas cannot become verified success.
  Observation is not causal proof against external clients or game actions.

### Confirmed primary contracts

- Official operation: POST `/Destiny2/Actions/Items/PullFromPostmaster/`, scope
  `MoveEquipDestinyItems`, throttle 0.1 s; instanced items require BOTH reference
  hash and instance ID. Response is the standard int32 envelope.
- Official `Destiny.Requests.Actions.DestinyPostmasterTransferRequest`: fields
  `itemReferenceHash` (uint32), `stackSize` (int32), `itemId` (int64),
  `characterId` (int64), `membershipType` (int32).
- `DestinyItemComponent`: instance ID is nullable; non-instanced items have no
  distinct local qualities besides stack size; `bucketHash` is the actual
  container bucket. `Destiny.ItemLocation.Postmaster` is 4.
- Sources: Bungie official `openapi.json` and generated pages:
  <https://raw.githubusercontent.com/Bungie-net/api/master/openapi.json>
  <https://bungie-net.github.io/multi/operation_post_Destiny2-PullFromPostmaster.html>
  <https://bungie-net.github.io/multi/schema_Destiny-Requests-Actions-DestinyPostmasterTransferRequest.html>
  <https://bungie-net.github.io/multi/schema_Destiny-Entities-Items-DestinyItemComponent.html>

### Remaining contract gates (before pull implementation)

- Non-instanced `itemId` wire convention: omitted versus `"0"`.
- Partial-pull and duplicate-hash selection semantics for `stackSize`.
- Postmaster bucket identification: do not hard-code an unverified hash.
- Currency/material destination coverage: existing components 102/201 omit 103;
  establish coverage and avoid counting overlapping summaries twice.
- Representative sanitized fixtures and eventual owner-run live verification.

## Tasks

- [x] **GPM-001 — Establish branch, tracker and recovery copy.**
- [x] **GPM-002 — Validated non-instanced stack observation contracts.**
  Domain types and invariant tests, preserving duplicates and separating
  observations from actionable instance IDs. No production acquisition yet.
- [ ] **GPM-003 — Persist complete inventory projections.** Explicit
  Postmaster ownership, stack-aware snapshots/equality, SQLite migration,
  atomic replacement/readback, fakes and legacy mutation safety regressions.
- [ ] **GPM-004 — Decode Postmaster and stacks with coverage evidence.**
  Correct character classification; preserve quantity/container/bucket;
  establish destination component coverage and sanitized fixture journeys.
- [ ] **GPM-005 — Expose Postmaster reads.** CLI/MCP search or dedicated
  listing with owner/location parity, stack quantities and honest coverage;
  docs and skill alongside tests.
- [ ] **GPM-006 — Resolve pull wire contracts.** Primary-source contract
  evidence, explicit stack ambiguity/capacity rules, request fixtures.
- [ ] **GPM-007 — Dedicated pull adapter.** Core request/gateway and strict
  endpoint adapter, sanitized outcomes, one dispatch and encoding tests.
- [ ] **GPM-008 — Verified pull service.** Owned instance/stack selection,
  membership/revision preflight, bounded quantity, conservative post-action
  evidence, no retry and provider-lag tests.
- [ ] **GPM-009 — CLI/MCP pull exposure.** Thin adapters, pipeline/stdio
  coverage, docs, skill and exact outcome/exit semantics.
- [ ] **GPM-010 — Close technical verification and record live gaps.**
  Functional checks, assessment/required independent verification, commits,
  pending owner-run checks and readiness for Loadouts.

## Verification and delivery

- Deterministic behavior changes use observed RED → GREEN → alternate cases
  → refactor. Passive tracking documentation has no meaningful RED.
- Focused Rust tests per unit; closure: fmt, clippy default/all-features,
  workspace tests default/all-features, diff check, help/stdio surfaces.
- Approximate 400 changed lines per unit is advisory; preserve meaningful tests
  and clarity. Split by coherent behavior, never minify or omit evidence.
- Reviewable local work-unit commits; delivery slices determined from actual
  diffs. Push/PR/merge remain separate user decisions. RDD switch last read off.

## Progress and next step

- GPM-001: branch created from clean `4adc624`; tracker created before source
  writes. Full Engram copy uses topic `odd/ghostlink-postmaster/tasks`.
- GPM-002 COMPLETE (independently verified): worker `muwy0kc9-2-eynx`
  completed `core/src/stacks.rs`, exports and ten invariant tests. Observed RED:
  missing types (E0432). GREEN: 10 stack tests, 141 core tests including rustdoc;
  fmt/clippy/diff check passed after formatting cleanup. Hashes accept the
  existing full uint32 range (including zero); quantities are positive int32.
  No decoding, snapshot, persistence or action changes yet.
- Native assessment was unassessable because untracked files require a scope
  declaration; its conservative independent-verifier plan was followed.
  Verifier `muwy5mk7-3-wq4a`: PASS, no findings; 141 core tests (130 unit,
  10 integration, 1 compile-fail rustdoc), fmt/clippy/diff check all passed.
  Untracked source files were inspected separately from `git diff --check`.
- Explorer `muwy5mlp-4-e2z0` maps GPM-003 migration/snapshot surfaces read-only.
  Pull-contract gaps do not block this persistence planning.
- Feature A live checks remain pending; no claim of live provider validation.
