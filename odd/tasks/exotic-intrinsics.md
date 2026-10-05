# Exotic Intrinsics in Inspect — COMPLETE

## Objective

Expose intrinsic plugs (plug category `intrinsics`) in item inspection
`plug_details` with role `intrinsic` and their official description, so exotic
armor perks (e.g. Orpheus Rig → Uncanny Arrows) become visible to agents.

## Evidence (live DB, schema v14)

- Exotic weapons already surface their intrinsic as role `perk` (Ace of Spades
  → Memento Mori, ordinal 0, category `intrinsics`).
- Exotic armor does not: Orpheus Rig has Uncanny Arrows at ordinal 8, category
  `intrinsics`, with a stored description, but inspect returns empty
  `perk_names` and no `plug_details`.

## Decisions

- Role `intrinsic` for every plug with category `intrinsics`, weapons and
  armor alike, in `plug_details` only.
- `perk_names`, `mod_names`, search and perk matching stay unchanged.
- Human inspect output shows the intrinsic with its description.

## Tasks

- [x] 1. Add `intrinsic` role to inspection plug details (core, app, CLI,
      MCP, docs, tests).
- [x] 2. Live check against a copy of the real DB, then close.

## Evidence log

- Task 1 — role `intrinsic` (exact category `intrinsics`), ordered first,
  replaces the weapon `perk` entry for the same plug; legacy fields, search
  and matching unchanged. Writer: RED exit 101, GREEN; fmt 0, clippy 0,
  tests 0 (1,365 passed, 0 failed).
- Task 2 — live check on a copy of the real DB: Orpheus Rig → intrinsic
  Uncanny Arrows with description; Ace of Spades → intrinsic Memento Mori
  first, no duplicate perk entry, `perk_names` unchanged. All 139 owned exotic
  instances inspect with exit 0; 136 show an intrinsic with description. The
  3 without (Aeon Safe/Soul/Swift) carry intrinsic plug 951340179, which has
  a blank name and description in Bungie's Manifest (data limitation).
