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
- Redemption/destructive side effects CONFIRMED: top-level item definition
  doesPostmasterPullHaveSideEffects indicates potential destruction, not a reliable
  prediction of whether it will happen. Official docs require checking the flag.
  Acquisition/persistence must retain explicit false versus unknown; reject true
  or unknown rather than authorize unintended redemption. This gate is NOT yet
  implemented in the pull service (which does not exist).
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
- [x] **GPM-005a — Shared Postmaster read projection.** Dedicated core
  result and QueryService method over the synchronized committed snapshot;
  separate real instances and non-instanced stacks, optional names, multiplicity,
  owner filtering, empty groups, freshness and qualified currency coverage.
- [x] **GPM-005b — CLI Postmaster listing.** `ghostlink postmaster`
  with optional character ID/class and JSON; shared QueryOperations forwarding,
  pipeline/help checks, CLI docs and skill, no pull action.
- [x] **GPM-005c — MCP Postmaster listing.** `get_postmaster` with CLI
  parity, strict schema, forwarding wrappers, discovery/stdio checks and MCP docs.
- [x] **GPM-006a1 — Acquire qualified definition metadata.** Flat optional
  side-effect flag, instance kind, signed int32 maximum stack size and uniqueness
  label; strict raw decoding/normalization, explicit false/empty versus unknown,
  default-unknown literal propagation and fake preservation. Pre-v18 publication
  refuses any known field before mutation; no data loss in interim storage.
- [x] **GPM-006a2 — Persist definition metadata in SQLite v18.** Additive
  nullable columns, all publication/readback/catalog/equality paths, metadata-only
  change detection and guarded downgrade/preservation tests. No readiness claim
  between acquisition and durable support; no pull eligibility from unknown data.
- [ ] **GPM-006 — Resolve pull wire contracts.** Primary-source contract
  evidence, explicit stack ambiguity/capacity rules, request fixtures. Non-instance
  itemId, partial/duplicate selection and per-material coverage remain unresolved;
  this task is not complete just because a limited instanced slice is possible.
- [x] **GPM-007a — Instanced pull core contract.** Genuine positive int64
  IDs, uint32 reference hash, representable membership type, fixed quantity one;
  immutable request and one-shot gateway/outcomes. No anonymous stack request.
- [x] **GPM-007b — Instanced pull adapter.** Dedicated exact allowlisted
  endpoint, both hash/real ID, credential/dispatch bounds, sanitized outcomes and
  explicit uncertainty; no inherited transfer-specific refusal assumptions.
- [x] **GPM-008a1 — Fresh runtime bucket core contract.** Acquisition envelope
  pairing existing InventorySnapshotInput with unavailable or complete instanced
  actual-bucket observations. Exact ID/owner/location correspondence and uint32
  validation; default scoped gateway method delegates once and marks unavailable.
  No default definition-bucket substitution, publication or action dispatch.
- [x] **GPM-008a2 — Acquire instanced runtime buckets.** Scoped decoder/gateway
  observations, retain omitted evidence as unavailable; existing reads compatible.
  Exact source surfaces: adapters/src/profile.rs, profile_gateway.rs and export-only
  lib.rs. Tests stay in existing nested modules; fixtures mutate in memory only.
  Share one normalization pass and one scoped acquisition; requested currency
  coverage adjustment precedes immutable envelope creation. Missing any accepted
  instance bucket gives None; zero instances gives Some(empty). Preserve current
  rejection of explicit null or malformed present bucketHash in BOTH decoders;
  no new nullable policy flag or weakened stack bucket validation.
- [ ] **GPM-008a3 — Publish then expose fresh evidence.** Share sync acquisition,
  membership validation/filtering/publication. Return observations only after
  successful replace_current alongside THAT returned snapshot, never an unrelated
  reload. No proof on freshness skips/failure. Ephemeral buckets do not advance
  inventory revision: planning/execution reacquire and compare proof fingerprints.
  Scout `mux9qcie-14-kxib` mapped additive sync_with_observations outcome:
  SkippedFresh(metadata) versus Refreshed(private PublishedProfileAcquisition).
  Keep ordinary sync on the legacy scoped loader for ordering/override compatibility;
  share orchestration but select acquisition route, no extra resolve/load/retry.
  New route preserves post-filter candidate, compares returned snapshot via
  InventorySnapshotInput::with_metadata and full content_eq, plus exact checked_at
  and source/Profile provenance (excluded from content_eq). Revalidate complete
  observations against returned input. No public/default published-proof constructor.
  Returned mismatch suppresses proof but cannot roll back already-successful
  malicious repository publication. Exact app surfaces: src/sync.rs, export-only
  src/lib.rs, narrow src/test_support.rs; actual journey test is
  crates/destiny-adapters/tests/profile_sync_journey.rs, not app/tests.
  Canonicalization/no-op metadata and changed bucket with unchanged revision need
  coverage; zero post-write unrelated load_current calls for proof. No writer yet.
