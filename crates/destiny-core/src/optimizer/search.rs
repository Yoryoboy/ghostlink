//! DIM process-worker/process.ts:72-1200 (`process`): the five-bucket search with
//! subtree pruning. DIM spells the helmet/gauntlet/chest/leg loops out four times;
//! `Searcher::descend` runs the same checks in the same order once per level, so the
//! relation (which sets are evaluated, retained and reported) is unchanged.
//! One `Searcher` belongs to one thread: its `LoSessionInfo` memos are not shared.
//! Not ported (speed only, DIM says "correctness never depends on the seed"):
//! `seedExactStatRanges` (process.ts:1411+), which pre-seeds the displayed ranges.
use super::driver::Progress;
use super::set_tracker::{
    HeapEntry, HeapSetTracker, RETURNED_ARMOR_SETS, TieBreak, encode_stat_mix,
};
use super::*;

/// process.ts:40-65 `ProcessInputs`. Armor stats (items, `mod_stat_totals`) are in
/// `ARMOR_STATS` order; `desired_stat_ranges` sets the session's stat order.
#[derive(Clone, Debug)]
pub struct SearchInputs {
    /// Helmet, gauntlets, chest, legs, class item (`ArmorBucketHashes`).
    pub buckets: [Vec<ProcessItem>; 5],
    /// Selected mods' total contribution to each stat.
    pub mod_stat_totals: [i32; 6],
    pub general_mods: Vec<ProcessMod>,
    pub activity_mods: Vec<ProcessMod>,
    /// Set bonus hash and the number of pieces wanted from it.
    pub set_bonuses: Vec<(u32, u32)>,
    /// Armor perk hash and how many items of the set must have it.
    pub required_perks: Vec<(u32, u32)>,
    pub desired_stat_ranges: Vec<DesiredStatRange>,
    pub any_exotic: bool,
    pub auto_mod_options: AutoModData,
    pub auto_stat_mods: bool,
    /// Only sets where at least one stat exceeds its minimum are returned.
    pub strict_upgrades: bool,
    /// Stop after the first set that fits (and is a strict upgrade if requested).
    pub stop_on_first_set: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchError {
    /// `desired_stat_ranges` must hold each armor stat exactly once.
    InvalidStatRanges,
}

/// The parts of DIM's `ProcessArmorSet` the tracker does not order on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmorSetData {
    /// Item ids in bucket order.
    pub armor: [String; 5],
    /// Overall stats (set stats plus mods and auto stat mods) in the session stat order.
    pub stats: [i32; 6],
    /// Stats from the armor alone, including the chosen exotic tuning mod.
    pub armor_stats: [i32; 6],
    pub stat_mods: Vec<u32>,
}
pub type ArmorSet = HeapEntry<ArmorSetData>;

/// process-worker/types.ts:110-146.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SkipReasons {
    pub no_exotic: u64,
    pub double_exotic: u64,
    pub skipped_low_tier: u64,
    pub insufficient_set_bonus: u64,
    pub insufficient_perks: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProcessStatistics {
    pub num_processed: u64,
    pub num_valid_sets: u64,
    pub skip_reasons: SkipReasons,
    pub lower_bounds_exceeded: RejectionRate,
    pub mods_statistics: ModAssignmentStatistics,
}
impl ProcessStatistics {
    /// process-wrapper.ts:319-345 `combineProcessInfo`.
    pub(super) fn add(&mut self, other: &Self) {
        self.num_processed += other.num_processed;
        self.num_valid_sets += other.num_valid_sets;
        let (a, b) = (&mut self.skip_reasons, &other.skip_reasons);
        a.no_exotic += b.no_exotic;
        a.double_exotic += b.double_exotic;
        a.skipped_low_tier += b.skipped_low_tier;
        a.insufficient_set_bonus += b.insufficient_set_bonus;
        a.insufficient_perks += b.insufficient_perks;
        let rate = |a: &mut RejectionRate, b: &RejectionRate| {
            a.times_checked += b.times_checked;
            a.times_failed += b.times_failed;
        };
        rate(
            &mut self.lower_bounds_exceeded,
            &other.lower_bounds_exceeded,
        );
        let (a, b) = (&mut self.mods_statistics, &other.mods_statistics);
        rate(&mut a.early_mods_check, &b.early_mods_check);
        rate(&mut a.auto_mods_pick, &b.auto_mods_pick);
        a.final_assignment.mod_assignment_attempted += b.final_assignment.mod_assignment_attempted;
        a.final_assignment.mods_assignment_failed += b.final_assignment.mods_assignment_failed;
        a.final_assignment.auto_mods_assignment_failed +=
            b.final_assignment.auto_mods_assignment_failed;
    }
}

