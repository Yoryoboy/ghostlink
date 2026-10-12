# Build mod fitting: DIM port

`build check` and MCP `analyze_build` share the additive `mod_fit` object.
Assignment is pure and follows DIM commit `29c122e`:

- `evaluation` and nullable `reason` qualify assignment evidence.
- `invalid_mods`: resolved requested mods that fit no active ArmorMods socket
  in the entire inventory, or have an unsupported category. They are excluded
  before fitting and do not themselves imply `mods_dont_fit`.
- `unassigned_mods`: valid inventory mods which cannot fit this build's resolved
  armor after allowed energy upgrades (including invalid-for-this-set mods).
- `needs_upgrades_for_mods`: mods fail as-is but fit with the analyzer's inferred
  energy rules. This is not an upgrade action or Armor 3.0 stat-masterwork claim.
- `item_mod_assignments`: item-order `{instance_id, mods}` proposals. Hashes are
  resolved variants, not necessarily the saved hashes; these are not socket positions.
- `upgrade_costs`: positive `{material_hash, amount}` entries in DIM rarity order.
  These are energy/artifice upgrade ranking costs, not inventory affordability,
  a complete stat-masterwork quote, or a provider-validated execution plan.
- `uses_seasonal_mods`: a direct seasonal mod or a successful assignment which
  cannot fit after re-resolving all valid mods with no cheaper variants unlocked.
  Missing expensive-variant inputs leave this field/finding null with a reason,
  while independently proven fit evidence remains evaluated.

Null is unknown, never empty or false. Assignment requires five resolved equipped
armor entries, socket membership, energy and rarity. Missing catalogs or material
inputs stay `not_evaluated` with explicit reasons. Known absent tier sockets/plug
sets yield DIM's empty upgrade-plug list; no guessed prices are substituted.

## Assignment and ranking

Categorization checks all owned armor, then the resolved set. Bucket mods use
stable picky-first greedy placement, energy and exclusion groups. Artifice prefers
current placement, then nonexotics, and consumes each candidate once. Tuning follows
helmet/arms/chest/legs/class order and DIM's exotic request-count rule; directional
restriction uses raw instance reusable membership, including disabled/noninsertable
entries. Unobserved membership remains unknown, not unrestricted.

General/activity mods use hash-deduplicated five-position Heap permutations.
Activity is tested before general on each item, against specialty metadata and
remaining assumed energy. Original capacity zero rejects independent mods even
when an upgrade floor would otherwise allow them. Existing energy used is not
subtracted: this proposes replacement mods rather than stacking on current mods.
Inputs longer than five keep DIM's trailing-entry behavior.

Candidates rank by fewer unassigned independent mods, then lexicographic material
cost, then fewer newly inserted independent mods. Exact ties retain Heap order.
Materials rank Exotic Cipher, Ascendant Shard, Enhancement Prism, Enhancement Core,
Glimmer; unranked materials do not affect DIM's ordering. Costs accumulate by tier,
subtract already-paid tiers, and cache the first item's model per rarity. Duplicate
materials sum for energy upgrades; exotic artifice enhancement uses the first
matching material entry and forces maximum energy before its extra cost.

Upgrade plugs come from the first ArmorTier socket (GhostTier fallback), its
resolved Manifest plug set, matching energy type/category and availability mode 2.
Visibility, enabled, currentlyCanRoll and raw reusable flags are not gates here.
Request-local assembly loads these sets, their plug definitions and requirement
sets, plus the exotic enhancement definition and normal-cost mod variants.
No new network requests, component acquisition or migration are introduced.

Seasonal costs first resolve using enabled account/target-character unlocks;
missing plug-set responses mean no unlocks. All twelve legacy stat-mod hashes
migrate. Missing/non-pluggable definitions use deprecated placeholder `444600262`;
a missing/non-pluggable placeholder remains unknown. Saved/exported builds keep
original hashes. Fashion `modsByBucket` is not mixed into `parameters.mods`.

The global `needs_armor_upgrades` finding combines mod upgrades and the stat
upgrade comparison below. Either proven upgrade requirement makes it true;
false requires both checks to establish no upgrade requirement. Unknown inputs
stay explicit. No optimizer/percentage claims.

## DIM evidence

Paths below are under `src/app/`, except the named data/API sources.

