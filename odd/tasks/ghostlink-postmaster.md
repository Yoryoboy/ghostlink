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
- [x] **GPM-008a3 — Publish then expose fresh evidence.** Share sync acquisition,
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
- [x] **GPM-008b — Qualify equipment capacity policy.** Current public Manifest
  index, exact supported bucket policy/provenance and definition-version binding;
  missing/changed/unqualified evidence closes the proof. User decisions
  (interactive, after scout `capacity-policy` map): pinned audited policy checked
  against the freshly obtained public Manifest index (new version closes proof
  until re-audit), NOT acquired bucket definitions; any stack record in a target
  equipment bucket refuses the proof (never counted, quantities never slots).
  Destination bucket comes from item definition bucketTypeHash (normal bucket),
  never runtime Lost Items; missing/unsupported destination refuses.
- [x] **GPM-008b1 — Core capacity policy contract.** Pure core module
  equipment_capacity.rs + integration tests + lib exports: validated policy
  bound to definition version and bucket artifact identity covering exactly the
  eight supported equipment buckets; qualification against observed index
  evidence; capacity proof counting each owner Character-location instance in the
  destination bucket once (equipped included) from complete observations.
  Fail closed on unavailable observations, version/artifact mismatch, unsupported
  or missing destination, stacks in bucket, overflow and occupancy >= capacity.
  Not dispatch authority or freshness certification.
- [x] **GPM-008b2 — Pinned policy and public index qualification adapter.**
  Audited constants (version 244213.26.06.29.2000-1-bnet.65864, bucket artifact,
  capacity 10) plus fresh public index comparison via existing Manifest gateway.
  Parent re-audit 2026-10-07 (public unauthenticated GET): index ErrorCode 1,
  version 244213.26.06.29.2000-1-bnet.65864, en DestinyInventoryBucketDefinition
  path exactly `/common/destiny2_content/json/en/DestinyInventoryBucketDefinition-a8ba855a-93c7-4014-8f2e-d92357fdfb42.json`.
  Artifact: all eight equipment buckets itemCount 10, scope 0, location 1,
  category 3, enabled, non-redacted, non-FIFO, hasTransferDestination true.
  Surfaces: new adapters/src/equipment_capacity_policy.rs, adapters lib.rs
  exports, manifest.rs crate-visible index helpers plus concrete-gateway fetch.
  No core ManifestIndex/port change (avoids literal blast radius); service port
  wiring belongs to GPM-008.
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
- GPM-008 split (scout `pull-service` map, 2026-10-07) into four ordered units.
  User decisions: planning performs automatic forced fresh acquisition (no
  explicit-sync requirement); post-action uses bounded re-observation: at most 3
  forced PostAction observations, 2 s injectable delay between, stop at first
  verified arrival, NEVER redispatch; final outcome from last observation with
  all evidence retained. Plans: private non-Clone, consumed by value, no ticket
  reminting (avoid mutation.rs authorize_execution/lock borrowed-plan pattern).
  Verified = target instance unique in post-action published pair at
  Character(destination owner), observed actual bucket == definition destination.
  Acknowledged is never application proof; unchanged revision proves nothing.
- [x] **GPM-008c — Capacity port and deterministic action fakes.** Required
  ManifestGateway::fetch_equipment_capacity_evidence (adapter delegates to the
  inherent method; app fake gets separate evidence queue/counter); observation-
  aware profile fake and queued pull-outcome fake. Surfaces: core ports.rs,
  adapters manifest.rs, app test_support.rs.
- [x] **GPM-008d — Opaque pull planning and eligibility.** New app module
  instanced_pull.rs (+ tests), lib exports. Forced fresh acquisition, owned
  Postmaster(owner) selection, definition gates (explicit false side-effect flag,
  instanced kind, supported normal bucket, definition version == published),
  qualified capacity proof, eligibility fingerprint, private non-Clone plan.
- [x] **GPM-008e — Consumed-plan preflight and one-shot dispatch.** Reacquire,
  reprove, compare membership/revision/version/fingerprint, consume plan on every
  path, one gateway call; not exposed until GPM-008f.
- [x] **GPM-008e1 — Dispatch fault-injection tests.**
  Separate test-only unit before GPM-008f exposure: expiry during slow reads,
  first/final clock errors, final inventory/definition read failures and drift
  before final reload. Assert exact refusals/errors and zero gateway calls.
  No production edits, shared fake expansion or atomic-race guarantee implied.
  User authorized this test-only follow-up separately after GPM-008e closure.
  One bounded writer may edit only instanced_pull/tests.rs. No production or
  shared-fake edits; report demonstrated defects rather than widening scope.
  This adds coverage for existing behavior, so a failing production RED is not
  presumed or fabricated; record actual first-run and focused-check results.
  User explicitly authorized the verified local work-unit commit `5db5f02`.
