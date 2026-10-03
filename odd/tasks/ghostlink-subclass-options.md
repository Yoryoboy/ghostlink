# Unlocked subclass options (WU-031)

## Objective and scope

Expose which subclass options (supers, aspects, fragments, grenades, melees,
class abilities, movement) are available per character class and element, and
whether each is unlocked.

Out of scope: unlocking/purchasing options, non-subclass plug sets (shaders,
ornaments, mods).

## Live evidence (2026-10-03)

- `profilePlugSets` and `characterPlugSets` arrive with component 305, which
  the profile sync already requests; the decoder ignores them today.
- Profile plug sets: 924 sets; items `{ plugItemHash, canInsert, enabled }`.
  Subclass plugs follow the `<class|shared>.<element>.<kind>` categories.
- Character plug sets: per-character extras (e.g. `hunter.stasis.totems`,
  `shared.stasis.grenades`).
- `canInsert` is coherent for the Hunter (most recently played) but not for
  Titan/Warlock (equipped Hammer of Sol reports `canInsert: false`).

## Decisions

- Decode plug sets during the existing profile sync; no new provider call.
- Persist only subclass plugs (`classify_subclass_plug` != None, "Empty ...
  Socket" placeholders excluded) in a new table (migration v12), with scope
  (profile or character id), `can_insert`, `enabled`.
- Status per option: `unlocked` / `locked` from `can_insert` only for the class
  of the most recently played character (`dateLastPlayed`); other classes get
  `unknown`. `shared.*` plugs (fragments, grenades) are evaluated the same way
  as the most recent class.
- Options are listed for a character's own class plus `shared.*`, grouped by
  element and kind.

## Open questions

- OPEN (user to verify in-game): Hunter Strand reports `canInsert: false` for Whirling Maelstrom and Threaded Specter while Threaded Specter is equipped on Prismatic. If they are unlocked in-game, `canInsert` is not a reliable unlock signal even for the most recent class and the status rule must change. Until then, outputs also expose raw `can_insert`/`enabled`.

## Constraints and acceptance

- Sync stays bounded; snapshot publication remains atomic.
- CLI/MCP parity; test-first; live smoke before closing provider-facing tasks.

## Tasks

- [x] **SO-001 — Decode and persist subclass plug sets.** `PlugSetEntry`/`PlugSetScope`, migration v12 `subclass_plug_options`. RED→GREEN; workspace 1184, clippy clean; live sync stored 304 options (181 can_insert). Commit `f252ac2`.
- [x] **SO-002 — Options query.** `QueryService::subclass_options`, `SubclassElement`, `OptionStatus`; migration v13 `characters.date_last_played` (live: Hunter 2026, Warlock 2021, Titan 2020). RED→GREEN; workspace 1188, clippy clean. Commit `bee7fba`.
- [x] **SO-003 — CLI command.** `ghostlink subclass-options [--character] [--element] [--locked]`, JSON in `subclass_options_output.rs`. RED→GREEN; workspace 1195, clippy clean; live smoke OK (Hunter reliable w/ locked list; Titan unknown). Commit `964dec1`.
- [x] **SO-004 — MCP tool and docs.** `get_subclass_options` (character + element inputs), exact CLI parity, README with status caveat + open question. RED→GREEN; stdio 11, clippy clean. Commit `a6cfa71`.

## Progress

- Branch `feature/subclass-options` stacked on `feature/loadouts`.

## Next step

WU-031 complete. Open: verify canInsert vs in-game unlocks (Threaded Specter, Whirling Maelstrom, Thread of Binding, Phantom Surge).
