# Perk Descriptions — IN PROGRESS

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
- [ ] 3. Expose descriptions in item inspect/search via an additive
      structured plug list (CLI JSON/human, MCP, docs).
- [ ] 4. Full workspace verification (fmt, clippy, tests) and close.

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