/// One item with its deterministic rank among every (id, tuning mod) in the input.
pub(super) struct Entry {
    pub item: ProcessItem,
    pub rank: u32,
}

/// What one thread's `Searcher` hands back (process.ts:1200+ `ProcessResult`).
pub(super) struct SliceOutput {
    pub sets: Vec<ArmorSet>,
    pub stat_ranges: [MinMaxStat; 6],
    pub statistics: ProcessStatistics,
}

struct TuningEntry {
    /// Mod hash and tuned stats minus base stats, in stat order.
    variants: Vec<(u32, [i32; 6])>,
    min_deltas: [i32; 6],
    max_deltas: [i32; 6],
    max_net_gain: i32,
}

/// process.ts:1221+ `BucketSoA`, as plain vectors.
struct Bucket {
    entries: Vec<Entry>,
    exotic: Vec<i32>,
    artifice: Vec<i32>,
    wildcard: Vec<i32>,
    stats: Vec<[i32; 6]>,
    set_idx: Vec<i32>,
    perks: Vec<i32>,
    has_exotic: bool,
    max_perks: Vec<i32>,
    max_set_contrib: i32,
    tuning: Vec<Option<TuningEntry>>,
    max_stats: [i32; 6],
    min_stats: [i32; 6],
    has_artifice: bool,
    max_total: i32,
}

/// What the buckets after a level can still contribute, for subtree pruning.
struct Suffix {
    combos: u64,
    exotic: bool,
    max_perks: Vec<i32>,
    max_set_contrib: i32,
    max_stats: [i32; 6],
    min_stats: [i32; 6],
    max_total: i32,
    artifice: i32,
}

/// Everything immutable a search over one slice reads.
pub(super) struct Context {
    buckets: [Bucket; 5],
    suffix: [Suffix; 4],
    desired: Vec<DesiredStatRange>,
    max_constraints: [i32; 6],
    mod_stats: [i32; 6],
    perk_counts: Vec<i32>,
    set_counts: Vec<i32>,
    any_exotic: bool,
    has_mods: bool,
    strict_upgrades: bool,
    stop_on_first_set: bool,
    general_mods_bonus: i32,
    max_tuning_delta: [i32; 6],
    min_tuning_delta: [i32; 6],
    max_tuning_net_gain: i32,
}

fn add(a: &[i32; 6], b: &[i32; 6]) -> [i32; 6] {
    std::array::from_fn(|s| a[s] + b[s])
}

