# Ghostlink Armor Mods (WU-024)

## Objective and scope
Let `search-inventory` (CLI and MCP) find owned armor by socketed mod name (e.g. `--mod "Harmonic Siphon"`), restrict `--perk` to weapons (review advisory R3-armor-intrinsics), and fix `freshness: stale` reported on the first read after a refresh.

Out of scope (next work unit, WU-025): item stats (profile component 304, `DestinyStatDefinition`, `--stat`).

## Decisions
- Plugs are classified at query time from definitions as weapon perk, armor mod or other; no storage kind column. Sync keeps weapon perks, armor mods and unknown plugs.
- Armor mods: `enhancements.v2_*`, `enhancements.universal`, `enhancements.season_*`, `enhancements.raid_*`. Excluded: masterworks (`*.masterworks*`), ghost mods (`enhancements.ghosts_*`), tuning plugs (`core.gear_systems.armor_tiering.plugs.tuning.mods`) and cosmetics.
- `--perk` matches only items whose definition slot is Kinetic, Energy or Power; `--mod` only Helmet, Gauntlets, Chest, Legs or ClassItem. An unresolved item definition fails these filters.
- `--mod` matching is exact, case-insensitive, on the normalized mod name, like `--perk`.
- Freshness after a refresh is evaluated against a time not earlier than the refreshed `checked_at`; the future-timestamp guard in `FreshnessInput` stays.
- Existing databases pick up armor mods on the next profile sync; no migration needed.

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Fake and SQLite repositories keep identical matching rules; CLI and MCP stay in step.
- Checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features`, clippy with and without `--all-features` (`-D warnings`), `git diff --check`.
- Delivery: work-unit commits on `feature/ghostlink-armor-mods`; push/PR/merge are user decisions.

## Tasks
- [x] **GAM-001 — Establish tracker and branch.** Branch `feature/ghostlink-armor-mods` from `main` at `ed81435`.
- [ ] **GAM-002 — Plug kinds and slot-guarded matching.** Core `PlugKind` classification, sync retains armor mods, `--perk` weapon-slot guard (R3), `mod_name` criterion with fake/SQLite parity.
- [ ] **GAM-003 — Mod surfaces.** CLI `--mod`, MCP `mod_name`, mod names in hit output, README.
- [ ] **GAM-004 — Post-refresh freshness.** Regression test and fix for `stale` on the first read after a refresh.
- [ ] **GAM-005 — Verification and live validation.** Full checks, live sync and `--mod`/`--perk` searches, close tracker.

## Progress
Exploration (read-only): weapon-perk allowlist `destiny-core/src/domain.rs:204-207` (`intrinsics` also matches exotic armor intrinsics); sync filtering `destiny-app/src/sync.rs:~155-180`; perk matching `destiny-core/src/query.rs:378-391`, SQLite `sqlite/search.rs:175-212` (instance definition resolved only when definition filters are set). Freshness: CLI composition reads `now` once (`destiny-cli/src/runtime.rs:~363`) while the gateway stamps `checked_at` later (`profile_gateway.rs:1132,1159`); `FreshnessInput::status` (`destiny-core/src/state.rs:76`) returns `Stale` when `checked_at > now`, evaluated in `destiny-app/src/queries.rs:~392`.

## Next step
GAM-002.
