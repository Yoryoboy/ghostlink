# Build mod fitting: bounded DIM port

`build check` and MCP `analyze_build` share an additive `mod_fit` object:

- `evaluation` and nullable `reason` qualify the whole assignment result.
- `invalid_mods`: resolved requested mods which cannot fit any active
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
activity, artifice and tuning assignment remain outside this slice.
Seasonal cost variants now resolve before categorization, using DIM's static
110-pair table at commit `29c122e`. Enabled account-wide plus target-character
plug-set entries select reduced variants; `canInsert` and other characters do not.
If neither variant is unlocked, resolution falls back to the normal variant,
including when plug-set responses are absent, matching DIM's empty-set default.
The ornament and Harmonic Resistance unlock exceptions do not affect any cost pair.
All twelve legacy stat-mod hashes migrate before cost resolution. Missing or
non-pluggable definitions become DIM's deprecated placeholder (`444600262`);
a missing/non-pluggable placeholder retains
`deprecated_mod_placeholder_definition_unavailable`. Reported hashes are resolved
hashes, while saved/exported build hashes remain unchanged. Fashion
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
| Preserve requested order/duplicates and saved hashes; migrate legacy stat mods, resolve available seasonal variants, then use deprecated placeholder for non-pluggable definitions | `loadout-drawer/loadout-utils.ts:880-936`; `loadout/mod-utils.ts:120-132`; `src/data/d2/reduced-cost-mod-mappings.ts:1-117` |
| Enabled profile/target-character unlocks; missing responses default to empty | `inventory/selectors.ts:333-346`; `records/plugset-helpers.ts:20-40` |
| R3-full-set-membership is not a deviation: DIM gates fitting on length === 5, not distinct bucket membership | `loadout-analyzer/analysis.ts:454`; regression fixture covers repeated buckets |

Ghostlink's missing-data handling is intentionally more conservative than DIM's
fallbacks. This does not authorize approximating its assignment ordering.
Pure deterministic fixtures cover greedy ordering, duplicates, exclusion groups,
energy failure without socket consumption, Tier-5 capacity, invisible/disabled
sockets, unknown versus observed-empty membership, seasonal unlock/fallback choices,
legacy migrations, deprecated placeholders and cannot-roll Manifest membership. The shared output reaches both CLI and MCP without new
transport-specific wiring, migrations or component requests.

## BE-006b2 resolution slice and split remainder

Port the remainder as a separate work unit, preserving this completed slice:

1. General/activity Heap permutations, specialty socket metadata and energy
   validation (`mod-permutations.ts:3-64`, `mod-assignment-utils.ts:437-528,952-962,1022-1036`).
2. Greedy artifice and slot-ordered tuning assignment, including the exotic
   candidate rule (`mod-assignment-utils.ts:359-420`).
3. Material-cost ranking and assignment-change tie breaking, with actual
   insertion-material requirements and energy-upgrade plugs
   (`mod-assignment-utils.ts:162-265,481-528,964-989`). Ghostlink does not yet
   project all cost inputs; do not replace them with guessed costs.
4. Complete UsesSeasonalMods coverage (`analysis.ts:449-501`), including expensive-
   variant re-fitting. Resolution and deprecated-placeholder InvalidMods coverage
   are implemented in this slice; seasonal dependence is still `not_evaluated`.

### BE-006b2b data boundary: completed prerequisites

Migration 32 projects plug `insertionMaterialRequirementHash`,
`energyCapacity.energyType` and `plugAvailability`, alongside the existing raw
`energyCapacity.capacityValue` (currently named `aspect_capacity` in core).
Absent fields stay unknown; zero remains zero. DIM's upgrade-plug selection needs
these fields (`inventory/store/energy.ts:18-49`).
`DestinyMaterialRequirementSetDefinition` is now fetched and projected through the
shared armor-catalog port: ordered `materials[].itemHash/count`, including zero
counts, duplicates and observed-empty sets. Other material flags do not participate
in DIM's ranking (`loadout/mod-assignment-utils.ts:162-265,964-989`). DIM ranks five
fixed material item hashes; no additional material display catalog is needed.

