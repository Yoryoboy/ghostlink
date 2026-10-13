# Ghostlink Build Engine (Phase 4) — IN PROGRESS

Roadmap: `Destiny AI Companion — Objetivo, alcance y arquitectura.md` §17 and §23
(build requirements, inventory matching, missing items, substitutions,
coverage, roll matching, prepare/execute build plan).

## Working rule (owner, 2026-10-11)

DIM first, with autonomy: for any blocker or doubt, check how Destiny Item
Manager solves it; if clear and it fits Ghostlink, apply it and record the
DIM evidence here. Ask the owner only when DIM is ambiguous or does not fit
(CLI process per command, SQLite projection, MCP for agents).

- DIM clone: `/tmp/dim-research-3/DIM` (commit `29c122e`, 2026-10-10).
- DIM API types: `/tmp/dim-research-3/dim-api/api/shapes/loadouts.ts`.
- Re-clone with `git clone --depth 1` if missing.

## DIM model (research 2026-10-11)

- A build is a DIM `Loadout`: `id`, `name`, `notes`, `classType`,
  `equipped[]`/`unequipped[]` of `LoadoutItem {id?, hash, amount?,
  socketOverrides?, craftedDate?}`, optional `LoadoutParameters`
  (`statConstraints` with `minStat/maxStat` and legacy tiers ×10, `mods`,
  `modsByBucket`, `perks`, `setBonuses`, `exoticArmorHash`, `query`,
  `assumeArmorMasterwork`, `autoStatMods`, `artifactUnlocks`, clear flags,
  `inGameIdentifiers`). Evidence: `dim-api/api/shapes/loadouts.ts:3-254`.
- Sources: DIM saved loadouts, equipped snapshot
  (`DIM src/app/loadout-drawer/loadout-utils.ts:178`), Bungie in-game
  loadouts converted to the same model
  (`src/app/loadout/loadout-type-converters.ts:120`), imports of DIM JSON
  `/loadouts?loadout=<JSON>` and `dim.gg` share IDs
  (`src/app/loadout/loadout-share/loadout-import.ts:14-125`). No third-party
  build-site adapters; those sites hand out DIM links.
- Resolution: instanced items by instance id, then crafted date; subclasses
  and some buckets by hash; missing items become placeholders with the
  original spec kept (`loadout-utils.ts:554-737`,
  `loadout-item-conversion.ts:27-99`). No silent same-hash substitution.
- Analysis: DIM Loadout Analyzer findings (missing items, invalid mods,
  fragment slots, incomplete armor, exotic mismatch, mods do not fit, stat
  constraints unmet, armor upgrades needed, seasonal mods, invalid search,
  better stats available) (`src/app/loadout-analyzer/types.ts:40-88`). No
  percentage "coverage"; Ghostlink reports counts derived from findings.
- Substitutes: whole-set Loadout Optimizer over owned armor
  (`src/app/loadout-builder/item-filter.ts`, `process-worker/types.ts`).
- Apply: DIM `loadout-apply.ts:355-660` phases (dequip with replacements,
  transfers, bulk equip, socket overrides, mods, optional clearing).

## Decisions

- D1 (DIM): the build definition model is the DIM `Loadout` API shape, so
  DIM JSON imports round-trip. Unknown parameter fields are preserved.
- D2 (DIM): sources in order: import of DIM JSON/URL (offline), equipped
  snapshot, in-game loadout conversion. Stored locally in SQLite (Ghostlink
  equivalent of DIM Sync).
- D3 (DIM): resolution never substitutes silently; substitutes come only from
  the optimizer task.
- D4 (owner, 2026-10-12): no `dim.gg` support. Share import would need a
  DIM API key (`X-API-Key`, `src/app/dim-api/dim-api-helper.ts:9,35`) and a
  third-party network dependency. Ghostlink imports only full DIM links and
  JSON; `dim.gg` share ids keep the typed error, which should tell the user
  to open the link in DIM and copy the full loadout link or JSON.

## Constraints

- Test-first for behavior (RED, GREEN, refactor).
- Fake and SQLite repositories keep parity; CLI and MCP stay in step.
- New persisted content is decoded, synced and stored in the same commit.
- Gates: `cargo test --workspace --all-features`, fmt, clippy
  `-D warnings` (with and without `--all-features`), `git diff --check`.
- One work-unit commit per task on `feature/ghostlink-build-engine`;
  push/PR/merge are owner decisions.

## Tasks

- [x] **BE-001 — Tracker and branch.** Worktree `../ghostlink-builds`, branch
  `feature/ghostlink-build-engine` from `origin/main` `a86584e`.
- [x] **BE-002 — Core build model.** DIM `Loadout` types in `destiny-core`
  with serde round-trip; parse DIM API JSON and `/loadouts?loadout=` URLs;
  DIM import normalization (fresh id, legacy tiers ×10).
  Done: `destiny-core/src/build.rs` (dependency-free) and
  `destiny-adapters/src/dim_loadout.rs` (parse keeps identity, import
  assigns a fresh UUID; `dim.gg` share ids return a typed online-import
  error). DIM evidence: fresh id and separate clamping
  `loadout-import.ts:92-125`; modern values win, tiers ×10
  `loadout-params.ts:20-22`; caps 10 tiers / 200 stat
  `known-values.ts:38-39`; no-exotic/any-exotic sentinels
  `loadout-builder/types.ts:157-161`; `clearSpace` migration and
  `amount || 1` `loadout-type-converters.ts:69-118`. Workspace:
  2049 passed all-features, 2019 default, 0 failed; clippy both modes,
  fmt, diff check clean.
- [x] **BE-003a — Build store and sources.** SQLite migration and repository
  (fake parity); create from import, equipped snapshot and in-game loadout;
  list/show/delete/export application use cases.
  Done: migration 23 `builds` (membership-scoped, DIM-shaped JSON plus
  indexed id/name/class/source; survives profile replacement; down step
  refuses to drop stored builds), `BuildRepository` + `BuildCodec` ports,
  SQLite and fake parity, `destiny-app/src/builds.rs`. DIM evidence:
  equipped snapshot buckets/classType/armor mods `loadout-utils.ts:69,178-218`;
  UUID ids `loadout-utils.ts:118-120`; in-game conversion skips unresolved
  instances `loadout-type-converters.ts:186`, subclass/artifact overrides and
  in-game identifiers `:213-238`. Reported omissions (no invented data):
  equipped socket overrides (no persisted socket indices), artifact unlocks,
  crafted dates → BE-005. Workspace: 2055 passed all-features, 2025 default,
  0 failed; clippy both modes, fmt, diff check clean.