- [x] **GPM-008f — Bounded observation and honest terminal outcomes.**
  Verified / not yet observed / uncertain / refused with attempt evidence; lag,
  wrong owner/bucket, missing target, unavailable observations, refresh failure.
  Implementation map complete; user selected preserving confirmed refusals
  even when arrival is observed, with arrival evidence shown separately.
  Natural ordered subunits:
- [x] **GPM-008f1 — Observation evidence and exact arrival classification.**
  Private pure classification over the published acquisition pair; retain target
  metadata/actual buckets and explicit absent/unavailable/drift reasons. No new
  public dispatch path. Deterministic classifier tests alongside behavior.
- [x] **GPM-008f2 — Bounded verified execution and facade.**
  Consume the plan, call dispatch once, retain exact gateway outcome and all
  ordered observations. At most three forced PostAction acquisitions, existing
  RetryDelay pacing of two seconds only between attempts; never redispatch.
  Additive app exports only after safe execution is integrated and checked.
- [ ] **GPM-009 — CLI/MCP pull exposure.** Parent milestone; complete only
  after all integration units, pipeline/stdio coverage, docs/skill and exact
  outcome/exit semantics are verified and locally committed.
- [x] **GPM-009a — Preserve shared MCP acquisition evidence.**
  Forward observation-aware scoped acquisitions unchanged through the existing
  SharedGateway. Add ProfileAcquisition facade export only if required.
  Test exact Preflight/PostAction reasons, one inner acquisition, available and
  unavailable bucket evidence with fallback reads unreachable. Prove definition
  and inventory publication visibility across cloned SharedRepository handles
  using in-memory SQLite. No unused composition, new store/guard, public command
  or direct-core MCP dependency. Allowed source surfaces: app/src/lib.rs and
  mcp/src/main.rs under crates/destiny-*. Estimate 70–150 changed lines; tests
  remain with behavior, not a strict limit. Route: gentle-ai-worker, two-file
  write/preparation trigger; focused self-checks, native ASSESS and routed
  verification before a scoped local work-unit commit.
- [x] **GPM-009b — Shared pull result presentation.** Use destiny-output
  architecture with src/instanced_pull.rs, lib.rs and app facade exports only as
  needed. Human approved output/Cargo.toml solely for an existing-app test-support
  dev-dependency to exercise real public service outcomes. Preserve receipt, disposition, ordered classifications AND failures,
  selected IDs/hash/owner/buckets/publication metadata/timestamps independently.
  Sanitize typed pre-dispatch/planning/gateway failures; no whole snapshot or
  credentials. Focused serialization tests; estimate 250–400 lines. Delegated
  writer for multi-file unit; ASSESS/verification/commit before CLI integration.
- [x] **GPM-009c — CLI genuine-instance pull.** Add ghostlink pull ITEM with
  canonical positive int64 instance ID only; no names/hash/quantity/destination.
  Authenticated membership and fresh eligibility derive membership/owner.
  One invocation-owned store for sync and definitions, not two independent
  handles. Pending/uncertain/refused are nonzero; only Verified succeeds.
  command/runner/runtime/lib, pull/pull_output and pull_pipeline tests, docs/cli
  and ghostlink skill/exit-code reference stay together. Estimate 250–450 lines;
  delegated bounded writer, pipeline/help/skill checks, ASSESS/verification/commit.
- [ ] **GPM-009d — MCP genuine-instance pull (IN PROGRESS).** Add pull_item with closed ID-only
  schema through an additive mutation wrapper, shared output and production
  shared gateway/repository composition. Reuse existing operation guard.
  Update offline fixture observation acquisition, discovery counts/read-only
  exclusions; deterministic integration/stdio/parity tests and docs/mcp/skill.
  service/schema/server/main/lib, offline_fixture, tests/pull.rs/tests/stdio.rs.
  Estimate 400–650 lines for one coherent transport feature with its tests/docs;
  never omit coverage or compress readability to meet an advisory line budget.
  Delegated bounded writer, ASSESS/verification/commit before final GPM-010.
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

