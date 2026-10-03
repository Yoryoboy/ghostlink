# Ghostlink Item Stats (WU-025)

## Objective and scope
Let `search-inventory` (CLI and MCP) filter owned items by instance stats (e.g. `--stat "Weapons>=20"`) and show item stats in hits, using profile component 304 (ItemStats) with stat names resolved from the Manifest `DestinyStatDefinition` catalog.

## Decisions
- Stat names come from `DestinyStatDefinition` (new Manifest catalog), never hard-coded, so renamed or new armor stats work after `sync-manifest`.
- `--stat` syntax: `<Name><op><N>` with `op` in `>=`, `<=`, `=`; repeatable (all must hold); name match is exact, case-insensitive, normalized. An unknown stat name is a clear input error, not an empty result.
- Instance stats are stored per item (hash, value), ordered by hash; stat changes advance the profile revision like plug changes.
- Store all stats Bungie reports for an instance (weapons and armor); output shows named stats only.
- Lesson from WU-023: a new `ItemInstance` content field is decoded, synced and persisted in the same commit, or revision equality breaks downstream journeys.

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Bounded decode (per-item stat cap, value range), owner-private handling like 300/302/305; missing per-instance entry is an empty list.
- Fake and SQLite repositories keep identical matching rules; CLI and MCP stay in step.
- Response size checked live against the 16 MiB profile limit.
- Checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features`, clippy with and without `--all-features` (`-D warnings`), `git diff --check`.
- Delivery: work-unit commits on `feature/ghostlink-item-stats`; push/PR/merge are user decisions.

## Tasks
- [x] **GIS-001 — Establish tracker and branch.** Branch `feature/ghostlink-item-stats` from `main` at `ee67ac7`.
- [x] **GIS-002 — Manifest stat definitions.** Seventh Manifest component `DestinyStatDefinition`, bounded decode, `stat_definitions` table (schema migration, forced republish), repository port, sync report count. Delegated writer, test-first (RED: decoder missing; stale stat-version test failed). Seventh Manifest component, bounded decode, reversible STRICT v8 `stat_definitions` with forced same-version republish, atomic publication, normalized-name repository lookups with fake parity, sync report counts (text/JSON). Checks (writer): fmt, workspace all-features 1,077 passed/25 suites, clippy both variants, diff check. Parent live check: `sync-manifest` -> `updated`, 66 stat definitions (13 skipped); names include Weapons, Health, Class, Grenade, Super, Melee. Live finding for GIS-004: names are not unique (`Class` and `Melee` each map to two hashes), so a `--stat` name must match any of its hashes. Commit evidence: see git log.
- [ ] **GIS-003 — Instance stats.** Request component 304, bounded decode, `ItemInstance.stats`, `item_stats` table (migration), sync and fakes, in one coherent commit.
- [ ] **GIS-004 — Stat query and surfaces.** `--stat` criterion with shared matching (fake/SQLite parity, before truncation), CLI `--stat` (repeatable), MCP field, named stats in hit output, README.
- [ ] **GIS-005 — Verification and live validation.** Full checks, live sync size, stat searches, close tracker.

## Progress
Exploration (read-only, WU-024 explorer): Manifest index requires six components (`destiny-adapters/src/manifest.rs:98-110`, tests at `:1367`, `:1556-1569`, `:2088`, `:2165`); v4 `objective_definitions`/`activity_type_definitions` are the pattern for a new catalog. Profile query is an exact-string constant (`profile_gateway.rs:361`, tests `:1880`, `:2236`, `:2357`, negatives `:2377-2401`); component order must become `...302,304,305`. Revision identity compares full `ItemInstance` equality (`ports.rs:~135`).

## Next step
GIS-003.
