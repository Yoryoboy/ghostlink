# Armor inspection: DIM-derived, read-only model

BE-005c1 adds an armor model to CLI and MCP inspection without changing existing
inspection fields. It uses committed instance and Manifest projections only:
no migration, new component request, or Manifest fetch.

## Audit of the inherited partial

Evidence paths below are relative to DIM commit `29c122e` (`src/app/` unless
otherwise stated).

| Piece | Verdict and evidence |
|---|---|
| Investment-derived base and current stats | Keep. `inventory/store/stats.ts:134-169,224-264` builds investment stats, not API totals. Missing group, stat definitions or active plug definitions remain unknown in Ghostlink. |
| Intrinsic roll contributions | Keep. `utils/socket-utils.ts:213-220` identifies Intrinsics/ArmorStats; `inventory/store/stats.ts:318-390` applies these first, only when enabled, and updates base. Socket visibility does not gate contributions. |
| Interpolation | Keep implementation; correct the inherited negative-input test. `inventory/store/stats.ts:614-651` bypasses armor interpolation/clamping, uses piecewise interpolation elsewhere, and selects the first endpoint below the table. Magazine uses Math.round; other stats use ties-to-even. |
| Display maximum | Keep endpoint maximum and top-level fallback. `inventory/store/stats.ts:282-291` initializes from statGroup.maximumValue. Ghostlink additionally tolerates empty interpolation and keeps absent top-level maximum unknown rather than inventing 100. |
| Conditional stats | Keep armor-specific rule and duplicate consolidation. `inventory/store/stats-conditional.ts:52-78,155-160,190-210` preserves the first duplicate's flag, activates armor masterwork/Balanced Tuning only on base-zero stats, and leaves unrecognized conditional rules active. Weapon/subclass-specific rules are outside armor inspection. |
| Total base | Keep sum of six armor bases. `inventory/store/stats-custom.ts:52-72` sums evenly weighted bases separately from current values. |
| Assumed masterwork | Fix unresolved-plug handling. `utils/item-utils.ts:318-325` detects Armor 3.0 from a masterwork plug, including disabled plugs; `inventory/store/stats.ts:405-414` grants +5 only to base-zero stats (legacy armor: +2 each). An unresolved disabled plug cannot silently establish legacy armor; assumed values remain unknown. |
| Armor 3.0 masterwork tier | Fix raw-stat-presence check. `inventory/store/masterwork.ts:63-74,98-104` requires a nonempty active projected stat map, then uses the first conditional investment stat, defaulting to 0. `inventory/store/stats.ts:489-514` attaches active plug stats even for disabled plugs; zero-valued unconditional energy still makes the map nonempty. Energy capacity is not the tier. |
| Gear tier and energy | Keep raw values plus consumer fallback. `inventory/store/d2-item-factory.ts:544,586` reads instance energy and gearTier ?? 0. Bungie types: `destiny2/interfaces.d.ts:12435-12437,12531-12535`. |
| Request-local ports and JSON | Keep bounded definition/catalog lookups and additive CLI/MCP object; fix composition bounds and MCP repository forwarding. No adapter/schema migration required. |
| Human output | Complete missing additive English armor section, using the shared output module. |

No inherited production piece was discarded. The incorrect negative interpolation
expectation was replaced. Conservative unknown handling is intentional: legacy
empty socket projections do not establish a zero roll, and missing inputs never
fall back to component-304 totals.

## Review path

1. Review pure rules and regression fixtures in `destiny-core` armor_model files.
2. Review request-local assembly in `destiny-app/src/armor_model.rs`.
3. Compare shared output with CLI/MCP parity tests (known and unknown values).

## BE-005c2: tuning, sets and fragments

The approved narrowed scope adds read-only inspection fields using committed data:

- `armor_model.tuning`: the first visible socket whose inserted plug has category
  3481777685, exposing real `socket_index` and current `plug_hash`. Enabled is
  intentionally not a gate (`utils/socket-utils.ts:677-686`). Missing plug
  definitions cannot establish tuning; absent/unresolved tuning is `null`.