- [x] **BE-003b — Build CLI and MCP.** CLI
  `build import|list|show|delete|export|from-equipped|from-loadout`; MCP
  tools; docs (`docs/cli.md`, `docs/mcp.md`), skill.
  Done: nested `ghostlink build <sub>` (matches `agents`/`handler` groups);
  MCP `list_builds`, `get_build` (with DIM export), `save_build`
  (`source` import|equipped|in_game_loadout), `delete_build`; shared output
  in `destiny-output/src/builds.rs`; no new exit codes. DIM evidence: URL
  forms `loadout-import.ts:57-88`. Known gap: unresolved in-game items are
  reported at creation but not persisted (`build show` shows null) → BE-004
  resolves against current inventory. Workspace: 2064 passed all-features,
  2032 default, 0 failed; clippy both modes, fmt, diff check, offline
  `--help` smoke clean.

Review follow-ups (non-blocking): bound per-bucket mod list length
(`build.rs:190`); tighten `dim.gg` share-id detection heuristic
(`dim_loadout.rs:333`); equipped snapshot captures disabled plugs as mods
(`destiny-app/src/builds.rs:176-185`); missing plug definitions skipped
silently (`builds.rs:398-401`).
- [x] **BE-004a — Transferability data.** DIM resolves hash-based items
  (subclass, artifact, emblems) with a target-held copy, else the first
  candidate only if not `notransfer` (`loadout-utils.ts:720-737`);
  `notransfer = nonTransferrable || transferStatus === NotTransferrable`
  (`store/d2-item-factory.ts:520-522`); `NotTransferrable = 2`
  (`bungie-api-ts` 5.10.0 `destiny2/interfaces.d.ts:8995`). Done: definition
  `non_transferrable` and instance `transfer_status` decoded from data
  already acquired (no new Bungie components), synced and persisted
  (migration 24, guarded downgrade), fake/SQLite parity, revision tests.
  Commit `c231a0d` (verified alone: 2055 all-features / 2023 default, 0
  failed); review approved and burned.
- [x] **BE-004b — Resolution and analysis v1.** DIM resolution semantics;
  findings supported by current data (missing items, exotic mismatch,
  invalid/nonmatching search, location of each item); CLI `build check`,
  MCP `analyze_build`.
  Done: `destiny-core/src/build_analysis.rs` (pure resolution and findings),
  `BuildService::check` with ordinary freshness, target character selector
  (unique character of the build class by default, else required). DIM
  evidence: resolution `loadout-utils.ts:554-737`; incomplete armor
  `analysis.ts:138-155`; exotic by definition name `analysis.ts:402-419`;
  `ItemsDoNotMatchSearchQuery` implemented search-only with
  `evaluation: "search_only"` (optimizer suppression `analysis.ts:293-307,
  362-367` → BE-008). Crafted-date fallback, stats, mods, upgrades and
  fragments reported `not_evaluated` with reasons. `build export` keeps
  printing raw DIM JSON (documented). Workspace: 2076 passed all-features,
  2044 default, 0 failed; clippy both modes, fmt, diff check, offline help.

