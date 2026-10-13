//! Pure parts of DIM's loadout analyzer (loadout-analyzer/analysis.ts:250-367, utils.ts:16-60).
use super::*;
use crate::mod_assignment::{ARMOR_BUCKETS, AssignmentMod};
use std::collections::BTreeMap;
use std::time::Duration;

/// DIM `ResolvedStatConstraint`: every armor stat, `ignored` when the build omits it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedStatConstraint {
    pub stat_hash: u32,
    pub min_stat: u16,
    pub max_stat: u16,
    pub ignored: bool,
}

/// DIM loadout-params.ts:14-34 `resolveStatConstraints`: constrained stats first in
/// priority order (the last constraint for a hash wins, the first sets the priority), then
/// ignored stats in game order.
pub fn resolve_stat_constraints(constraints: &[StatConstraint]) -> Vec<ResolvedStatConstraint> {
    let mut resolved: Vec<_> = ARMOR_STATS
        .iter()
        .enumerate()
        .map(|(game_index, hash)| {
            let c = constraints.iter().rev().find(|c| c.stat_hash == *hash);
            let priority = constraints
                .iter()
                .position(|c| c.stat_hash == *hash)
                .unwrap_or(100 + game_index);
            let range = ResolvedStatConstraint {
                stat_hash: *hash,
                min_stat: c.map_or(0, |c| c.min_stat.unwrap_or(0)),
                max_stat: c.map_or(0, |c| c.max_stat.unwrap_or(MAX_STAT as u16)),
                ignored: c.is_none(),
            };
            (priority, range)
        })
        .collect();
    resolved.sort_by_key(|(priority, _)| *priority);
    resolved.into_iter().map(|(_, range)| range).collect()
}

/// loadout-analyzer/utils.ts:16-60: merges the loadout's own (assumed) stats with the saved
/// constraints so a set meeting the merged ranges satisfies both. The flag is true when the
/// merged ranges already imply a strict upgrade, so the search needs no strict-upgrade pass.
pub fn merge_strict_upgrade_stat_constraints(
    existing: Option<&[ResolvedStatConstraint]>,
    parameters: &[ResolvedStatConstraint],
) -> (Vec<DesiredStatRange>, bool) {
    let mut implies_strict_upgrade = false;
    let ranges = parameters
        .iter()
        .map(|constraint| {
            let min_stat = if constraint.ignored {
                0
            } else {
                constraint.min_stat
            };
            let max_stat = if constraint.ignored {
                0
            } else {
                constraint.max_stat
            };
            let existing = existing
                .and_then(|e| e.iter().find(|c| c.stat_hash == constraint.stat_hash))
                .filter(|_| !constraint.ignored);
            let min_stat = match existing {
                Some(existing) => {
                    let existing_min = if existing.ignored {
                        0
                    } else {
                        existing.min_stat
                    };
                    let effective = max_stat.min(existing_min);
                    implies_strict_upgrade |= min_stat > effective;
                    min_stat.max(effective)
                }
                None => min_stat,
            };
            DesiredStatRange {
                stat_hash: constraint.stat_hash,
                min_stat,
                max_stat,
            }
        })
        .collect();
    (ranges, implies_strict_upgrade)
}

/// The analyzer runs with `stop_on_first_set`, so it gets a tight budget (owner-confirmed
/// 2026-10-12): when it runs out, the finding is not evaluated, never guessed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnalyzerBudget {
    pub time_limit: Duration,
    /// Visited combinations, pruned subtrees included (see `SearchBudget`).
    pub max_combos: u64,
}
impl Default for AnalyzerBudget {
    fn default() -> Self {
        Self {
            time_limit: Duration::from_secs(2),
            max_combos: 1_000_000_000,
        }
    }
}