- GPM-009c COMPLETE — `bec28a8aff9d7cb79186229474ac2a6f98f4b974`. Writer
  `muyi7tq6-c-7fxq` implementation and corrected docs verified/committed. Baseline
  6d2065ca7ce5f81b9a54ed9a38bf998f6b2892cd; ten authorized paths,1130diff lines
  (1120additions/10deletions), including602pipeline-test lines. Natural overage
  preserves readable actual pipeline coverage and SQLiteownership proof, not
  expanded product scope. Three newfiles intent-to-add normalized for ASSESS.
  CLI pull accepts canonicalpositiveint64 itemIDonly; invalidinput beforeauth/
  service/action. Runtime privatehandles share one invocationSQLiteconnection
  for sync/definitions; fresh concreteManifest qualification, authenticated
  membership, existingOAuthpullgateway/delay and consumedplan execute_verified.
  OnlyVerifiedexit0; sharedmapperJSON and ordered sanitizedtext retained.
  Writer actualparserRED0pass/1fail101,minGREEN1,alternate8then10. Two wrong
  testaccessors caused intermediatecompilefailure, fixed; fmt1corrected.
  FinalPASS: pipeline10,skill5,CLI253across20suites,output9,app38,root/pullhelp,
  fmt,CLIall-target/all-featureclippy,diff/newfilewhitespace. No unresolved
  writerblocker, dependency/lockfile/unlisted/MCP/livechanges.
  Parent pull+pull_outputreadback, exactpathset/HEAD/RDDoff/hashreadback done.
  ASSESSmediumruntimeLarge10paths1130lines,selfverificationstands,
  independentVerifier=false,reviewDue=true/slice_budget_reached,nativeunknown/
  unconsumed. Exactinformationalcontinuationrelayednotexecuted:
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Additional bounded gentle-ai-verify route for concrete runtime/store lifetime,
  freshqualification, exits/privacy and actual pipeline; full diff exceedsparent
  evidencebudget and publicaction composition needs independentreadback.
  Interim verifier `muyiqz09-d-izl0`: pipeline10,skill5,help/fmt/diff PASS;
  pull_repository filter0NOTcoverage, actualruntimeSQLitevisibility1PASS after
  correcttestselection. Concrete runtimecomposition appears sound. Candidate
  documentationfinding SKILL.md26 adds pull to unqualified exit43='applied but
  pending provider refresh', contradicting pullNotYetObserved43 and section87.
  This can cause false confirmed-application claims. Parentspotreadconfirmed.
  Settled verifier `muyiqz09-d-izl0` BLOCKED P2/medium documentation-only
  contradiction. Seven authorized checks PASS,16 actualtests (pipeline10,
  skill5,actualstorevisibility1); zero filter excluded from coverage. Finalten
  candidatehashes,Cargo.lock,index,HEAD/branch andintentflags unchanged before
  declaredparenttrackerwrite; authorizedbookkeeping does not invalidate proof.
  Route: resume originalwriter for ONLY SKILL.md no-blindretry/exit43rule.
  Explicitly separate pull43 (NotYetObserved, no confirmedapplication) from
  existingothermutation applied-pending semantics. Preserve runtime/JSON/exits/
  otherdocs; passiveprose has no meaningful behavioralRED. Focusedskill5 and
  pipeline10checks/diff plus exactnineothercandidatehashes unchanged required;
  Correction continuation `muyizjtn-e-gec0` COMPLETE, soleSKILL.md26phrase
  explicitpull43arrivalunobserved/applicationnotconfirmed vsotheractionsemantics.
  OrdinarypassiveprosechecksPASS skill5/pipeline10/diff, no fabricatedRED.
  Nineothercandidatehashes,index,Cargo.lock,tracker,HEAD/branch unchanged;
  newSKILLSHA38ac6eacd9a4ccdd553fbb58f4d8da2eaafd94f577cd55ae01a286dea9fc521a.
  ParentcorrectedrulereadbackPASS; freshASSESSmediumLarge,selfverificationstands
  (11paths1173linesincludingtracker),nativeunknown/unconsumed,RDDoff,
  reviewDue=slice_budget_reached; sameexactcontinuationrelayednotexecuted.
  Independent finalrecheck `muyj360l-f-c86j` PASS, previousP2resolved; skill5/
  diffPASS, correctedskill/nineotherhashes/index/lock/tracker/status/HEAD/branch/
  intentflags unchanged. Runtimechecks not rerun after prose-only correction.
  Parentfinalskill5PASS, freshRDDoff, branch/HEAD/emptycontentindex/exact10paths/
  tenhashes/diffguardsPASS; staged ONLY authorizedtenfiles beforeworkunitcommit.
  No nativeapproval/live-readinessclaim. Fullworkspacechecksremain010.