Review follow-ups (BE-004a, non-blocking): legacy search read path and
`transfer_status` (`sqlite/search.rs:206`); add direct `notransfer`
coverage (`domain.rs:123-126`).
- [x] **BE-005 — Armor 3.0 data.** Base stats, set bonuses, energy, tuning,
  masterwork, socket indices for inventory plugs (DIM sources first).
  DIM sources (research 2026-10-11, paths under DIM `src/app/`):
  base stats = definition `investmentStats` + enabled intrinsic/ArmorStats
  plugs, scaled by stat group (`inventory/store/stats.ts:134,224,318-390`),
  never component-304 totals relabelled; energy = component 300 `energy`
  (`d2-item-factory.ts:544`); Armor 3.0 masterwork tier = masterwork plug
  conditional investment stat (`masterwork.ts:43-104`), not energy; gear tier
  = 300 `gearTier` (`d2-item-factory.ts:586`), not rarity; tuning socket
  category + plug set 1155052024 (`utils/item-utils.ts:409-432`,
  `loadout-builder/types.ts:136`); sets = `equippingBlock.equipableItemSetHash`
  + `DestinyEquipableItemSetDefinition` (`item-popup/SetBonus.tsx:23-64`,
  `utils/socket-utils.ts:782-809`); socket index = component 305 array
  position joined to definition `socketEntries[i]` (`inventory/store/sockets.ts:117-156`);
  fragment capacity = aspects' `plug.energyCapacity.capacityValue`
  (`loadout-drawer/loadout-utils.ts:745-769,946-954`); crafted date =
  component 309 crafting objective (`inventory/store/crafted.ts:23-90`).
  - [x] **BE-005a — Instance and socket fidelity.** Component 300 energy
    and `gearTier`; component 305 real socket indices with enabled/visible
    flags (today `decode_plugs` drops entries and SQLite `ordinal` is
    filtered order). Decoder → core → SQLite → fake parity.
    Done: migration 25 (`item_energy`, `item_gear_tiers`, `item_sockets`,
    STRICT, cascading, downgrade refuses populated data); absent tier stays
    `None` (DIM `?? 0` applied by consumers); raw sockets kept beside the
    unchanged publication-filtered plug view; all fields in revision
    equality. Legacy rows stay unknown until the next sync (no backfill).
    DIM evidence `d2-item-factory.ts:544,586`, `sockets.ts:133-141`.
    Workspace: 2082 passed all-features, 2050 default, 0 failed; clippy both
    modes, fmt, diff check, pattern-insertion check clean.
  - [x] **BE-005b1 — Manifest item armor fields.** Item `investmentStats`
    (with conditional flag), `stats.statGroupHash`,
    `equippingBlock.equipableItemSetHash`, numeric `plug.plugCategoryHash`,
    `plug.energyCost`, aspect `plug.energyCapacity.capacityValue`.
    Done: `destiny-core/src/armor_definition.rs`,
    `sqlite/armor_definitions.rs`, migration 26 (STRICT, cascading, guarded
    downgrade); the migration invalidates Manifest freshness so the next run
    re-downloads the same version once (existing rows lack the new fields).
    DIM evidence `inventory/store/stats.ts:140,230,342-390`,
    `masterwork.ts:98-104`, `d2-item-factory.ts:563,817-818`,
    `loadout-drawer/loadout-utils.ts:946-954`. Workspace: 2091 passed
    all-features, 2059 default, 0 failed; clippy both modes, fmt, diff check,
    pattern-insertion check clean.
  - [x] **BE-005b2 — Manifest item sockets.** Item socket entries (real
    index, socket type, initial plug, reusable/randomized plug sets, inline
    plug items, plug sources, default visibility) and socket categories.
    Done: `sqlite/definition_sockets.rs`, migration 27 (STRICT, cascading,
    guarded downgrade, once-per-upgrade Manifest refresh). Review follow-up
    from BE-005b1 fixed: single-hash reads no longer use
    `(?1 IS NULL OR hash = ?1)`; cached single-hash and all-rows statements,
    `EXPLAIN QUERY PLAN` test asserts indexed SEARCH on all seven tables.
    DIM evidence `inventory/store/sockets.ts:133-156,596,658-686,728-754`,
    `search/items/search-filters/perks-set.ts:54`;
    `preventInitializationOnVendorPurchase` is unused by DIM and omitted.
    Workspace: 2098 passed all-features, 2066 default, 0 failed; clippy both
    modes, fmt, diff check clean.
  - [x] **BE-005b3 — Stat group and armor set catalogs.**
    DestinyStatGroupDefinition and DestinyEquipableItemSetDefinition.
    Done: `destiny-core/src/armor_catalog.rs`, `sqlite/armor_catalogs.rs`,
    migration 28 (STRICT, guarded downgrade, once-per-upgrade refresh,
    indexed single-hash reads); 2 extra Manifest requests, each capped at
    16 MiB decompressed (live size not measured). DIM evidence
    `inventory/store/stats.ts:249,282,291,614-641` (top-level
    `maximumValue` fallback; absence kept unknown),
    `item-popup/SetBonus.tsx:55,102,213`. Workspace: 2104 passed
    all-features, 2072 default, 0 failed; clippy both modes, fmt, diff check.
  - [x] **BE-005b4 — Socket and plug set catalogs.**
    DestinySocketTypeDefinition (whitelist `sockets.ts:607-612`),
    DestinySocketCategoryDefinition (categoryStyle `sockets.ts:664-681`),
    DestinyPlugSetDefinition (currentlyCanRoll `sockets.ts:734,768`).
    Table names confirmed in DIM `destiny2/d2-definitions.ts:56-95,121-149`.
    Done: `sqlite/socket_catalogs.rs`, migration 29 (STRICT, guarded
    downgrade, once-per-upgrade refresh, indexed reads); 3 extra Manifest
    requests (world tables 9 → 12). Bounds set from the live Manifest
    244213.26.06.29.2000-1-bnet.65864 measured 2026-10-11 (plug sets 5,422
    records / 10.4 MB / max 824 items; socket types 1,874 / 1.3 MB / max
    whitelist 19; stat groups 112; categories 65; sets 56): per-kind record
    caps plug sets 50,000, socket types 20,000, others 10,000; plug sets 64 MiB,
    others 16 MiB. DIM evidence `sockets.ts:601,607-612,664-681,734,765,768,802`,
    `utils/socket-utils.ts:640-642`. Workspace: 2112 passed all-features,
    2080 default, 0 failed; clippy both modes, fmt, diff check.
  - [x] **BE-005c — DIM armor model.** Base/masterwork/tuning/set-bonus
    derivations with fixtures. Split: **BE-005c1** stats (investment +
    intrinsic plugs, interpolation, conditional stats, total base),
    Armor 3.0 masterwork tier and +5 rule, gear tier, energy, additive
    `inspect-item` exposure; **BE-005c2** tuning stat, set bonuses, fragment
    capacity.
    - [x] **BE-005c1 done** (resumed 2026-10-12 after the provider-side
      subagent outage). The partial was audited against DIM: kept
      investment + enabled intrinsic-first stats, conditional merging,
      armor interpolation bypass, gear tier `?? 0` (raw unknown kept),
      component-300 energy; fixed masterwork eligibility (requires nonempty
      active projected plug stats, `masterwork.ts:63-74,98-104`,
      `stats.ts:489-514`), no silent Armor 2.0 assumption for unresolved
      plugs (`item-utils.ts:318-325`, `stats.ts:405-414`), CLI text
      exposure; replaced the wrong negative-interpolation expectation
      (`stats.ts:626-640`). Additive `inspect-item` CLI text/JSON and MCP;
      read-only `ArmorCatalogRepository` forwarding in CLI runtime and MCP.
      Audit in `docs/armor-model.md`. 360 new production lines. Workspace:
      2122 passed all-features, 2090 default, 0 failed; clippy both modes,
      fmt, diff check. Follow-up: split large inspection assembly/rendering
      files (app queries, CLI output, MCP service).
    - [x] **BE-005c2 done (narrowed).** Visible tuning socket + current
      tuning plug (`socket-utils.ts:677-686`, no enabled gate); tuned stat
      deliberately unknown: DIM scans raw instance `reusablePlugItems`
      (`item-utils.ts:409-432`, `sockets.ts:77-79,133-140,784`) but
      Ghostlink drops non-insertable/disabled plugs at decode
      (`profile.rs:476-529`, `domain.rs:161`), so neither available plugs
      nor Manifest plug sets are used to infer it. Per-item set identity,
      set perk hashes and `requiredSetCount` thresholds
      (`d2-item-factory.ts:817-818`, `SetBonus.tsx:55-64`), no names/icons.
      Fragment capacity = sum of inserted aspects' capacity
      (`socket-utils.ts:278-283`, `loadout-utils.ts:940-954`). Additive
      inspect-item CLI/JSON/MCP. Workspace: 2126 all-features, 2094
      default, 0 failed; clippy both modes, fmt, diff check.
    - [x] **BE-005c3 — Raw reusable plug fidelity.** Done: migration 30
      (STRICT observed-membership markers + ordered raw plug children,
      cascading, guarded downgrade, nullable set-perk name/icon columns from
      the already-fetched sandbox table, once-per-upgrade refresh, no
      inventory backfill: legacy membership stays unknown until resync).
      Exact tuned stat with all 30 tuner mappings and conflict -> unknown
      (`item-utils.ts:409-432`, `d2-known-values.ts:445-476`); set perk
      names/icons without isDisplayable gate (`SetBonus.tsx:55-57,108-109,
      175-184`). Raw lists keep oversized sockets (DIM retains them);
      defensive 65,536-entry ceiling rejects, never truncates. Selectable
      plug semantics unchanged. Workspace: 2132 all-features, 2100 default,
      0 failed; clippy both modes, fmt, diff check.
      Original scope: Preserve raw
      per-socket instance reusable plug membership (component 305, as DIM
      `sockets.ts:77-79,133-140,784`) through decode, sync and SQLite
      alongside the existing selectable plugs; derive the exact tuned stat
      (`item-utils.ts:409-432`); project `DestinySandboxPerkDefinition`
      name/icon for set perks.
  - [x] **BE-005d — Build capture completion.** Equipped subclass/artifact
    socket overrides, unlocked plug evidence (102/206), component 309 crafted
    dates and resolution fallback.
    - [x] **BE-005d1 — Crafted dates and DIM mod capture.** Component 309
      added to the profile request; migration 31 (STRICT, cascading
      objective-style and raw socket-objective tables, observed-empty
      markers, guarded downgrade, once-per-upgrade refresh, no backfill).
      Capture stores `craftedDate` from complete crafting objectives
      (`crafted.ts:23-90`, `loadout-utils.ts:524-534`); resolution tries
      instance id, then nonzero crafted date, last match, cross-hash
      (`loadout-utils.ts:694-710`); missing evidence stays unknown. Disabled
      inserted armor mods are retained like DIM (`loadout-utils.ts:538-546`,
      `sockets.ts:501-512`), so the `builds.rs:176-185` follow-up resolves
      as "matches DIM"; numeric categories replace name-based mod
      classification (`mod-utils.ts:53-65`, `known-values.ts:26-31`);
      fashion uses ArmorCosmetics indices (`loadout-utils.ts:204-218`).
      Workspace: 2140 all-features, 2108 default, 0 failed; clippy both
      modes, fmt, diff check.
    - [x] **BE-005d2 — Socket overrides.** Equipped subclass/artifact
      socket overrides (modern artifact `socketOverrides`, not legacy
      `artifactUnlocks`, `loadout-drawer-reducer.ts:119-122`), DIM
      empty/default rules and fragment gating, plug-membership validation
      (102/206), in-game override normalization/category conversion
      (`loadout-type-converters.ts:213-238`).
      Done: `build_capture.rs` socket rules; equipped capture and
      positional fragment gating (`loadout-utils.ts:136-174,178-190`);
      empty/default precedence (`sockets.ts:584-645`); membership ignores
      unlock/enabled flags like DIM (`socket-utils.ts:225-237`,
      `sockets.ts:683-687,792-825`), so the existing 102/206 requests need
      no new projection; in-game category conversion keeps empties
      (`loadout-type-converters.ts:190-238,249-260`). Fixed review advisory
      R3: incomplete crafting tuples stay unknown (`crafted.ts:65-90`).
      Additive `equipped_socket_overrides` in show/MCP. No migration.
      Workspace: 2145 all-features, 2113 default, 0 failed; clippy both
      modes, fmt, diff check. Follow-ups: capture diagnostics not
      persisted (`source_capture_evidence_not_persisted`); socket
      apply-readiness stays `not_evaluated` until BE-007.
    Review follow-up (BE-005c3, non-blocking): unreferenced display
    validation `destiny-app/src/manifest.rs:189`.
