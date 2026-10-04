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
- [x] **EQ-006 — CLI `ghostlink equip`.** `ghostlink equip <ITEM> --character <CHARACTER> [--json]` (character required); `equip.rs` orchestration, `equip_output.rs` stable JSON (status, item, character, steps[kind/from/to/outcome/reason], profile_refresh, observed) for MCP parity; runtime composes profile/transfer/equip gateways on one credential session; non-zero exit for refusals/incomplete. RED→GREEN (10 tests); workspace 1242, all-features 1253, fmt/clippy clean. Follow-up: gateway composition replicated from destiny-mcp (extract shared composition later).
- [ ] **EQ-007 — MCP `equip_item` and docs.** README, architecture non-goals update.

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

EQ-007 (MCP `equip_item` pinned to `EquipOutput::json()` parity, README, architecture non-goals). Then deferred independent reviews (user request) and live smoke with user in orbit/Tower.
