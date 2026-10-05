# Equip item (WU-032)

## Objective and scope

First step of the mutation layer for AI-assisted builds: equip one concrete
item on a character, transferring it first when needed, and verify it ends up
equipped. Exposed as `ghostlink equip` (first mutating CLI command) and the MCP
tool `equip_item`.

Out of scope (deferred to `build plan`/`build apply`): freeing an item equipped
on another character, making room by moving items to the Vault, orbit/activity
pre-check (component 204), subclass/mods/perks/loadout actions.

## Evidence (2026-10-04)

- Official docs: `POST /Destiny2/Actions/Items/EquipItem/`, scope
  `MoveEquipDestinyItems` (already used by TransferItem), body `itemId`,
  `characterId`, `membershipType`; character must be in a social space, in
  orbit, or offline; `ThrottleSecondsBetweenActionPerUser: 0.1`.
- Pipeline map: steps are `MoveStep { ordinal, from, to }`
  (`destiny-core/src/mutation.rs:307`); plan, evidence, ticket and
  verification are location-only. `EquipItem/` is deliberately refused by the
  transfer adapter allowlist (`destiny-adapters/src/transfer.rs:1064`, `:1092`).
- Projection already has `equipped` + character location per instance and
  `bucket_hash`/`slot`/`tier` per definition; no bucket capacity, no activity.
- CLI has no mutation command; `move_item` is MCP-only.

## Decisions

- Option A: generalize `MoveStep` into a step-kind model
  (`Transfer | Equip`, later `InsertPlug | SnapshotLoadout`) so `build apply`
  can reuse one heterogeneous executor. Start with a behavior-preserving
  refactor guarded by the existing suite.
- Atomic action stays single-call intent-authorized (like `move_item`).
- Refuse with a clear message (no side effects) when: item equipped on another
  character, exotic conflict in the same category, item not usable by the
  target class. Already equipped on target = verified no-op.
- `EquipItem/` gets its own exact allowlisted endpoint; transfer allowlist keeps
  refusing it.

## Open questions

- Exact `tier` string for exotics in stored definitions (verify in EQ-002).
- Bungie error statuses for equip refusals (activity location, exotic rule,
  class) — map in EQ-003 from `Exceptions.PlatformErrorCodes`.

## Constraints and acceptance

- `move_item` behavior and all existing tests unchanged.
- No secrets in output/evidence; no blind retry after uncertain dispatch;
  mandatory post-action refresh and verification (`equipped == true` on target).
- CLI/MCP parity; test-first; `cargo fmt --check`, clippy `-D warnings`,
  workspace tests; live smoke before closing provider-facing tasks.

## Tasks

