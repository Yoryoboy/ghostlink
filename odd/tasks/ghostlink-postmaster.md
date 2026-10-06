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
- Postmaster bucket identification RESOLVED from public official Manifest:
  `215593132` is Lost Items, location4, itemCount21, fifo=true,
  hasTransferDestination=false. Public Manifest succeeded without credentials.
  Definition source:
  <https://www.bungie.net/common/destiny2_content/json/en/DestinyInventoryBucketDefinition-a8ba855a-93c7-4014-8f2e-d92357fdfb42.json>
- Currency acquisition RESOLVED: component103 is requested and qualified as
  NotObserved (legacy decoder), Unavailable or Available (including empty/zero).
  This does not yet establish safe per-material destination reconciliation.
  Official DestinyProfileResponse confirms profileCurrencies is
  SingleComponentResponseOfDestinyInventoryComponent; characterInventories is
  character-keyed inventory. Do not count overlapping summaries twice.
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
- [x] **GPM-003b — Combined SQLite v16 and Postmaster safety.** Persist
  stacks and explicit Postmaster ownership in one migration; preserve all four
  child projections, indexes and metadata; atomic readback/replacement and
  existing move/equip/lock exclusions. Reject unsafe downgrade without changing
  data; no silent relabeling or projection invalidation.
- [x] **GPM-004a — Decode Postmaster and existing inventory stacks.**
  Correct character classification using verified location enum; preserve
  non-instanced quantity/container/runtime bucket in components 102/201;
  synthetic sanitized fixture journeys and fail-closed invalid ownership.
- [x] **GPM-004b1 — Persist currency coverage and zero balances.** Separate
  NotObserved/Unavailable/Available coverage from holdings and preserve it through
  snapshots/revisions/SQLite v17. Zero quantities allowed ONLY for profile currency
  balances; physical stacks remain positive. Legacy snapshots default NotObserved;
  no destructive downgrade of new information.
- [x] **GPM-004b2 — Acquire currencies and classify verified buckets.**
  Request103, decode profileCurrencies separately with explicit availability;
  normal gear reads tolerate genuinely unavailable optional currency data, not
  malformed currency records. Use officially verified Lost Items bucket for
  owner-aware classification; never sum overlapping source observations.
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
  GPM-003b is COMPLETE (worker checks and native assessment);
  GPM-004a, GPM-004b1 and GPM-004b2 are COMPLETE;
  GPM-005 is IN PROGRESS (read-only surface mapping).
- GPM-003a commit `1bca7d1` (321 additions / 8 deletions, including tests).
  Worker `muwy92h2-5-3djs`: completed snapshot stack fields, multiset
  equality/owner validation, fake preservation, mechanical literal propagation,
  and v15 SQLite rejection before transaction creation. RED: missing fields
  (E0560/E0609); GREEN: 3 focused core tests, 1 app and 1 SQLite regression.
  Workspace tests: 1,476 default / 1,493 all-features; fmt, clippy and diff check
  passed. Derived CLI runtime snapshot preserves first_snapshot.stacks.
- GPM-003a native assess was unassessable due undeclared untracked files;
  independent verifier `muwylpuq-6-7976`: PASS, no findings. All six authorized
  checks passed; source remained unchanged. Tracker-only concurrent update was
  confirmed as parent bookkeeping. Full-suite totals above remain worker evidence.
  No v16 migration in GPM-003a.
- GPM-003b commit `bdc8322`.
  Worker `muwyqnth-7-9weo`: combined v16 migration, durable stack
  roundtrip, owned ItemLocation::Postmaster and transfer/equip/lock exclusions.
  UP backs up/restores all four cascading child tables with foreign keys enabled;
  DOWN guards unrepresentable rows before permanent mutations. Duplicate stacks
  use snapshot-local ordinals, not provider identity. No acquisition/pull yet.
  RED: v15 refused stacks; missing Postmaster enum (E0599). GREEN: 120 SQLite
  tests and core/app/rendering regressions; workspace 1,486 / 1,503 tests,
  fmt/clippy/diff check passed. Two initial focused invocations used wrong feature
  or package name; corrected invocations passed, not hidden failures.