impl Context {
    /// process.ts:98-141 and :187-290: sorts buckets high-stat first and derives the
    /// per-level bounds. `stat_position[i]` is the `ARMOR_STATS` index of session stat `i`.
    pub(super) fn new(
        inputs: &SearchInputs,
        buckets: [Vec<Entry>; 5],
        stat_position: &[usize; 6],
        session: &LoSessionInfo,
    ) -> Self {
        let desired = inputs.desired_stat_ranges.clone();
        let max_constraints: [i32; 6] = std::array::from_fn(|i| i32::from(desired[i].max_stat));
        let project =
            |stats: &[i32; 6]| -> [i32; 6] { std::array::from_fn(|i| stats[stat_position[i]]) };
        let enabled_total = |stats: &[i32; 6]| -> i32 {
            (0..6)
                .filter(|i| max_constraints[*i] > 0)
                .map(|i| stats[i])
                .sum()
        };
        let num_perks = inputs.required_perks.len();
        let buckets: Vec<Bucket> = buckets
            .into_iter()
            .map(|mut entries| {
                // Visit high-stat items first so the heap fills with good sets early.
                entries.sort_by_key(|e| std::cmp::Reverse(enabled_total(&project(&e.item.stats))));
                let n = entries.len();
                let mut bucket = Bucket {
                    exotic: Vec::with_capacity(n),
                    artifice: Vec::with_capacity(n),
                    wildcard: Vec::with_capacity(n),
                    stats: Vec::with_capacity(n),
                    set_idx: Vec::with_capacity(n),
                    perks: vec![0; n * num_perks],
                    has_exotic: false,
                    max_perks: vec![0; num_perks],
                    max_set_contrib: 0,
                    tuning: Vec::with_capacity(n),
                    max_stats: [0; 6],
                    min_stats: [MAX_STAT; 6],
                    has_artifice: false,
                    max_total: 0,
                    entries: Vec::new(),
                };
                for (i, entry) in entries.iter().enumerate() {
                    let item = &entry.item;
                    let stats = project(&item.stats);
                    bucket.exotic.push(i32::from(item.is_exotic));
                    bucket.artifice.push(i32::from(item.is_artifice));
                    bucket
                        .wildcard
                        .push(i32::from(item.has_set_bonus_mod_socket));
                    bucket.set_idx.push(item.set_bonus.map_or(-1, |hash| {
                        inputs
                            .set_bonuses
                            .iter()
                            .position(|(h, _)| *h == hash)
                            .map_or(-1, |p| p as i32)
                    }));
                    bucket.has_exotic |= item.is_exotic;
                    let contrib = i32::from(bucket.set_idx[i] >= 0) + bucket.wildcard[i];
                    bucket.max_set_contrib = bucket.max_set_contrib.max(contrib);
                    for (p, (hash, _)) in inputs.required_perks.iter().enumerate() {
                        if item
                            .intrinsic_perks
                            .as_ref()
                            .is_some_and(|perks| perks.contains(hash))
                        {
                            bucket.perks[i * num_perks + p] = 1;
                            bucket.max_perks[p] = 1;
                        }
                    }
                    bucket.tuning.push(
                        item.tuning_variants
                            .as_ref()
                            .filter(|v| !v.is_empty())
                            .map(|variants| {
                                let variants: Vec<_> = variants
                                    .iter()
                                    .map(|v| {
                                        let tuned = project(&v.stats);
                                        (v.mod_hash, std::array::from_fn(|s| tuned[s] - stats[s]))
                                    })
                                    .collect();
                                let fold = |pick: fn(i32, i32) -> i32| -> [i32; 6] {
                                    std::array::from_fn(|s| {
                                        variants.iter().map(|v| v.1[s]).reduce(pick).unwrap_or(0)
                                    })
                                };
                                let max_net_gain = variants
                                    .iter()
                                    .map(|(_, d)| {
                                        (0..6)
                                            .filter(|s| desired[*s].max_stat > 0 && d[*s] > 0)
                                            .map(|s| d[s])
                                            .sum::<i32>()
                                    })
                                    .max()
                                    .unwrap_or(0)
                                    .max(0);
                                TuningEntry {
                                    min_deltas: fold(i32::min),
                                    max_deltas: fold(i32::max),
                                    max_net_gain,
                                    variants,
                                }
                            }),
                    );
                    bucket.max_stats = std::array::from_fn(|s| bucket.max_stats[s].max(stats[s]));
                    bucket.min_stats = std::array::from_fn(|s| bucket.min_stats[s].min(stats[s]));
                    bucket.max_total = bucket.max_total.max(enabled_total(&stats));
                    bucket.has_artifice |= item.is_artifice;
                    bucket.stats.push(stats);
                }
                bucket.entries = entries;
                bucket
            })
            .collect();
        let buckets: [Bucket; 5] = buckets.try_into().unwrap_or_else(|_| unreachable!());
        // Suffix bounds: the summed bounds of every bucket after each level.
        let lens: Vec<u64> = buckets.iter().map(|b| b.entries.len() as u64).collect();
        let suffix: [Suffix; 4] = std::array::from_fn(|level| {
            let after = &buckets[level + 1..];
            Suffix {
                combos: lens[level + 1..].iter().product(),
                exotic: after.iter().any(|b| b.has_exotic),
                max_perks: (0..num_perks)
                    .map(|p| after.iter().map(|b| b.max_perks[p]).sum())
                    .collect(),
                max_set_contrib: after.iter().map(|b| b.max_set_contrib).sum(),
                max_stats: after.iter().fold([0; 6], |acc, b| add(&acc, &b.max_stats)),
                min_stats: after.iter().fold([0; 6], |acc, b| add(&acc, &b.min_stats)),
                max_total: after.iter().map(|b| b.max_total).sum(),
                artifice: after.iter().map(|b| i32::from(b.has_artifice)).sum(),
            }
        });
        // process.ts:1356-1385: global headroom for exotic tuning (a set has at most one
        // exotic, so the best/worst delta of any exotic variant bounds every set).
        let (mut max_tuning_delta, mut min_tuning_delta, mut max_tuning_net_gain) =
            ([0; 6], [0; 6], 0);
        for entry in buckets.iter().flat_map(|b| b.tuning.iter().flatten()) {
            for s in 0..6 {
                max_tuning_delta[s] = max_tuning_delta[s].max(entry.max_deltas[s]);
                min_tuning_delta[s] = min_tuning_delta[s].min(entry.min_deltas[s]);
            }
            max_tuning_net_gain = max_tuning_net_gain.max(entry.max_net_gain);
        }
        Self {
            buckets,
            suffix,
            max_constraints,
            mod_stats: project(&inputs.mod_stat_totals),
            perk_counts: inputs
                .required_perks
                .iter()
                .map(|(_, c)| *c as i32)
                .collect(),
            set_counts: inputs.set_bonuses.iter().map(|(_, c)| *c as i32).collect(),
            any_exotic: inputs.any_exotic,
            has_mods: !(inputs.general_mods.is_empty() && inputs.activity_mods.is_empty()),
            strict_upgrades: inputs.strict_upgrades,
            stop_on_first_set: inputs.stop_on_first_set,
            general_mods_bonus: session.num_available_general_mods * MAJOR_STAT_BOOST,
            max_tuning_delta,
            min_tuning_delta,
            max_tuning_net_gain,
            desired,
        }
    }
}