- [ ] **GPM-008b — Qualify equipment capacity policy.** Current public Manifest
  index, exact supported bucket policy/provenance and definition-version binding;
  missing/changed/unqualified evidence closes the proof. Scope still to map fully.
- [ ] **GPM-008 — Verified pull service.** Owned instance/stack selection,
  membership/revision preflight, bounded quantity, conservative post-action
  evidence, no retry and provider-lag tests.
  Read-only scouts found no equipment capacity policy. Official bucket schema
  and current English Manifest establish all eight supported equipment buckets:
  itemCount=10, character scope, location=1, category=3, enabled, non-redacted,
  non-FIFO, transfer destination present. Version
  244213.26.06.29.2000-1-bnet.65864 points to bucket artifact
  DestinyInventoryBucketDefinition-a8ba855a-93c7-4014-8f2e-d92357fdfb42.json.
  Possible conservative proof: count ALL owner carried instances (equipped
  included) plus ALL owner character-inventory stack records; admit only total
  <10 using checked arithmetic. Duplicate stacks remain separate; quantities
  are not slot counts. Decoder rejects duplicate IDs and incomplete owner groups.
  This overcounts unrelated buckets and refuses most typical characters: refusal
  means capacity unproven, not target bucket full. Strategy not yet selected;
  policy applicability/freshness/version qualification remains a gate. Pinned
  public facts alone do not establish it. Private non-clone plans consumed by
  value avoid existing ticket-reminting semantics; bind definition version AND
  eligibility fingerprint. Force Preflight/PostAction acquisitions, no stale
  fallback. Parent selected fresh-only actual bucket evidence through successful
  existing sync publication instead of persisted ItemInstance/SQLite changes.
  Actual seams are InventorySnapshotInput and metadata-only sync, not previously
  conjectured acquisition/result types. Core contract is the next bounded unit;
  acquisition, successful-publication evidence and capacity policy follow before
  service dispatch. Normal-bucket inference and restrictive all-owner proof are
  not selected. No user-facing/runtime action binding from research alone.
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

- GPM-008a3 IN PROGRESS: successful-publication evidence outcome with same
  returned snapshot and full filtered-candidate correspondence. Exact four mapped
  app/journey surfaces; ordinary legacy loader compatibility and zero post-write
  reload for proof. No policy/service dispatch or durable buckets in this unit.
  Writer `muxa5mzh-15-272e` active; no other source writer. Tests include real
  in-memory SQLite journey plus isolated returned-snapshot mismatch controls.
