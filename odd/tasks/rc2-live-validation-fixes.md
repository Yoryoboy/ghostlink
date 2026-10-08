# rc.2 Live Validation Fixes (issue #71) — IN PROGRESS

Fix the findings from live validation of `v0.1.0-rc.2` before `0.1.0` ships.
Parent tracker: `odd/tasks/ghostlink-public-release.md` (PR-011).

## Scope and decisions (owner, 2026-10-08)

- Branch `fix/rc2-live-validation` from `main` at `691ad79`. One work-unit
  commit per task; push/PR/merge are owner decisions.
- **Reference model: Destiny Item Manager (DIM).** For open design questions,
  check how DIM does it first. Exploit SQLite to the maximum to make actions
  as fast as possible.
- **Drop the forced preflight.** Actions plan from the local SQLite projection
  (sync only when it is missing or past TTL), dispatch, and on Bungie
  acknowledgement apply the effect locally (DIM `itemMoved` /
  `itemLockStateChanged` reducers). Bungie is the final validator; its errors
  become clear refusals plus local invalidation.
- **Owner rule:** before any design question, check how DIM does it; if it
  is clear and fits, apply it directly. Only discuss when ambiguous or it
  does not fit Ghostlink.
- **No pending-effect window (as DIM).** Acknowledged effects are written to
  the projection and the projection is NOT invalidated after an action.
  Sync only on TTL expiry / missing projection / Bungie rejection. Discard
  profiles whose `responseMintedTimestamp` is not newer than the stored one
  (`d2-stores.ts:256-289`). Revisit an overlay only if rc.3 shows effects
  overwritten.
- Remove the `DESTINY_DEFINITION_VERSION` override; the version always comes
  from the stored Manifest.
- Test-first where applicable: `cargo test --workspace --all-features`; gates
  fmt, clippy `-D warnings`, `git diff --check`.

## Findings map

G6 stack pulls blocked · G2 stale state / slow chained actions · G3 false
`verified_noop` · G1 `get_loadouts` size · G4 definition version override ·
G5/G7 CLI UX.

## Tasks

- [x] **T1 — G6 stack pulls.** Absent `stackUniqueLabel` means "not unique"
  (`Some(false)`), not unknown (`crates/destiny-app/src/stack_pull.rs:352`).
  Add a fixture without the label (RED first).
  Done: like DIM (`!!stackUniqueLabel?.length`), uniqueness is
  `Some(label is non-empty)` once the instance kind is known (so the
  inventory block was decoded); the fingerprint stores an empty label.
  RED `absent_unique_label_means_not_unique` failed with
  `UnknownUniqueness`, then GREEN; the refusal-table case that pinned the old
  behavior was removed. Verifier: fmt, clippy `-D warnings`, workspace
  1,868 passed / 0 failed, `git diff --check`. Commit evidence: see git log
  (`fix(stack-pull): treat absent stackUniqueLabel as not unique`).
- [ ] **T2 — Acknowledged-effect store.** Repository API to apply
  acknowledged effects atomically (move/pull: location; lock: `locked`;
  equip: equipped flags + clear replaced same-slot item; stack pull:
  quantities), advance the projection revision. Decode and persist
  `responseMintedTimestamp`; reject non-newer profiles.
- Slice plan for T2/T3 (exploration 2026-10-08, estimates not measured):
  A1–A3 minted-timestamp type, decode + SQLite column, sync discard guard;
  B1–B2 effect enum + repository port (in-memory, SQLite); C1–C2 core
  ticket from committed projection + move/equip/equip_items effects; D lock;
  E1–E3 durable runtime bucket/coverage, cached capacity evidence,
  instance/stack pull effects; F1–F2 durable saved-loadout cache (206) +
  applied-loadout effects (DIM `ingame-loadout-apply.ts:50-83`). Each
  ~150–390 lines; total ~3.5–4.5k → stacked PRs to `main` (owner approved
  2026-10-08). Branch `feat/local-action-effects` from
  `fix/rc2-live-validation`.
  - [x] B1 pure core reducer `apply_inventory_effects` (relocate, lock,
    equip with caller-resolved unequip, instance pull); 11 tests, core 210
    pass, clippy clean. Commit: see git log (`feat(core): add acknowledged
    inventory effect reducer`).
  - [x] B2 `InventoryRepository::apply_acknowledged_effects(membership,
    effects)`: SQLite IMMEDIATE transaction updates changed item rows and
    bumps the revision once; keeps checked_at/source/invalidation; no-op
    writes nothing; in-memory parity; CLI/MCP wrappers forward. core,
    adapters, app tests + workspace clippy pass. Commit: see git log
    (`feat(inventory): persist acknowledged effects locally`).
- [ ] **T3 — DIM-style action pipeline.** Remove forced preflight acquisition
  from move, lock, equip, pull item, pull stack and apply loadout; plan and
  execute from the local projection; apply the effect from T2 on
  acknowledgement. Keep pull capacity checks local (with effects applied).
- [ ] **T4 — Bungie error mapping.** Map stale-state errors (item not found,
  already at destination, `DestinyNoRoomInDestination`, equipped-item
  conflicts) to clear refusals and invalidate the projection; never retry
  blindly. Check DIM's blind-move/no-room handling for pulls.
- [ ] **T5 — Honest no-op and response fields (G3).** `verified_noop` only
  from local state that includes our own effects; fix or remove
  `profile_refresh` and tool descriptions that claim invalidation.
- [ ] **T6 — G1 loadouts.** `get_loadouts` returns summaries (slot, name,
  subclass, item names); new per-loadout detail tool by slot/name with
  deduplicated plug text. CLI parity. Text/`structuredContent` duplication
  reviewed separately after checking Pi support.
- [ ] **T7 — G4 definition version.** Remove `DESTINY_DEFINITION_VERSION`
  (config, docs, example env, tests); mismatch refusals name both versions.
  Review whether `DESTINY_FRESHNESS_TTL_SECONDS` should remain user-facing.
- [ ] **T8 — G5/G7 CLI UX.** Human sentences for `pull`/`pull-stack`
  refusals; `pull-stack --character` accepts a class; `--item-hash` hint;
  `apply-loadout` selector consistency with MCP `name`/`slot`.
- [ ] **T9 — Docs, skill and live re-validation.** Update `docs/`,
  `skills/ghostlink/SKILL.md`; cut rc.3 and re-run chained actions live.

## Evidence

- Exploration (2026-10-08): DIM plans from in-memory store with no fetch
  before writes (`item-move-service.ts`, `destiny2-api.ts:186-263`), applies
  effects after ack (`reducer.ts:192-333`), 100 ms per-endpoint write spacing
  (`rate-limit-config.ts`). Ghostlink forces 2–4 GETs per action
  (`mutation.rs:1326-1355`, `sync.rs:255-280`) and
  `sqlite/inventory.rs:159-170` clears invalidation on any publication.