/// What the tracker retains per set; resolved into an `ArmorSetData` at the end.
struct SetData {
    indices: [usize; 5],
    stats: [i32; 6],
    mods: Vec<u32>,
    bonus_stats: [i32; 6],
    tuning_deltas: Option<[i32; 6]>,
}

/// DIM utils.ts:24-35 `getPower`; with no powered item DIM computes NaN, here 0.
fn power(armor: &[&ProcessItem; 5]) -> i32 {
    let powered: Vec<i32> = armor
        .iter()
        .filter(|i| i.power > 0)
        .map(|i| i32::from(i.power))
        .collect();
    if powered.is_empty() {
        0
    } else {
        powered.iter().sum::<i32>() / powered.len() as i32
    }
}

pub(super) struct Searcher<'a> {
    ctx: &'a Context,
    session: LoSessionInfo,
    progress: &'a Progress,
    tracker: HeapSetTracker<SetData>,
    stat_ranges: [MinMaxStat; 6],
    statistics: ProcessStatistics,
    energy_cache: SetEnergyCache,
    /// Running partial sums: index 0 is the empty prefix (mod stats), `level + 1`
    /// holds the totals after the item chosen at `level`.
    stats_prefix: [[i32; 6]; 5],
    perks_prefix: Vec<Vec<i32>>,
    sets_prefix: Vec<Vec<i32>>,
    indices: [usize; 5],
    combo_count: u64,
}

impl<'a> Searcher<'a> {
    pub(super) fn new(ctx: &'a Context, session: LoSessionInfo, progress: &'a Progress) -> Self {
        let mut stats_prefix = [[0; 6]; 5];
        stats_prefix[0] = ctx.mod_stats;
        Self {
            ctx,
            session,
            progress,
            tracker: HeapSetTracker::new(RETURNED_ARMOR_SETS),
            stat_ranges: [MinMaxStat {
                min_stat: MAX_STAT,
                max_stat: 0,
            }; 6],
            statistics: ProcessStatistics::default(),
            energy_cache: SetEnergyCache::default(),
            stats_prefix,
            perks_prefix: vec![vec![0; ctx.perk_counts.len()]; 5],
            sets_prefix: vec![vec![0; ctx.set_counts.len()]; 5],
            indices: [0; 5],
            combo_count: 0,
        }
    }

