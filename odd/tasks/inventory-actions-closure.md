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
- [x] **IAX-002 — Optimistic lock/unlock.**
- [x] **IAX-003 — Optimistic Postmaster pull and pull-stack.**
- [x] **IAX-004 — EquipLoadout adapter and port** (allowlist, request, receipt).
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
- IAX-002 implementation progress: lock/unlock now return acknowledgement-based
  `Applied` (CLI 0 / MCP success) without post-action profile verification, and
  reuse repository-owned durable freshness invalidation after dispatched attempts.
  Fresh no-op remains verified; refusal/auth/transient/uncertainty remain failures
  (41), with no retry. Invalidation storage failure fails closed (42), preserving
  the receipt and reporting "local state could not be updated after the action".
  RED observed stale-profile CLI pending/43 and MCP verified-success instead of
  `applied`; offline workspace GREEN: 1,843 passed / 0 failed. Docs/skill updated;
  checkbox intentionally remains open. No live account action or commit.
- IAX-003 RED progress: added stale-Postmaster acknowledgement regressions for
  both pull services. Offline `cargo test --workspace --all-features` observed
  `NotYetObserved` (instanced) and `Uncertain` (stack) instead of `Applied`;
  destiny-app had 439 passed / 2 failed. Implementation and CLI/MCP regressions
  remain pending; checkbox intentionally stays open. No live account action.
- IAX-003 implementation progress: pull and pull-stack return receipt-based
  `Applied` (CLI 0 / MCP success), with no post-action profile request, wait or
  retry. Every dispatched outcome invalidates repository freshness; invalidation
  failure retains the receipt and fails closed (CLI 42 / MCP error). Refusals and
  transport uncertainty remain failures (41). Storage-failure behavior is covered
  for both services, CLI pipelines and MCP stdio tools. Deleted tests covered
  arrival classification, observation ordering/timestamps, refresh retries and
  quantity-delta verification; preflight, membership/definition safety, capacity,
  quantity selection and single-dispatch coverage remain. Receipt/refusal,
  uncertainty and sanitized failure-output cases were replaced for the new contract.
  Offline GREEN: 1,818 passed / 0 failed / 0 ignored across 66 result suites;
  fmt check, clippy `-D warnings` and diff check PASS. Docs/skill and pull help
  updated. Checkbox intentionally stays open for parent review; no live action
  or commit.
- IAX-003 coverage follow-up: restored exact refusal receipts, applied dispatch/redaction and hostile control cleaning; retained stack equivalents and restored text control safety; workspace 1,820 passed / 0 failed.
- IAX-004 implementation progress: added the validated zero-based EquipLoadout
  request (0–11, matching the existing saved-loadout bound), narrow action port,
  bounded typed receipts, isolated exact endpoint allowlist and hardened shared
  transport machinery. Requires MoveEquipDestinyItems; callers pace at 1 action/s.
  Restricted-location refusals use the existing Bungie ErrorStatus mappings;
  unknown envelopes remain uncertain. Added queued fake gateway and offline
  transport/redaction/boundary tests. RED: missing core types/port; GREEN: 1,829
  passed / 0 failed / 0 ignored across 67 suites; fmt and clippy PASS. No service,
  CLI/MCP wiring, live account action or commit; checkbox remains open.
- IAX-001 committed as `635a552`.
- IAX-002 independent verification: fmt, clippy, all-features and default
  workspace tests (0 failures), diff check PASS; Applied only on HTTP 200 +
  ErrorCode 1; no post-action request; Vault routing via first committed
  character preserved; move/equip/pull untouched. No findings.
- IAX-002 committed as `0bf420f`.
- IAX-003 independent verification: first pass FAIL on deleted-test audit
  (44 deleted/renamed: 21 observation-only, 23 safety/mixed; three output safety
  tests only partially replaced). Coverage restored (test-only); narrow
  re-verification PASS: 1,820 passed / 0 failed, clippy and fmt PASS, production
  unchanged. Default features 1,796 / 0.
- IAX-003 committed as `0c71883`.
- IAX-004 parent spot check: isolated allowlist rejects other URLs before
  credentials; transport reuses the hardened transfer client; focused
  `loadout_action` suite 7 passed. Writer full suite 1,829 / 0. Independent
  full verification deferred to the IAX-005 verifier (covers both).
