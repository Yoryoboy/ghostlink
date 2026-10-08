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
  - [x] C1 move + single equip dispatch from the committed projection:
    `ExecutionTicket::from_committed_projection`, ordinary sync (fresh = 0
    acquisitions, stale/invalidated = 1); post-action invalidation kept
    until C2. RED `fresh_committed_move_dispatches_without_profile_acquisition`
    (ProviderUnavailable) then GREEN; stdio/CLI tests adapted (local no-op,
    local drift, typed provider rejection). Workspace tests + clippy pass.
    Size: +526/-228 (over the ~400 budget; bulk is adapted tests and the
    stdio fixture, not splittable without red intermediate commits).
    Commit: see git log (`feat(actions): dispatch move and equip from the
    local projection`).
  - [x] C2 move, equip and `equip_items` apply acknowledged effects
    (each acknowledged transfer leg; equip unequips the same-bucket item via
    definitions). Fully applied: no invalidation; incomplete/uncertain:
    acknowledged legs applied + invalidated; local persistence failure keeps
    the receipt, invalidates, never retries. RED for chained move freshness,
    equip projection and batch partial success, then GREEN; workspace tests
    + clippy pass. +435/-60. Note: equip planning already requires a
    definition for every equipped item (`UnknownRarity`), pre-existing.
    Commit: see git log (`feat(actions): apply acknowledged move and equip
    effects locally`).
  - [x] D lock/unlock: ordinary sync + committed-projection validation;
    acknowledged `SetLocked` persisted, no invalidation when applied.
    RED (forced acquisition, applied lock invalidated) then GREEN;
    unlock→lock chains with zero acquisitions and no false `verified_noop`
    (G3 root cause). Workspace tests + clippy pass. +190/-41. Commit: see
    git log (`feat(actions): lock and unlock from the local projection`).
  - [x] A1–A3 `responseMintedTimestamp` guard: `ProfileMintedAt` on
    `ProfileAcquisition` (builder, no snapshot constructor churn), migration
    20 nullable column, atomic publication; Ordinary sync discards
    same-membership profiles with older/equal minted time (projection,
    effects, revision, checked_at, invalidation kept; observations dropped).
    Preflight/PostAction still publish until E/F. Unusable minted values are
    treated as unknown (DIM `new Date(x ?? 0)`), never failing a sync; .NET
    7-digit fractions tested. Workspace 1,899 pass, clippy clean. +765/-42
    (size:exception). Commit: see git log (`feat(sync): discard profiles not
    newer than the stored one`).
  - [x] E1 instanced pulls from the local projection (`670e276`, fix
    `85681b8` known-full refuses even with unknown occupants): ordinary sync,
    occupancy by definition bucket (DIM) vs audited capacities, unknown
    occupancy dispatches, Bungie 1642 → typed destination-full, acknowledged
    `PullFromPostmaster` applied. No per-action Manifest evidence fetch.
  - [x] E2 `InventoryEffect::PullStack` + SQLite stack persistence
    (`7024f92`, DIM reducer.ts:231-295; not idempotent).
  - [x] E3 stack pulls from the local projection (`1d858b2`): ordinary sync,
    audited capacities, 1642 mapping, `PullStack` applied once, expansion
    bound (≤50 new rows). Capacity-evidence port kept for compatibility.
- [ ] **T3 — DIM-style action pipeline.** Remove forced preflight acquisition
  from move, lock, equip, pull item, pull stack and apply loadout; plan and
  execute from the local projection; apply the effect from T2 on
  acknowledgement. Keep pull capacity checks local (with effects applied).
- [ ] **T4 — Bungie error mapping.** Map stale-state errors (item not found,
  already at destination, `DestinyNoRoomInDestination`, equipped-item
  conflicts) to clear refusals and invalidate the projection; never retry
  blindly. Check DIM's blind-move/no-room handling for pulls.
- [ ] **T5 — Honest no-op and response fields (G3).** Includes native-review
  advisory R3-001 (`crates/destiny-mcp/src/schema.rs:379`): the `equip_item`
  description still says "the next ordinary read forces sync", false after C2
  when fully applied. `verified_noop` only
  from local state that includes our own effects; fix or remove
  `profile_refresh` and tool descriptions that claim invalidation.
- [x] **T6 — G1 loadouts.** Done on `feat/compact-loadouts` (from main),
  commit `b657b88`, PR #78; review-eaee74a19c8192e0. Plug texts deduplicated
  by content (`plug_insights` + per-plug `insight` index); fixture 56,441 →
  581 bytes. Original scope: `get_loadouts` returns summaries (slot, name,
  subclass, item names); new per-loadout detail tool by slot/name with
  deduplicated plug text. CLI parity. Text/`structuredContent` duplication
  reviewed separately after checking Pi support.
- [x] **T7 — G4 definition version.** Done on `fix/remove-definition-version-override`
  (from main), commit `b026fdd`, PR #76; native review review-73e258b88f25a6f6.
  Old key ignored. Mismatch message naming both versions deferred to T8.
  Original scope: Remove `DESTINY_DEFINITION_VERSION`
  (config, docs, example env, tests); mismatch refusals name both versions.
  Review whether `DESTINY_FRESHNESS_TTL_SECONDS` should remain user-facing.
- [ ] **T8 — G5/G7 CLI UX.** Human sentences for `pull`/`pull-stack`
  refusals; `pull-stack --character` accepts a class; `--item-hash` hint;
  `apply-loadout` selector consistency with MCP `name`/`slot`.
- [ ] **T9 — Docs, skill and live re-validation.** Update `docs/`,
  `skills/ghostlink/SKILL.md`; cut rc.3 and re-run chained actions live.

## Delivery

- Stacked PRs: #72 (T1) → main, #73 (B1+B2) → #72, #74 (C1+C2) → #73,
  #75 (D + tracker) → #74. #76 (T7) → main. Follow-ups stack on
  `feat/local-action-effects` (next: `feat/profile-minted-guard`).

## Evidence

- Native review (RDD on, 2026-10-08), one slice per lineage, all approved
  and acknowledged (authority burned): T1 `691ad79..3d5d39c`
  review-d5bfe684fb9934d6; B1+B2 `3d5d39c..5fdc960` review-74b305ea826a912f;
  C1+C2 `5fdc960..2286f97` review-39e8e68a24f8a08f (advisory R3-001 → T5);
  D `2286f97..8df605d` review-d303c06c0c266e3c; A `fd36f4f..5e1e4a3`
  review-932172cc582153fc (advisory R3-001 stale docs on malformed minted
  values, fixed in the follow-up docs commit). T6 review-eaee74a19c8192e0;
  E1 review-6360156695547b12 (advisory → fix `85681b8`,
  review-b350a2e156c22cc5); E2 review-f25d4dfae107b3ee (advisory → E3
  expansion bound); E3 review-2657454001cb4b39.

- Exploration (2026-10-08): DIM plans from in-memory store with no fetch
  before writes (`item-move-service.ts`, `destiny2-api.ts:186-263`), applies
  effects after ack (`reducer.ts:192-333`), 100 ms per-endpoint write spacing
  (`rate-limit-config.ts`). Ghostlink forces 2–4 GETs per action
  (`mutation.rs:1326-1355`, `sync.rs:255-280`) and
  `sqlite/inventory.rs:159-170` clears invalidation on any publication.