    pub(super) fn run(mut self) -> SliceOutput {
        self.descend(0, 0, 0, 0);
        // Report the unflushed remainder so a finished search visited every combination
        // and the budget still sees it (it may stop the other slices).
        self.progress.report(std::mem::take(&mut self.combo_count));
        let ctx = self.ctx;
        let mut sets = Vec::new();
        for entry in self.tracker.into_sorted() {
            let SetData {
                indices,
                stats,
                mods,
                bonus_stats,
                tuning_deltas,
            } = entry.data;
            let item_stats: Vec<&[i32; 6]> =
                (0..5).map(|b| &ctx.buckets[b].stats[indices[b]]).collect();
            sets.push(HeapEntry {
                enabled_stats_total: entry.enabled_stats_total,
                stats_total: entry.stats_total,
                stat_mix: entry.stat_mix,
                power: entry.power,
                tie_break: entry.tie_break,
                data: ArmorSetData {
                    armor: std::array::from_fn(|b| {
                        ctx.buckets[b].entries[indices[b]].item.id.clone()
                    }),
                    stats: std::array::from_fn(|s| stats[s] + bonus_stats[s]),
                    // The tunable item's base stats plus the chosen tuning mod (process.ts:1129).
                    armor_stats: std::array::from_fn(|s| {
                        item_stats.iter().map(|st| st[s]).sum::<i32>()
                            + tuning_deltas.map_or(0, |d| d[s])
                    }),
                    stat_mods: mods,
                },
            });
        }
        SliceOutput {
            sets,
            stat_ranges: self.stat_ranges,
            statistics: self.statistics,
        }
    }

    /// Adds visited combinations; false once the budget or another thread says stop.
    fn tick(&mut self, n: u64) -> bool {
        self.combo_count += n;
        if self.combo_count >= self.progress.cadence {
            return self.progress.report(std::mem::take(&mut self.combo_count));
        }
        true
    }

    /// Levels 0..4 are helmet..legs (process.ts:297-770), each with the exotic, perk,
    /// set bonus and `canPruneSubtree` checks; level 4 is the class item.
    /// Returns false when the whole search must stop.
    fn descend(
        &mut self,
        level: usize,
        exotic_prev: i32,
        wildcards_prev: i32,
        artifice_prev: i32,
    ) -> bool {
        if level == 4 {
            return self.innermost(exotic_prev, wildcards_prev, artifice_prev);
        }
        let ctx = self.ctx;
        let bucket = &ctx.buckets[level];
        let after = &ctx.suffix[level];
        for i in 0..bucket.entries.len() {
            self.indices[level] = i;
            let exotic = exotic_prev + bucket.exotic[i];
            if exotic > 1 {
                self.statistics.skip_reasons.double_exotic += after.combos;
                if !self.tick(after.combos) {
                    return false;
                }
                continue;
            }
            if ctx.any_exotic && exotic == 0 && !after.exotic {
                self.statistics.skip_reasons.no_exotic += after.combos;
                if !self.tick(after.combos) {
                    return false;
                }
                continue;
            }
            if !ctx.perk_counts.is_empty() && self.perk_counts_prune(level, i) {
                self.statistics.skip_reasons.insufficient_perks += after.combos;
                if !self.tick(after.combos) {
                    return false;
                }
                continue;
            }
            let wildcards = wildcards_prev + bucket.wildcard[i];
            if !ctx.set_counts.is_empty() && self.set_bonus_prune(level, i, wildcards) {
                self.statistics.skip_reasons.insufficient_set_bonus += after.combos;
                if !self.tick(after.combos) {
                    return false;
                }
                continue;
            }
            let artifice = artifice_prev + bucket.artifice[i];
            self.stats_prefix[level + 1] = add(&self.stats_prefix[level], &bucket.stats[i]);
            if self.can_prune_subtree(level, artifice) {
                self.statistics.skip_reasons.skipped_low_tier += after.combos;
                if !self.tick(after.combos) {
                    return false;
                }
                continue;
            }
            if !self.descend(level + 1, exotic, wildcards, artifice) {
                return false;
            }
        }
        true
    }

