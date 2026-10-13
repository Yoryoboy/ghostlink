//! Ranked armor sets for a stored build, as DIM's Loadout Optimizer lists them when opened
//! from that loadout (loadout-builder-reducer.ts:148-232; the set list is `process.ts`'s
//! 200-set tracker). Unlike the analyzer's strict-upgrade merge, the saved stat constraints
//! are the search ranges as they are, minimum and maximum (loadout-params.ts:14-34).
use super::analyzer::{filter_candidates, process_buckets, search_inputs};
use super::*;
use std::collections::BTreeSet;
use std::time::Duration;

/// Owner-confirmed 2026-10-12: `--limit` defaults to 10 and is capped at DIM's 200 sets.
pub const DEFAULT_SUBSTITUTES: usize = 10;
pub const MAX_SUBSTITUTES: usize = RETURNED_ARMOR_SETS;
/// An interactive CLI call waits at most this long before the result is flagged truncated.
pub const SUBSTITUTES_TIME_LIMIT: Duration = Duration::from_secs(10);

impl AnalyzerBudget {
    /// The substitutes search is the point of the command, so it gets the longer budget.
    pub const fn substitutes() -> Self {
        Self {
            time_limit: SUBSTITUTES_TIME_LIMIT,
            max_combos: 1_000_000_000,
        }
    }
}

/// One ranked set. Stat arrays follow `SubstitutesOutcome::stat_order`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstituteSet {
    /// Item ids in bucket order (helmet, gauntlets, chest, legs, class item).
    pub armor: [String; 5],
    /// Totals with locked mods and auto stat mods, Armor 3.0 tuning included.
    pub stats: [i32; 6],
    /// The armor alone (the chosen tuning mod included).
    pub armor_stats: [i32; 6],
    /// Auto stat mods the search picked.
    pub stat_mods: Vec<u32>,
    /// Tuning mods the set uses, preset ones and the exotic's chosen one.
    pub tuning_mods: Vec<u32>,
    pub power: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubstitutesOutcome {
    /// Best first; at most the requested limit.
    pub sets: Vec<SubstituteSet>,
    /// The stat hash of each stat array slot: the build's priority order.
    pub stat_order: [u32; 6],
    pub info: OptimizerAnalysis,
}

/// Full search (no stop-on-first, no strict-upgrade filter) with the saved ranges.
pub fn run_substitutes(
    inputs: &AnalyzerInputs,
    limit: usize,
) -> Result<SubstitutesOutcome, &'static str> {
    let filtered = filter_candidates(inputs)?;
    let ranges: Vec<_> = inputs
        .constraints
        .iter()
        .map(|c| DesiredStatRange {
            stat_hash: c.stat_hash,
            min_stat: if c.ignored { 0 } else { c.min_stat },
            max_stat: if c.ignored { 0 } else { c.max_stat },
        })
        .collect();
    let stat_order = std::array::from_fn(|i| ranges[i].stat_hash);
    let buckets = process_buckets(inputs, &filtered, &ranges);
    let search_inputs = search_inputs(inputs, buckets, ranges, false, false);
    let threads = inputs.threads.max(1);
    let result = search(
        &search_inputs,
        &SearchOptions {
            threads,
            budget: SearchBudget {
                max_combos: Some(inputs.budget.max_combos),
                time_limit: Some(inputs.budget.time_limit),
            },
        },
    )
    .map_err(|_| "stat_ranges_invalid")?;
    let tuning: BTreeSet<u32> = inputs
        .candidates
        .iter()
        .flat_map(|c| c.tuning.iter().flatten().map(|p| p.hash))
        .collect();
    let sets = result
        .sets
        .into_iter()
        .take(limit.min(MAX_SUBSTITUTES))
        .map(|set| {
            let (tuning_mods, stat_mods) =
                set.data.stat_mods.iter().partition(|m| tuning.contains(m));
            SubstituteSet {
                armor: set.data.armor,
                stats: set.data.stats,
                armor_stats: set.data.armor_stats,
                stat_mods,
                tuning_mods,
                power: set.power,
            }
        })
        .collect();
    Ok(SubstitutesOutcome {
        sets,
        stat_order,
        info: OptimizerAnalysis {
            evaluation: "evaluated",
            reason: None,
            candidates: Some(std::array::from_fn(|b| search_inputs.buckets[b].len())),
            combos_total: Some(result.combos_total),
            combos_visited: Some(result.combos_visited),
            truncated: result.truncated,
            threads: Some(threads),
            font_note: false,
        },
    })
}
