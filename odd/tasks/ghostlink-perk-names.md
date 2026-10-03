# Ghostlink Perk Names (WU-023)

## Objective and scope
Let `search-inventory` (CLI and MCP) find owned weapons by perk name (e.g. `--perk Rampage`) using the plugs actually socketed on each item instance (Bungie profile component 305), with names resolved from the synced `DestinyInventoryItemDefinition` catalog.

Out of scope: armor mods (next work unit, together with item stats), substring perk matching, perk rolls not socketed (component 310 reusable plugs).

## Decisions
- Plug filtering happens in the application layer during profile sync using definition plug categories; plugs with unknown definitions are kept and ignored by query-time classification.
- Weapon perks only: traits, barrels, magazines, intrinsics and similar weapon perk plugs. Cosmetic plugs (shaders, ornaments, mementos, trackers) are never stored, so cosmetic changes do not churn the profile revision. Armor mods come in a later work unit.
- Classification uses the Manifest `plug.plugCategoryIdentifier` (stored on definitions), not localized display strings.
- `--perk` matching is exact, case-insensitive, on the normalized perk name; `rampage` matches the Rampage trait and not "Rampage Spec".
- Plugs live in a new store alongside the existing component-302 perk summaries; `--perk-hash` keeps its current semantics.
- Builds on WU-022 (branch stacked on `feature/ghostlink-search-filters`, PR #20); rebase if that PR changes before merge.

## Constraints and acceptance
- Test-first for each behavior task (observe RED, then GREEN).
- Component 305 decode is bounded (per-item socket cap) and tolerates owner-private data like 300/302; a missing socket entry is an empty plug list.
- Fake and SQLite repositories keep identical matching rules; CLI and MCP stay in step.
- Response size measured live against the 16 MiB profile limit.
- Checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and without `--all-features`, `git diff --check`.
- Delivery: work-unit commits on `feature/ghostlink-perk-names`; push/PR/merge are user decisions.

## Tasks
- [x] **GPN-001 — Establish tracker and branch.** Branch `feature/ghostlink-perk-names` from `feature/ghostlink-search-filters` at `a575a3b`.
- [x] **GPN-002 — Manifest plug category.** Decode and store `plug.plugCategoryIdentifier` on definitions (schema migration, forced republish like v5), with a provider-neutral weapon-perk classification helper. Delegated writer (constructor-only sites added by parent), test-first (RED: three missing-field errors). Bounded optional `plug_category` decoded and normalized; provider-neutral exact allowlist classification (intrinsics, frames, barrels, magazines, magazines_gl, arrows, blades, guards, batteries, grips, scopes, stocks, tubes, hafts, bowstrings, origins); reversible v6 column; forced same-version republish. Checks (writer): fmt, workspace all-features 1,037 passed/25 suites, clippy both variants, diff check. Parent live check: `sync-manifest` -> `updated` (v6 republish); live categories confirm the allowlist (Rampage -> `frames`/Trait and Enhanced Trait; barrels/magazines/intrinsics/origins/scopes present; cosmetics use `shader`, `armor_skins_*`, `mementos`, `emote`). Note `intrinsics` also covers exotic armor intrinsics. Commit evidence: see git log.
- [x] **GPN-003 — Profile sockets.** Request component 305, decode bounded socket plugs per instance, add them to the domain and in-memory fakes; keep only weapon-perk plugs when classification is available.
- [x] **GPN-004 — Plug storage.** Merged into GPN-003 (single commit) after GPN-003 alone left SQLite reading empty plugs, which falsely advanced revisions and broke three mutation-planning journeys. Delegated writer, test-first (RED: socket validation, SQLite round-trip loss, missing `new_with_definitions`). Component 305 requested and decoded (owner-private with data accepted, per-item cap, missing entry -> empty); ordered `ItemInstance.plugs`; `ProfileSyncService` drops known non-weapon-perk plugs before publication using definition plug categories (unknown definitions kept); CLI and MCP composition pass the SQLite definition repository; reversible v7 `item_plugs` table with transactional writes, reads and corruption checks; cosmetic-only changes keep the revision. Checks (writer): fmt, workspace all-features 1,052 passed/25 suites (all seven mutation-planning journeys pass), clippy both variants, diff check. Commit evidence: see git log.
- [ ] **GPN-005 — Perk-name query and surfaces.** `perk_name` criterion with shared matching (fake/SQLite parity), CLI `--perk`, MCP `perk_name`, perk names in hit output, README.
- [ ] **GPN-006 — Verification and live validation.** Full checks, live sync size, `--perk` searches, close tracker.

## Progress
Exploration (read-only): profile query is an exact-string constant (`destiny-adapters/src/profile_gateway.rs:361`, test copies at `:1880`, `:2236`, `:2357`, negative list `:2377-2400`); `weekly_profile.rs` has its own query. Perk summaries decode at `profile.rs:~488`; `item_perks` table at `sqlite/migrations.rs:266`; schema at v5. Manifest keeps plug records (no itemType filter) but stores no plug category. `query.rs:39-41` doc says no textual perk search exists.

## Next step
GPN-005.
