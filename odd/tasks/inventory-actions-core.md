# Inventory Actions Core (Feature A) — IN PROGRESS

## Objective

Close the small gaps of the Inventory Actions phase with the existing
preflight → action → verification pattern:

- `ghostlink move` in the CLI (today `move_item` is MCP-only);
- lock/unlock one owned item (Bungie `SetLockState`) in CLI and MCP.

Feature B (Postmaster, multi-item equip, apply in-game loadouts) follows on
its own branch in the same worktree.

## Decisions

- Worktree `../ghostlink-inventory`, branch `feature/inventory-actions-core`
  from `origin/main` at `c353045`.
- CLI move reuses the shared app `MutationService` (`plan_move` +
  `execute_move_verified`); no MCP dependency. Destination is exactly one of
  `--character <id|class>` or `--vault`. Move gets its own transport-neutral
  output contract; equip output is not reused as the move schema.
- Lock is its own mutation seam (core port, exact-endpoint adapter, app
  service, thin CLI/MCP adapters). Verification compares the refreshed
  `instance.locked` to the requested state. Equip policy (class, exotic,
  equipped state) is not reused.
- Bungie contract assumption to validate live: POST
  `Destiny2/Actions/Items/SetLockState/` with
  `{state, itemId, characterId, membershipType}`, OAuth scope
  `MoveEquipDestinyItems`; for Vault items any owned character id is accepted.
- Dropped from scope: "first-read freshness stale". Already fixed by GAM-004
  (`crates/destiny-app/src/queries.rs:910-917`, regression tests at
  `queries.rs:2600`); older follow-up notes are superseded.

## Constraints and acceptance

- Test-first per behavior task (observe RED, then GREEN).
- CLI and MCP stay thin; shared logic lives in destiny-app/destiny-core.
- Docs move with behavior: `docs/cli.md`, `docs/mcp.md`, `README.md`,
  `skills/ghostlink/SKILL.md` (skill drift test), `docs/mvp-implementation-plan.md`.
- Checks: `cargo fmt --all -- --check`, clippy with `--all-features -D warnings`,
  `cargo test --workspace` and `--all-features`, `git diff --check`.
- One Conventional Commit per task; push/PR/merge are user decisions.

## Tasks

- [x] **IAC-001 — Establish worktree, branch and tracker.**
- [ ] **IAC-002 — CLI `ghostlink move`.** Parsing (`--character` xor `--vault`),
  runner/runtime composition, thin orchestration, move output contract and exit
  codes, tests, docs and skill.
- [ ] **IAC-003 — Lock seam in core and adapter.** Provider-neutral request,
  gateway and outcome types; `SetLockState` adapter with endpoint allowlist and
  sanitized outcomes; adapter journey tests.
- [ ] **IAC-004 — Lock app service.** Plan with fresh preflight (owned
  instance, lockable, verified no-op when already in the requested state),
  execute, post-action refresh and verification; fakes and tests.
- [ ] **IAC-005 — Expose lock in CLI and MCP.** `ghostlink lock` /
  `ghostlink unlock`, MCP `set_lock_state` tool, offline fixture, docs, skill.
- [ ] **IAC-006 — Close.** Full checks, tracker evidence, live validation
  notes for the user.

## Evidence

- IAC-001: worktree created at `c353045`.
