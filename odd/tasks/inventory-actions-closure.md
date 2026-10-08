# Inventory Actions Closure (Phase 3) — IN PROGRESS

Close Phase 3 (Inventory Actions): optimistic action semantics, apply saved
in-game loadouts, batch equip, cleanups, and a final interactive live validation.

## Scope and decisions (user, 2026-10-08)

- Order: optimistic semantics → apply loadout → batch equip → cleanups → live
  validation. Build Engine starts only after this closes.
- **Optimistic semantics for every action** (move, equip, lock/unlock, Postmaster
  pull, pull-stack): a Bungie acknowledgement without error is success (exit 0,
  MCP success). The mandatory post-action refresh/verification is removed because
  Bungie profile lag produced false negatives (EQ-007). Bungie errors, refusals,
  preflight failures and partially executed sequences still fail; no blind retry.
- After an acknowledged action the local projection must not present the old
  state as fresh: the next read must force a provider sync (cache invalidation),
  instead of writing guessed state into SQLite.
- Apply loadout uses the official
  `POST /Destiny2/Actions/Loadouts/EquipLoadout/` with body
  `{loadoutIndex, characterId, membershipType}`; scope `MoveEquipDestinyItems`;
  throttle 1 action/s; character must be in orbit, a social space or offline.
- Batch equip uses `POST /Destiny2/Actions/Items/EquipItems/` (currently
  rejected by the equip allowlist on purpose).
- Live validation is last and interactive: the user opens the game, the agent
  drives the CLI, the user confirms in-game. No live account action before that.
- Worktree `../ghostlink-actions`, branch `feature/inventory-actions-closure`
  from `origin/main` at `9dc4ad9`. Work-unit commits only; push/PR/merge are
  user decisions.
- Test-first: `cargo test --workspace --all-features` runner; gates fmt, clippy
  `-D warnings`, `git diff --check`.

## Mapping evidence (read-only explorer, 2026-10-08)

- Move/equip share `MutationService::verify_attempt`
  (`crates/destiny-app/src/mutation.rs:1318`); outcomes `VerifiedSuccess`,
  `VerifiedNoOp`, `AppliedPendingProviderRefresh`, `IncompleteExecution`,
  `VerificationFailed` (`:621–636`).
- Lock has `verify_lock` (`crates/destiny-app/src/lock.rs:272`, `:287–342`).
- Pull/stack observation loop: `instanced_pull.rs:820`, `stack_pull.rs:616`;
  dispositions `Verified`, `NotYetObserved`, `Uncertain`, `Refused`.
- CLI exit mapping: move/equip/lock 0/41/42/43; pull `Uncertain`→42,
  `NotYetObserved`→43. MCP success predicates: `crates/destiny-mcp/src/server.rs:387–455`.
- Loadouts read-only: app `loadouts.rs:88–145`, adapter `loadout.rs` component 206.
- Equip port is single-item (`destiny-core/src/ports.rs:1176`); `EquipItems/`
  is rejected by `destiny-adapters/src/equip.rs:258–283`.
- Action allowlists are per adapter: equip `:23–27`, transfer `:83`, lock
  `:22–26`, pull `instanced_pull.rs:33–39`.

## Tasks

- [x] **IAX-001 — Optimistic move and equip.** Replace post-action
  verification in the move/equip path with acknowledgement-based success;
  invalidate local freshness; CLI exit 0 and MCP success on acknowledgement;
  keep incomplete/refusal/preflight failures.
- [ ] **IAX-002 — Optimistic lock/unlock.**
- [ ] **IAX-003 — Optimistic Postmaster pull and pull-stack.**
- [ ] **IAX-004 — EquipLoadout adapter and port** (allowlist, request, receipt).
- [ ] **IAX-005 — Apply loadout service, CLI and MCP** (resolve slot and
  character, optimistic outcome, docs/skill).
- [ ] **IAX-006 — EquipItems batch adapter and port** (per-item results).
- [ ] **IAX-007 — Batch equip service, CLI and MCP** (combined plan, transfers
  from Vault first, one EquipItems call, per-item outcomes, docs/skill).
- [ ] **IAX-008 — Cleanups.** Close Postmaster tracker status; replace
  duplicated bucket hashes in `destiny-app/src/loadouts.rs` and
  `destiny-cli/src/loadouts_output.rs` with `ItemSlot::from_equipment_bucket`.
- [ ] **IAX-009 — Interactive live validation** (user in game, agent drives CLI):
  move, equip, lock/unlock, pull, pull-stack, apply loadout, batch equip.

## Progress and evidence

- 2026-10-08: worktree and branch created; tracker written.
- IAX-001 implementation progress: move/equip return acknowledgement-based
  `Applied` (CLI 0 / MCP success), without mandatory post-action verification.
  Replaced the provisional timestamp marker with repository-owned invalidation:
  SQLite migration 19 preserves content, revision and real acquisition time;
  reopen retains invalidation, ordinary reads force sync, successful publication
  clears it. Uncertain/refused/partial steps remain failures; lock/pull unchanged.
  Incomplete move `expected_location` is receipt-derived, not observed state.
  Offline workspace GREEN: 1,837 tests passed, zero failed, 66 result suites
  (including empty suites and doctests). Documentation/skill updated; checkbox
  remains open for parent review. No live account validation or commit.
- IAX-001 independent verification (gentle-ai-verify): fmt, clippy `-D warnings`,
  `cargo test --workspace --all-features` 1,837 passed / 0 failed, default
  features 1,816 / 0, `git diff --check` all PASS. Applied only when every step
  is Completed (`mutation.rs:1258–1277`); no post-action profile request;
  migration 19 additive with downgrade refusal while invalidated. Parent fixed
  stale doc comment and RefreshFailed wording ("local state could not be
  updated after the action"; storage failure after acknowledgement still exits
  42, fail-closed). Lock/pull unchanged.
