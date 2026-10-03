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
- [x] **GBS-002 — Armor archetypes.** Classify `armor_archetypes`, keep on sync, archetype in armor hit output, `--archetype` (CLI) / `archetype` (MCP), README incl. power scale. Delegated writer, test-first (RED: classification regression). `PlugKind::ArmorArchetype` for `armor_archetypes`, sync retention, armor-only exact `archetype` criterion (Paragon shader excluded), first-socket archetype in hit output (CLI text/JSON, MCP), CLI `--archetype`, MCP `archetype`, parity test, README archetype table and Edge of Fate power scale. Checks (writer): fmt, build, workspace all-features 1,090 passed/26 suites, clippy both variants, diff check. Parent live check after forced refresh: `--slot helmet --class hunter --archetype Specialist` -> 5 helmets (Exodus Down Mask, Dark Age Mask, Iron Battalion Cowl, Mask of the Taken King, Wormhusk Crown) with archetype and stats; `--archetype Paragon` -> 7 armor pieces. Commit evidence: see git log.
- [x] **GBS-003 — Forgiving name matching.** Exact, partial, suggestion resolution shared by perk, mod, stat and archetype; typed no-match error with suggestions; matched names reported in output. Delegated writer, test-first (RED: resolver behavior tests). Shared resolution (exact -> substring, max 10 -> typed error with up to 3 bounded-Levenshtein suggestions) for perk, mod, archetype and stat names over stored categorized definitions; items match any resolved name; CLI `matched <kind>: ...` lines, JSON/MCP `resolved`; sanitized errors; `AppError` became Clone (not Copy) to carry details; README matching rules. Checks (writer): fmt, build, workspace all-features 1,094 passed/27 suites, clippy both variants, diff check. Parent live check: `--perk ramp` -> matched Rampage, Rampager's Handle, Solar Rampart; `--perk rampge` -> did you mean Rampage; `--mod siphon` and `--perk frame` -> too many matches with examples; `--archetype spec` -> Specialist; `--stat "Weap>=35"` -> Weapons. Note: debug-build searches take ~2-3.5 s (CPU-bound); a release build is a follow-up. Commit evidence: see git log.
- [ ] **GBS-004 — Alternative plugs.** Component 310 decode and storage, available perk/mod names in hits, opt-in flag to match available plugs.
- [ ] **GBS-005 — Verification and live validation.** Full checks, live searches, close tracker.

## Progress
Research 2026-10-03: Edge of Fate power cap 550, soft cap 440 (IGN, gamingpromax); Armor 3.0 archetypes are plug items in an armor socket (Bungie API issue #1981). Live DB: archetype definitions exist with category `armor_archetypes`; 0 stored on items because sync classifies them as Other.

## Next step
GBS-004.