- GPM-003b ASSESS: medium risk, 14 changed paths / 753 changed lines;
  writerSelfVerification=true, independentVerifier=false. RDD off, outcome unknown,
  candidate not consumed. reviewDue=true (slice_budget_reached); no native review
  initiated. Exact provider continuation (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
- GPM-004a commit `f9d6a48`.
  Worker `muwze6zb-8-27zq`: keeps missing/null-ID inventory stacks,
  classifies explicit character location4 as Postmaster(owner), rejects equipped
  or unowned Postmaster, preserves omitted character-location compatibility.
  RED: discarded stacks and wrong location; GREEN: 125 focused tests and 1,511
  all-features workspace tests. fmt/clippy/diff check passed. Initial seven
  transfer regressions were fixed before completion by preserving legacy absent
  location behavior. Fixtures are synthetic; no live validation.
- GPM-004a ASSESS: medium risk, 4 paths / 476 changed lines; writer verification
  stands, independentVerifier=false. RDD off, outcome unknown, not consumed;
  reviewDue=true (slice_budget_reached). Exact continuation (not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
- GPM-004b exploration had no public HTTP tools; parent retrieved official
  Manifest/OpenAPI and pinned secondary DIM source at
  `d7c02e5cf9ba19e750bc626f497a7a069be23b08`. Bucket proof is primary above.
  Component wrapper privacy Private is not itself missing data; unavailable
  wrappers cannot become empty holdings. Preserve availability durably because
  legacy/incomplete snapshots remain readable after restart.
- GPM-004b design: zero currency balances are observations, not physical stacks;
  preserve them with scoped domain validation and v17 (do not rewrite committed
  v16). Coverage enum is per-snapshot, included in equality and readback. Keep
  ordinary gear reads available when optional currencies are unavailable, while
  stronger currency/pull claims require Available fresh coverage.
- GPM-004b1 commit `0a08c0d`.
  Worker `mux008qq-a-wocy`: ProfileCurrenciesCoverage
  NotObserved/Unavailable/Available on both snapshots, cloning/equality/fakes and
  SQLite17. Currency-only zero balances, physical stacks remain positive; known
  coverage or zero balances prevent destructive downgrade. RED: rejected zero,
  missing field/enum, unsupported SQLite roundtrip; GREEN: 16 core tests, 1 fake
  coverage test, 128 SQLite tests, 1,505 / 1,522 workspace tests. fmt/clippy/diff
  clean. Initial wrong feature and test FK setup corrected before final pass.
  ASSESS medium (29 paths / 763 diff lines), writerSelfVerification=true,
  independentVerifier=false; RDD off, outcome unknown, not consumed. reviewDue
  slice_budget_reached, no native review initiated. Exact continuation:
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
- GPM-004b2 worker `mux0emlt-b-y1b8`: component103 acquisition and
  strict optional currency decoding; requested omission becomes Unavailable,
  legacy omission NotObserved. Private usable data accepted; available empty/zero
  retained; invalid rows reject entire snapshot. Multiplicity/provenance retained.
  Verified runtime bucket identifies character Postmaster even location1/absent;
  equipped and unowned Postmaster refused. Existing public signatures unchanged.
  RED: wrong query, coverage, bucket classification (after correcting test import).
  GREEN: 131 profile tests, one durable/atomic journey, six weekly tests and 1,528
  all-features workspace tests. fmt/clippy/diff passed after format/lint correction.
  Journey added after implementation, not separately RED. Synthetic fixtures only;
  compiler/clippy passed despite false-positive lens JSON indexing diagnostics.
  ASSESS medium (5 paths / 456 diff lines), writerSelfVerification=true,
  independentVerifier=false; RDD off, outcome unknown, not consumed. reviewDue
  slice_budget_reached, no native review initiated. Exact continuation:
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
- DIM (secondary, not an official guarantee) uses runtime buckets for Postmaster
  and sends item.id (normalized to '0' for noninstances), hash and amount in
  PullFromPostmaster; source files bungie-api/destiny2-api.ts:192-203 and
  inventory/store/d2-item-factory.ts:359-371,523-535 at that pinned commit.
  Partial/duplicate stack addressing still requires conservative treatment.
  DIM also checks doesPostmasterPullHaveSideEffects; verify the official field
  and refuse unintended redemption/side effects before enabling pull actions.
- Migration hazard: dropping/rebuilding `items` can cascade-delete item_perks,
  item_plugs, item_stats and item_available_plugs. Foreign keys are enabled and
  the runner is already in a transaction; foreign_keys=OFF inside migration SQL
  is not a valid workaround. Preservation tests are mandatory.
- Keep `--character` and carried-inventory filters restricted to Character,
  never implicitly Postmaster(owner); lock's fallback remains Vault-only.
  Pull-contract gaps do not block safe snapshot/persistence work.
- Feature A live checks remain pending; no claim of live provider validation.
