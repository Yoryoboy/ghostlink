# Equipped subclass configuration (WU-028)

## Objective and scope

Expose each character's equipped subclass configuration (super, aspects,
fragments, grenade, melee, class ability, movement, transcendence) through the
CLI and MCP, using the profile components already requested (205 equipment,
305 item sockets).

Out of scope: unlocked/available subclass options (plug sets, components
105/206), seasonal artifact, in-game loadouts. Those are follow-up WUs.

## Decisions

- Subclass item = equipped instance with bucket hash `3284755031`.
- Subclass plugs are classified by `plugCategoryIdentifier`, never by localized
  names. Pattern `<hunter|titan|warlock|shared>.<element>.<kind>` with kinds:
  `supers`, `aspects`/`totems` (Stasis), `fragments`/`trinkets` (Stasis),
  `grenades`/`prism_grenade`, `melee`, `class_abilities`, `movement`,
  `transcendence`. Also bare `shared.fragments`.
- No migration: grouping uses the plug kind; socket order is kept through the
  existing `item_plugs.ordinal`.
- "Empty ... Socket" plugs are reported as empty slots, not as options.

## Constraints and acceptance

- Subclass plugs survive sync (`retain_searchable_plugs`) and persist.
- Existing weapon/armor plug behavior unchanged.
- CLI and MCP outputs match (parity test).
- Test-first for classifier, sync retention, query, and output.

## Tasks

- [x] **SUB-001 — Classify and retain subclass plugs.** `SubclassPlugKind`, `classify_subclass_plug`, `PlugKind::Subclass`; RED→GREEN observed; workspace tests 1111 passed, clippy clean. Commit `83bb332`.
- [x] **SUB-002 — Subclass query.** `QueryService::character_subclasses` + `SubclassResult` types, reusing existing snapshot reads (no new repo method). RED (E0599)→GREEN; workspace 1114 passed, clippy clean. Commit `09ea038`.
- [x] **SUB-003 — CLI command.** `ghostlink subclass [--character <id|class>]`, grouped text + snake_case JSON; `QueryOperations::character_subclasses`. RED→GREEN; workspace 1120 passed, clippy clean; live smoke OK on 3 characters. Commit `2b68801`.
- [x] **SUB-004 — MCP tool and docs.** MCP tool `get_subclass`, exact CLI/MCP JSON parity tests, README. RED→GREEN; workspace 1125 passed, stdio (offline-fixtures) 11 passed, clippy clean. Commit `810c0f2`.

## Progress

- Branch `feature/subclass-config` created.

## Next step

WU-028 complete. Follow-ups: unlocked subclass options (plug sets), seasonal artifact, in-game loadouts.

Notes: Prismatic transcendent grenade (`prism_grenade`) is shown under Grenade; fragment sockets with no aspects equipped are disabled in 305 and therefore absent.