/// What the optimizer-backed findings were computed from; additive output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptimizerAnalysis {
    /// "evaluated" or "not_evaluated"; the default is a run that never happened.
    pub evaluation: &'static str,
    pub reason: Option<&'static str>,
    /// Candidates left per bucket (helmet..class item) after the pre-filter.
    pub candidates: Option<[usize; 5]>,
    pub combos_total: Option<u64>,
    pub combos_visited: Option<u64>,
    pub truncated: bool,
    /// `available_parallelism` handed to the search; a stop-on-first search runs one slice.
    pub threads: Option<usize>,
    /// DIM `betterStatsAvailableFontNote`; set by the caller, which owns the Font mods.
    pub font_note: bool,
}
impl Default for OptimizerAnalysis {
    fn default() -> Self {
        Self::not_evaluated("optimizer_not_run")
    }
}
impl OptimizerAnalysis {
    pub fn not_evaluated(reason: &'static str) -> Self {
        Self {
            evaluation: "not_evaluated",
            reason: Some(reason),
            candidates: None,
            combos_total: None,
            combos_visited: None,
            truncated: false,
            threads: None,
            font_note: false,
        }
    }
}

/// Everything the analyzer search needs, already mapped from projected data.
pub struct AnalyzerInputs<'a> {
    /// All armor of the build's class that the optimizer can use.
    pub candidates: &'a [ArmorCandidate],
    /// Ids of the loadout's five armor pieces.
    pub loadout_ids: &'a [String],
    /// The saved query's verdict per candidate; `None` when the build has no query.
    pub query_matches: Option<&'a dyn Fn(&ArmorCandidate) -> bool>,
    pub locked_exotic: Option<LockedExotic>,
    pub energy_rules: EnergyRules,
    /// `loadoutParameters.setBonuses`, which only the pre-filter reads.
    pub saved_set_bonuses: BTreeMap<u32, u32>,
    /// Set bonus piece counts of the loadout's own armor, which only the search reads.
    pub loadout_set_bonuses: Vec<(u32, u32)>,
    pub bucket_mods: BTreeMap<i64, Vec<AssignmentMod>>,
    pub general_mods: Vec<ProcessMod>,
    pub activity_mods: Vec<ProcessMod>,
    pub mod_stat_totals: [i32; 6],
    pub auto_mod_options: AutoModData,
    pub auto_stat_mods: bool,
    pub constraints: &'a [ResolvedStatConstraint],
    /// The loadout's assumed stats in `ARMOR_STATS` order.
    pub assumed_stats: [i32; 6],
    pub budget: AnalyzerBudget,
    pub threads: usize,
}

/// The two optimizer-backed findings after DIM's suppression rule. `None` is a finding the
/// budget left undecided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyzerOutcome {
    pub items_do_not_match: Option<bool>,
    pub better_stats: Option<bool>,
    pub info: OptimizerAnalysis,
}

/// `filterItems` (item-filter.ts:67-292) over the candidates, with the loadout's own exotic,
/// set bonuses and bucket mods and no pins or exclusions.
pub(super) fn filter_candidates(inputs: &AnalyzerInputs) -> Result<ItemsByBucket, &'static str> {
    let params = FilterParams {
        items: inputs.candidates,
        bucket_mods: inputs.bucket_mods.clone(),
        activity_mod_slots: inputs.activity_mods.iter().filter_map(|m| m.tag).collect(),
        locked_exotic: inputs.locked_exotic.clone(),
        energy_rules: inputs.energy_rules,
        set_bonuses: inputs.saved_set_bonuses.clone(),
        ..Default::default()
    };
    let pass_all = |_: &ArmorCandidate| true;
    let search_filter = inputs.query_matches.unwrap_or(&pass_all);
    let (filtered, _) = filter_items(&params, search_filter)
        .map_err(|FilterError::BucketSocketsUnavailable { .. }| "bucket_mod_sockets_unavailable")?;
    Ok(filtered)
}