- `tuned_stat_hash` follows DIM's raw membership scan (`utils/item-utils.ts:409-432`,
  mapping `search/d2-known-values.ts:445-476`, stat IDs `src/data/d2/generated-enums.ts:1422-1467`).
  BE-005c2 kept it unknown; BE-005c3 now preserves raw membership. Disabled and
  non-insertable tuners count; repeated same-stat tuners agree; conflicting
  directions, balanced-only and empty lists stay unknown. No selectable-plug or
  Manifest-plug-set fallback is used. Current tuning contributions remain included
  in current stats.
- `armor_model.armor_set`: identity from nonzero `equipableItemSetHash`
  (`inventory/store/d2-item-factory.ts:817-818`), set name and all projected
  `sandbox_perk_hash`/`required_set_count` pairs (`item-popup/SetBonus.tsx:55-64`).
  An unresolved catalog retains the hash with unknown name/perks. BE-005c3 adds
  optional perk name/icon; no active-bonus claims are emitted. Thresholds are
  requirements, not activation evidence. Event selector membership is not evaluated.
- Subclass inspection adds `fragment_capacity: {capacity: number|null}` outside
  the armor-only object. DIM identifies four aspect categories
  (`utils/socket-utils.ts:278-283`) and sums inserted aspects'
  `energyCapacity.capacityValue` without enabled/visible gates
  (`loadout-drawer/loadout-utils.ts:940-954`). A resolved plug without capacity
  contributes zero as in DIM; missing socket projections or plug definitions
  stay unknown. Real socket indices are used, never filtered positions.

All values are inspection only, not upgrade actions or build-analysis findings.
Equipped-set activation is deferred: build analysis does not currently assemble
this item/catalog model, and DIM also accounts for event selector sockets
(`utils/socket-utils.ts:793-809`). A separate equipped-set analysis task should
count resolved armor pieces and compare thresholds (`SetBonus.tsx:23-64`).
## BE-005c3: raw reusable fidelity and set-perk display

- DIM `inventory/store/sockets.ts:77-79,133-140,784` attaches the complete instance
  reusable list to the corresponding real socket, with no size-based omission.
  Ghostlink now stores hash, `canInsert`, `enabled`, duplicates and provider order
  on `IndexedSocket.reusable_plugs`, independently of publication filtering.
  Missing per-socket data remains `None`; observed empty data is `Some([])`.
  As in DIM, only corresponding real sockets acquire membership. The existing
  selectable view still filters flags, deduplicates and omits lists above 64.
- Oversized raw lists are retained intact (500-entry regression fixture), never
  truncated. Safety ceilings remain explicit: 32 real sockets, existing profile
  response byte bound, and 65,536 raw entries per socket, matching the Manifest
  plug-set ceiling. Exceeding that ceiling rejects acquisition/publication instead
  of manufacturing partial membership. This defensive ceiling is not a DIM rule.
- Migration 30 adds STRICT `item_reusable_sockets` (observed membership marker) and
  `item_reusable_plugs` (ordered children), cascading through real sockets/items.
  Legacy rows are not backfilled. Downgrade refuses observed membership, including
  empty lists, or populated perk display fields. All fields participate in revisions.
- `reusable_membership_known` distinguishes legacy/unobserved tuning from an
  observed list without a unique direction. JSON reasons are respectively
  `raw_reusable_plugs_not_projected` and `no_unique_directional_tuner`; a known
  stat has a null reason. CLI text and MCP share the same projection.
- DIM `item-popup/SetBonus.tsx:55-57,108-109,175-184` resolves sandbox definitions
  and uses their name/icon without an `isDisplayable` gate. Ghostlink extends the
  already-fetched sandbox candidates, joins only referenced display fields into
  set-perk catalogs during atomic Manifest publication, and persists them as nullable
  `catalog_set_perks.name`/`icon`. Missing/redacted references stay unknown; this
  is not a new sandbox catalog or a new request. Description composition is unchanged.
  Migration 30 invalidates Manifest freshness once so the same version refreshes
  after upgrade. Inventory membership requires an ordinary profile resync.

## BE-005d: crafted identity and equipped armor capture (first slice)

DIM evidence refers to commit `29c122e`:

- `inventory/store/crafted.ts:23-90`: only weapons' first crafting-category
  socket (3583996951) supplies crafting information. Component 309 objectives
  are joined by the inserted plug hash; Objective `uiStyle` identifies level
  (2), level progress (3), and timestamp (4). All three are required. The
  timestamp is seconds, not sync time. Disabled/invisible sockets still count.
