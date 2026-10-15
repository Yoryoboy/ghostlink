# Optimizer-backed build findings (BE-008d)

`ghostlink build check` and MCP `analyze_build` run DIM's loadout analyzer search
for two findings: `better_stats_available` and `items_do_not_match_search_query`.
Evidence is DIM 29c122e `loadout-analyzer/analysis.ts:250-367` and
`loadout-analyzer/utils.ts:16-60`. The analyzer is read-only: it never changes build
resolution, the apply plan or any inventory (`build apply` does not run it).

## When it runs

* Only for a full 5-piece resolved armor set with evaluated stats.
* Skipped (both findings `not_evaluated`, reason `analysis_blocked_by_finding`) when
  `not_a_full_armor_set`, `mods_dont_fit` or `does_not_respect_exotic` is present;
  `blocking_finding_not_evaluated` when one of them is unknown (types.ts:72-76).
* Search capability gaps keep their reason (`search_engine_capability_unavailable`,
  `query_context_unavailable`). An invalid query is DIM's pass-all stub.

## What it does

1. Any general stat mod forces auto stat mods on. Artifice and tuning mods are dropped,
   and so are general mods once stat mods are automatic. The rest is split into bucket,
   activity (Maverick and the raid slots included) and general `ProcessMod`s against the
   loadout armor's plugged mod sockets; unknown categories and misfits are unassigned
   and ignored, as DIM does.
2. `filter_items` runs over all armor of the target character's class in the Vault and
   on characters (not the Postmaster; DIM's postmaster items are not armor-bucket items)
   that has energy and all six stats, with the loadout's exotic, the saved query and set
   bonuses, and no pins or exclusions. Its strictly-worse dedupe carries energy,
   activity-slot and set-bonus columns.
3. `items_do_not_match_search_query`: a loadout piece that exists but did not survive.
4. `better_stats_available`: `mergeStrictUpgradeStatConstraints` (loadout stats as
   minimums), `stop_on_first`, strict upgrades unless the merge already implies one.
   Better stats replace the items finding. The Font note is `optimizer.font_note`.

## Budget

2 s and 1,000,000,000 combinations per build. When the budget ends without a set the
findings are `not_evaluated` with `optimizer_budget_exhausted`, never guessed (owner
decision 2026-10-12). `threads` is `available_parallelism`; a stop-on-first search runs
one slice (BE-008c).

## Output

Additive `optimizer` object in `build check --json` / `analyze_build`: `evaluation`,
`reason`, `candidates_per_bucket`, `combinations_total`, `combinations_visited`,
`truncated`, `threads`, `font_note`. Text output prints an `Optimizer:` line and the
Font note.

## Typed unknowns and simplifications

* Required armor perks (`parameters.perks`) are `armor_perk_requirements_unsupported`:
  DIM finds the intrinsic perk through a plug category identifier Ghostlink does not
  store.
* Missing stats, plug definitions, plug sets, raw reusable plugs or tuning membership
  make the run `not_evaluated` with that reason instead of dropping candidates.
* Assumed loadout stats below zero count as zero in the merge.
* `include_runtime_stat_benefits` and `auto_stat_mods` default to off when unsaved.

# Substitutes (BE-008e)

`ghostlink build substitutes` and MCP `build_substitutes` run the same search as DIM's
Loadout Optimizer opened from the build (`loadout-builder-reducer.ts:148-232`) and list the
best sets. They reuse the analyzer's adapter (same candidates, pre-filter, locked mods, auto
stat mods and tuning variants) and read-only rules: `build check` and `build apply` never
see them (D3).

| | Analyzer (`build check`) | Substitutes |
|---|---|---|
| Stat ranges | loadout stats as minimums (`mergeStrictUpgradeStatConstraints`) | the saved minimums **and maximums** (`loadout-params.ts:14-34`) |
| Search | stop on first set, strict upgrades | full, no strict-upgrade filter |
| Budget | 2 s, `not_evaluated` when it runs out | 10 s, results flagged `truncated` |
| Output | two findings | up to `--limit` sets (default 10, max 200 = DIM's tracker size) |

* Order is DIM's `isWorse` (`set-tracker.ts:21-32`): enabled stat total, stat mix in priority
  order, total, power; remaining ties break on the item ids (then the tuning mod), so the list
  is the same for any thread count or candidate order.
* Stats are in the build's priority order (constrained stats first). Unsaved constraints
  mean every stat is ignored, as in the analyzer.
* `stats[].total` includes locked mods, automatic stat mods and tuning; `armor_only` is the
  armor (with the chosen exotic tuning). `tuning_mods` are the tuning mods the set uses, apart
  from `stat_mods`.
* The same typed unknowns as the analyzer apply (`not_evaluated` with a `reason`, empty
  `sets`), and a blocking finding (`mods_dont_fit`, `not_a_full_armor_set`,
  `does_not_respect_exotic`) stops it too. A budget stop is not an unknown: it returns the
  partial list with `truncated: true`.

Deliberate differences from DIM: DIM pins the loadout's equipped armor when it opens a loadout
(`loadout-builder-reducer.ts:185-197`, with a TODO to leave the items free), which would only
list the loadout itself; substitutes leave every piece free and mark the build's own pieces.
DIM's saved per-class stat order and default constraints are not read. The deterministic id
tie-break and the 10 s budget are Ghostlink additions (owner-confirmed 2026-10-12). The Font
note is not repeated here.