| Decision | DIM file:line (commit `29c122e`) |
|---|---|
| Eager inventory/set categorization, full-set analyzer gate | `loadout/mod-assignment-utils.ts:84-151,346-370`; `loadout-analyzer/analysis.ts:442-478` |
| Numeric bucket matching and greedy sockets/energy/exclusions | `loadout/mod-utils.ts:15-21,157-159`; `loadout/mod-assignment-utils.ts:576-675` |
| Membership without flag filtering; resolved set includes cannot-roll entries | `utils/socket-utils.ts:225-237`; `inventory/store/sockets.ts:584-645,677-687,792-825` |
| As-is floor 1, upgraded floor 9, assumed maximum 10/11, capacity-based analyzer assumption | `loadout/armor-upgrade-utils.ts:12-49`; `loadout-analyzer/analysis.ts:158-190`; `search/d2-known-values.ts:29-30` |
| Heap order/deduplication; specialty first visible plugged socket | `loadout/mod-permutations.ts:3-64`; `search/specialty-modslots.ts:43-150`; `utils/item-utils.ts:69-100` |
| Artifice/tuning preference and single-use candidates | `loadout/mod-assignment-utils.ts:359-420`; `utils/item-utils.ts:298-325,409-432` |
| Energy upgrade plug discovery | `inventory/store/energy.ts:18-49`; `src/data/d2/generated-enums.ts:178,616,1603,1625`; bungie-api-ts `destiny2/interfaces.d.ts:6550-6553` (availability mode 2) |
| Cumulative per-rarity costs, already-paid subtraction, enhancement | `loadout/mod-assignment-utils.ts:162-249,964-989` |
| Candidate evaluation, lexicographic costs, changes, exact-tie order | `loadout/mod-assignment-utils.ts:255-265,437-528,952-962,1007-1036` |
| Expensive-variant re-fit and direct seasonal list | `loadout-analyzer/analysis.ts:449-501`; `src/data/d2/seasonal-armor-mods.json:1-7` |
| Legacy migrations, deprecated fallback, cost resolution and scoped unlocks | `loadout-drawer/loadout-utils.ts:880-936`; `loadout/mod-utils.ts:120-132`; `inventory/selectors.ts:333-346`; `records/plugset-helpers.ts:20-40` |

Migration 32 already projects `insertionMaterialRequirementHash`, capacity
`energyType`, `plugAvailability` and ordered requirement-set materials.
Parent-measured Manifest `244213.26.06.29.2000-1-bnet.65864`: 116,945 bytes,
332 sets, maximum 4 materials/set. Existing ceilings: 16 MiB, 10,000 records,
256 materials/set. STRICT SQLite tables, guarded downgrade, one same-version
Manifest refresh, no inferred inventory backfill; world requests remain 13.

## Fragment findings and remaining stat dependency

`empty_fragment_slots` / `too_many_fragments` select the first resolved subclass,
without an equip gate, compare truthy saved fragment overrides with saved aspect
capacity, and sum selected aspect capacity (default zero). No current-aspect
fallback; nonzero empty plugs count. Missing projections/definitions remain unknown.
Evidence: `loadout-analyzer/analysis.ts:90-98,420-438`,
`loadout-drawer/loadout-utils.ts:745-770,946-954`, `utils/socket-utils.ts:77-112,278-290`.

## BE-006c2: stats and stat upgrades

The additive `stats` object reports `evaluation`, nullable `reason`, six
`values: [{stat_hash, as_is, assumed}]`, `satisfies_constraints` and
`needs_upgrades_for_stats`. Missing base stats, energy, plug definitions or
assignment inputs leave totals null with an explicit reason. No API component
304 totals are relabelled as base stats. No provider request or migration is added.

| Decision | DIM evidence, commit `29c122e` (under `src/app/`) |
|---|---|
| Sum base rolls, not current socket totals; legacy maximum-energy armor gets +2 each; Armor 3 base-zero stats get current masterwork contributions or assumed +5 | `loadout-drawer/loadout-utils.ts:284-325,355-413` |
| Infer masterwork setting from energy, threshold 10 even for Tier 5 | `loadout-analyzer/analysis.ts:158-190`; `loadout/armor-upgrade-utils.ts:12-49` |
| Fit separately with as-is and assumed rules, including when all mods fit as-is | `loadout-drawer/loadout-utils.ts:326-349`; `loadout-analyzer/analysis.ts:516-541` |
| Include assigned and unassigned mods; exclude invalid-for-set mods; use all saved subclass override plugs, not current sockets | `loadout/stats.ts:101-187`; `loadout/mod-assignment-utils.ts:346-352` |
| Consolidate duplicate investments with first activation flag; Balanced Tuning/masterworks require assigned base-zero stats; Persistence/Focus affect class-specific stats | `inventory/store/stats-conditional.ts:52-107,155-174,190-210` |
| Optional runtime Fonts: 20/40/50 for 1/2/3+ stacks, including reduced variants; disabled by default | `loadout/stats.ts:25-76,189-210`; `loadout-analyzer/analysis.ts:92` |
| Last duplicate constraint wins; unknown stat hashes are ignored; minima only, with totals clamped to zero for comparison, not output | `loadout-builder/loadout-params.ts:14-42`; `loadout-analyzer/analysis.ts:516-541` |
| Stat upgrades needed only if assumed totals satisfy minima and as-is totals do not; combine with mod upgrade requirement | `loadout-analyzer/analysis.ts:235-239,535-540` |

Totals are hypothetical benefits of planned mods, including unassigned ones;
constraint satisfaction does not imply mod fit or socket application readiness.
There is no stat-material quote, upgrade action or optimizer evaluation. Maximum
constraints stay unevaluated; `better_stats_available` is evaluated by the analyzer
search, see [optimizer findings](optimizer.md).

### R3 mixed-tier rarity cache

The cache key is faithfully rarity-only, with the first item's maximum tier:
`loadout/mod-assignment-utils.ts:180-226`. A lower-tier first item can leave a
later Tier-5 capacity 11 outside the cache. DIM then returns an empty cost array;
its reduction at `:964-989` can produce undefined/NaN costs. Ghostlink preserves
an explicit `energy_cost_original_capacity_out_of_range` or
`energy_cost_target_capacity_out_of_range` unknown instead of inventing Tier-11
prices or silently changing the cache key. A mixed-tier regression tests this
boundary and the Tier-5-first evaluated alternate. No cache-key fix is warranted.
The parent owns ODD checklist updates.
