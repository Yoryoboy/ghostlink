# Ghostlink Build Search (WU-026)

## Objective and scope
Make `search-inventory` (CLI and MCP, scriptable via `--json`) useful for planning builds:
- armor archetypes (`--archetype Specialist`) shown and searchable;
- forgiving name matching for `--perk`, `--mod`, `--stat` and `--archetype` (exact, then partial, then a "did you mean" suggestion);
- alternative (selectable, not socketed) weapon perks and armor mods visible and searchable;
- documentation aligned with the Edge of Fate power scale (cap 550, soft cap 440).

## Decisions
- Archetypes are armor plugs with Manifest plug category `armor_archetypes` (Brawler, Bulwark, Grenadier, Gunner, Paragon, Specialist); classify by category, never by name (a shader is also named "Paragon"). Sync keeps them.
- Name resolution order: exact normalized name; otherwise unique-or-multiple partial (substring) matches are used and reported; otherwise no match returns a typed error with the closest suggestion(s). Never silently guess a different name; output states which name(s) were matched so scripts can verify.
- Alternatives come from profile component 310 (ItemReusablePlugs); stored separately from socketed plugs. Searches default to socketed; a flag opts into "socketed or available".
- Power: no code limit exists; README examples use the 10–550 scale.

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Fake and SQLite repositories keep identical matching; CLI and MCP stay in step.
- A new `ItemInstance` content field is decoded, synced and persisted in the same commit.
- Response size checked live against the 16 MiB profile limit.
- Checks: fmt, workspace all-features tests, clippy with and without `--all-features`, diff check.
- Delivery: work-unit commits on `feature/ghostlink-build-search`; push/PR/merge are user decisions.

## Tasks
- [x] **GBS-001 — Establish tracker and branch.** Branch `feature/ghostlink-build-search` from `main` at `381281f`.
- [ ] **GBS-002 — Armor archetypes.** Classify `armor_archetypes`, keep on sync, archetype in armor hit output, `--archetype` (CLI) / `archetype` (MCP), README incl. power scale.
- [ ] **GBS-003 — Forgiving name matching.** Exact, partial, suggestion resolution shared by perk, mod, stat and archetype; typed no-match error with suggestions; matched names reported in output.
- [ ] **GBS-004 — Alternative plugs.** Component 310 decode and storage, available perk/mod names in hits, opt-in flag to match available plugs.
- [ ] **GBS-005 — Verification and live validation.** Full checks, live searches, close tracker.

## Progress
Research 2026-10-03: Edge of Fate power cap 550, soft cap 440 (IGN, gamingpromax); Armor 3.0 archetypes are plug items in an armor socket (Bungie API issue #1981). Live DB: archetype definitions exist with category `armor_archetypes`; 0 stored on items because sync classifies them as Other.

## Next step
GBS-002.
