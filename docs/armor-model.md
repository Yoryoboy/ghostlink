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
- `tuned_stat_hash` is deliberately always `null`, with reason
  `raw_reusable_plugs_not_projected`. DIM scans raw reusable plug membership and
  returns undefined for conflicting tuners (`utils/item-utils.ts:409-432`,
  mapping `search/d2-known-values.ts:445` onward). DIM retains the raw instance
  list (`inventory/store/sockets.ts:77-79,133-140,784`); Ghostlink's
  `profile.rs:476-529` filters disabled/non-insertable alternatives and oversized
  lists. Neither selectable plugs nor Manifest plug sets are used to guess a
  tuned stat. Current tuning contributions remain included in current stats.
- `armor_model.armor_set`: identity from nonzero `equipableItemSetHash`
  (`inventory/store/d2-item-factory.ts:817-818`), set name and all projected
  `sandbox_perk_hash`/`required_set_count` pairs (`item-popup/SetBonus.tsx:55-64`).
  An unresolved catalog retains the hash with unknown name/perks. No perk names,
  icons or active-bonus claims are emitted; thresholds are requirements, not
  activation evidence. Event selector membership is not evaluated.
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
BE-005c3 is parent-owned: raw reusable plug fidelity, exact tuned stat, and set
perk display names/icons. No decode, sync, migration or fetch changes here.
