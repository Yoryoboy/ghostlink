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

- [x] **EQ-001 — Generalize mutation steps.** `MutationStep { ordinal, action: StepAction }`, `StepAction::Transfer { from, to }` only variant; executor dispatches by kind, stop via `StepOutcome::continues()`; MCP flat step JSON and `MoveStep` Debug shape preserved. Refactor (RED n/a); workspace 1198 passed (unchanged; worker's 1199 baseline was a miscount, `#[test]` count identical), all-features 1209, fmt/clippy clean; independent verify pass-with-notes.
- [ ] **EQ-002 — Equip planning.** Resolve item + character; transfer steps + equip step; refusals; no-op.
- [ ] **EQ-003 — EquipItem adapter.** Dedicated allowlisted endpoint, body, status classification, throttle.
- [ ] **EQ-004 — Execution and verification.** Heterogeneous step dispatch; verify equipped on target.
- [ ] **EQ-005 — Offline journey.** Real-stack scripted journey for equip scenarios.
- [ ] **EQ-006 — CLI `ghostlink equip`.**
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

## Next step

EQ-002.