Parent-measured live Manifest `244213.26.06.29.2000-1-bnet.65864`: **116,945 bytes,
332 requirement sets, maximum 4 materials per set**. Safety ceilings allow seasonal
headroom: 16 MiB decompressed, 10,000 records (including skipped redacted records),
256 materials per set. One additional world-table request: 12 → 13. SQLite stores
`definition_material_inputs`, `material_requirement_sets` and `material_requirements`:
STRICT, bounded integers, cascading children, indexed single-hash queries, guarded
downgrade. The upgrade invalidates only Manifest freshness, forcing a same-version
refresh once. Legacy schemas reject unrepresentable publication; no inferred
backfill. Fake/SQLite parity includes every new field in revision identity.

R3-category-coverage: **no deviation**. The four aspect and four fragment hashes
match `utils/socket-utils.ts:278-290` and `src/data/d2/generated-enums.ts:1604-1619`.
A regression exercises all sixteen aspect/fragment category pairs.

This is a data-only boundary, not material-cost calculation or assignment.
Request-local energy-upgrade plug and material assembly belongs with the remaining
ranking port. General/activity permutations, specialty metadata, artifice/tuning,
assignment-change tie breaking and expensive-variant re-fitting remain deferred;
matching `not_evaluated` reasons are intentionally retained. Port them together
without guessed prices before BE-006c2 stat aggregation.

The full port plus dependencies exceeds the roughly 700-production-line budget.
The ODD tracker is outside the allowed edit surfaces and was not changed. This
proposal does not mark BE-006b as fully complete; BE-006c remains responsible for
loadout stat constraints and upgrades for stats. Fragment findings are implemented
in the bounded BE-006c slice below.

## BE-006c: fragment findings and stat dependency

The existing findings envelope now evaluates `empty_fragment_slots` and
`too_many_fragments` for CLI text/JSON and MCP. No migrations or extra provider
requests: build checking additionally loads saved socket-override definitions.

| Decision | DIM evidence (commit `29c122e`, under `src/app/`) |
|---|---|
| Select the first resolved subclass, without an equip gate | `loadout-analyzer/analysis.ts:90-98` |
| Compare truthy override count against capacity; smaller = empty, larger = too many | `loadout-analyzer/analysis.ts:420-438` |
| Use only the first aspect category and saved aspect overrides; no current-aspect fallback | `loadout-drawer/loadout-utils.ts:745-770` |
| Sum capacity, defaulting resolved definitions without capacity to zero | `loadout-drawer/loadout-utils.ts:946-954` |
| Count existing real fragment sockets across every fragment category; no enabled/visible gate | `utils/socket-utils.ts:77-85,100-112,278-290` |

No resolved subclass means neither finding, matching DIM. Unknown socket/category
projections or selected aspect definitions remain `not_evaluated`, respectively
`subclass_socket_categories_unavailable` / `selected_aspect_definition_unavailable`.
A defensive checked sum reports `fragment_capacity_overflow`, never wraps.
Nonzero empty-plug hashes count: the analyzer uses truthiness, not capture's
empty-plug classification. This is not apply-time validity or unlocked evidence.

### Split proposal: stat constraints and upgrades

Stat aggregation cannot faithfully bypass the unfinished BE-006b2b assignment:
`loadout-drawer/loadout-utils.ts:284-358` calls `fitMostMods`, then passes item
assignments into `getTotalModStatChanges`. `loadout/stats.ts:101-214` evaluates
conditional contributions using each assigned armor item (unassigned mods have
no item), plus subclass overrides and optional runtime Font benefits. Artifact
socket overrides are not a separate contribution in this analyzer path.
Implementing the complete assignment engine (over 1,000 DIM lines, plus material
catalog persistence) and stat rules together would exceed the roughly 700-new-
production-line boundary. Preserve this fragment slice and complete BE-006b2b
before a separate BE-006c stat slice; do not claim current totals as loadout totals.

That slice must follow `analysis.ts:158-190,228-265,516-561`: compare as-is and
inferred-masterwork totals, test only nonignored minima with negative totals
clamped to zero, and set `NeedsArmorUpgrades` when upgrades alone meet the minima.
**The non-optimizer analyzer ignores maxima**; maxima participate only in the
optimizer path. `loadout-utils.ts:365-417` uses base stats and masterwork rules,
not current component totals. Preserve item-conditional tuning fidelity through
assignment. Better-stat findings stay unknown with the BE-008 optimizer reason.
Current stat and stat-upgrade findings remain explicitly `not_evaluated`; this
slice does not mark the full BE-006c task complete.