    /// process.ts:1403-1425 `perkCountsPrune`. On a prune the level's counts may be
    /// left partially updated, as in DIM.
    fn perk_counts_prune(&mut self, level: usize, i: usize) -> bool {
        let ctx = self.ctx;
        let n = ctx.perk_counts.len();
        let (done, rest) = self.perks_prefix.split_at_mut(level + 1);
        let (prev, out) = (&done[level], &mut rest[0]);
        for p in 0..n {
            let count = prev[p] + ctx.buckets[level].perks[i * n + p];
            out[p] = count;
            if count + ctx.suffix[level].max_perks[p] < ctx.perk_counts[p] {
                return true;
            }
        }
        false
    }

    /// process.ts:1427-1450 `setBonusPrune`.
    fn set_bonus_prune(&mut self, level: usize, i: usize, wildcards: i32) -> bool {
        let ctx = self.ctx;
        let set = ctx.buckets[level].set_idx[i];
        let (done, rest) = self.sets_prefix.split_at_mut(level + 1);
        let (prev, out) = (&done[level], &mut rest[0]);
        let mut deficit = 0;
        for (s, needed) in ctx.set_counts.iter().enumerate() {
            let count = prev[s] + i32::from(set == s as i32);
            out[s] = count;
            deficit += (needed - count).max(0);
        }
        deficit - wildcards > ctx.suffix[level].max_set_contrib
    }

    /// process.ts:200-286 `canPruneSubtree`: no completion could enter the tracker,
    /// raise an observed max stat range or lower an observed min stat range.
    fn can_prune_subtree(&self, level: usize, partial_artifice: i32) -> bool {
        let ctx = self.ctx;
        let after = &ctx.suffix[level];
        let partial = &self.stats_prefix[level + 1];
        let bonus_bound =
            (partial_artifice + after.artifice) * ARTIFICE_STAT_BOOST + ctx.general_mods_bonus;
        let (mut upper_total, mut partial_total) = (0, 0);
        for (i, &value) in partial.iter().enumerate() {
            if ctx.max_constraints[i] > 0 {
                upper_total += (value + after.max_stats[i] + ctx.max_tuning_delta[i])
                    .min(ctx.max_constraints[i]);
                partial_total += value;
            }
        }
        // The per-stat-clamped sum or the partial plus the best single-item totals.
        let bound_total = upper_total.min(partial_total + after.max_total);
        if self
            .tracker
            .could_insert(bound_total + bonus_bound + ctx.max_tuning_net_gain)
        {
            return false;
        }
        // Mods spent bringing other stats up to their minimums can't also boost stat i.
        let mut reserved = [0; 6];
        let mut total_reserved = 0;
        for j in 0..6 {
            let filter = &ctx.desired[j];
            if filter.max_stat > 0 && filter.min_stat > 0 {
                let best =
                    (partial[j] + after.max_stats[j] + ctx.max_tuning_delta[j]).min(MAX_STAT);
                if i32::from(filter.min_stat) > best {
                    reserved[j] = i32::from(filter.min_stat) - best;
                }
            }
            total_reserved += reserved[j];
        }
        for i in 0..6 {
            let range = &self.stat_ranges[i];
            let reserved_for_others = total_reserved - reserved[i];
            let mod_budget = (bonus_bound - reserved_for_others).max(0);
            if range.max_stat < i32::from(ctx.desired[i].min_stat)
                || (partial[i] + after.max_stats[i] + ctx.max_tuning_delta[i] + mod_budget)
                    .min(MAX_STAT)
                    > range.max_stat
            {
                return false;
            }
            if ctx.max_constraints[i] > 0
                && (partial[i] + after.min_stats[i] + ctx.min_tuning_delta[i])
                    .min(ctx.max_constraints[i])
                    < range.min_stat
            {
                return false;
            }
        }
        true
    }