- GPM-008a2 COMPLETE: retain actual buckets from a single scoped profile
  acquisition using shared normalization; exact three adapter surfaces above.
  Work-unit commit: `5c113f0`.
  Test-first, no core/sync/SQLite/runtime-action changes. Explicit null/present
  malformed buckets stay rejected; omitted buckets give unavailable evidence.
  Writer `mux9p7td-13-u1ap` COMPLETE: decode_profile_with_observations and
  production scoped override share one UniqueValue parse/acquisition. Accepted
  raw records retain actual bucket and normalized owner/location; omission makes
  whole proof unavailable. Authenticated currency adjustment precedes pairing.
  Observed RED missing decoder API and default gateway None; GREEN seven tests.
  Checks: 4 observation decoder/3 gateway/50 profile/78 profile gateway/9 currency/
  11 Postmaster/6 core contract; 1,582 default / 1,600 all-features offline tests,
  fmt/clippy/diff pass. Initial enum names/format errors fixed; zero-match core
  filter corrected to explicit test target. No live validation or out-of-scope edit.
  Parent source read confirms shared single parse/acquisition and unchanged scopes.
  ASSESS medium: 4 paths/460 diff lines, runtime large writer profile; writer
  self-verification stands, independentVerifier=false. RDD off, outcome unknown,
  candidate not consumed; reviewDue=slice_budget_reached. Exact native continuation
  (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  No native approval, capacity or runtime action binding. Read-only sync scout
  complete; publication unit follows this verified closure.
- GPM-008a1 COMPLETE (independently verified): core fresh runtime bucket
  acquisition contract only. Work-unit commit: `e68233b`.
  Parent-approved source surfaces: core/src/profile_observations.rs, ports.rs,
  lib.rs and tests/profile_observations.rs. No adapter, sync, SQLite or dispatch
  writes in this unit. Deterministic test-first and self-verification required;
  parent assessment/required independent verification precedes closure/commit.
  Writer `mux99b2t-z-fllp` COMPLETE: 395 additions across four allowed paths.
  ProfileAcquisition::new/input/observations/into_parts pairs immutable input with
  None (unavailable) or Some(complete observations). InstancedBucketObservation
  validates uint32 actual bucket and exact instance/location correspondence.
  Default scoped loader delegates once without resolving membership/retrying.
  RED missing types/method; GREEN six focused tests and two compile-fail docs.
  Input/duplicate stacks/currency coverage/metadata preserved; incomplete/extra/
  duplicate IDs, wrong owner/location and invalid input/buckets refused.
  Worker 167 core tests, 1,575 default / 1,593 all-features offline tests;
  fmt/clippy pass after formatting correction. Parent source spot check agrees.
  ASSESS unassessable due untracked module/tests; high-risk independent-verifier
  plan followed. Verifier `mux9gxhj-10-0xvc` PASS: all six focused/167 core/
  1,575 default / 1,593 all-features plus fmt/clippy/diff/explicit whitespace
  green; inspected all new files and unchanged source hashes. Non-blocking test
  limitation: wrong owner/location cases were confounded by missing second ID.
  Parent mechanically strengthened that one test: valid complete control plus
  complete ID vectors with only first location wrong. Production unchanged; no
  meaningful RED for already-correct behavior. Final independent recheck
  `mux9lryz-12-cvnj` PASS: full valid control accepted and every wrong owner/location
  vector complete, resolving confounding. All six focused/167 core/1,575 default/
  1,593 all-features, fmt/clippy/diff/new-file whitespace pass; production and
  build-input hashes unchanged. No mutation-run RED or native approval claimed.
  No adapter/sync/publication/durability/revision/action proof in this core unit.
  Scout `mux9htj8-11-e5fo` complete; exact adapter surfaces and missing-versus-
  malformed bucket decision recorded above. No blanket null acceptance.
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
  GPM-005a is COMPLETE (independently verified);
  GPM-005b is COMPLETE (independently verified);
  GPM-005c is COMPLETE (independently verified);
  GPM-006a1 is COMPLETE (worker checks and native risk assessment);
  GPM-006a2 is COMPLETE (durable SQLite v18, worker checks and risk assessment);
  GPM-006 is IN PROGRESS (wire contracts with explicit technical gates).
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
- GPM-004b2 commit `eb75019`.
  Worker `mux0emlt-b-y1b8`: component103 acquisition and
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
- GPM-005a commit `37d7196`.
  Worker `mux0umr1-d-fkx1`: dedicated DTOs and inherent
  QueryService::postmaster, no QueryOperations/frontends yet. Fourteen tests
  cover owner filtering, duplicate preservation, optional names, currency coverage,
  metadata/freshness/errors and contradictory repository state. RED E0599;
  GREEN after numeric fixture/test-fake setup corrections; fmt failures fixed.
  Worker 1,542 all-features tests, focused 14 app/1 existing core test, fmt/clippy
  and diff check passed. Test fakes cfg(test) repaired default test compilation;
  no production fixture exposure. Definition readiness only selected holdings;
  metadata errors always propagate. Approx. 832 authored diff lines, mostly tests.
  ASSESS unassessable (new untracked files require explicit declaration), so high
  risk independent-verifier plan applies. Verifier `mux19144-e-qkwu` PASS,
  no blocking findings: 14 focused app/1 core regression, 1,525 default and 1,542
  all-features tests (zero failures/ignored), fmt/clippy/diff passed. Explicitly
  inspected both untracked files and their whitespace; deterministic-order test
  bypasses fake canonicalization. Scope matches five candidate files plus tracker.
  RDD off; no native approval, live accounts or external contracts validated.
  CodeGraph status resolved parent index; no index initialized, checkout-local
  evidence used exclusively.
- GPM-005 exploration `mux0qx3l-c-l38n`: dedicated projection avoids
  search's required ItemInstance/definition. Existing load_current snapshot and
  ordinary sync gate suffice; no migration/SQL search change. Separate instance
  and stack arrays per owned character preserve duplicate stack rows; optional
  definitions never erase observations. Exact ID/unique case-insensitive class
  filtering; propagate refresh errors rather than serve stale holdings. Metadata
  reports committed revision/timestamp/source/freshness, not live guarantees.
  Component103 coverage describes only currency balances, not Postmaster source
  completeness; legacy snapshots cannot prove coverage retrospectively.
  Gear has no persisted runtime bucket: omit it, never substitute definition
  bucket. Stack runtime buckets are retained. Split shared projection, CLI and
  MCP into separate work units; no extra runtime/account-scoping rewrite.
- GPM-005b commit `84b1e1b`.
  Worker `mux1fmc6-g-cud2`: strict CLI postmaster command,
  required QueryOperations forwarding, text/JSON output, offline SQLite/service
  pipeline and README/CLI docs/skill. RED: InvalidSubcommand, missing request,
  trait/module/dispatch/fake. GREEN: seven focused CLI tests; 5 skill drift,
  9 setup and 14 app tests; help/fmt/clippy/diff passed, 1,531 default / 1,548
  all-features tests. Intermediate fixture readiness, category names/imports,
  sanitization expectations and formatting errors corrected before final pass.
  Approx. 502 authored diff lines. No MCP changes/actions/live validation.
  JSON: kind=postmaster; metadata checked_at/freshness/revision/source;
  profile_currencies_coverage not_observed/unavailable/available; character groups
  with character_id/class, instances(instance_id/item_hash/name/quantity),
  stacks(item_hash/name/bucket_hash/quantity). IDs strings, numeric fields numeric,
  optional names/classes null; duplicates preserved, no inferred gear bucket.
  Text uses control-character sanitization and honest committed-read qualifiers.
  ASSESS unassessable due new untracked files; high-risk independent verification
  required. Verifier `mux1ulxs-h-c0wn` PASS, no blocking counterexample:
  7 focused CLI, 5 skill drift, 9 setup, 14 app; help/fmt/clippy and diff passed;
  1,531 default / 1,548 all-features tests, zero failures/ignored. All three
  untracked files explicitly read/whitespace checked. Pipeline exercises runner,
  trait, service and SQLite and refresh refusal. Duplicate stacks are injected
  for renderer assertions rather than passed through this SQLite CLI fixture;
  app tests separately cover stack projection. Synthetic only, not native approval.
- GPM-005c commit `215e30c`.
  Worker `mux1zi63-i-2jdy` partial: get_postmaster schema,
  dispatch and required forwarding through four wrappers; CLI JSON parity and
  actual child-process cases. Selector `character` follows existing MCP conventions;
  omitted/null selects all. Dedicated metadata DTO preserves CLI parity without
  existing MCP dataset/definition-version extras. RED missing discovery; focused
  GREEN 5 tests. Stdio currently E0433: app does not export StackObservation/
  StackContainer; other checks/docs/main expectations remain pending. No outside
  edits or completion claimed. Parent inspected existing app facade contract and
  approved ONLY additive re-exports in crates/destiny-app/src/lib.rs, avoiding a
  new destiny-core dependency explicitly forbidden by its architecture comment.
  Same writer resumed as `mux28y88-j-vgq0`: COMPLETE implementation and docs.
  Corrected E0433 then 18 stdio tests passed; focused 5 MCP/5 skill/14 app passed,
  1,535 default / 1,553 all-features workspace tests, fmt/clippy/diff green.
  Formatting/trailing-whitespace failures corrected; approx. 604 authored lines.
  Fixture refreshes stale synthetic seeds and commits actual gear/stacks through
  all four wrappers; no live validation or pull feature. Additive app exports only,
  no dependency changes. ASSESS unassessable due new untracked parity test;
  high-risk independent verification required. Verifier `mux2l3js-k-rm5b` PASS,
  no blocking counterexample. 5 MCP/18 stdio/5 skill/14 app, 1,535 default / 1,553
  all-features tests; fmt/clippy/diff passed and untracked parity file inspected.
  Confirmed all four wrappers and actual publication of duplicate stacks. Child
  fixture storage is IN-MEMORY FAKE, not SQLite: transport/service integration is
  proven, not durable readback or live-provider correctness. Default workspace
  compilation passes; stdio requires offline-fixtures. RDD off, no native approval.
- GPM-006 public parent research: official generated operation/request/item
  inventory block/item definition pages fetched without account access. Confirmed
  both reference and instance ID for instanced pulls; request integer widths,
  0.1s throttle and mutation scope. No explicit non-instance zero/omission, partial
  or duplicate selection guarantees in these pages. Inventory block exposes
  maxStackSize/stackUniqueLabel/normal/recovery bucket and isInstanceItem; these
  do not identify current rows or establish destination coverage.
  Official top-level side-effect flag passage retrieved from:
  <https://bungie-net.github.io/multi/schema_Destiny-Definitions-DestinyInventoryItemDefinition.html>.
  Scout `mux2r6j2-l-k25k` mapping definition safety metadata and remaining contracts
  read-only. Do not equate absent flag with explicit false or acknowledge pulls
  as observed success. No action code or live-account request initiated.
  Scout confirms absent safety/capacity fields in core definitions/candidates,
  raw Manifest decoding and normalized SQLite definition records; SQLite is v17.
  Follow-up `mux2usid-m-paqf` enumerates affected literal/fake/readback paths and
  coherent metadata acquisition/persistence boundaries before any writer.
  Parent targeted public search found official API wiki Postmaster Actions:
  <https://github.com/Bungie-net/api/wiki/Postmaster-Actions>.
  Retrieved passage confirms provider's conservative flag can have false positives;
  no consistent way exists to predict destructive side effects, and apparently
  benign items may be refused. Do not create a client override/whitelist. Wiki
  supplies no non-instance itemId or partial/duplicate selection guarantee.
- GPM-006a footprint scout `mux2usid-m-paqf`: four flat optional fields fit
  existing records/equality/clone paths. Tolerant OptionalField decoding would
  erase malformed safety values and is prohibited for these fields. Strict signed
  int32 width validation preserves supplied capacities; nonpositive capacities
  are not usable action proof. Uniqueness labels retain exact empty/text values.
  Approx. 34 mechanical constructor-bearing paths plus scoped behavior surfaces.
  Parent chose acquisition/bridge then v18 persistence as two review units; the
  intermediate bridge intentionally refuses Manifest publications with ANY known
  field, including false or empty label, before permanent mutation. It is local
  development scaffolding, not release/operational readiness. v17 remains intact;
  v18 uses additive columns rather than rebuilding items. No action code yet.
  Worker `mux30h29-n-7dlu` implementing GPM-006a1 within enumerated behavior
  surfaces and mechanical-only unknown literal propagation; no migration in this
  unit. Signed nonpositive capacity metadata is retained but never usable action
  proof; strict errors must not be swallowed by tolerant outer raw wrappers.
  GPM-006a1 commit `cbce8d4`.
  COMPLETE: four fields retained, strict object parsing before filtering prevents
  swallowed child errors; normalization/exhaustive identity paths updated.
  39 source files, +578/-4 (constructor defaults only outside behavior surfaces).
  RED malformed decoder acceptance/E0609; GREEN core130, decoder26, SQLitedefs16,
  appManifest16/default28/all-features and 1,539 default / 1,557 all-features workspace
  tests. Bridge tests exercise 66 combinations/three publication paths, preserve
  all committed tables and observe zero SQLite writes. Expanded cases post-GREEN.
  fmt/clippy/diff green after missing-literal and formatting failures corrected.
  ASSESS medium (40 paths / 645 diff lines); writerSelfVerification=true,
  independentVerifier=false. RDD off, outcome unknown/not consumed, reviewDue
  slice_budget_reached. Exact continuation (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Known Manifest metadata publication remains intentionally blocked until v18;
  this is not operational metadata or pull readiness.
  GPM-006a2 worker `mux3owle-o-mpny` implementing SQLite v18 in four scoped
  SQLite files: additive nullable fields, true single/catalog readback and content
  identity, transactional publication, migration preservation and guarded downgrade.
  No source writes outside those files or pull action changes are authorized.
  GPM-006a2 commit `6d3d8e4`.
  COMPLETE: v18 nullable additive columns; single/catalog exact Option readback,
  strict 0/1 boolean decoding, metadata content identity; ordinary/description/world
  publication supported. Pre-v18 known metadata refusal preserved, including 66
  bridge combinations. Upgrade preserves catalog/children/tags/world/inventory,
  stacks/coverage/indexes/metadata; DOWN refuses ANY non-NULL field before data
  loss, while unknown-only downgrade preserves legacy data. Migrations 1-17 intact.
  RED restart/identity test LocalStorage; GREEN 20 definition, 31 migration and
  134 SQLite tests; 1,544 default / 1,562 all-features offline workspace tests,
  zero failed/ignored. fmt/clippy/diff green after fixture-column and formatting
  failures corrected. 597 source diff lines, primarily tests across four files.
  ASSESS medium (5 paths / 602 diff lines), writerSelfVerification=true,
  independentVerifier=false; RDD off, outcome unknown/not consumed, reviewDue
  slice_budget_reached. Exact continuation (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Temporary v17 publication block is lifted on v18; no pull eligibility/service,
  live-provider correctness or native approval claimed.
  Targeted public searches for official non-instanced/stackSize/partial Postmaster
  discussion returned no primary evidence resolving the remaining wire gates.
  Sources checked: generated operation/request schemas and official Postmaster
  Actions wiki; DIM's zero-ID convention remains secondary only. Further code
  cannot create that missing provider contract. A bounded instanced subset can
  be developed without closing full-stack support. User selected continuing with
  identified equipment only while stacks remain blocked (interactive choice
  continue_instanced_subset). This is staged implementation, not a scope reduction
  or full-feature completion. No live account operations were authorized.
  Scout `mux7ycdi-p-vx43` mapped core request/gateway and hardened adapter.
  Existing opaque IDs/transfer canonical helper permit zero or oversized strings,
  so the new instanced request validates canonical positive signed-int64 IDs itself.
  Quantity fixed one; hash uint32 (including zero), membership type signed-int32
  representability. Ownership remains fresh service proof, not syntax validation.
  Split GPM-007 into core contract then adapter units; no shared-action refactor.
  Existing transfer refusals do not prove pull-specific semantics. Unknown or
  post-dispatch ambiguous errors cannot become deterministic non-application.
  Capacity remains a later service prerequisite; maxStackSize does not establish
  destination bucket capacity. No service/frontend actions yet.
  GPM-007a worker `mux81k5v-q-e1dw` implementing only core instanced_pull.rs,
  ports trait and lib exports: immutable request, representability tests and
  honest acknowledgement/refusal/uncertainty. Adapter is the next unit.
  GPM-007a commit `0fb310b`.
  Implementation COMPLETE: request private fields/read-only accessors/fixed1,
  seven tests and five compile-fail docs. RED E0432; GREEN 7 focused, 159 core,
  1,556 default / 1,574 all-features offline tests, zero failed/ignored; fmt/clippy
  and diff/new-file whitespace passed after formatting correction. Core trait
  fake only, no transport/provider proof. ASSESS unassessable due untracked module;
  high-risk independent-verifier required. Verifier `mux87zkx-r-7x6l` PASS,
  no blocking counterexample. Explicitly read all 301 untracked lines; 7 focused,
  159 core including six total doctests, 1,556 default / 1,574 all-features tests,
  fmt/clippy/diff and whitespace passed. Candidate/tracker/lockfile checksums
  unchanged. Compile-fail examples use valid exports and intended restrictions.
  Syntax does not prove provider identity; fresh eligibility/capacity/service remain
  future. RDD/native approval unknown; core seam only, not transport proof.
  GPM-007b COMPLETE (independently verified, commit `012bdc9`): dedicated
  hardened instanced pull adapter and isolated shared transport policy correction.
  No runtime binding or user-facing action before service verification exists.
  The following records initial implementation and correction evidence.
  Worker `mux8cha7-s-ib04` implementing the adapter. Unknown provider codes,
  server/throttle/redirect responses without non-application proof remain Uncertain;
  no borrowed transfer-specific refusal mappings. Essential acknowledgement fields
  use bounded typed int32 parsing, including duplicate-key refusal tests.
  Implementation COMPLETE: 650-line adapter/eight tests plus lib exports only.
  Exact five-field body; HTTP200 typed success acknowledges, 401/403 auth refusal,
  all other received results uncertain. Shared actual/declared response cap 8 KiB;
  duplicate typed fields, numeric widths, read faults and redaction covered.
  RED missingAPI E0432; fixture membership-ID failures corrected before GREEN.
  Worker 8 focused/24 equip/19 lock/43 transfer and 1,564 default / 1,582
  all-features offline tests, fmt/clippy/diff/newfile whitespace green.
  ASSESS unassessable due untracked adapter; independent-verifier required.
  Verifier `mux8kjh2-t-uh1w` found BLOCKER before tests: inherited shared
  HttpTransferTransport::new builder lacks no_gzip while reqwest gzip is enabled;
  automatic decompression can hide original Content-Length. Required disabled
  decompression posture is not met. Verifier continuing approved read-only checks;
  verifier completed BLOCK despite all authorized checks green (8/24/19/43
  focused and 1,564 default / 1,582 all-features tests). Source-derived encoded
  gzip/header counterexample; reader tests supplied already-decoded cursors.
  Retry inspection found default protocol-NACK retries but http2/http3 absent in
  tested build, so no observed POST resubmission claimed; explicit never-retry
  policy required for feature-independent one-attempt semantics. No other blocker.
  Parent approved bounded additive transfer.rs pull-specific constructor with all
  automatic decompression disabled and retry::never; existing action constructors
  unchanged. Deterministic loopback-only synthetic HTTP regression is permitted,
  never external/Bungie/account requests. Not closed/committed or runtime-bound.
  Corrective worker `mux8s5rf-w-koi2` COMPLETE: shared hardened builder plus
  crate-private pull-only constructor disables gzip/brotli/zstd/deflate and
  explicitly retry::never; existing transfer/equip/lock public policy unchanged.
  Three real-client credential-free loopback regressions added. Observed RED:
  old policy 9 pass/2 fail (gzip decoded and original length lost); GREEN 11.
  Identity/unknown encoding/raw bytes checked; no protocol-NACK replay claimed.
  Worker checks 11 pull/46 transfer/24 equip/19 lock, 1,567 default / 1,585
  all-features offline tests plus fmt/clippy/diff/new-file whitespace pass.
  Parent builder spot check agrees; re-ASSESS still unassessable due untracked
  module. Required independent recheck `mux91aev-x-q0xl` returned PASS:
  reviewed all 651 new adapter lines and complete lib/transfer diffs; source
  hashes unchanged during checks. Production-config loopback confirms raw gzip
  and original length preservation, cap refusal, identity and unknown encoding.
  Exact allowlist-before-secrets, five-field body, typed bounded envelope,
  one-shot dispatch, sanitized outcomes and old action policy remain intact.
  Independent checks: 11 pull/46 transfer/24 equip/19 lock; 1,567 default /
  1,585 all-features, fmt/clippy/diff/explicit untracked whitespace all pass.
  Previous gzip BLOCK resolved. Retry::never is configuration evidence only,
  not a protocol replay test. Technical PASS is not native RDD approval.
  Work-unit commit: `012bdc9` — hardened instanced Postmaster pull gateway.
  No external-provider/TLS/live, runtime binding, capacity, global pacing or
  recovery verification claimed. Read-only service scout `mux92juo-y-hf00`
  continues mapping usable runtime bucket and policy-qualification surfaces.
- GPM-005b scope scout `mux1d2co-f-c4dl`: only QueryService and CLI
  runner RecordingService implement QueryOperations. Add required object-safe
  postmaster method and true forwarding; no fallback stub or MCP changes needed.
  MCP QueryReadService implements its own InventoryReadService trait with a concrete
  QueryService. Help/setup expectations are dynamic; skill drift requires a
  documented public postmaster invocation. Existing mapped CLI/app/docs surfaces
  suffice; no extra edit paths identified.
- Keep `--character` and carried-inventory filters restricted to Character,
  never implicitly Postmaster(owner); lock's fallback remains Vault-only.
  Pull-contract gaps do not block safe snapshot/persistence work.
- Feature A live checks remain pending; no claim of live provider validation.
