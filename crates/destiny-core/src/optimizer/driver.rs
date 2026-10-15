//! DIM process/process-wrapper.ts:236-340 (slicing the longest bucket round-robin,
//! `combineResults`) plus a work/time budget for a CLI process. Each slice runs its
//! own `Searcher` (and `LoSessionInfo`) on a scoped `std::thread`.
//! Ghostlink deviations, all so the output does not depend on the thread count:
//! * sets and displayed stat ranges are order-independent (see `set_tracker`), so any
//!   thread count returns the same ones; the pruning statistics are informational;
//! * `strict_upgrades` filters the merged top sets (DIM filters each worker's top 200
//!   before merging, which would make the result depend on the slicing);
//! * `stop_on_first_set` always runs one slice, since "the first set" is only
//!   well defined for one visiting order;
//! * with a budget, a truncated search is not deterministic (time, thread timing).
use super::search::{Context, Entry, Searcher, SliceOutput};
use super::set_tracker::{HeapSetTracker, RETURNED_ARMOR_SETS};
use super::*;
use crate::armor_model::ARMOR_STATS;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// process.ts's `comboCount >= 100000` flush: how often progress and the budget are
/// checked, counted in combinations (skipped subtrees count their whole size) across
/// all threads; each of N slices reports every `PROGRESS_CADENCE / N`.
pub const PROGRESS_CADENCE: u64 = 100_000;

/// Stops a search early. Both limits are checked on the aggregate `PROGRESS_CADENCE`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SearchBudget {
    /// Combinations visited (evaluated or pruned) across all threads.
    pub max_combos: Option<u64>,
    pub time_limit: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchOptions {
    /// Slices of the longest bucket searched in parallel; 0 and 1 mean one thread.
    pub threads: usize,
    pub budget: SearchBudget,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            threads: 1,
            budget: SearchBudget::default(),
        }
    }
}

/// process.ts's `ProcessResult` plus the budget outcome.
#[derive(Clone, Debug)]
pub struct SearchResult {
    /// Best first, at most `RETURNED_ARMOR_SETS`.
    pub sets: Vec<ArmorSet>,
    /// The displayed stat ranges in the session stat order.
    pub stat_ranges: Vec<MinMaxStat>,
    /// Every combination the buckets can form.
    pub combos_total: u64,
    /// Combinations visited, pruned subtrees included; equals the total unless the
    /// search was truncated or stopped on its first set.
    pub combos_visited: u64,
    /// The budget ran out before the search finished: sets and ranges are partial.
    pub truncated: bool,
    pub statistics: ProcessStatistics,
}

/// Shared by the threads of one search.
pub(super) struct Progress {
    visited: AtomicU64,
    stopped: AtomicBool,
    truncated: AtomicBool,
    total: u64,
    budget: SearchBudget,
    deadline: Option<Instant>,
    /// Combinations one slice visits between reports.
    pub(super) cadence: u64,
}
impl Progress {
    fn new(total: u64, budget: SearchBudget, slices: usize) -> Self {
        Self {
            visited: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            truncated: AtomicBool::new(false),
            total,
            budget,
            deadline: budget.time_limit.map(|limit| Instant::now() + limit),
            cadence: PROGRESS_CADENCE.div_ceil(slices.max(1) as u64),
        }
    }
    /// Records progress and says whether the search may continue.
    pub(super) fn report(&self, combos: u64) -> bool {
        let visited = self.visited.fetch_add(combos, Ordering::Relaxed) + combos;
        if self.stopped.load(Ordering::Relaxed) {
            return false;
        }
        let over = self.budget.max_combos.is_some_and(|max| visited >= max)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline);
        if over && visited < self.total {
            self.truncated.store(true, Ordering::Relaxed);
            self.stopped.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }
}

/// The session stat order must be a permutation of the six armor stats; returns each
/// session stat's `ARMOR_STATS` index.
fn stat_positions(ranges: &[DesiredStatRange]) -> Result<[usize; 6], SearchError> {
    if ranges.len() != 6 {
        return Err(SearchError::InvalidStatRanges);
    }
    let mut positions = [0; 6];
    for (i, range) in ranges.iter().enumerate() {
        positions[i] = ARMOR_STATS
            .iter()
            .position(|h| *h == range.stat_hash)
            .ok_or(SearchError::InvalidStatRanges)?;
    }
    let mut seen = positions;
    seen.sort_unstable();
    if seen != [0, 1, 2, 3, 4, 5] {
        return Err(SearchError::InvalidStatRanges);
    }
    Ok(positions)
}