- [x] **BE-006 — Analysis v2.** Stat constraints, mods fit, armor upgrades,
  equipped set-bonus activation (`SetBonus.tsx:23-64`, event selectors
  `socket-utils.ts:793-809`).
  - [x] **BE-006a — Equipped set activation.** `build_sets.rs`: per-set
    counts and threshold activation (`SetBonus.tsx:23-64`), native-set
    precedence and first visible enabled event selector
    (`socket-utils.ts:782-809`), 19 selector mappings
    (`loadout/known-values.ts:51-71`); partial counts are lower bounds,
    unknown stays null. Additive `equipped_set_bonuses` in `build check`
    text/JSON and MCP. Review advisory R3-fragment-position: not a
    deviation (`socket-utils.ts:77-85`, `loadout-utils.ts:136-174`),
    regression test added. Workspace: 2149 all-features, 2117 default,
    0 failed; clippy both modes, fmt, diff check.
  - [x] **BE-006b — Mods fit and armor upgrades.** DIM mod assignment
    (`loadout/mod-assignment-utils.ts`, >1,000 lines with dependencies);
    unassignable mods; upgrades needed for mods.
    - [x] **BE-006b1 — Bucket-specific assignment.** `mod_assignment.rs`
      (pure): categorization and full-set fitting
      (`mod-assignment-utils.ts:84-151,346-370`, `analysis.ts:442-478`),
      numeric bucket matching (`mod-utils.ts:15-21`), stable greedy
      picky-first ordering, visibility/type gates, energy and exclusions
      (`mod-assignment-utils.ts:576-675`), membership without flag
      filtering (`socket-utils.ts:225-237`), energy-upgrade assumptions
      (`analysis.ts:158-190`, `armor-upgrade-utils.ts:12-22,43-49`), Tier-5
      max (`d2-known-values.ts:29-30`). Additive `mod_fit` in build check
      text/JSON and MCP; `docs/build-mods.md`. Workspace: 2155
      all-features, 2123 default, 0 failed; clippy both modes, fmt.
    - [x] **BE-006b2a — Mod resolution.** Seasonal reduced-cost
      variants by availability (`mod-utils.ts:120-132`,
      `reduced-cost-mod-mappings.ts:1-117`), all 12 legacy migrations and
      deprecated placeholders before fitting (`loadout-utils.ts:880-936`),
      target-scoped unlocks (`selectors.ts:333-346`,
      `plugset-helpers.ts:20-40`), eager rejection
      (`mod-assignment-utils.ts:99-145`). Saved/exported builds keep their
      hashes. Advisory R3-full-set-membership: not a deviation
      (`analysis.ts:454` checks five entries), regression added.
      Workspace: 2162 all-features, 2130 default, 0 failed; clippy both
      modes, fmt.
    - [x] **BE-006b2b1 — Material prerequisites.** Migration 32 (three
      STRICT tables, bounded integers, cascading children, guarded
      downgrade, one same-version Manifest refresh): plug
      `insertionMaterialRequirementHash`,
      DestinyMaterialRequirementSetDefinition (ordered itemHash/count;
      `mod-assignment-utils.ts:162-265,964-989` ranks by fixed item
      hashes, no extra catalog), capacity `energyType` and
      `plugAvailability` (`energy.ts:18-49`). Live 244213.26.06.29.2000-1:
      116,945 bytes, 332 sets, max 4 materials; caps 16 MiB, 10,000
      records, 256 materials/set. World requests 12 -> 13. Advisory
      R3-category-coverage: no deviation (`socket-utils.ts:278-290`,
      `generated-enums.ts:1604-1619`), regression for all 16 pairs.
      Workspace: 2172 all-features, 2140 default, 0 failed; clippy both
      modes, fmt.
    - [x] **BE-006b2b2 — Permutations, specialty, artifice/tuning.**
      `mod_assignment/independent.rs` (pure): Heap order + hash dedup
      (`mod-permutations.ts:3-64`), specialty pairs and visible/plugged
      first match (`specialty-modslots.ts:43-150`,
      `item-utils.ts:69-100`), artifice preference and single-use
      candidates (`mod-assignment-utils.ts:359-381`), legacy exotic
      assumption vs Armor 3 (`armor-upgrade-utils.ts:29-36`,
      `item-utils.ts:298-325`), canonical tuning order and exotic-count
      rule (`mod-assignment-utils.ts:383-420`), shared 30-hash tuning map.
      Free mods now evaluated in build check/MCP. Workspace: 2179
      all-features, 2147 default, 0 failed; clippy both modes, fmt.
    - [x] **BE-006b2b3 — Ranking and re-fitting.** General/activity
      candidate evaluation, cumulative material costs, energy-upgrade
      plug input assembly in `build_mods.rs`, lexicographic ranking,
      assignment-change tie-breaking, expensive-variant re-fitting.
      Done (partial from the crashed worker audited and kept, tests
      strengthened): `mod_assignment/costs.rs` upgrade-plug selection and
      cumulative costs (`energy.ts:18-49`, `sockets.ts:683-687,792-825`,
      `mod-assignment-utils.ts:162-249`); `mod_assignment/ranking.rs`
      activity-first evaluation, fit/cost/change priority, exact-tie Heap
      order (`mod-assignment-utils.ts:437-528,952-989,1007-1036`);
      seasonal re-fitting and findings (`analysis.ts:449-501`). Mutation
      checks confirmed the tests catch removals. Workspace: 2188
      all-features, 2156 default, 0 failed; clippy both modes, fmt.
  - [x] **BE-006c — Stat constraints and fragment findings.** Stat
    aggregation vs `statConstraints`; fragment capacity findings;
    optimizer-based findings stay suppressed until BE-008.
    - [x] **BE-006c1 — Fragment findings.** First resolved subclass
      without equip gate (`analysis.ts:90-98`), fragment comparison with
      truthy override count (`analysis.ts:420-438`), first aspect
      category from saved overrides with no current fallback
      (`loadout-utils.ts:745-770`), capacity sum default zero
      (`loadout-utils.ts:946-954`). Workspace: 2164 all-features, 2132
      default, 0 failed; clippy both modes, fmt.
    - [x] **BE-006c2 — Stats and stat upgrades.** `build_stats.rs`
      (core + app): base/masterwork totals (`loadout-utils.ts:284-325,
      355-413`), separate assignment fits and unassigned benefits
      (`loadout-utils.ts:326-349`, `loadout/stats.ts:101-187`),
      conditional tuning/class effects (`stats-conditional.ts:52-107,
      155-174,190-210`), runtime Fonts (`loadout/stats.ts:25-76,189-210`),
      masterwork inference (`analysis.ts:158-190`), non-ignored minima
      only and stat upgrades (`loadout-params.ts:14-42`,
      `analysis.ts:516-541`). Advisory R3-mixed-tier-cost-cache: not a
      deviation (DIM caches by rarity, `mod-assignment-utils.ts:180-226,
      964-989`); mixed-tier misses stay unknown. Workspace: 2194
      all-features, 2162 default, 0 failed; clippy both modes, fmt.