- GPM-009d IN PROGRESS — delegated bounded MCP writer, multi-file/preparation
  trigger. Reuse serveroperationmutex and productiongateway/repositoryclones;
  additive mutationwrapper, sharedmapperJSON and closedcanonicalIDonlyschema.
  Compose freshconcreteManifestqualification/pinnedpolicy, existingOAuthgateway/
  delay; no independentpullstore or newlock. Offlinefixture mustforward actual
  acquisitions and expose deterministic arrival for stdio proof, not infer
  productionvisibilityfromfixture'sseparatelyseededstores. Mutationfake defaults/
  forwarding/read-only discoveryexclusions/counts preserved. Ordered failures
  andreceipt/disposition separate; isErrortrue unlessVerified.
  Exactderivedsurfaces: MCPsrc service/schema/server/main/lib, bin/offline_fixture,
  tests/pull.rs/tests/stdio.rs,docs/mcp.md,ghostlinkSKILL. Behavior/tests/docs stay
  together; estimated400–650linesadvisory, preserve coverage/readability and report
  naturaloverage. Test-first functionalschema/dispatch/oneaction/stdio/serialization
  checks, nativeASSESS/routedverification and scopedlocalcommit before010.
  No live/account/publishing authorization; anonymousstacks006remain gated.

- GPM-009 mapped by read-only scout `muyfk9g9-7-zepk`; GPM-009a COMPLETE,
  GPM-009b/009c COMPLETE; GPM-009d IN PROGRESS. 009a work-unit `66c3addde367627158e3ab7ad812c0865f114b18`
  (176 source diff lines; app facade export and MCP forwarding/regressions).
  Worker `muyg1hqg-8-ov4t`: semantic RED forbidden scoped fallback, 0 passed/
  1 failed exit101 after two test compile errors corrected; minimal GREEN1.
  Final available/unavailable exact envelope equality, membership/reasons and
  one inner call assertions; real SQLite inventory/definition clone visibility.
  Final PASS: shared_gateway1, shared_repository2, rc002_dispatch1, equip5,
  app instanced_pull38, MCP156 across9 suites, app7 doctests, fmt, MCP
  all-target/all-feature clippy and diff. Initial formatting exit1 corrected.
  `ghostlink --bin destiny-mcp shared_gateway` selected zero tests and was NOT
  coverage; main.rs tests actually run through destiny-mcp library process module.
  Parent regression readback and fresh shared_gateway1/155filtered rerun PASS;
  exact branch/HEAD/empty-index/pathset/hash/diff guards passed before localcommit.
  ASSESS: medium2paths176lines, runtime large writer, writerSelfVerification=true,
  independentVerifier=false, reviewDue=false/under_budget. Candidate unconsumed/
  native unknown; RDD freshly clone-local off. No native approval or live calls.
  Over estimate70–150 to retain readable fixtures and real SQLite checks.
  009b route: bounded shared-output writer; serialization and sanitized failure
  tests before production mapping, then focused checks/ASSESS/routedverification.
  Writer `muygi0qr-9-znlh` preparation settled interaction_required, no edits,
  clean source baseline HEADa477b88. Opaque outcome/attempt constructors require
  service-backed fixtures; app deterministic fakes need test-support feature.
  Human initially declined for lack of explanation, then explicitly APPROVED
  the same narrow two-choice addition after read-only explanation. Add only
  crates/destiny-output/Cargo.toml for dev-dependency on existing destiny-app
  with test-support; existing three surfaces retained. No new package/install,
  Cargo.lock edits, runtime dependency or executor behavior changes authorized.
  Dependent crates do not inherit app cfg(test); test-support feature enables
  existing public fakes. Normal production builds do not activate this feature
  via dev-dependency; tests and explicit all-features do.
  Resumed writer continuation `muyh4f74-a-iul8` COMPLETE implementation; 009b
  COMPLETE — work-unit `7634555a44b810fc31a1d0753664de89995933a5`. Four authorized paths,
  834 additions/1 deletion; new826-line module includes about255 production
  lines plus real-service fixtures/tests. Retain exhaustive mappings/readability
  and coverage despite advisory250–400 estimate; no transport/executor expansion.
  Semantic RED: service Verified but mapper is_success false,0passed/1failed101;
  minimumGREEN1. Compile error wrong CapacityRefusal variant corrected. Intermediate
  refactor7passed/1failed wrong acquisition-counter expectation fixed without
  production behavior change. Final9real-service mapper tests PASS.
  Writer closure PASS: output9, appinstanced38, app7doctests, fmt, outputall-target/
  all-featureclippy, MCP+ghostlinkall-target/all-featurechecks, diff. New-file
  whitespace checked; no-index1 means expected file difference/no diagnostics.
  Parent production/manifest readback and allfour source hashes match. Authorized
  intent-to-add normalized newfile for ASSESS without staging candidate content.
  ASSESSmedium4paths835lines/runtimeLarge, writerSelfVerification=true and
  independentVerifier=false; reviewDue=true/slice_budget_reached, nativeunknown/
  unconsumed. FreshRDDoff. Exact informational continuation relayed, not executed:
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Additional bounded gentle-ai-verify route selected for real-service mapping/
  enum/privacy fidelity because full826-line test/fixture diff exceeds parent
  evidence budget. Read-only targeted checks, no redundant full workspace run.
  Independent verifier `muyhx369-b-5yrq` PASS: actualmapper9,app7doctests,
  defaultproductionoutputcheck,fmt/diff; all candidate/tracker/index/Cargo.lock
  hashes unchanged. No demonstrated blocker/evidence loss/privacy leak/base-only
  finding. Sourceevidence confirms receipt/disposition independence, classified
  arrival+timestampfailure, ordered/null attempts and closed sanitized categories.
  No historical RED/fullworkspace/clippy/transport rerun by verifier; writer
  checks above remain the record. Codegraph resolved parent/appsindex; no init/
  parent-index reuse, scoped source inspection fallback. Not native approval.
  Parent final reported mapper9 rerun PASS, freshRDDoff, fourhashes/exactpaths/
  branch/HEAD/empty-content-index/diff guards PASS before staging only sources.
  009c next: delegated CLI writer, named multi-file/preparation trigger. Exact
  command/runner/runtime/lib,pull/pull_output and newpull_pipeline tests; cli.md,
  ghostlink skill and exit-codes reference. Existing composition's store ownership
  seam must be resolved locally in runtime, not independently opened pull handles
  or broad runtime refactor. Plan/pinnedManifestqualification/execute_verified,
  authenticated membership/fresh owner, positive canonical int64 ID only.
  Functional parser/help/invalid selection/fake-pipeline tests and JSON/text/exit
  semantics/docs accompany behavior. Only Verified exitzero; no actual account
  access during development. ASSESS/routedverification before scopedlocalcommit.
  Settled map: server.rs139–146/216–242 acquires the same operation Arc inside
  direct/blocking dispatch and holds it across complete read/mutation calls.
  Existing rc002 guard-lifetime and equip serialization regressions apply.
  Production main.rs82–87 composes one stdio server; 274–330 clones shared
  gateway/repository handles. SharedRepository501–529/635–652 uses one SQLite
  connection for inventory and definitions; per-call lock is not operation
  boundary. No production background publisher/bypass found. Lazy-version and
  personal/weekly connections still execute under server guard; public weekly
  is definition-read-only. External processes/game activity remain excluded
  from local serialization guarantee; WAL is not a remote-action-wide lock.
  Parent spot check confirms SharedGateway465–491 lacks observation-aware
  forwarding; this is the prerequisite evidence-loss seam. Reuse existing store
  clones for eventual pull definitions. Do not construct an unused pull service
  or unused clones in 009a; attach composition with 009d.
  No public exposure yet. GPM-008f complete for genuine-ID equipment only;
  anonymous stacks remain GPM-006 gated and owner-run checks remain GPM-010
  pending. No full Postmaster/live-readiness claim.
  Full task mirror is submitted under odd/ghostlink-postmaster/tasks (6504).
  Engram readback truncates near 50,000 characters; matching prefix checked,
  tail readback unavailable. Local full file remains recoverable authority.