- `loadout-drawer/loadout-utils.ts:524-534,694-710`: capture `craftedDate`, then
  resolve instance ID before nonzero crafted date. DIM's `keyBy` keeps the last
  date match and does not add a hash or owner constraint. Missing projections
  remain unknown; absent evidence never permits same-hash substitution.
- Migration 31 adds STRICT objective-style children and observed socket markers
  with ordered objective hash/progress children. Cascading references preserve
  profile replacement semantics; guarded downgrade refuses populated styles or
  observed objectives, including empty lists. Manifest freshness is invalidated
  once; legacy inventory is not backfilled. All raw objectives participate in
  profile revision equality, including order and absent progress.
- `loadout-drawer/loadout-utils.ts:193,204-218,538-546`,
  `inventory/store/sockets.ts:501-512`, `loadout/mod-utils.ts:53-65`: inserted
  armor mods are captured without an enabled gate. The old disabled-plug
  follow-up (`builds.rs:176-185` in the original report) is resolved as
  **matches DIM**. Capture now reads raw sockets so publication filtering does
  not discard disabled inserted mods. Known numeric mod categories, pinned
  empty-plug hashes, consumable bucket exclusion and item-type presence replace
  name-based classification. Observed cosmetics use ArmorCosmetics socket
  indexes and retain default plugs. Legacy socket-less projections retain the
  prior partial capture and explicitly report socket-category limitations.

## BE-005d2: socket overrides

DIM evidence (commit `29c122e`, paths under `src/app/`):

- `loadout-drawer/loadout-utils.ts:136-174,178-190`: equipped subclass/artifact
  capture walks socket categories in definition order and indexes in category
  order. Fragment capacity is consumed by every fragment position, even empty
  ones, before membership/empty tests; inactive positions are not captured.
  Missing capacity suppresses fragment capture with an omission, never zero
  inferred capacity. Disabled/invisible plugs are not independently excluded.
- `inventory/store/sockets.ts:584-645`: empty choice precedence is raw reusable
  known-empty, Manifest plug-set known-empty, inline known-empty, initial plug.
  Inventory-only sockets (except the locked ghost activity type), Y2 masterwork,
  pinned no-default socket categories, and pinned whitelist identifiers cannot
  be emptied. Abilities/supers therefore retain default choices.
- `utils/socket-utils.ts:225-237`, `inventory/store/sockets.ts:683-687,792-825`:
  membership uses empty choice, reusable-before-randomized Manifest plug set,
  or raw instance reusable membership. It explicitly does **not** validate
  unlocked/enabled/canInsert flags. The inserted plug must resolve as pluggable;
  Ghostlink does not use selectable/publication-filtered plugs as evidence.
  Existing 102/206 acquisition remains unchanged: no extra unlocked evidence,
  component or migration is needed for this DIM capture contract.
- `loadout-drawer/loadout-drawer-reducer.ts:119-122`: artifact configuration is
  artifact-item `socketOverrides`, not legacy `parameters.artifactUnlocks`.
- `loadout/loadout-type-converters.ts:190-210,213-238,249-260`: in-game armor
  mods use ArmorMods socket indexes and exclude known empty hashes; cosmetics
  use ArmorCosmetics indexes and keep empties. Subclass/artifact overrides drop
  only `UNSET_PLUG_HASH` (zero), retaining empty hashes despite the converter's
  broader comment. Missing categories do not fall back to plug names.
- `inventory/store/crafted.ts:65-90`: the R3 advisory was a real analysis gap:
  a nonempty objective list missing level, level-progress or timestamp now
  reports crafted-date evidence unavailable rather than known item-not-found.
  Observed empty objectives still establish no crafted tuple; existing DIM
  NaN progress semantics are unchanged.

No new schema or provider requests. The shared app service feeds CLI and MCP;
show/save add `equipped_socket_overrides`, and check retains override specs
without claiming apply-time socket validity. Stored DIM JSON preserves overrides
but not source diagnostics: later show/get reports
`source_capture_evidence_not_persisted`. Missing catalogs, plug definitions,
raw membership or socket projections remain explicit capture omissions.