/// `mapArmorToProcessItems` for every kept candidate, per bucket.
pub(super) fn process_buckets(
    inputs: &AnalyzerInputs,
    filtered: &ItemsByBucket,
    ranges: &[DesiredStatRange],
) -> [Vec<ProcessItem>; 5] {
    std::array::from_fn(|b| {
        let bucket = ARMOR_BUCKETS[b];
        let mods = inputs
            .bucket_mods
            .get(&bucket)
            .map_or(&[][..], Vec::as_slice);
        filtered[&bucket]
            .iter()
            .flat_map(|item| {
                map_armor_to_process_items(
                    item,
                    &inputs.energy_rules,
                    mods,
                    ranges,
                    inputs.auto_stat_mods,
                )
            })
            .collect()
    })
}

pub(super) fn search_inputs(
    inputs: &AnalyzerInputs,
    buckets: [Vec<ProcessItem>; 5],
    desired_stat_ranges: Vec<DesiredStatRange>,
    strict_upgrades: bool,
    stop_on_first_set: bool,
) -> SearchInputs {
    SearchInputs {
        buckets,
        mod_stat_totals: inputs.mod_stat_totals,
        general_mods: inputs.general_mods.clone(),
        activity_mods: inputs.activity_mods.clone(),
        set_bonuses: inputs.loadout_set_bonuses.clone(),
        required_perks: Vec::new(),
        desired_stat_ranges,
        any_exotic: inputs.locked_exotic == Some(LockedExotic::Any),
        auto_mod_options: inputs.auto_mod_options.clone(),
        auto_stat_mods: inputs.auto_stat_mods,
        strict_upgrades,
        stop_on_first_set,
    }
}

/// loadout-analyzer/analysis.ts:275-367 for a full, unblocked armor set.
pub fn run_analyzer(inputs: &AnalyzerInputs) -> Result<AnalyzerOutcome, &'static str> {
    let filtered = filter_candidates(inputs)?;
    let kept = |id: &String| filtered.values().flatten().any(|c| &c.id == id);
    // analysis.ts:293-307: a loadout piece that exists but did not survive the filter.
    let excluded = inputs.query_matches.is_some()
        && inputs
            .loadout_ids
            .iter()
            .any(|id| inputs.candidates.iter().any(|c| &c.id == id) && !kept(id));

    let existing: Vec<_> = inputs
        .constraints
        .iter()
        .map(|c| ResolvedStatConstraint {
            min_stat: ARMOR_STATS
                .iter()
                .position(|h| *h == c.stat_hash)
                .map_or(0, |i| inputs.assumed_stats[i].clamp(0, MAX_STAT) as u16),
            max_stat: MAX_STAT as u16,
            ..*c
        })
        .collect();
    let (ranges, implies_strict) =
        merge_strict_upgrade_stat_constraints(Some(&existing), inputs.constraints);
    let buckets = process_buckets(inputs, &filtered, &ranges);
    let search_inputs = search_inputs(inputs, buckets, ranges, !implies_strict, true);
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
    // A truncated search that found nothing proves nothing (owner-confirmed 2026-10-12).
    let better_stats =
        (!result.sets.is_empty() || !result.truncated).then_some(!result.sets.is_empty());
    let info = OptimizerAnalysis {
        evaluation: if better_stats.is_some() {
            "evaluated"
        } else {
            "not_evaluated"
        },
        reason: better_stats
            .is_none()
            .then_some("optimizer_budget_exhausted"),
        candidates: Some(std::array::from_fn(|b| search_inputs.buckets[b].len())),
        combos_total: Some(result.combos_total),
        combos_visited: Some(result.combos_visited),
        truncated: result.truncated,
        threads: Some(threads),
        font_note: false,
    };
    // analysis.ts:362-367: better stats replace the "items don't match" finding.
    let items_do_not_match = match (excluded, better_stats) {
        (false, _) | (true, Some(true)) => Some(false),
        (true, Some(false)) => Some(true),
        (true, None) => None,
    };
    Ok(AnalyzerOutcome {
        items_do_not_match,
        better_stats,
        info,
    })
}