- GPM-008f2 COMPLETE — scoped local work-unit commit
  `39c424a20d883f3642356183a52b68dd1716509b`, 1,187 source diff lines across
  app instanced_pull.rs/tests.rs and additive lib.rs exports (841 test additions).
  Consumed non-Clone plan, private preflight, one gateway call maximum. Gateway
  Err is pre-dispatch-only and incurs no observation expense. At most three
  forced PostAction acquisitions; existing infallible RetryDelay waits exactly
  two seconds only between attempts. Stop first valid arrival or integrity/
  clock failure; retain dispatch, exact receipt, timestamps and ALL attempts.
  Final disposition derives from the last valid attempt/failure, not old proof.
  Confirmed refusals stay Refused even after arrival; Ack/Uncertain may become
  Verified only with exact valid published-pair evidence. Plan expiry gates
  dispatch, not already-dispatched observation. Selected-target getters expose
  no whole inventory/socket payload. No causal/atomic guarantee; caller-local
  inventory-publication/definition-use serialization is still required.
  Writer `muyeomxv-3-0ryt` observed initial missing-API/import compile RED (two
  errors, exit 101), not assertion RED, then 35 focused/7 doctests. Corrected
  clock-fixture/counter assertions and large-result-error clippy (boxed failure
  evidence); every final initial closure command passed. Reported partial
  conservatively because intermediate failures occurred; none remained open.
  Initial app 231/448 all-features, workspace 1,652/1,670 all-features, fmt and
  app/workspace default/all-feature clippy all passed.
  Independent verifier `muyf3omf-4-t3c5` found one medium blocker: another
  character's Postmaster was misclassified as StillInPostmaster/NotYetObserved
  after Ack. No false Verified demonstrated. Correction `muyfaf87-5-u9kx`:
  three-line owner guard plus 183 regression/timestamp coverage lines.
  Actual assertion RED 35 passed/two failed, exit 101; minimal GREEN 37 then 38.
  Other-owner Postmaster is WrongCharacterOwner/Uncertain; original-owner Ack
  remains NotYetObserved; all confirmed refusals remain Refused. Two timestamp
  boundary cases retain classified arrival pairs PLUS failure and ordered prior
  evidence without Verified. No fabricated timestamp RED. Final correction
  checks PASS: 38 focused/7 doctests, 234 app default/451 all-features, fmt,
  app all-target/all-feature clippy and diff. Workspace totals above remain
  historical pre-correction evidence, not rerun; GPM-010 will recheck closure.
  Independent final recheck `muyffskb-6-pvy7` PASS: 38 focused/7 doctests,
  fmt/diff and source hashes unchanged; previous blocker resolved, no remaining
  targeted blocker. Authorized offline exact-doctest rustc replay proved solely
  E0382 (reused moved plan), no import/bound/artifact error; not repeated after
  unchanged-API correction.
  ASSESS medium (4 paths/1,230 lines including tracker before closure), runtime
  large writer; RDD off/unknown/not consumed, independentVerifier=false,
  reviewDue slice_budget_reached. Extra independent check added for public
  action/timestamp boundary. Native continuation (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Parent source/regression readback, rerun diff check and verified source hashes/
  expected branch/HEAD/empty-index guards passed before staging only three app
  source paths. RDD freshly off. No runtime binding, live calls, native review
  approval, publishing or stack support claimed; those remain later work.
- GPM-008f1 COMPLETE — work-unit commit
  `1f6af5e40e67c35489d761a660dc91e51a0f98d6` (264 source diff lines).
  Writer `muyec2nt-2-9csc`: private PullArrival/PullObservation and pure classifier
  from exact PublishedProfileAcquisition. Requires membership/version/hash,
  Character(exact owner), actual normal bucket. Retains metadata, optional target
  and actual observations on every failure; revision is deliberately irrelevant.
  Duplicate target IDs/incomplete available buckets are structurally excluded by
  validated constructors; no constructor weakened, no causal/atomic guarantee.
  RED: 18 missing-API compilation errors (exit 101), not assertion RED. GREEN:
  24 focused, 6 doctests; app default 219 / all-features 436 tests as reported.
  Alternate missing-target and changed/unchanged revision controls pass. Initial
  fmt failure corrected within allowed files; final fmt/clippy/diff exit 0.
  ASSESS medium (3 paths/292 lines), runtime large, writer checks stand,
  independentVerifier=false; RDD off, outcome unknown/not consumed, under_budget.
  Parent source/test readback, rerun diff check and guarded hashes/scope passed.
  Workspace checks deferred to 008f2; no required check skipped. No executor,
  exports, runtime binding, shared-fake edits or live calls in this unit.
  Scout `muycj3ju-1-ujk1` maps successful publication pair seams.
  User selected option 1: confirmed receipt refusal remains Refused even after
  exact observed arrival; retain arrival evidence separately. Apply uniformly
  to Rejected, AuthenticationRefused and Transient, never upgrade to success.
  Acknowledged/Uncertain may become verified only with exact valid arrival proof.
  Reuse existing infallible RetryDelay in 008f2; no new delay infrastructure.
  Gateway Err stays pre-dispatch-only, with no PostAction observations.
  Estimated 500-800 diff lines including tests; split coherent private classifier
  (008f1) from bounded executor/public facade (008f2), not by file type.
  User authorized sequential work through GPM-010; session standing permission
  covers scoped local work-unit commits. Existing no publishing/live restrictions
  remain. Baseline for 008f1 was `c5acf50`; 008f2 builds on its verified commit.
  Observe test-first RED before 008f2 implementation, then GREEN/alternate cases/
  refactor; widen no shared fakes and start no 009 binding before 008f closes.
- GPM-008e1 COMPLETE — user-authorized local work-unit commit
  `5db5f02aaff316e75043e750d999acb6209aa860`. Writer `muy9pfbq-2-w8t1` added
  328 lines only in crates/destiny-app/src/instanced_pull/tests.rs; base `0350775`.
  Commit-time guard confirmed verified source hashes, expected branch/HEAD,
  empty index and diff check; only the verified test file was staged.
  No source changes or new test runs during commit closure. RDD freshly off.
  Six deterministic tests cover 12 fault cases plus a near-expiry success control:
  slow-read expiry (exact and beyond), first/final clock errors, final inventory
  error/missing snapshot, final definition error/version drift, final reload
  membership ID/type/revision/published-version drift. Every fault asserts the
  exact refusal/error, zero gateway calls and empty requests. Wrappers installed
  after planning; stage counters establish valid publication/eligibility before
  late faults. Success control dispatches once one second before expiry.
  Coverage-only addition: no meaningful production RED. Initial compilation
  exit 101 for two wrapper API mistakes, corrected (not safety-regression RED).
  First runnable focused GREEN 20; final GREEN 21. Two intermediate fmt failures
  corrected; final fmt, app all-target/all-feature clippy and diff checks exit 0.
  Full app check PASS: 149 unit, 61 integration and 6 doctests. No required check
  skipped. Workspace suites and live requests NOT run for this test-only unit.
  ASSESS medium (2 paths/346 lines before this bookkeeping), runtime large
  writer: writerSelfVerification=true, independentVerifier=false; RDD off,
  outcome unknown, candidate not consumed, reviewDue=false/under_budget.
  Parent structural spot check confirms stage placement, wrappers forwarding
  and real gateway assertions; rerun diff check/scope/hash checks pass.
  Production SHA-256 unchanged:
  `30afcfe53266a51a40111a03dbbf4f3422fedfdb67077c83fe1bc2100c4ff40c`.
  Tests SHA-256:
  `58dc3e1d983671b4c686a3f3d350b1653a7668279f1e42bbf2a7b46e4a32c739`.
  No demonstrated production defect, shared-fake expansion, GPM-008f or live
  action. Post-final-reload drift still requires caller serialization; no atomic
  guarantee. Work-unit closed; next pending unit is GPM-008f bounded observation
  and honest outcomes. No separate independent verifier mandated.
- GPM-008e COMPLETE — explicitly user-authorized local work-unit commit
  `c5348a991ca5ecc8fe5e7f6fe5349ba8a832cdf1`. Independent focused reverification
  `muy9hamq-1-l63j` PASS: 15 tests (zero failed/ignored), fmt, app all-targets/
  all-features clippy with -D warnings, diff check; every command exit 0.
  Parent reran diff check and pinned both unchanged source SHA-256 hashes before
  staging only the two verified source paths. No source correction or new RED
  applicable on this resume. Full workspace suites and compile-fail doctests
  were NOT rerun; historical totals below are retained, not fresh evidence.
  RDD freshly off (clone-local); no native verdict or publishing/live actions.
  Worktree `/home/yoryo/apps/ghostlink-inventory`, `feature/ghostlink-postmaster`.
  Writer `muy8swuu-c-gp1h` completed implementation;
  no public/runtime binding. Two source paths, 588 changed lines:
  shared eligibility, private consumed-plan preflight_dispatch, typed refusals
  and terminal attempt context preserving the exact gateway outcome/error.
  Worker RED missing preflight_dispatch; GREEN 15 focused / 210 app tests;
  1,631 default / 1,649 all-features offline workspace tests, fmt/clippy/diff pass.
  ASSESS medium (3 paths/597 lines), large runtime writer profile:
  independentVerifier=false; reviewDue=slice_budget_reached, RDD off, outcome
  unknown. Native continuation (informational, not executed):
  `gentle-ai review status --cwd=/home/yoryo/apps/ghostlink-inventory --contract=gentle-ai.review-integration/v2 --next-transition=true`.
  Parent spot check confirms private dispatch, consumed plan, final local/time
  checks and sole gateway call. Targeted independent verifier `muy952ou-d-pwa7`
  PASS: no deterministic dispatch-safety defect under documented caller-local
  publication serialization. Observed 15 focused tests, fmt/diff clean; candidate
  hashes unchanged. No new full-suite or historical RED evidence claimed.
  At GPM-008e closure, coverage gaps remained: expiry during slow reads,
  first/final clock faults, final inventory/definition read failures and drift
  before final reload. Injected tests were absent then; GPM-008e1 above now
  supplies this coverage in the verified test-only work-unit commit `5db5f02`.
  Drift AFTER final local reads is not atomically prevented; caller serialization
  is required. Test-local wrappers in existing tests.rs suffice; shared fakes
  need not be expanded. Recommend a separate test-only unit before exposing 008f,
  not growing this 588-line implementation slice silently. No runtime blockers
  demonstrated; 008e verified and locally committed. Separate bounded dispatch
  fault tests are now verified and committed in GPM-008e1 (`5db5f02`).
  Public GPM-008f execution has not started.
  No retries, PostAction observations, rollback, account requests or GPM-008f.
- GPM-008d COMPLETE — work-unit commit `65ad8d1`: forced Preflight planning from the
  successful publication pair, exact owned Postmaster selection, version-bound
  definition safety gates, qualified capacity and private non-Clone plan with
  eligibility fingerprint. Worker RED missing planning API; GREEN seven tests.
  Independent verifier PASS; 1,621 default / 1,639 all-features offline workspace
  tests, zero failures/ignored; fmt/clippy/diff clean. ASSESS unassessable due
  untracked files; required independent-verifier plan satisfied, RDD off.
  User authorized removing the vacuous disconnected-fake assertion and local
  commit. Production unchanged; no meaningful behavior RED for test deletion.
  Final independent recheck PASS: 7 planning tests, 4 app doctests, fmt, app
  all-target/all-feature clippy, diff and new-file whitespace. Full workspace
  suites not repeated after test-only deletion; previous results retained.
  Preliminary no-index diff returned 1 for file difference, no whitespace errors;
  explicit whitespace recheck passed. Historical RED not independently observed.
  No GPM-008e implementation, dispatch or live account operation in this closure.
- GPM-008c COMPLETE (writer verified; ASSESS medium 3 paths/295 lines, large
  runtime writer profile, independentVerifier=false, reviewDue under_budget).
  Required ManifestGateway::fetch_equipment_capacity_evidence; adapter trait impl
  delegates via inherent path (parent confirmed no recursion; port test passes).
  App fakes: independent capacity evidence queue/counter, observation-aware
  profile acquisitions with reason/count tracking (empty queue falls back to plain
  snapshot with unavailable observations), queued instanced-pull fake recording
  requests; exhaustion returns sanitized ProviderUnavailable. RED 13 missing-API
  app errors + absent trait method; GREEN 45 focused app/1 adapter; 191 app,
  65 manifest, 1,612 default / 1,630 all-features, fmt/clippy/diff pass.
  Work-unit commit: `9664e6d`. Next: GPM-008d opaque pull planning and eligibility.
- GPM-008b2 COMPLETE (independently verified); GPM-008b therefore complete.
  New adapters/src/equipment_capacity_policy.rs (207 lines): audited pinned
  constants, pinned_equipment_capacity_policy through core constructor,
  decode_equipment_capacity_evidence (strict envelope, only en bucket path
  required), qualify_pinned_equipment_capacity with distinct Decode/Qualification
  errors. manifest.rs: pub(crate) helper visibility plus concrete
  BungieManifestGateway::fetch_equipment_capacity_evidence (fresh GET, same
  endpoint/key/cap/retry, no cache); lib.rs exports. Worker RED 26 missing-API
  errors; GREEN 4 policy/64 manifest/519 adapters lib; 1,608 default / 1,626
  all-features, fmt/clippy/diff pass. ASSESS unassessable (untracked) so verifier
  ran: PASS all six points. Non-blocking finding: oversize test used an empty
  body (confounded by JSON failure). Parent fixed it with a decodable body over
  the announced cap; mutation check (announced length None) made it FAIL, real
  code PASS; 519 adapters lib, fmt, adapters clippy all-features, diff clean.
  Correspondence only; no live authenticity/dispatch claim. Work-unit commit:
  `03c5fd3`. Next: GPM-008 verified pull service (map scope first).
- GPM-008b1 COMPLETE (independently verified): EquipmentCapacityPolicy,
  EquipmentCapacityEvidence, exact-match QualifiedEquipmentCapacityPolicy,
  private EquipmentCapacityProof and typed CapacityRefusal in new
  core/src/equipment_capacity.rs (234 lines), tests/equipment_capacity.rs (242),
  lib.rs exports (+5). Worker RED 17 missing-API errors; GREEN 6 focused tests,
  176 core. ASSESS unassessable (untracked files) so independent verification ran:
  PASS all five points, no defects; fmt, clippy default/all-features, 6 focused,
  1,602 default / 1,620 all-features offline tests, diff/whitespace clean.
  Overflow tested at private checked helper only. Correspondence only, not
  freshness/authenticity/dispatch authority. Work-unit commit: `5fa5747`.
  Next: GPM-008b2 pinned policy and public index qualification adapter.
- GPM-008a3 COMPLETE (independently verified): successful-publication evidence
  outcome with same returned snapshot and full filtered-candidate correspondence.
  Exact four mapped app/journey surfaces; ordinary legacy loader compatibility and
  zero post-write reload for proof. No policy/service dispatch or durable buckets.
  Writer `muxa5mzh-15-272e` complete. ASSESS medium (4 paths/799 lines), small
  writer-profile fallback raised it to required independent verification:
  gentle-ai-verify PASS on all six contract points; fmt, clippy default/all-features,
  1,593 default / 1,611 all-features offline tests, zero failures; diff check pass.
  Parent confirmed git scope equals the four surfaces. RDD off; historical RED not
  re-observed by verifier. Work-unit commit: `afe746f`.
  Next: GPM-008b capacity policy (scope still to map).
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
