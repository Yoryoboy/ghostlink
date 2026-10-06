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
- [x] **GPM-003a — Stack-aware snapshot contracts.** Preserve stack
  observations through metadata cloning, multiset equality and in-memory
  replacement. Validate owner references; mechanically propagate empty stacks
  through existing constructors. Until durable support lands, SQLite explicitly
  rejects non-empty stack snapshots rather than dropping data.
- [ ] **GPM-003b — Combined SQLite v16 and Postmaster safety.** Persist
  stacks and explicit Postmaster ownership in one migration; preserve all four
  child projections, indexes and metadata; atomic readback/replacement and
  existing move/equip/lock exclusions. Reject unsafe downgrade without changing
  data; no silent relabeling or projection invalidation.
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
  writes. Tracker and GPM-002 committed together as `1dc9158`.
  Full Engram copy uses topic `odd/ghostlink-postmaster/tasks`.
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
- Explorer `muwy5mlp-4-e2z0` completed the GPM-003 map. Split into GPM-003a
  (snapshot contract plus fail-closed durable bridge) and GPM-003b (one combined
  v16 migration and Postmaster ownership). GPM-003a is COMPLETE;
  GPM-003b is IN PROGRESS (implementation delegation).
- GPM-003a worker `muwy92h2-5-3djs`: completed snapshot stack fields, multiset
  equality/owner validation, fake preservation, mechanical literal propagation,
  and v15 SQLite rejection before transaction creation. RED: missing fields
  (E0560/E0609); GREEN: 3 focused core tests, 1 app and 1 SQLite regression.
  Workspace tests: 1,476 default / 1,493 all-features; fmt, clippy and diff check
  passed. Derived CLI runtime snapshot preserves first_snapshot.stacks.
- GPM-003a native assess was unassessable due undeclared untracked files;
  independent verifier `muwylpuq-6-7976`: PASS, no findings. All six authorized
  checks passed; source remained unchanged. Tracker-only concurrent update was
  confirmed as parent bookkeeping. Full-suite totals above remain worker evidence.
  No v16 migration yet.
- Migration hazard: dropping/rebuilding `items` can cascade-delete item_perks,
  item_plugs, item_stats and item_available_plugs. Foreign keys are enabled and
  the runner is already in a transaction; foreign_keys=OFF inside migration SQL
  is not a valid workaround. Preservation tests are mandatory.
- Keep `--character` and carried-inventory filters restricted to Character,
  never implicitly Postmaster(owner); lock's fallback remains Vault-only.
  Pull-contract gaps do not block safe snapshot/persistence work.
- Feature A live checks remain pending; no claim of live provider validation.