    /// process.ts:560-770: the class item loop and the per-set evaluation.
    fn innermost(&mut self, exotic_p4: i32, wildcards_p4: i32, artifice_p4: i32) -> bool {
        let ctx = self.ctx;
        for ci in 0..ctx.buckets[4].entries.len() {
            self.indices[4] = ci;
            if !self.tick(1) {
                return false;
            }
            if !self.evaluate(ci, exotic_p4, wildcards_p4, artifice_p4) {
                return false;
            }
        }
        true
    }

    fn evaluate(&mut self, ci: usize, exotic_p4: i32, wildcards_p4: i32, artifice_p4: i32) -> bool {
        let ctx = self.ctx;
        let class = &ctx.buckets[4];
        let skips = &mut self.statistics.skip_reasons;
        let exotic_sum = exotic_p4 + class.exotic[ci];
        if exotic_sum > 1 {
            skips.double_exotic += 1;
            return true;
        }
        if ctx.any_exotic && exotic_sum == 0 {
            skips.no_exotic += 1;
            return true;
        }
        let num_perks = ctx.perk_counts.len();
        for p in 0..num_perks {
            if self.perks_prefix[4][p] + class.perks[ci * num_perks + p] < ctx.perk_counts[p] {
                skips.insufficient_perks += 1;
                return true;
            }
        }
        // Set bonuses; each slot can use one wildcard if present.
        let mut wildcards = wildcards_p4 + class.wildcard[ci];
        for (s, needed) in ctx.set_counts.iter().enumerate() {
            let count = self.sets_prefix[4][s] + i32::from(class.set_idx[ci] == s as i32);
            if count < *needed {
                if wildcards < needed - count {
                    skips.insufficient_set_bonus += 1;
                    return true;
                }
                wildcards -= needed - count;
            }
        }

        // At most one item in a valid set carries tuning variants (only exotics do).
        let tuned_slot = (0..5).find(|b| ctx.buckets[*b].tuning[self.indices[*b]].is_some());
        let tuning = tuned_slot.and_then(|b| ctx.buckets[b].tuning[self.indices[b]].as_ref());
        let num_variants = tuning.map_or(1, |t| t.variants.len());
        let armor: [&ProcessItem; 5] =
            std::array::from_fn(|b| &ctx.buckets[b].entries[self.indices[b]].item);
        // The energy profile doesn't depend on tuning: shared across the variants.
        self.energy_cache.result = None;
        let num_artifice = artifice_p4 + class.artifice[ci];
        // The most stat points mods could add, assuming everything were assignable.
        let max_mod_bonus = num_artifice * ARTIFICE_STAT_BOOST + ctx.general_mods_bonus;

        if let Some(tuning) = tuning {
            // Decide once per set whether any variant could matter (process.ts:615-651).
            let stats = add(&self.stats_prefix[4], &class.stats[ci]);
            let mut total_base = 0;
            let mut may_matter = false;
            for (index, &value) in stats.iter().enumerate() {
                let filter = &ctx.desired[index];
                let range = &mut self.stat_ranges[index];
                if filter.max_stat > 0 {
                    let min_value =
                        (value + tuning.min_deltas[index]).min(ctx.max_constraints[index]);
                    range.min_stat = range.min_stat.min(min_value);
                    total_base += value.min(ctx.max_constraints[index]);
                }
                let best = value + tuning.max_deltas[index];
                if range.max_stat < i32::from(filter.min_stat)
                    || best > range.max_stat
                    || (range.max_stat < MAX_STAT && best + max_mod_bonus > range.max_stat)
                {
                    may_matter = true;
                }
            }
            if !may_matter
                && !self
                    .tracker
                    .could_insert(total_base + tuning.max_net_gain + max_mod_bonus)
            {
                self.statistics.num_processed += num_variants as u64;
                self.statistics.skip_reasons.skipped_low_tier += num_variants as u64;
                return true;
            }
        }

        for variant in 0..num_variants {
            self.statistics.num_processed += 1;
            let mut stats = add(&self.stats_prefix[4], &class.stats[ci]);
            let delta = tuning.map(|t| t.variants[variant].1);
            if let Some(delta) = delta {
                stats = add(&stats, &delta);
            }
            // Stats clamped to the maximum constraint, and what each minimum still needs.
            let effective: [i32; 6] = std::array::from_fn(|s| stats[s].min(ctx.max_constraints[s]));
            let mut needed = [0; 6];
            let mut total_needed = 0;
            let mut total_stats = 0;
            for index in 0..6 {
                let filter = &ctx.desired[index];
                if filter.max_stat > 0 {
                    let value = effective[index];
                    let range = &mut self.stat_ranges[index];
                    range.min_stat = range.min_stat.min(value);
                    total_stats += value;
                    if filter.min_stat > 0 {
                        let need = i32::from(filter.min_stat) - value;
                        if need > 0 {
                            total_needed += need;
                            needed[index] = need;
                        }
                    }
                }
            }
            // Could the best-case mod bonus reach the minimums at all?
            self.statistics.lower_bounds_exceeded.times_checked += 1;
            if total_needed > max_mod_bonus {
                self.statistics.lower_bounds_exceeded.times_failed += 1;
                continue;
            }
            if (ctx.has_mods || total_needed > 0)
                && pick_and_assign_slot_independent_mods(
                    &self.session,
                    &mut self.statistics.mods_statistics,
                    &armor,
                    (total_needed > 0).then_some(&needed),
                    num_artifice,
                )
                .is_none()
            {
                continue;
            }
            // The set satisfies every constraint: raise the displayed max ranges. Once
            // they have converged, skip sets that provably can't raise any of them.
            let may_improve_max = (0..6).any(|index| {
                let max_seen = self.stat_ranges[index].max_stat;
                max_seen < i32::from(ctx.desired[index].min_stat)
                    || stats[index] > max_seen
                    || (max_seen < MAX_STAT && stats[index] + max_mod_bonus > max_seen)
            });
            let found_improvement = may_improve_max
                && update_max_stats(
                    &self.session,
                    &armor,
                    &stats,
                    num_artifice,
                    &ctx.desired,
                    &mut self.stat_ranges,
                    Some(&mut self.energy_cache),
                );
            if !self.tracker.could_insert(total_stats + max_mod_bonus) {
                self.statistics.skip_reasons.skipped_low_tier += 1;
                continue;
            }
            let Some(StatModPick {
                mut mods,
                bonus_stats,
            }) = pick_optimal_stat_mods(
                &self.session,
                &armor,
                &stats,
                &ctx.desired,
                Some(num_artifice),
                Some(&mut self.energy_cache),
            )
            else {
                self.statistics
                    .mods_statistics
                    .final_assignment
                    .mods_assignment_failed += 1;
                continue;
            };
            let final_stats: [i32; 6] = std::array::from_fn(|s| effective[s] + bonus_stats[s]);
            let final_total: i32 = final_stats.iter().sum();
            if !self.tracker.could_insert(final_total) {
                self.statistics.skip_reasons.skipped_low_tier += 1;
                continue;
            }
            // Preset tuning mods stay in armor order; the tunable item gets the chosen one
            // (process.ts:700-722).
            let chosen = tuning.map(|t| t.variants[variant].0);
            for (slot, item) in armor.iter().enumerate() {
                let tuning_mod = if Some(slot) == tuned_slot {
                    chosen
                } else {
                    item.included_tuning_mod
                };
                mods.extend(tuning_mod);
            }
            self.statistics.num_valid_sets += 1;
            self.tracker.insert(HeapEntry {
                enabled_stats_total: final_total,
                stats_total: stats.iter().sum(),
                stat_mix: encode_stat_mix(&final_stats, &ctx.desired),
                power: power(&armor),
                tie_break: self.tie_break(chosen),
                data: SetData {
                    indices: self.indices,
                    stats,
                    mods,
                    bonus_stats,
                    tuning_deltas: delta,
                },
            });
            if ctx.stop_on_first_set && (!ctx.strict_upgrades || found_improvement) {
                return false;
            }
        }
        true
    }

    fn tie_break(&self, tuning_mod: Option<u32>) -> TieBreak {
        (
            std::array::from_fn(|b| self.ctx.buckets[b].entries[self.indices[b]].rank),
            tuning_mod.unwrap_or(0),
        )
    }
}