- [x] **BE-007 — Apply build.** DIM apply phases; covers dim-parity U4b/U4c.
  Owner decision (2026-10-12): implement offline with fakes now; the live
  `EquipLoadout` check (dim-parity U4b/U4c) and live apply validation move
  to BE-010. A Ghostlink build is not an in-game loadout, so DIM-style
  preparation is needed regardless of that check.
  - [x] **BE-007a — Moves and equips.** DIM apply phases on top of the
    existing move/equip actions: cross-character and Vault transfers,
    dequip with replacements, space, grouped equips, typed per-item
    outcomes; CLI `build apply`, MCP tool.
    Data decisions (parent, DIM-first): project `equippingBlock.uniqueLabel`
    (never approximate unique rules from rarity, `item-move-service.ts:
    535-546`); DIM user tags and manual-move timestamps have no Ghostlink
    equivalent and are treated as DIM treats untagged, unmoved items
    (neutral); Vault capacity reuses existing DIM-style capacity logic.
    - [x] **BE-007a1 — Unique labels and pure planner.** Migration 33
      (`uniqueLabel` projection, STRICT, guarded downgrade, refresh);
      `build_apply.rs` pure planner: DIM phase order and exclusions
      (`loadout-apply.ts:355-604,417-436`), replacement preference and
      ranking (`item-move-service.ts:216-257,1220-1279`), space destination
      and ranking (`item-move-service.ts:677-741,1138-1211`), grouped
      equips, typed skips; `docs/build-apply.md`. Not exposed in CLI/MCP
      yet. Workspace: 2202 all-features, 2170 default, 0 failed; clippy
      both modes, fmt.
    - [x] **BE-007a2a — Planner completion and dry-run.** Displacement
      ranking with saved loadouts (`item-move-service.ts:1138-1211`,
      `loadout/selectors.ts:109-120`), lone vs grouped dequip
      (`loadout-apply.ts:437-513`), inline single equip vs bulk
      (`loadout-apply.ts:424-433,526-540`), clear-space flags
      (`loadout-apply.ts:680-692,822-955`, `item-move-service.ts:775-790`),
      Postmaster first (`loadout-apply.ts:395-396`,
      `item-move-service.ts:1072-1083`), per-Vault-bucket slots
      (`stores-helpers.ts:65-66,101-107`); fixed occupancy counting any
      known bucket. CLI `ghostlink build apply <ID|--name N> [--character C]
      [--dry-run] [--json]`: dry-run emits `build_apply_plan` with
      `dispatched: 0` (test asserts zero provider calls); without
      `--dry-run` exits 40 `unsupported_operation`. Production passes no
      Vault evidence yet, so Vault-staged moves skip as
      `vault_capacity_unavailable`; stacks skip as `stack_unsupported`.
      Tests written after code; mutation checks confirmed they fail when
      behavior is removed. Workspace: 2213 all-features, 2181 default,
      0 failed; clippy both modes, fmt.
    - [x] **BE-007a2b — Vault evidence.** Migration 34
      (`inventory_buckets`, `vault_vendors`, `vault_accepted_items`; STRICT,
      guarded downgrade, once-per-upgrade refresh). Vault vendor 1037843411
      (`d2-known-values.ts:343`), `acceptedItems` correspondence with
      later-wins and 0 = none (`d2-buckets.ts:59-66`), capacity from bucket
      `itemCount` (`d2-buckets.ts:47`, `stores-helpers.ts:65-66,101-107`).
      Vendor table is stream-decoded keeping only the Vault vendor. Live
      244213.26.06.29.2000-1: buckets 19 KB / 60; vendors 38.9 MB / 2,170
      (Vault vendor 2,576 B, 14 entries). Caps: buckets 10,000 / 4 MiB;
      vendors 20,000 scanned / 128 MiB; 256 accepted entries. World
      requests 13 -> 15; +38.9 MB per Manifest refresh (once per game
      patch; DIM reads the same table). Users run `ghostlink sync-manifest`
      once after schema 34. `VaultEvidence::from_catalogs` feeds
      `BuildService::plan_apply`. Workspace: 2219 all-features, 2187
      default, 0 failed; clippy both modes, fmt. Follow-up: conditional
      vendor fetch if the download cost matters.
    - [x] **BE-007a2c — Executor and CLI.** `build_apply_exec.rs`:
      failed steps never abort the plan (`loadout-apply.ts:527-578`), bulk
      equip takes only arrived items (`:580-593`), failed grouped dequip
      fails its group (`:471-523`), per-item bulk-equip failures by Bungie
      code (`:595-611`); planner steps carry `needs`, so dependents of a
      failed step are skipped. Ghostlink adaptations: uncertain step =
      failed (never projected or retried); rate-limit/auth/unavailable
      stops the rest as `provider_stopped` (fail-fast instead of DIM
      retries); 100 ms pacing like batch equip; stale-plan refusal when the
      projection revision changed. No confirmation prompt (move, equip and
      apply-loadout have none). `ghostlink build apply` executes; results
      applied/no_op/partial/failed/refused; exits 0, 41, 42 or category
      exit, no new codes. Fakes panic on any unscripted mutation. Over the
      unit budget (~1,100 production, ~1,170 test lines), kept as one unit
      because executor and CLI are tested together. Workspace: 2238
      all-features, 2206 default, 0 failed; clippy both modes, fmt.
    - [x] **BE-007a2d — MCP tool.** Done: MCP `apply_build`
      ({id | name, character?, dry_run?}, no top-level oneOf, under the
      500-byte description budget) returns the CLI `BuildOutput`;
      outermost mutation decorator under the existing mutation lock;
      24 tools. Grouped equips above 17 items are chunked in plan order:
      a Ghostlink adaptation, since DIM sends one `equipItems` request and
      a build never gets near the limit (`destiny2-api.ts:229-246`,
      `loadout-apply.ts:580-594`). Exit-42 persistence-failure tests in
      executor, CLI and MCP (characterization, written after the code).
      Workspace: 2251 all-features, 2218 default, 0 failed; clippy both
      modes, fmt. Follow-up: offline stdio fixture cannot run a real apply.
      Original scope: `apply_build` with `dry_run`, schema
      and description byte budget, mutation decorator chain, gateway
      composition in `destiny-mcp/src/main.rs`, mutation-tool tests,
      `docs/mcp.md`. Also: chunk grouped equips above the 17-item
      `EquipItemsRequest` limit (check DIM), and a dedicated test for the
      local-state persistence failure path (exit 42).
    - Original BE-007a2 scope: **Planner completion and exposure.** Saved-loadout
      displacement ranking, final clear-space flags, Postmaster/stacks,
      single-item inline equip parity, source grouping across interleaved
      transfers; Vault correspondence/capacity (`stores-helpers.ts:65-66,
      101-107`; today missing evidence yields `vault_capacity_unavailable`);
      CLI `build apply --dry-run` with no-dispatch verification; executor
      with partial-failure semantics, throttle and error mapping; MCP
      mutation tool.
  - [x] **BE-007b — Sockets.** Mods, aspects, fragments and subclass via
    `InsertSocketPlugFree` as DIM applies them.
    - [x] **BE-007b1 — Subclass and artifact overrides.** New
      `InsertSocketPlugFree` adapter (`plug{socketIndex, socketArrayType 0,
      plugItemHash}`, itemId, characterId, membershipType; 0.5 s spacing,
      no retry); free insertions only (`advanced-write-actions.ts:41-103`).
      Override gathering, vault/class guards, default abilities
      (`loadout-apply.ts:272-299`); interchangeable aspects/fragments and
      emptied leftovers (`:1142-1192`); active fragment sockets
      (`:1196-1199`, `loadout-utils.ts:745-771`); artifact pass
      (`:1010-1131`); plug already in place needs no request, failures
      never stop the next plug (`:1303-1366`). Planner `Sockets` phase with
      `needs`; executor dispatch; dry-run shows plugs. Deviations:
      acknowledged plugs invalidate freshness (`local_state:
      refresh_pending`) instead of editing the projection; subclass and
      artifact buckets still skip as `unsupported_bucket`, so builds with a
      subclass report `partial`; core-settings protected-plug lists not
      checked locally; empty `{}` socketOverrides stored as none. OAuth: no
      new scope expected (`MoveEquipDestinyItems`), unconfirmed until
      BE-010. Over the unit budget (~1,220 production, ~1,260 test lines),
      kept as one unit because adapter, planner, executor and wiring land
      together. Workspace: 2279 all-features, 2246 default, 0 failed;
      clippy both modes, fmt.
    - [x] **BE-007b2 — Armor mods.** `build_mod_apply.rs`: armor set =
      build armor marked equip, else equipped piece
      (`loadout-apply.ts:1282-1296`); unlocked mods only (`:306-328`); mod
      phase after socket overrides (`:629-672`); early exit when every mod
      is in place (`:1302-1310,1410-1458`); `fitMostMods` with in-game
      rules (`:1317-1330`); only required steps, sequential, failures don't
      stop the next (`:1383-1405,1461-1546`); `pickPlugPositions`,
      `createPluggingStrategy`, `isAssigningToDefault`
      (`mod-assignment-utils.ts:576-614,689-770,780-1000,1037`); hidden
      artifice sockets count (`:586-593`); shared free-insert check.
      `clearMods` included. Plan gains `mod_skipped` and `mods_in_place`.
      Workspace: 2294 all-features, 2261 default, 0 failed; clippy both
      modes, fmt. (The all-five-pieces deviation was removed in BE-007b3.)
    - [x] **BE-007b3 — Fashion and subclass equips.** Done: subclass and
      artifact are equip-only planner buckets after armor
      (`d2-bucket-categories.ts:21-23`, `loadout-apply.ts:118-129,261-271`),
      moves only when transferable (`:386-405`), excluded from clear-space;
      `EquipmentCapacityPolicy` untouched; build apply has its own
      `build_equip_effect`. Mods plug on fewer than five pieces
      (`loadout-apply.ts:1282-1330`, `mod-assignment-utils.ts:286-354`).
      Fashion (`build_fashion_apply.rs`): unlock check
      (`loadout-apply.ts:308-320,329-338`), socket fit (`:1365-1380`,
      `socket-utils.ts:228-237`), mods before fashion (`:1388-1405`), transmog
      swap (`:1489-1500`, `advanced-write-actions.ts:144-151`),
      irreversible-shader guard (`advanced-write-actions.ts:105-135,165-175`),
      exotic ornament unlocks (`selectors.ts:372-389`). Migration 35:
      `definition_item_classes` and `definition_item_categories` for plug
      definitions, no new Manifest table. Parent verified enums
      (`DestinyItemSubType` Shader 20, Ornament 21) and live data
      (244213.26.06.29.2000-1: 14,536 plugs, 39,814 category rows, max 6 per
      plug vs cap 64; 721 shaders, 3,688 ornaments). Approximation: plug-set
      identity is "unlocked in any plug set or enabled on the item" because
      `PlugSetEntry` has no plug-set hash. Workspace: 2323 all-features, 2290
      default, 0 failed; clippy both modes, fmt. Follow-up: project the
      plug-set hash and embed DIM's universal-ornament plug-set list.
      Original scope: Fashion needs
      shader/ornament classification and item category hashes (not in
      `ItemDefinition`, migration needed): irreversible-shader guard
      (`advanced-write-actions.ts:105-135`), transmog swap, exotic-ornament
      unlocks. Subclass/artifact equips need `EquipmentCapacityPolicy`
      (validated to eight weapon/armor buckets) reworked; until then builds
      with a subclass report `partial`. Also: plug mods on fewer than five
      armor pieces like DIM.