- [x] **EQ-001 — Generalize mutation steps.** `MutationStep { ordinal, action: StepAction }`, `StepAction::Transfer { from, to }` only variant; executor dispatches by kind, stop via `StepOutcome::continues()`; MCP flat step JSON and `MoveStep` Debug shape preserved. Refactor (RED n/a); workspace 1198 passed (unchanged; worker's 1199 baseline was a miscount, `#[test]` count identical), all-features 1209, fmt/clippy clean; independent verify pass-with-notes. Commit `4048c31`.
- [x] **EQ-002 — Equip planning.** `StepAction::Equip { character }`, `EquipRequest`/`EquipPlan`/`EquipPreflight`, `plan_equip`/`preflight_equip`; `MAX_MUTATION_STEPS = 3` (moves still 2). Refusals: equipped elsewhere, wrong/unknown class, not equippable, exotic conflict (other slot, same category), unknown rarity, unknown neighbor slot; same-slot exotic replacement allowed; non-equipment neighbors (subclass/ghost/emblem) skipped. Equip dispatch fails closed (`EquipNotYetSupported`) before any transfer. Equipment bucket mapping centralized in `ItemSlot::from_equipment_bucket`. RED→GREEN; workspace 1213, all-features 1224, fmt/clippy clean. Verify found unknown-rarity bypass (fixed); parent review found every-exotic-refused neighbor bug (fixed, realistic regression test). Commit `5c6154c`.
- [x] **EQ-003 — EquipItem adapter.** `EquipGateway`/`EquipItemRequest`/`EquipOutcome`/`EquipRejection` (NotInSocialSpace 1634/1671/1655/1654, ExoticLimit 1641, Unequippable 1640/1638/1639, ItemNotFound 1623, CharacterNotFound 1620, InvalidRequest; 1672 Transient; unknown Uncertain); `BungieEquipGateway` with exact EquipItem-only allowlist checked before credential access; shared hardened POST helpers extracted from transfer.rs (behavior unchanged); `QueuedEquipGateway` fake. Verify fixes: shared HTTP request Debug no longer prints raw URL/body keys (pre-existing leak), credential-access-recording ordering test, fragment/traversal/case negatives. Hyper pre-write retry documented. RED→GREEN; workspace 1224, fmt/clippy clean. No live smoke yet (EQ-004/005). Commit `876226e`.
- [x] **EQ-004 — Execution and verification.** `execute_equip` / `execute_equip_verified` share the ordered executor with gateways `(&mut transfer, &mut equip)`; `StepOutcome::EquipRejected(EquipRejection)` (+ MCP `EquipRejectionView`, additive); mandatory PostAction refresh after any attempted call; success = on target AND equipped; consistent partial progress = incomplete; contradiction = verification failure; no retry. Move entry points still reject equip plans. RED→GREEN (17 queued-fake scenarios); workspace 1225, all-features 1236, fmt/clippy clean. Independent review deferred to feature end (user request). Commit `27997e4`.
- [x] **EQ-005 — Offline journey.** `crates/destiny-adapters/tests/equip_execution_journey.rs`: 7 real-stack scenarios (vault and cross-character routes, no-op, 1641 ExoticLimit and 1634 NotInSocialSpace after transfer, uncertain equip with/without effect) with exact EquipItem wire checks, read-only profile requests, realistic neighbors, SQLite reopen, secret scans. No production bug found. Workspace 1232, all-features 1243, fmt/clippy clean. Commit `875c92b`.
- [x] **EQ-006 — CLI `ghostlink equip`.** `ghostlink equip <ITEM> --character <CHARACTER> [--json]` (character required); `equip.rs` orchestration, `equip_output.rs` stable JSON (status, item, character, steps[kind/from/to/outcome/reason], profile_refresh, observed) for MCP parity; runtime composes profile/transfer/equip gateways on one credential session; non-zero exit for refusals/incomplete. RED→GREEN (10 tests); workspace 1242, all-features 1253, fmt/clippy clean. Follow-up: gateway composition replicated from destiny-mcp (extract shared composition later). Commit `bbe9a32`.
- [ ] **EQ-007 — MCP `equip_item` and docs.** IN PROGRESS after restart; exact CLI JSON parity, README, architecture non-goals update. User approved a neutral `destiny-output` crate (shared JSON presentation; CLI exit codes remain CLI-specific) and the five additional workspace/CLI dependency paths. Independent reviews and authorized live smoke remain pending.

## Review corrections (authorized by user)

Independent correctness and security reviews requested changes despite green
checks (1244 default / 1258 all-features). No live actions performed.

- [x] **RC-001 — Early selector validation and membership binding.** Implemented;
  independent safety verification passed (app 2, MCP 4, stdio 1 focused tests).
  Remaining direct `preflight_move` ordering gap corrected with observed RED
  (one membership call vs zero) and GREEN (3 focused app tests); parent readback
  confirms guard follows intent authorization and precedes refresh. Changes
  remain uncommitted; final full-suite verification is scheduled in RC-004. Shared selector validation before clock,
  repository/profile/credentials in MCP (including direct callers); schema item
  limit 256, normalized Unicode expansion and numeric IDs covered. Planning and
  fresh preflight reject membership mismatch before no-op for equip and move.
  Runtime RED observed (257-character input, forbidden clock access, cross-account
  planning/no-op); GREEN: 1248 default / 1262 all-features, fmt/clippy/diff clean.
  Zero-match focused runs were replaced by appropriate all-feature commands.
- [x] **RC-002 — Revision-bound execution.** Independent verifier PASS, including
  RC-001 follow-up. Focused tests: app RC002 2, RC001 3, MCP equip 5, MCP RC002 2;
  fmt/diff clean. Changes remain uncommitted. Executor rejects ticket/current-revision mismatch before dispatch;
  shared MCP operation guard covers direct/async read and mutation dispatch
  through preflight, ordered calls and post-action verification. Lock order:
  operation -> service -> individual repository/gateway call. Refresh does not
  reacquire operation guard. Runtime RED: stale move/equip tickets dispatched;
  concurrent publication overtook dispatch. GREEN: zero stale-ticket calls,
  deterministic channel/interleaving tests; 1253 default / 1267 all-features,
  fmt/clippy/diff clean. Local process guarantee only; reads may wait behind
  mutations. External game/cross-process writes are not blocked.
- [x] **RC-003 — Dispatch-aware authentication evidence.** Implemented and
  independently verified; final bounded review PASS. Typed `AuthenticationRefused` for received HTTP 401/403
  in equip and move, recorded as attempted/stopping (never completed/uncertain)
  before propagating authentication failure. PostAction refresh mandatory;
  previous transfer progress and original evidence survive failed refresh.
  Pre-dispatch expired credentials/NotDispatched remain unattempted. Existing
  valid JSON unchanged; additive `authentication_refused` evidence in shared
  CLI/MCP projection. Runtime RED in scripted real-adapter auth scenarios;
  GREEN: 8 focused equip regressions, 1 move regression; workspace 1262 default /
  1276 all-features; fmt/clippy/diff clean. Final P2 correction implemented:
  definitive auth refusal remains IncompleteExecution even when fresh target
  holds; fresh observation and authentication reason retained. Uncertain plus
  confirmed target still succeeds. Runtime RED observed in real-adapter
  concurrent-target equip regression (0 passed, 1 failed); GREEN 17 equip /
  11 move journeys, MCP equip 5, stdio parity 1. Mixed auth -> availability
  refresh-failure regression preserves original step and suppresses observation.
  Worker final workspace 1265 / all-features 1279, fmt/clippy/diff pass.
  Independently confirmed 1265 default / 1279 all-features, 28 adapter journeys,
  MCP equip 5, stdio parity 1; fmt/clippy/diff pass. Changes remain uncommitted
  under the user's no-commit constraint.
- [x] **RC-004 — Independent re-review.** Final bounded review PASS. First
  final functional review REQUEST
  CHANGES; focused security PASS. RC-001/002 and original RC-003 dispatch/refresh
  defect resolved. All independent checks pass: 1262 default / 1276 all-features,
  fmt/clippy/diff clean. Remaining P2: refreshed target state after definitive
  AuthenticationRefused incorrectly yields VerifiedSuccess, CLI exit 0 and MCP
  isError false. Correction IN PROGRESS under RC-003: preserve non-success auth
  outcome with fresh observation; retain uncertain-plus-confirmed-state success.
  P2/P3 corrections independently verified (`muub5y54-g-mpjz`): non-success
  authentication outcome with fresh observation, HTTP403 -> HTTP503 failure
  retains prior progress/auth evidence without stale observations. All final
  offline checks PASS (1265 / 1279); no remaining findings in correction scope.
  Previous security PASS remains applicable to unchanged allowlists/credentials.
  No live calls or commits; EQ-007 final closure awaits authorized live smoke.

Revision, no-op identity and authentication evidence gaps are inherited from
move; equip exposes them too. They are not new move regressions. Reproductions
must be observed in deterministic tests before fixes are accepted.

## Progress

- Branch `feature/equip-item` from `main` (`5f96cf8`).

## Notes for EQ-002 (from EQ-001 verify)

- `MovePlan::new` still derives transfer-only steps; `MAX_MOVE_STEPS = 2`
  (`destiny-core/src/mutation.rs:87`) cannot fit two transfers + Equip;
  `via_vault = steps.len() > 1` must not count Equip (`:461`).
- No-op/success checks are location-only (`mutation.rs:522`,
  app `mutation.rs:961`, `:1318`): Equip needs equipped-state postconditions.
- Rejection evidence is `TransferRejection`; Debug impl (`:331`) and flat MCP
  mapping (`destiny-mcp/src/service.rs:1605`) need explicit Equip handling.

## Follow-ups (outside this WU)

- Weapon/armor bucket hashes still duplicated in `destiny-app/src/loadouts.rs:239`
  and `destiny-cli/src/loadouts_output.rs:90,139` (pre-existing, WU-030);
  migrate to `ItemSlot::from_equipment_bucket`.

## Next step

Offline implementation and RC-001 through RC-004 verification are accepted.
Live smoke FAILED verification: Ghost Primus instance `6917529045285107589`
from Vault to Hunter `2305843009300568190`, one authorized current-source equip
call exited 42 (`verification_failed`). Transfer/equip steps reported completed;
mandatory refresh reported refreshed but observed Vault / equipped=false.
Subsequent freshness-managed inspection also Vault / false, revision 7 unchanged
with newer checked_at; this is not proof of another provider request or eventual
live state. No retry/fallback/restoration. Source/config unchanged.
Bounded read-only diagnosis complete: PostAction bypasses freshness and fetches
HTTP before content dedupe; no reversed transfer direction established. CLI has
no `sync` command (attempt exited 2 without provider read). Latest inspect is
cached Vault / false; eventual game state and cause remain unknown. Source/config
unchanged; hash drift was solely this tracker update. Internal Forced sync API
exists, but no executable CLI route. Proposed next step: expose explicit forced
profile read, subject to user authorization; do not replay mutation or assume
provider eventual consistency. Later authorized forced sync succeeded:
revision 8 (previous 7), checked_at 1791150892; exact Ghost Primus instance
observed on the target Hunter with equipped=true. Desired state now confirmed
by newly acquired profile, without another mutation. Historical immediate
verification remains FAIL; cause unknown, EQ-007 closure remains pending.
Commit/push/PR remain unperformed.
