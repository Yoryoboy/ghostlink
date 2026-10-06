# Clarity Community Insights — IN PROGRESS

## Objective

Add Clarity's community-researched perk data (numeric values, corrections) as
a second, clearly attributed layer next to the official Bungie `description`,
so agents can calculate builds instead of guessing numbers.

Source: <https://github.com/Database-Clarity/Live-Clarity-Database>
(`live` branch). Terms: <https://www.d2clarity.com/partnerships>.

## Decisions

- **Always on.** No config flag. The repo is private today, but the public
  release machinery (PR #50) is merged: the user commits to closing a Clarity
  partnership (Discord ticket) before publishing any release that includes
  Clarity data.
- **Synced inside `sync-manifest`.** No new command. Clarity is checked after
  the Manifest; data is downloaded only when `versions.json` changes. A
  Clarity failure never fails or rolls back the Manifest publication; it is
  reported in the sync output.
- **Source file:** `descriptions/vaultZero.json` (plain text with `[Stasis]`,
  `[enh]` tokens like Bungie's notation). `crayon.json` contains Discord emoji
  markup and is not used.
- **Verbatim storage.** Text stored byte-for-byte; no trimming or whitespace
  normalization. Invalid/oversized entries are dropped whole, never edited.
  Presentation-only adjustments (indentation) happen at render time.
- **Own storage.** Separate table and version record (schema v15); the
  generic `sync_metadata` dataset enums are not widened.
- **Output:** optional `clarity` string next to `description` on the five
  plug surfaces (subclass, subclass options, artifact, loadouts, inspect
  `plug_details`). No per-response version/date.
- **Matching:** exact hash first; then same name within the same kind (e.g.
  Prismatic aspect copies); a different variant (enhanced vs base) is shown
  but marked as a variant match.
- **Credits:** README Credits section with Clarity, https://d2clarity.com and
  the Discord https://d2clarity.com/discord, plus docs and skill rule to cite
  Clarity when using its numbers.

## Evidence

- Raw URLs return 200 `text/plain; charset=utf-8`:
  `.../live/versions.json` (27 B), `.../live/descriptions/vaultZero.json`
  (~868 KB, 1,765 entries, 1,756 with `en` text, max 1,311 chars).
- Hash variants: Prismatic Ascension 2835214901 vs Clarity 4194622039 (Arc);
  enhanced Unburied Treasure 3804163130 vs Clarity base 3132659494.

## Tasks (chained PRs)

- [x] 1. PR A — data: fixed-URL transport, decode, v15 storage, sync inside
      `sync-manifest` with isolated failure; README Credits.
- [ ] 2. PR B — exposure: `clarity` field on five surfaces with matching rule
      and variant marking; CLI JSON/human, MCP, parity tests.
- [ ] 3. PR C — agents: docs and skill rule to cite Clarity.
- [ ] 4. Live check after each PR on a copy of the real DB.

## Evidence log
- PR A — `clarity_insights` + singleton `clarity_metadata` (schema v15),
  fixed-URL transport (Bungie allowlist untouched), verbatim decode with exact
  raw version token, Manifest-first orchestration in explicit `sync-manifest`
  and CLI auto-sync; Clarity failure reported, never fatal. README Credits.
  Writer: compile-level RED (exit 101), GREEN; fmt 0, clippy 0, tests 0
  (1,425 passed, 0 failed).
  Live (copy of real v14 DB): migration to v15, `sync-manifest` exit 0,
  `clarity: updated`, version 2.0623, 1,756 stored / 9 skipped; second run
  `unchanged`. Stored rows vs fresh raw vaultZero.json: 1,756 expected,
  0 mismatches (text, name, kind), 0 extra. Auto-sync on an empty DB
  (`--json weekly-reset`) exit 0, Clarity populated, progress on stderr,
  stdout valid JSON. Not exercised live: network failure (covered by tests).