- [ ] **BE-008 — Substitutes.** Optimizer-based armor sets.
  DIM map (2026-10-12, DIM 29c122e): inputs `process-worker/process.ts:
  40-65`, item mapping `process/mappers.ts:61-190` (assumed masterwork
  stats `loadout-utils.ts:356-405`, capacity `armor-upgrade-utils.ts:12-50`,
  tuning variants), pre-filter `item-filter.ts:67-292` with strictly-worse
  dedupe `utils/stats.ts:15-80`, five-loop search with subtree pruning
  `process.ts:72-1200`, auto stat mods `process-utils.ts:333-610`, 200-set
  min-heap `set-tracker.ts:23-60`. Analyzer findings
  ItemsDoNotMatchSearchQuery (`analysis.ts:293-307`) and
  BetterStatsAvailable (`:309-367`, `stopOnFirstSet`, strict upgrades).
  Defaults (proposed by the parent, CONFIRMED by the owner 2026-10-12): `--limit` default 10, max 200 (DIM's cap); per-invocation time
  budget with a `truncated` flag on timeout (analyzer: `not_evaluated` with
  a budget reason); deterministic tie-break on item ids; candidate armor
  from every store DIM includes; masterwork assumption and unlocked plugs
  from the build parameters and synced profile; substitutes respect saved
  constraints (min and max) like DIM's optimizer, while the analyzer
  follows DIM's strict-upgrade merge.
  - [x] **BE-008a — Model and pre-filter.** `optimizer/{mod,items,filter,
    stat_dupe}.rs` (680 production lines, internal): ProcessItem mapping and
    tuning variants (`process/mappers.ts:61-190`), assumed masterwork
    (`loadout-utils.ts:356-405`), capacity/artifice rules
    (`armor-upgrade-utils.ts:12-50`), constants (`types.ts:105-155`),
    Balanced Tuning on base-zero stats (`stats-conditional.ts:62-78,
    190-210`), pre-filter order (`item-filter.ts:67-292`), strictly-worse
    dedupe (`utils/stats.ts:15-80`; a linear scan with the same relation
    instead of DIM's trie), exotic class-item keys, desired stat ranges
    (`loadout-params.ts:14-34`). Missing inputs are typed unknown
    (`bucket_sockets: None` -> `FilterError::BucketSocketsUnavailable`).
    BE-008d must build the `ArmorCandidate` adapter and pass DIM's extra
    dedupe columns (energy, activity-slot tags, set-bonus membership)
    through `get_stats`, as DIM's optimizer callback does. Workspace: 2350
    all-features, 2317 default, 0 failed; clippy both modes, fmt.
  - [x] **BE-008b — Auto mods and stat math.** `optimizer/{auto_mods,
    stat_mods}.rs` (~890 production lines, over budget, kept as one unit:
    two separate files tested together). Auto-mod map from plug sets
    731468111 / 4285066582 with unlocked cheaper variants
    (`process/mappers.ts:189-249`); picks cache and memoized
    `chooseAutoMods` (`auto-stat-mod-utils.ts:55-366`); session and Heap
    permutations (`process-utils.ts:50-134,612-623`,
    `mod-permutations.ts:16-57`); `updateMaxStats`,
    `pickAndAssignSlotIndependentMods`, `pickOptimalStatMods`,
    `greedyPickStatMods` (`process-utils.ts:170-610`), DIM quirks kept.
    DIM tier, exact-stat, `updateMaxStats` and memoization tests ported on
    synthetic data. Missing plug sets/definitions are typed unknown;
    Maverick and unknown-category activity tags not represented. Workspace:
    2384 all-features, 2351 default, 0 failed; clippy both modes, fmt.
    For BE-008c: project `ProcessItem.stats` into the session stat order,
    one session per thread. For BE-008d: `unlocked` list, general/activity
    `ProcessMod` split, decision on Maverick-style tags.
  - [x] **BE-008c — Search core.** `optimizer/{search,driver,
    set_tracker}.rs` (~1,100 production lines without comments, over
    budget; one unit because the single test file covers all three and they
    do not compile apart). Inputs and bucket sort (`process.ts:40-65,
    98-141`), suffix bounds and pruning (`:187-290`, `canPruneSubtree`
    `:200-286`, `perkCountsPrune` `:1403-1425`, `setBonusPrune`
    `:1427-1450`), levels as one recursion (`:297-770`), innermost
    evaluation with tuning `mayMatter` (`:560-770`), result mapping and
    strict upgrades (`:1117-1142`), tracker (`set-tracker.ts:1-190`),
    slicing and merge (`process-wrapper.ts:296-390`), 100k-combo cadence.
    Deviations (deterministic for any thread count): `could_insert` uses
    `>=` and ties break on item ranks then tuning mod; strict-upgrade filter
    after the merge; stop-on-first runs one slice; `max_combos` /
    `time_limit` budget with `truncated` and combos visited/total; power 0
    instead of NaN. `seedExactStatRanges` not ported (ranges verified
    against a brute-force reference). 28 tests incl. thread counts
    1/2/3/5/8 on shuffled input. Perf (informational): 1.89M combos pruned
    to 19,818 evaluated in 12 ms release; 100k all-tied combos in ~150 ms
    release. Workspace: 2412 all-features, 2379 default, 0 failed; clippy
    both modes, fmt. Follow-ups: tie-heavy inventories cannot prune under
    `>=` (budget bounds it; consider a cheaper tie pre-check); port
    seeding if real inventories converge slowly.
  - [x] **BE-008d — Analyzer wiring.** BetterStatsAvailable and full
    ItemsDoNotMatchSearchQuery in `build check` / `analyze_build`. A first
    worker ended without a report; its partial (`optimizer/analyzer.rs`,
    build_stats/mod_assignment refactors) was audited and kept. App adapter
    `destiny-app/src/build_optimizer.rs` (candidates per
    `mappers.ts:65-190`, dedupe columns `item-filter.ts:262-300`, mods via
    `categorizeArmorMods` `mod-assignment-utils.ts:84-150` and
    `autoAssignmentPCHs` `loadout-builder/types.ts:114`, Maverick through
    existing specialty sockets `specialty-modslots.ts:36-37,111-117`).
    Findings per `analysis.ts:250-367`, blocking set `types.ts:72-76`,
    strict-upgrade merge `utils.ts:16-60`, Font note
    (`loadout/stats.ts:190-205`). 2 s + combo cap per build;
    `not_evaluated` on exhaustion or missing inputs. Required armor perks
    -> `not_evaluated` (`armor_perk_requirements_unsupported`: plug category
    identifiers not projected). Postmaster/Other excluded from candidates.
    ~1,100 production lines, one unit (findings need the adapter to be
    testable). Tests written after code; mutation checks fail as expected.
    `docs/optimizer.md`. Workspace: 2433 all-features, 2400 default,
    0 failed; clippy both modes, fmt. Untested paths: real auto-mod plug-set
    loading with Font/general stat mods, tuning variants end to end,
    real-vault timing. Follow-up: apply DIM's All+artifice exotic ->
    ArtificeExotic (`analysis.ts:211-217`) to the existing stats and mod
    findings too.
  - [x] **BE-008e — Substitutes command.** `ghostlink build substitutes`
    and MCP tool `build_substitutes`; D3: never alters check or apply
    resolution (`check` runs the analyzer, `substitutes`/`plan_apply` do
    not). `optimizer/substitutes.rs` reuses the analyzer's filter, process
    buckets and search inputs with the saved min and max ranges as they are
    (`loadout-params.ts:14-34`), no stop-on-first, no strict-upgrade merge;
    limit 1..=200, default 10; fixed 10 s budget with `truncated`. Nothing
    is pinned (DIM pins the loadout's armor when opened from it,
    `loadout-builder-reducer.ts:185-197`, which would only list the build
    itself); build pieces carry `in_build`. Also covers BE-008d test gaps
    (real auto-mod plug sets with Font/general mods, tuning variants end
    to end). The worker ended without a recoverable report; the parent
    audited the partial and kept it. Docs: `cli.md`, `mcp.md`,
    `optimizer.md`, skill. Workspace: 2450 all-features, 2417 default,
    0 failed; clippy both modes, fmt.
- [x] **BE-009 — dim.gg import.** Dropped by owner decision D4 (won't do).
  Remaining check: the `dim.gg` typed error guides users to the full DIM
  link/JSON (with follow-up `dim_loadout.rs:333`); verify in BE-010.
- [ ] **BE-010 — Live validation.** Isolated DB copy
  (`~/.cache/ghostlink-be010/destiny.db`, `DESTINY_DATABASE_PATH`; the
  real `~/.local/share/ghostlink/destiny.db` is schema 19 and must not be
  migrated by branch binaries), release binary in
  `~/.cache/ghostlink-be010/target`. DIM clone moved to
  `~/.cache/dim-research/DIM` (8.146.0, 285604b).
  - [x] **BE-010a — dim.gg message.** D4 wording (open in DIM, import the
    full `loadouts?loadout=` link or JSON); reason code kept. `cea266c`.
    RDD review of 73b5a1c..cea266c approved and acknowledged
    (`review-c2af53a090474870`); advisories: piece fallback zeros
    (`build_substitutes.rs:116-118`), unexplained combo cap
    (`optimizer/substitutes.rs:21`), truncated-with-no-sets wording
    (`destiny-output/src/build_substitutes.rs:102-103`).
  - Live finding (2026-10-12): `build check` on the Hunter's equipped
    snapshot (284 ms) leaves stats, analyzer and substitutes
    `not_evaluated`: (1) `armor_base_stats_unavailable` because the Armor
    3.0 stat-roll plugs (`plugCategoryHash` 748854354 `armor_stats`, e.g.
    481842213/627959599/462856101, verified on Bungie's Manifest API) have
    a blank name and `decode_record` skips blank names
    (`manifest.rs:1362`), so `build_armor_stats` sees an enabled plug
    without a definition; (2) `raw_reusable_plugs_not_projected` because
    armor mod sockets are absent from component 310 and stay `None`
    (`profile.rs:525-527`), while DIM reads an absent socket as no raw
    plugs (`sockets.ts:77-79`, `reusablePlugData?.[index]`).
  - [x] **BE-010b1 — Nameless stat plugs.** `definitions.name` is
    `CHECK length 1..256` with FKs from `definition_armor` and
    `definition_investment_stats`, so instead of a table rebuild or a
    fabricated name: blank-name, non-redacted `armor_stats` records are
    decoded apart (other blank names still skipped) and stored as
    `ArmorCatalogKind::StatPlug` (id 8, not in `ALL`) in STRICT
    `definition_stat_plugs` / `definition_stat_plug_stats`, published with
    the world catalogs (accepted deviation: no new port method). Migration
    36 clears the manifest sync metadata like v26-v35, with a downgrade
    guard. `inspect_armor_model`, `masterwork_plug_stats` and the optimizer
    set-selector resolve nameless plugs through it. Tests: decode keep/skip,
    SQLite round trip, migration 36, sync publication, masterwork lookup,
    app inspect with the real plugs 481842213/627959599/462856101 (total
    base 75; live: Hunter chest total base 75 after re-import). Code landed before tests; each test was shown failing with
    its production change reverted (lesson: handoffs put the public RED
    test as step 1 with existing types). Workspace: 2456 all-features,
    2423 default, 0 failed; clippy both modes, fmt. Open: core mod
    assignment still rejects nameless inserted plugs
    (`mod_assignment/independent.rs:60,75,130`,
    `inserted_plug_definition_unavailable`); the legacy
    `acquire_and_publish` path does not publish stat plugs.
  - [ ] **BE-010b2 — Absent 310 sockets.** When component 310 was
    returned, every socket not listed for an instance projects
    `reusable_plugs: Some(vec![])` (DIM `sockets.ts:77-79`); a missing
    component (privacy/unavailable) keeps `None`.
  - [ ] **BE-010c — Live re-run.** check, substitutes timing, apply
    dry-run; then live apply and EquipLoadout U4b/U4c.
