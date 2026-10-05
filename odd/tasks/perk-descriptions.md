# Perk Descriptions — COMPLETE

## Objective

Persist the official Bungie `displayProperties.description` of inventory item
definitions and expose it next to every resolved plug name (perks, mods,
aspects, fragments, abilities, supers, artifact perks, exotic intrinsics) in
CLI and MCP output, so AI agents receive the in-game tooltip text.

This is piece 1 of 3 of the perk-knowledge initiative:

1. Official Bungie descriptions (this feature).
2. Clarity community data sync into a separate table (future).
3. Combined official + community exposure with provenance (future).

## Decisions

- **Separate description catalog.** Descriptions live in their own
  hash → text catalog (`item_descriptions`), published atomically with the
  Manifest. `ItemDefinition` stays unchanged to avoid churn across 42
  construction sites. Same "text by hash" pattern Clarity will reuse.
- **Additive output.** Existing `perk_names` / `mod_names` string arrays and
  all existing fields stay unchanged; descriptions are added as new fields or
  new structured lists. No existing JSON contract breaks.
- **Empty descriptions are omitted** (not stored, not serialized).
- **Multiline text is preserved.** Name normalization (whitespace collapse) is
  not applied to descriptions; control characters other than whitespace are
  rejected/dropped; bounded at 2048 characters, reusing the existing policy in
  `destiny-core/src/world_definitions.rs`.
- **Existing caches are backfilled** by invalidating Manifest sync metadata in
  the migration (precedent: v11), so the next sync republishes.

## Constraints

- `destiny-core` stays provider-, transport- and serialization-free.
- No raw provider payloads cross into core or output.
- Offline, credential-free automated checks.
- Technical artifacts in English.

## Tasks

- [x] 1. Description catalog: adapter decode, core port, shared text
      validation, SQLite migration v14 (table + metadata invalidation),
      atomic publication with the Manifest, lookup by hash.
- [x] 2. Expose descriptions in subclass, subclass options, artifact and
      loadout plug outputs (CLI JSON/human, MCP, docs).
- [x] 3. Expose descriptions in item inspect via an additive structured
      plug list (CLI JSON/human, MCP, docs). Search stays description-free
      to keep hit lists compact; agents inspect an item for detail.
- [x] 4. Full workspace verification (fmt, clippy, tests) and close.
- [x] 5. Sandbox perk fallback: when an item description is empty, compose it
      from its displayable `DestinySandboxPerkDefinition` descriptions
      (aspects, fragments, artifact perks, mods). Also remove the duplicated
      plug name in human `subclass` output.

## Evidence

- Task 1 — `c9bbd3e` feat(manifest): persist official item descriptions
  catalog. Port `WorldDefinitionRepository::lookup_item_description`.
  Worker ran fmt, clippy `-D warnings`, full workspace tests: all pass. Parent
  moved a misplaced test that had captured the `decode_inventory_items` doc
  comment; focused adapter test and clippy re-run: pass.
- Task 2 — optional `description` on subclass slots, subclass options,
  artifact slots and loadout plugs (CLI JSON/human, MCP, docs). Independent
  verifier with unmasked exit codes: fmt 0, clippy `-D warnings` 0, workspace
  tests 0 (1,353 passed, 0 failed, 0 ignored).
- Task 3 — inspect-only `plug_details: [{hash, name, role, description?}]`,
  roles `perk`, `mod`, `available_perk`, `available_mod`, `archetype`. No
  `intrinsic` role: existing inspect fields exclude exotic intrinsics. Writer
  observed RED (exit 101, six intended failures) then GREEN.
- Task 4 — final tree (Tasks 1–3): fmt 0, clippy `-D warnings` 0, workspace
  tests 0 with 1,358 passed, 0 failed across 38 result lines (writer run with
  unmasked exit codes, logs under /tmp/perk-inspect-*.log; parent spot-checked
  the green log).

- Live check (copy of real DB in /tmp/gl-live, real installed DB untouched):
  v13→v14 migration and `sync-manifest` exit 0; 20,077/20,077 item
  descriptions stored, byte-identical to raw Manifest, zero drops, max 658
  chars. `subclass`, `subclass-options`, `artifact`, `loadouts`,
  `inspect-item` exit 0. Gap found: aspects, fragments, artifact perks and
  mods (~824 plugs) have empty item descriptions; their effect text lives in
  `DestinySandboxPerkDefinition` via `perks[].perkHash` → Task 5.

- Task 5 — sandbox perk fallback composed in app normalization (own text
  wins; fallback keeps `perks[]` order, displayable non-blank only, dedupe,
  joined with blank line, same validation). Sandbox component is required:
  its failure fails sync before publication. No v15 (v14 unreleased, already
  forces backfill). Human renderers no longer repeat single-plug names;
  multi-plug rows show per-plug blocks. Writer: fmt 0, clippy 0, tests 0
  (1,362 passed, 0 failed).
  Live re-check on a fresh copy of the real v13 DB: migration + sync exit 0;
  20,931 descriptions = 20,077 own (0 mismatches) + 854 composed (0
  mismatches vs an independent Python oracle over the raw Manifest). Hunter
  coverage: subclass 14/14, artifact 8/8, subclass-options 48/48,
  loadouts 245/245 named plugs with descriptions.

## Follow-ups

- Exotic intrinsic descriptions in inspect (needs inspect to surface intrinsics).
- Piece 2: Clarity community data sync into a separate table.
