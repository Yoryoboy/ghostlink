# Ghostlink Build Engine (Phase 4) — IN PROGRESS

Roadmap: `Destiny AI Companion — Objetivo, alcance y arquitectura.md` §17 and §23
(build requirements, inventory matching, missing items, substitutions,
coverage, roll matching, prepare/execute build plan).

## Working rule (owner, 2026-10-11)

DIM first, with autonomy: for any blocker or doubt, check how Destiny Item
Manager solves it; if clear and it fits Ghostlink, apply it and record the
DIM evidence here. Ask the owner only when DIM is ambiguous or does not fit
(CLI process per command, SQLite projection, MCP for agents).

- DIM clone: `/tmp/dim-research-3/DIM` (commit `29c122e`, 2026-10-10).
- DIM API types: `/tmp/dim-research-3/dim-api/api/shapes/loadouts.ts`.
- Re-clone with `git clone --depth 1` if missing.

## DIM model (research 2026-10-11)

- A build is a DIM `Loadout`: `id`, `name`, `notes`, `classType`,
  `equipped[]`/`unequipped[]` of `LoadoutItem {id?, hash, amount?,
  socketOverrides?, craftedDate?}`, optional `LoadoutParameters`
  (`statConstraints` with `minStat/maxStat` and legacy tiers ×10, `mods`,
  `modsByBucket`, `perks`, `setBonuses`, `exoticArmorHash`, `query`,
  `assumeArmorMasterwork`, `autoStatMods`, `artifactUnlocks`, clear flags,
  `inGameIdentifiers`). Evidence: `dim-api/api/shapes/loadouts.ts:3-254`.
- Sources: DIM saved loadouts, equipped snapshot
  (`DIM src/app/loadout-drawer/loadout-utils.ts:178`), Bungie in-game
  loadouts converted to the same model
  (`src/app/loadout/loadout-type-converters.ts:120`), imports of DIM JSON
  `/loadouts?loadout=<JSON>` and `dim.gg` share IDs
  (`src/app/loadout/loadout-share/loadout-import.ts:14-125`). No third-party
  build-site adapters; those sites hand out DIM links.
- Resolution: instanced items by instance id, then crafted date; subclasses
  and some buckets by hash; missing items become placeholders with the
  original spec kept (`loadout-utils.ts:554-737`,
  `loadout-item-conversion.ts:27-99`). No silent same-hash substitution.
- Analysis: DIM Loadout Analyzer findings (missing items, invalid mods,
  fragment slots, incomplete armor, exotic mismatch, mods do not fit, stat
  constraints unmet, armor upgrades needed, seasonal mods, invalid search,
  better stats available) (`src/app/loadout-analyzer/types.ts:40-88`). No
  percentage "coverage"; Ghostlink reports counts derived from findings.
- Substitutes: whole-set Loadout Optimizer over owned armor
  (`src/app/loadout-builder/item-filter.ts`, `process-worker/types.ts`).
- Apply: DIM `loadout-apply.ts:355-660` phases (dequip with replacements,
  transfers, bulk equip, socket overrides, mods, optional clearing).

## Decisions

- D1 (DIM): the build definition model is the DIM `Loadout` API shape, so
  DIM JSON imports round-trip. Unknown parameter fields are preserved.
- D2 (DIM): sources in order: import of DIM JSON/URL (offline), equipped
  snapshot, in-game loadout conversion. Stored locally in SQLite (Ghostlink
  equivalent of DIM Sync).
- D3 (DIM): resolution never substitutes silently; substitutes come only from
  the optimizer task.
- D4 (owner pending): `dim.gg` share import needs a DIM API key
  (`X-API-Key`, `src/app/dim-api/dim-api-helper.ts:9,35`) and a third-party
  network dependency. Deferred to BE-009.

## Constraints

- Test-first for behavior (RED, GREEN, refactor).
- Fake and SQLite repositories keep parity; CLI and MCP stay in step.
- New persisted content is decoded, synced and stored in the same commit.
- Gates: `cargo test --workspace --all-features`, fmt, clippy
  `-D warnings` (with and without `--all-features`), `git diff --check`.
- One work-unit commit per task on `feature/ghostlink-build-engine`;
  push/PR/merge are owner decisions.

## Tasks

- [x] **BE-001 — Tracker and branch.** Worktree `../ghostlink-builds`, branch
  `feature/ghostlink-build-engine` from `origin/main` `a86584e`.
- [ ] **BE-002 — Core build model.** DIM `Loadout` types in `destiny-core`
  with serde round-trip; parse DIM API JSON and `/loadouts?loadout=` URLs;
  DIM import normalization (fresh id, legacy tiers ×10).
- [ ] **BE-003 — Build store and sources.** SQLite migration and repository
  (fake parity); create from import, equipped snapshot and in-game loadout;
  CLI `build import|list|show|delete|from-equipped|from-loadout`; MCP tools.
- [ ] **BE-004 — Resolution and analysis v1.** DIM resolution semantics;
  findings supported by current data (missing items, exotic mismatch,
  invalid/nonmatching search, location of each item); CLI `build check`,
  MCP `analyze_build`.
- [ ] **BE-005 — Armor 3.0 data.** Base stats, set bonuses, energy, tuning,
  masterwork, socket indices for inventory plugs (DIM sources first).
- [ ] **BE-006 — Analysis v2.** Stat constraints, mods fit, armor upgrades.
- [ ] **BE-007 — Apply build.** DIM apply phases; covers dim-parity U4b/U4c.
- [ ] **BE-008 — Substitutes.** Optimizer-based armor sets.
- [ ] **BE-009 — dim.gg import.** Blocked on owner decision D4.
- [ ] **BE-010 — Live validation.**
