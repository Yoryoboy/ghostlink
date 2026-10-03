# In-game loadouts (WU-030)

## Objective and scope

Expose each character's saved in-game loadouts: loadout name, and per item the
item name/type plus the configured plugs (weapon perks, armor mods, subclass
super/aspects/fragments/abilities, artifact perks).

Out of scope: applying/equipping loadouts (mutation), loadout icon/color art,
unlocked subclass options.

## Live evidence (2026-10-03, component 206)

- `characterLoadouts.data[cid].loadouts[]`: 6 slots per character, each
  `{ nameHash, colorHash, iconHash, items[] }`.
- Unused slot: every `itemInstanceId` is `"0"`.
- A used loadout holds 10 items: 3 weapons, 5 armor, subclass, artifact.
  Each item is `{ itemInstanceId, plugItemHashes[] }`; `plugItemHashes` is
  indexed by socket; `2166136261` means "not set" and is skipped.
- Item instances resolve against the synced inventory snapshot; plug hashes
  resolve against item definitions. Loadout names need
  `DestinyLoadoutNameDefinition` (not loaded today).

## Decisions

- Live-only read via a dedicated gateway call
  `GET /Destiny2/{t}/Profile/{id}/?components=206` with its own allowlist
  predicate (same pattern as the artifact gateway).
- Load `DestinyLoadoutNameDefinition` into the manifest store for names.
  Color/icon kept as raw hashes only.
- Item names/types come from the synced snapshot + definitions; an instance
  missing from the snapshot is reported with `name: null` (not an error).
- Subclass plugs are grouped with the existing `SubclassPlugKind` classifier.
- Unused slots are omitted.

## Constraints and acceptance

- Bounded strict decoding; CLI/MCP parity; test-first; live smoke test before
  closing each task that touches provider data.

## Tasks

- [x] **LO-001 — Loadout name definitions.** Migration v11 `loadout_name_definitions`, `WorldDefinitionRepository::lookup_loadout_name`. RED→GREEN; workspace 1151, clippy clean; live sync-manifest stored 22 names. Commit `7b5049c`.
- [ ] **LO-002 — Loadouts gateway.** `?components=206` URL/allowlist, bounded decoder, domain types.
- [ ] **LO-003 — Loadouts query.** Join instances/definitions, classify plugs, omit unused slots.
- [ ] **LO-004 — CLI command.** `loadouts [--character]`, text and JSON.
- [ ] **LO-005 — MCP tool and docs.** `get_loadouts`, parity tests, README.

## Progress

- Branch `feature/loadouts` stacked on `feature/artifact`.

## Next step

LO-002.