/// process-wrapper.ts:352-390 `sliceInputForConcurrency`: sort the longest bucket
/// high-stat first and deal it round-robin so every slice gets a spread of items.
fn slice_buckets(
    mut buckets: [Vec<Entry>; 5],
    threads: usize,
    enabled_total: impl Fn(&Entry) -> i32,
) -> Vec<[Vec<Entry>; 5]> {
    let longest = (0..5).fold(0, |best, b| {
        if buckets[b].len() > buckets[best].len() {
            b
        } else {
            best
        }
    });
    let count = threads.min(buckets[longest].len());
    if count <= 1 {
        return vec![buckets];
    }
    let mut sorted = std::mem::take(&mut buckets[longest]);
    sorted.sort_by_key(|e| std::cmp::Reverse(enabled_total(e)));
    let mut slices: Vec<Vec<Entry>> = (0..count).map(|_| Vec::new()).collect();
    for (i, entry) in sorted.into_iter().enumerate() {
        slices[i % count].push(entry);
    }
    slices
        .into_iter()
        .map(|slice| {
            let mut slice_buckets: [Vec<Entry>; 5] = std::array::from_fn(|b| {
                if b == longest {
                    Vec::new()
                } else {
                    buckets[b].iter().map(clone_entry).collect()
                }
            });
            slice_buckets[longest] = slice;
            slice_buckets
        })
        .collect()
}
fn clone_entry(entry: &Entry) -> Entry {
    Entry {
        item: entry.item.clone(),
        rank: entry.rank,
    }
}

/// Runs the Loadout Optimizer search (process.ts `process` through process-wrapper's
/// slicing and merging).
pub fn search(inputs: &SearchInputs, options: &SearchOptions) -> Result<SearchResult, SearchError> {
    let positions = stat_positions(&inputs.desired_stat_ranges)?;
    let order: [u32; 6] = std::array::from_fn(|i| inputs.desired_stat_ranges[i].stat_hash);
    let combos_total: u64 = inputs.buckets.iter().map(|b| b.len() as u64).product();
    let mut result = SearchResult {
        sets: Vec::new(),
        // process.ts:178-184: nothing to search leaves the ranges open.
        stat_ranges: vec![
            MinMaxStat {
                min_stat: 0,
                max_stat: MAX_STAT
            };
            6
        ],
        combos_total,
        combos_visited: 0,
        truncated: false,
        statistics: ProcessStatistics::default(),
    };
    if combos_total == 0 {
        return Ok(result);
    }

    // Ranks make the tie-break independent of input order (see `set_tracker`).
    let mut keys: Vec<(&str, Option<u32>)> = inputs
        .buckets
        .iter()
        .flatten()
        .map(|i| (i.id.as_str(), i.included_tuning_mod))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let ranks: BTreeMap<_, _> = keys
        .into_iter()
        .enumerate()
        .map(|(r, k)| (k, r as u32))
        .collect();
    let buckets: [Vec<Entry>; 5] = std::array::from_fn(|b| {
        inputs.buckets[b]
            .iter()
            .map(|item| Entry {
                rank: ranks[&(item.id.as_str(), item.included_tuning_mod)],
                item: item.clone(),
            })
            .collect()
    });

    let threads = if inputs.stop_on_first_set {
        1
    } else {
        options.threads.max(1)
    };
    let ranges = &inputs.desired_stat_ranges;
    let slices = slice_buckets(buckets, threads, |e| {
        (0..6)
            .filter(|i| ranges[*i].max_stat > 0)
            .map(|i| e.item.stats[positions[i]])
            .sum()
    });
    let progress = Progress::new(combos_total, options.budget, slices.len());
    let run = |buckets: [Vec<Entry>; 5]| -> SliceOutput {
        let session = LoSessionInfo::new(
            &inputs.auto_mod_options,
            &inputs.general_mods,
            &inputs.activity_mods,
            inputs.auto_stat_mods,
            &order,
        );
        let context = Context::new(inputs, buckets, &positions, &session);
        Searcher::new(&context, session, &progress).run()
    };
    let outputs: Vec<SliceOutput> = if slices.len() == 1 {
        slices.into_iter().map(run).collect()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = slices.into_iter().map(|s| scope.spawn(|| run(s))).collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        })
    };

    // process-wrapper.ts:296-317 `combineStatRanges`; :294-303 re-ranks the sets.
    let mut ranges_out = vec![
        MinMaxStat {
            min_stat: MAX_STAT,
            max_stat: 0
        };
        6
    ];
    let mut tracker = HeapSetTracker::new(RETURNED_ARMOR_SETS);
    for output in outputs {
        for (merged, range) in ranges_out.iter_mut().zip(&output.stat_ranges) {
            merged.min_stat = merged.min_stat.min(range.min_stat);
            merged.max_stat = merged.max_stat.max(range.max_stat);
        }
        result.statistics.add(&output.statistics);
        for set in output.sets {
            if tracker.could_insert(set.enabled_stats_total) {
                tracker.insert(set);
            }
        }
    }
    result.stat_ranges = ranges_out;
    result.sets = tracker.into_sorted();
    if inputs.strict_upgrades {
        // process.ts:1117-1142: keep sets where an enabled, unfixed stat beats its minimum.
        result.sets.retain(|set| {
            ranges.iter().zip(&set.data.stats).any(|(range, value)| {
                range.max_stat > 0
                    && range.min_stat < range.max_stat
                    && *value > i32::from(range.min_stat)
            })
        });
    }
    result.combos_visited = progress.visited.load(Ordering::Relaxed);
    result.truncated = progress.truncated.load(Ordering::Relaxed);
    Ok(result)
}
