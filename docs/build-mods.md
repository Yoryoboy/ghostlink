# Build mod fitting: bounded DIM port

`build check` and MCP `analyze_build` share an additive `mod_fit` object:

- `evaluation` and nullable `reason` qualify the whole assignment result.
- `invalid_mods`: requested bucket-specific mods which cannot fit any active
  ArmorMods socket in the entire inventory. These are excluded before fitting,
  as DIM does; invalid mods do not themselves imply `mods_dont_fit`.
- `unassigned_mods`: valid mods that still cannot fit the build's resolved armor
  after the analyzer's allowed energy upgrades. Includes invalid-for-this-set
  mods; duplicate requests remain duplicate entries.
- `needs_upgrades_for_mods`: true only when mods cannot fit as-is but can fit
  with DIM's assumed energy rules. This is not an upgrade action, material quote,
  or claim about Armor 3.0 stat-masterwork tiers.

Null lists are unknown, never empty. A missing full resolved set, raw membership,
Manifest data, energy or rarity retains `not_evaluated` with a reason. General,
activity, artifice and tuning mods are deliberately outside this first slice.
Both halves of every pinned seasonal cost pair remain unevaluated: Ghostlink
cannot yet reproduce DIM's unlocked-cost-variant resolution. Fashion
`modsByBucket` is not mixed into `parameters.mods`.

The global `needs_armor_upgrades` finding is true when mod upgrades are proven.
When mods need no upgrades but positive minimum stat constraints exist, it stays
unknown until BE-006c implements upgrades for stats. There are no invented
findings or percentage scores. Existing energy used is not subtracted from
capacity: assignment plans replace the armor's mods rather than stack new mods
on top of the current ones.

## DIM evidence

Paths are under DIM commit `29c122e`, `src/app/` unless noted.

| Decision | File:line evidence |
|---|---|
| Eager membership against all inventory, then re-categorize valid mods against the resolved set | `loadout/mod-assignment-utils.ts:84-151,346-370`; `loadout-analyzer/analysis.ts:121-135,442-478` |
| Five numeric plug-category to armor-bucket mappings | `loadout/mod-utils.ts:15-21`; `src/data/d2/generated-enums.ts:91-96` |
| Only inserted active ArmorMods sockets; categorization ignores visibility/enabled, assignment requires visibility and excludes type 1843767421 | `loadout/mod-assignment-utils.ts:99-112,576-598` |
| Stable socket-set-size sort (999 without set), then stable fewest-compatible-sockets mod sort; first fitting socket, energy check, exclusion check, consume only on success | `loadout/mod-assignment-utils.ts:596-613,623-675` |
| Membership is empty/default OR resolved Manifest plug set OR raw instance membership; no whitelist-only substitute or flag filtering | `utils/socket-utils.ts:225-237`; `inventory/store/sockets.ts:584-645,677-687,792-825` |
| Manifest set size counts resolved pluggable entries, including cannot-roll entries and duplicates | `inventory/store/sockets.ts:798-825` |
| Finisher/fastball groups, including duplicate-mod exclusion | `loadout/mod-utils.ts:157-159`; `src/data/d2/mutually-exclusive-mods.json:2-13` |
| Analyzer as-is vs upgrade fitting; only full resolved sets are assigned | `loadout-analyzer/analysis.ts:454-478` |
| Infer assumption using capacity <10, not stat masterwork tier or Tier-5 maximum | `loadout-analyzer/analysis.ts:158-190` |
| As-is floor 1; upgraded floor 9; All/ArtificeExotic or nonexotic Legendary uses maximum 10/11 for gear tier 5 | `loadout-builder/types.ts:165-180`; `loadout/armor-upgrade-utils.ts:12-22,43-49`; `search/d2-known-values.ts:29-30` |
| Preserve requested order/duplicates, do not use cosmetics; seasonal variants require unlock resolution | `loadout-drawer/loadout-utils.ts:200-218,901-936`; `loadout/mod-utils.ts:120-132`; `src/data/d2/reduced-cost-mod-mappings.ts:1-117` |

Ghostlink's missing-data handling is intentionally more conservative than DIM's
fallbacks. This does not authorize approximating its assignment ordering.
Pure deterministic fixtures cover greedy ordering, duplicates, exclusion groups,
energy failure without socket consumption, Tier-5 capacity, invisible/disabled
sockets, unknown versus observed-empty membership, seasonal pairs and cannot-roll
Manifest membership. The shared output reaches both CLI and MCP without new
transport-specific wiring, migrations or component requests.

## Split proposal: BE-006b2 (not completed)

Port the remainder as a separate work unit, preserving this completed slice:

1. General/activity Heap permutations, specialty socket metadata and energy
   validation (`mod-permutations.ts:3-64`, `mod-assignment-utils.ts:437-528,952-962,1022-1036`).
2. Greedy artifice and slot-ordered tuning assignment, including the exotic
   candidate rule (`mod-assignment-utils.ts:359-420`).
3. Material-cost ranking and assignment-change tie breaking, with actual
   insertion-material requirements and energy-upgrade plugs
   (`mod-assignment-utils.ts:162-265,481-528,964-989`). Ghostlink does not yet
   project all cost inputs; do not replace them with guessed costs.
4. Legacy and seasonal mod resolution, deprecated placeholders and complete
   InvalidMods/UsesSeasonalMods coverage (`loadout-utils.ts:880-936`,
   `analysis.ts:480-501`).

The full port plus dependencies exceeds the roughly 700-production-line budget.
The ODD tracker is outside the allowed edit surfaces and was not changed. This
proposal does not mark BE-006b as fully complete; BE-006c remains responsible for
loadout stat constraints, fragments and upgrades for stats.
