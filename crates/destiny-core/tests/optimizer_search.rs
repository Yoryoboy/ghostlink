//! BE-008c search core, mirroring DIM 29c122e loadout-builder/process-worker/
//! process.test.ts and set-tracker.test.ts. DIM's process tests are snapshots over its
//! test manifest, so the same scenarios run on synthetic armor and are checked against
//! a brute-force reference that enumerates every combination without any pruning,
//! using the already-ported mod functions.
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::optimizer::*;
use std::cmp::Ordering;

const PERK: u32 = 999_999;
const SET_A: u32 = 111_111;
const SET_B: u32 = 222_222;
const TAG: u32 = 1764679361;

/// Small deterministic generator (xorshift) so corpora are stable across runs.
struct Rng(u64);
impl Rng {
    fn next(&mut self, below: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 33) as u32 % below
    }
}

fn item(bucket: usize, index: usize, rng: &mut Rng) -> ProcessItem {
    let mut stats = [0; 6];
    for stat in &mut stats {
        *stat = 2 + rng.next(24) as i32;
    }
    ProcessItem {
        id: format!("b{bucket}-{index:03}"),
        hash: (bucket * 1000 + index) as u32,
        name: format!("Item {bucket}-{index}"),
        is_exotic: false,
        is_artifice: rng.next(5) == 0,
        remaining_energy_capacity: 10,
        power: 1800 + rng.next(30) as u16,
        stats,
        compatible_activity_mod: None,
        set_bonus: None,
        intrinsic_perks: None,
        has_set_bonus_mod_socket: false,
        included_tuning_mod: None,
        tuning_variants: None,
    }
}
fn corpus(per_bucket: usize, seed: u64) -> [Vec<ProcessItem>; 5] {
    let mut rng = Rng(seed);
    std::array::from_fn(|b| (0..per_bucket).map(|i| item(b, i, &mut rng)).collect())
}
fn standard_auto_mods() -> AutoModData {
    let mut data = AutoModData::default();
    for (i, stat) in ARMOR_STATS.iter().enumerate() {
        let base = 1000 + 10 * i as u32;
        data.general_mods.insert(
            *stat,
            GeneralAutoMods {
                minor: AutoMod {
                    hash: base + 1,
                    cost: 1,
                },
                major: AutoMod {
                    hash: base + 2,
                    cost: 3,
                },
            },
        );
        data.artifice_mods.insert(*stat, 2000 + i as u32);
    }
    data
}
fn ranges(f: impl Fn(usize) -> (u16, u16)) -> Vec<DesiredStatRange> {
    ARMOR_STATS
        .iter()
        .enumerate()
        .map(|(i, stat_hash)| {
            let (min_stat, max_stat) = f(i);
            DesiredStatRange {
                stat_hash: *stat_hash,
                min_stat,
                max_stat,
            }
        })
        .collect()
}
fn inputs(buckets: [Vec<ProcessItem>; 5]) -> SearchInputs {
    SearchInputs {
        buckets,
        mod_stat_totals: [0; 6],
        general_mods: Vec::new(),
        activity_mods: Vec::new(),
        set_bonuses: Vec::new(),
        required_perks: Vec::new(),
        desired_stat_ranges: ranges(|_| (0, 200)),
        any_exotic: false,
        auto_mod_options: standard_auto_mods(),
        auto_stat_mods: false,
        strict_upgrades: false,
        stop_on_first_set: false,
    }
}
fn general_mod() -> ProcessMod {
    ProcessMod {
        hash: 5001,
        energy_cost: 3,
        tag: None,
    }
}
fn activity_mod() -> ProcessMod {
    ProcessMod {
        hash: 5002,
        energy_cost: 2,
        tag: Some(TAG),
    }
}
fn variants(exotic: &ProcessItem) -> Vec<TuningVariant> {
    let mut directional = exotic.stats;
    directional[1] += 5;
    directional[4] -= 5;
    let mut balanced = exotic.stats;
    balanced[0] += 1;
    balanced[1] += 1;
    balanced[4] += 1;
    vec![
        TuningVariant {
            mod_hash: 111,
            stats: directional,
        },
        TuningVariant {
            mod_hash: 222,
            stats: balanced,
        },
    ]
}

// ---------------------------------------------------------------------------
// Brute-force reference (DIM semantics, no pruning, no early exits).

struct Candidate {
    key: (i32, u64, i32, i32),
    tie: (Vec<(String, Option<u32>)>, u32),
    set: ArmorSetData,
    stats: [i32; 6],
    bonus: [i32; 6],
}
fn cmp_better_first(a: &Candidate, b: &Candidate) -> Ordering {
    b.key.cmp(&a.key).then_with(|| a.tie.cmp(&b.tie))
}

fn reference(input: &SearchInputs) -> (Vec<Candidate>, Vec<MinMaxStat>) {
    let order: [u32; 6] = std::array::from_fn(|i| input.desired_stat_ranges[i].stat_hash);
    let project = |stats: &[i32; 6]| -> [i32; 6] {
        std::array::from_fn(|i| {
            stats[ARMOR_STATS
                .iter()
                .position(|h| *h == order[i])
                .expect("armor stat")]
        })
    };
    let desired = &input.desired_stat_ranges;
    let session = LoSessionInfo::new(
        &input.auto_mod_options,
        &input.general_mods,
        &input.activity_mods,
        input.auto_stat_mods,
        &order,
    );
    let has_mods = !(input.general_mods.is_empty() && input.activity_mods.is_empty());
    let mod_stats = project(&input.mod_stat_totals);
    let mut stat_ranges = vec![
        MinMaxStat {
            min_stat: MAX_STAT,
            max_stat: 0,
        };
        6
    ];
    let mut statistics = ModAssignmentStatistics::default();
    let mut found = Vec::new();
    let b = &input.buckets;
    for h in &b[0] {
        for g in &b[1] {
            for c in &b[2] {
                for l in &b[3] {
                    for k in &b[4] {
                        let armor = [h, g, c, l, k];
                        let exotics = armor.iter().filter(|i| i.is_exotic).count();
                        if exotics > 1 || (input.any_exotic && exotics == 0) {
                            continue;
                        }
                        if !input.required_perks.iter().all(|(hash, count)| {
                            armor
                                .iter()
                                .filter(|i| {
                                    i.intrinsic_perks.as_ref().is_some_and(|p| p.contains(hash))
                                })
                                .count() as u32
                                >= *count
                        }) {
                            continue;
                        }
                        let wildcards =
                            armor.iter().filter(|i| i.has_set_bonus_mod_socket).count() as i32;
                        let deficit: i32 = input
                            .set_bonuses
                            .iter()
                            .map(|(hash, count)| {
                                let have =
                                    armor.iter().filter(|i| i.set_bonus == Some(*hash)).count()
                                        as i32;
                                (*count as i32 - have).max(0)
                            })
                            .sum();
                        if deficit > wildcards {
                            continue;
                        }
                        let tuned = armor.iter().position(|i| i.tuning_variants.is_some());
                        let options: Vec<Option<&TuningVariant>> = match tuned {
                            Some(slot) => armor[slot]
                                .tuning_variants
                                .as_ref()
                                .unwrap()
                                .iter()
                                .map(Some)
                                .collect(),
                            None => vec![None],
                        };
                        let num_artifice = armor.iter().filter(|i| i.is_artifice).count() as i32;
                        let max_mod_bonus = num_artifice * ARTIFICE_STAT_BOOST
                            + session.num_available_general_mods * MAJOR_STAT_BOOST;
                        for variant in options {
                            let mut stats = mod_stats;
                            for (slot, item) in armor.iter().enumerate() {
                                let own = match (variant, tuned) {
                                    (Some(v), Some(t)) if t == slot => project(&v.stats),
                                    _ => project(&item.stats),
                                };
                                for s in 0..6 {
                                    stats[s] += own[s];
                                }
                            }
                            let mut effective = [0; 6];
                            let mut needed = [0; 6];
                            let mut total_needed = 0;
                            for s in 0..6 {
                                effective[s] = stats[s].min(i32::from(desired[s].max_stat));
                                if desired[s].max_stat > 0 {
                                    stat_ranges[s].min_stat =
                                        stat_ranges[s].min_stat.min(effective[s]);
                                    let need = i32::from(desired[s].min_stat) - effective[s];
                                    if desired[s].min_stat > 0 && need > 0 {
                                        needed[s] = need;
                                        total_needed += need;
                                    }
                                }
                            }
                            if total_needed > max_mod_bonus {
                                continue;
                            }
                            if (has_mods || total_needed > 0)
                                && pick_and_assign_slot_independent_mods(
                                    &session,
                                    &mut statistics,
                                    &armor,
                                    (total_needed > 0).then_some(&needed),
                                    num_artifice,
                                )
                                .is_none()
                            {
                                continue;
                            }
                            update_max_stats(
                                &session,
                                &armor,
                                &stats,
                                num_artifice,
                                desired,
                                &mut stat_ranges,
                                None,
                            );
                            let Some(pick) = pick_optimal_stat_mods(
                                &session,
                                &armor,
                                &stats,
                                desired,
                                Some(num_artifice),
                                None,
                            ) else {
                                continue;
                            };
                            let fin: [i32; 6] =
                                std::array::from_fn(|s| effective[s] + pick.bonus_stats[s]);
                            let total: i32 = fin.iter().sum();
                            let mut mods = pick.mods.clone();
                            for (slot, item) in armor.iter().enumerate() {
                                let tuning = match (variant, tuned) {
                                    (Some(v), Some(t)) if t == slot => Some(v.mod_hash),
                                    _ => item.included_tuning_mod,
                                };
                                mods.extend(tuning);
                            }
                            let powered: Vec<i32> = armor
                                .iter()
                                .filter(|i| i.power > 0)
                                .map(|i| i32::from(i.power))
                                .collect();
                            let power = if powered.is_empty() {
                                0
                            } else {
                                powered.iter().sum::<i32>() / powered.len() as i32
                            };
                            let mut armor_stats: [i32; 6] = [0; 6];
                            for (slot, item) in armor.iter().enumerate() {
                                let own = match (variant, tuned) {
                                    (Some(v), Some(t)) if t == slot => project(&v.stats),
                                    _ => project(&item.stats),
                                };
                                for s in 0..6 {
                                    armor_stats[s] += own[s];
                                }
                            }
                            let mut full: [i32; 6] = [0; 6];
                            for s in 0..6 {
                                full[s] = stats[s] + pick.bonus_stats[s];
                            }
                            found.push(Candidate {
                                key: (
                                    total,
                                    encode_stat_mix(&fin, desired),
                                    stats.iter().sum(),
                                    power,
                                ),
                                tie: (
                                    armor
                                        .iter()
                                        .map(|i| (i.id.clone(), i.included_tuning_mod))
                                        .collect(),
                                    variant.map_or(0, |v| v.mod_hash),
                                ),
                                set: ArmorSetData {
                                    armor: std::array::from_fn(|i| armor[i].id.clone()),
                                    stats: full,
                                    armor_stats,
                                    stat_mods: mods,
                                },
                                stats,
                                bonus: pick.bonus_stats,
                            });
                        }
                    }
                }
            }
        }
    }
    found.sort_by(cmp_better_first);
    found.truncate(RETURNED_ARMOR_SETS);
    if input.strict_upgrades {
        found.retain(|c| {
            (0..6).any(|i| {
                let f = &desired[i];
                f.max_stat > 0
                    && f.min_stat < f.max_stat
                    && c.stats[i] + c.bonus[i] > i32::from(f.min_stat)
            })
        });
    }
    (found, stat_ranges)
}

fn assert_matches_reference(input: &SearchInputs, threads: usize) -> SearchResult {
    let (expected, expected_ranges) = reference(input);
    let result = search(
        input,
        &SearchOptions {
            threads,
            budget: SearchBudget::default(),
        },
    )
    .expect("search");
    assert!(!result.truncated);
    assert_eq!(result.combos_visited, result.combos_total);
    assert_eq!(result.stat_ranges, expected_ranges);
    assert_eq!(result.sets.len(), expected.len());
    for (got, want) in result.sets.iter().zip(&expected) {
        assert_eq!(got.data, want.set);
        assert_eq!(
            (
                got.enabled_stats_total,
                got.stat_mix,
                got.stats_total,
                got.power
            ),
            want.key
        );
    }
    result
}

fn small(seed: u64) -> SearchInputs {
    inputs(corpus(5, seed))
}

// ---------------------------------------------------------------------------
// process.test.ts scenarios.

#[test]
fn no_constraints() {
    let result = assert_matches_reference(&small(1), 1);
    assert_eq!(result.sets.len(), RETURNED_ARMOR_SETS);
    assert_eq!(result.combos_total, 5u64.pow(5));
}

#[test]
fn stat_minimums_and_maximums_with_auto_stat_mods() {
    let mut input = small(2);
    input.auto_stat_mods = true;
    input.desired_stat_ranges =
        ranges(|i| (if i < 2 { 40 } else { 0 }, if i == 5 { 0 } else { 100 }));
    let result = assert_matches_reference(&input, 1);
    assert!(!result.sets.is_empty());
    // DIM's updateMaxStats raises an ignored stat's displayed max too; its min stays open.
    assert_eq!(result.stat_ranges[5].min_stat, MAX_STAT);
    assert!(result.stat_ranges[5].max_stat > 0);
}

#[test]
fn unreachable_stat_minimums_exercise_the_lower_bounds_prune() {
    let mut input = small(3);
    input.auto_stat_mods = true;
    input.desired_stat_ranges = ranges(|i| (if i < 3 { 150 } else { 0 }, 200));
    let result = assert_matches_reference(&input, 1);
    assert!(result.sets.is_empty());
    assert!(result.statistics.lower_bounds_exceeded.times_failed > 0);
}

#[test]
fn any_exotic_with_synthesized_exotics() {
    let mut input = small(4);
    input.any_exotic = true;
    for (b, i) in [(0, 0), (0, 1), (2, 0), (3, 3)] {
        input.buckets[b][i].is_exotic = true;
    }
    let result = assert_matches_reference(&input, 1);
    assert!(!result.sets.is_empty());
    let exotic_ids: Vec<_> = input
        .buckets
        .iter()
        .flatten()
        .filter(|i| i.is_exotic)
        .map(|i| i.id.clone())
        .collect();
    for set in &result.sets {
        assert_eq!(
            set.data
                .armor
                .iter()
                .filter(|id| exotic_ids.contains(id))
                .count(),
            1
        );
    }
    assert!(result.statistics.skip_reasons.double_exotic > 0);
}

#[test]
fn required_perks() {
    let mut input = small(5);
    for (b, i) in [(0, 0), (0, 2), (1, 1), (4, 0)] {
        input.buckets[b][i].intrinsic_perks = Some(vec![PERK]);
    }
    input.required_perks = vec![(PERK, 2)];
    let result = assert_matches_reference(&input, 1);
    assert!(!result.sets.is_empty());
    assert!(result.statistics.skip_reasons.insufficient_perks > 0);
}

#[test]
fn set_bonuses_with_wildcards() {
    let mut input = small(6);
    for bucket in &mut input.buckets {
        bucket[0].set_bonus = Some(SET_A);
        bucket[1].set_bonus = Some(SET_A);
        bucket[2].set_bonus = Some(SET_B);
        bucket[3].has_set_bonus_mod_socket = true;
    }
    input.set_bonuses = vec![(SET_A, 2), (SET_B, 2)];
    let result = assert_matches_reference(&input, 1);
    assert!(!result.sets.is_empty());
    assert!(result.statistics.skip_reasons.insufficient_set_bonus > 0);
}

#[test]
fn set_bonus_satisfied_only_via_wildcard() {
    let mut input = small(7);
    input.buckets[0][0].set_bonus = Some(SET_A);
    input.buckets[1][0].has_set_bonus_mod_socket = true;
    input.buckets[2][0].has_set_bonus_mod_socket = true;
    input.set_bonuses = vec![(SET_A, 2)];
    let result = assert_matches_reference(&input, 1);
    // Every valid set holds the set piece and a wildcard socket or two wildcards.
    assert!(!result.sets.is_empty());
}

#[test]
fn locked_general_and_activity_mods() {
    let mut input = small(8);
    input.auto_stat_mods = true;
    input.general_mods = vec![general_mod(), general_mod()];
    input.activity_mods = vec![activity_mod()];
    for item in input.buckets.iter_mut().flatten() {
        item.compatible_activity_mod = Some(TAG);
    }
    let result = assert_matches_reference(&input, 1);
    assert!(!result.sets.is_empty());
}

#[test]
fn strict_upgrades() {
    let mut input = small(9);
    input.strict_upgrades = true;
    input.auto_stat_mods = true;
    input.desired_stat_ranges = ranges(|i| (if i == 0 { 50 } else { 0 }, 150));
    let result = assert_matches_reference(&input, 1);
    for set in &result.sets {
        assert!(
            (0..6).any(|i| set.data.stats[i] > i32::from(input.desired_stat_ranges[i].min_stat))
        );
    }
}

#[test]
fn stop_on_first_set_finds_a_set() {
    let mut input = small(10);
    input.stop_on_first_set = true;
    let result = search(&input, &SearchOptions::default()).unwrap();
    assert_eq!(result.sets.len(), 1);
    assert!(result.combos_visited < result.combos_total);
    assert!(!result.truncated);
    // Parallel slicing never changes which single set a stop-on-first run reports.
    let parallel = search(
        &input,
        &SearchOptions {
            threads: 4,
            ..SearchOptions::default()
        },
    )
    .unwrap();
    assert_eq!(parallel.sets[0].data, result.sets[0].data);
}

#[test]
fn stop_on_first_set_with_strict_upgrades_waits_for_an_improvement() {
    let mut input = small(10);
    input.stop_on_first_set = true;
    input.strict_upgrades = true;
    input.auto_stat_mods = true;
    input.desired_stat_ranges = ranges(|_| (0, 200));
    let result = search(&input, &SearchOptions::default()).unwrap();
    assert_eq!(result.sets.len(), 1);
}

#[test]
fn tail_resolved_exotic_tuning_matches_the_bucket_expansion() {
    let patch = |input: &mut SearchInputs| {
        input.auto_stat_mods = true;
        input.desired_stat_ranges[0].min_stat = 20;
        for bucket in &mut input.buckets {
            bucket.truncate(2);
        }
        input.buckets[0][0].is_exotic = true;
    };
    let mut tail = small(11);
    patch(&mut tail);
    let exotic = tail.buckets[0][0].clone();
    tail.buckets[0][0].tuning_variants = Some(variants(&exotic));
    let mut expanded = small(11);
    patch(&mut expanded);
    expanded.buckets[0].remove(0);
    for v in variants(&exotic) {
        expanded.buckets[0].push(ProcessItem {
            included_tuning_mod: Some(v.mod_hash),
            stats: v.stats,
            ..exotic.clone()
        });
    }
    let tail_result = assert_matches_reference(&tail, 1);
    let expanded_result = assert_matches_reference(&expanded, 1);
    let key = |r: &SearchResult| {
        r.sets
            .iter()
            .map(|s| {
                (
                    s.enabled_stats_total,
                    s.stat_mix,
                    s.data.stats,
                    s.data.armor_stats,
                    s.data.stat_mods.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&tail_result), key(&expanded_result));
    assert_eq!(tail_result.stat_ranges, expanded_result.stat_ranges);
    assert!(
        tail_result
            .sets
            .iter()
            .any(|s| s.data.stat_mods.iter().any(|m| *m == 111 || *m == 222))
    );
}

#[test]
fn tail_resolved_tuning_matches_when_the_tracker_boundary_prunes_variants() {
    let mut input = small(12);
    input.auto_stat_mods = true;
    for (b, i) in [(0, 0), (0, 3), (2, 2)] {
        input.buckets[b][i].is_exotic = true;
        let exotic = input.buckets[b][i].clone();
        input.buckets[b][i].tuning_variants = Some(variants(&exotic));
    }
    let result = assert_matches_reference(&input, 1);
    assert_eq!(result.sets.len(), RETURNED_ARMOR_SETS);
    assert!(
        result
            .sets
            .iter()
            .any(|s| s.data.stat_mods.iter().any(|m| *m == 111 || *m == 222))
    );
}

#[test]
fn pre_tuned_items_report_their_included_tuning_mod() {
    let mut input = small(13);
    input.buckets[1][0].included_tuning_mod = Some(333);
    let result = assert_matches_reference(&input, 1);
    assert!(result.sets.iter().any(|s| s.data.stat_mods.contains(&333)));
}

#[test]
fn an_empty_bucket_yields_no_combinations_and_open_ranges() {
    let mut input = small(14);
    input.buckets[2].clear();
    let result = search(
        &input,
        &SearchOptions {
            threads: 3,
            ..SearchOptions::default()
        },
    )
    .unwrap();
    assert!(result.sets.is_empty());
    assert_eq!((result.combos_total, result.combos_visited), (0, 0));
    assert!(!result.truncated);
    for range in &result.stat_ranges {
        assert_eq!((range.min_stat, range.max_stat), (0, MAX_STAT));
    }
}

#[test]
fn stat_ranges_must_cover_the_six_armor_stats() {
    let mut input = small(15);
    input.desired_stat_ranges.pop();
    assert_eq!(
        search(&input, &SearchOptions::default()).unwrap_err(),
        SearchError::InvalidStatRanges
    );
    let mut input = small(15);
    input.desired_stat_ranges[0].stat_hash = 1;
    assert_eq!(
        search(&input, &SearchOptions::default()).unwrap_err(),
        SearchError::InvalidStatRanges
    );
}

#[test]
fn mod_stat_totals_shift_the_set_stats() {
    let mut input = small(16);
    input.mod_stat_totals = [0, 10, 0, 0, 0, 5];
    input.desired_stat_ranges = ranges(|_| (0, 200)).into_iter().rev().collect();
    assert_matches_reference(&input, 1);
}

// ---------------------------------------------------------------------------
// Ghostlink: deterministic ordering, thread-count independence, budget, perf.

#[test]
fn results_are_independent_of_item_order_and_thread_count() {
    let mut input = small(20);
    input.auto_stat_mods = true;
    input.desired_stat_ranges = ranges(|i| (if i == 1 { 30 } else { 0 }, 100));
    for (b, i) in [(1, 0), (3, 2)] {
        input.buckets[b][i].is_exotic = true;
    }
    // Equal stats everywhere on two buckets forces ties across the whole heap.
    for b in [3, 4] {
        for item in &mut input.buckets[b] {
            item.stats = [10; 6];
            item.power = 1800;
        }
    }
    let baseline = assert_matches_reference(&input, 1);
    let mut shuffled = input.clone();
    for bucket in &mut shuffled.buckets {
        bucket.reverse();
    }
    let sets = |r: &SearchResult| r.sets.iter().map(|s| s.data.clone()).collect::<Vec<_>>();
    for threads in [1, 2, 3, 5, 8] {
        for candidate in [&input, &shuffled] {
            let result = search(
                candidate,
                &SearchOptions {
                    threads,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
            assert_eq!(sets(&result), sets(&baseline), "threads={threads}");
            assert_eq!(
                result.stat_ranges, baseline.stat_ranges,
                "threads={threads}"
            );
            assert_eq!(result.combos_total, baseline.combos_total);
            assert_eq!(result.combos_visited, result.combos_total);
        }
    }
}

#[test]
fn strict_upgrade_filtering_is_applied_after_merging_slices() {
    let mut input = small(21);
    input.strict_upgrades = true;
    input.desired_stat_ranges = ranges(|i| (if i == 0 { 60 } else { 0 }, 80));
    let single = search(&input, &SearchOptions::default()).unwrap();
    let many = search(
        &input,
        &SearchOptions {
            threads: 4,
            ..SearchOptions::default()
        },
    )
    .unwrap();
    let sets = |r: &SearchResult| r.sets.iter().map(|s| s.data.clone()).collect::<Vec<_>>();
    assert_eq!(sets(&single), sets(&many));
}

#[test]
fn a_combo_budget_truncates_on_the_progress_cadence() {
    let input = inputs(corpus(12, 30)); // 248,832 combinations
    let full = search(&input, &SearchOptions::default()).unwrap();
    assert!(!full.truncated);
    let limited = search(
        &input,
        &SearchOptions {
            threads: 1,
            budget: SearchBudget {
                max_combos: Some(100_000),
                time_limit: None,
            },
        },
    )
    .unwrap();
    assert!(limited.truncated);
    assert_eq!(limited.combos_total, full.combos_total);
    assert!(limited.combos_visited >= 100_000);
    assert!(limited.combos_visited < limited.combos_total);
    assert!(!limited.sets.is_empty());
    // A budget at least as large as the search never truncates.
    let roomy = search(
        &input,
        &SearchOptions {
            threads: 1,
            budget: SearchBudget {
                max_combos: Some(full.combos_total + 1),
                time_limit: None,
            },
        },
    )
    .unwrap();
    assert!(!roomy.truncated);
    assert_eq!(roomy.sets.len(), full.sets.len());
}

#[test]
fn an_expired_time_limit_truncates_on_the_first_cadence_check() {
    let input = inputs(corpus(12, 31));
    let result = search(
        &input,
        &SearchOptions {
            threads: 2,
            budget: SearchBudget {
                max_combos: None,
                time_limit: Some(std::time::Duration::ZERO),
            },
        },
    )
    .unwrap();
    assert!(result.truncated);
    assert!(result.combos_visited < result.combos_total);
}

#[test]
fn a_realistic_inventory_is_pruned_to_a_small_fraction_of_its_combinations() {
    // 18 candidates per bucket after the pre-filter is a heavy but realistic
    // hoard: 18^5 = 1,889,568 combinations. Counting evaluated sets is stable where
    // wall time is not.
    let mut input = inputs(corpus(18, 40));
    input.auto_stat_mods = true;
    input.desired_stat_ranges = ranges(|i| (if i < 2 { 20 } else { 0 }, 100));
    let started = std::time::Instant::now();
    let result = search(&input, &SearchOptions::default()).unwrap();
    eprintln!(
        "perf: combos={} processed={} valid={} elapsed={:?}",
        result.combos_total,
        result.statistics.num_processed,
        result.statistics.num_valid_sets,
        started.elapsed()
    );
    assert_eq!(result.combos_total, 18u64.pow(5));
    assert_eq!(result.combos_visited, result.combos_total);
    assert!(!result.truncated);
    assert_eq!(result.sets.len(), RETURNED_ARMOR_SETS);
    assert!(
        result.statistics.num_processed < result.combos_total / 10,
        "processed {} of {}",
        result.statistics.num_processed,
        result.combos_total
    );
}

#[test]
fn a_tie_heavy_inventory_with_auto_mods_still_completes_within_its_combination_count() {
    // Worst case for the tie-admitting `could_insert`: every item has the same stats,
    // so nothing is ever strictly worse than the retained sets. Every set is evaluated
    // (no pruning can apply) but the search must still finish and stay deterministic.
    let mut input = inputs(corpus(10, 41)); // 100,000 combinations
    input.auto_stat_mods = true;
    for item in input.buckets.iter_mut().flatten() {
        item.stats = [12; 6];
        item.is_artifice = false;
    }
    for item in input.buckets[0].iter_mut() {
        item.power = 1800;
    }
    let started = std::time::Instant::now();
    let single = search(&input, &SearchOptions::default()).unwrap();
    eprintln!(
        "perf (ties): combos={} processed={} valid={} elapsed={:?}",
        single.combos_total,
        single.statistics.num_processed,
        single.statistics.num_valid_sets,
        started.elapsed()
    );
    assert_eq!(single.combos_visited, single.combos_total);
    assert!(single.statistics.num_processed <= single.combos_total);
    let many = search(
        &input,
        &SearchOptions {
            threads: 4,
            ..SearchOptions::default()
        },
    )
    .unwrap();
    let sets = |r: &SearchResult| r.sets.iter().map(|s| s.data.clone()).collect::<Vec<_>>();
    assert_eq!(sets(&single), sets(&many));
}

// ---------------------------------------------------------------------------
// set-tracker.test.ts.

fn entry(total: i32, mix: u64, stats_total: i32, power: i32, tie: u32) -> HeapEntry<u32> {
    HeapEntry {
        enabled_stats_total: total,
        stat_mix: mix,
        stats_total,
        power,
        tie_break: ([tie, 0, 0, 0, 0], 0),
        data: tie,
    }
}

#[test]
fn tracker_returns_sets_best_first_by_total_mix_stats_total_power() {
    let mut tracker = HeapSetTracker::new(10);
    tracker.insert(entry(60, 5, 70, 1800, 1));
    tracker.insert(entry(61, 1, 61, 1700, 2));
    tracker.insert(entry(60, 6, 60, 1700, 3));
    tracker.insert(entry(60, 5, 71, 1700, 4));
    tracker.insert(entry(60, 5, 71, 1750, 5));
    let order: Vec<_> = tracker.into_sorted().into_iter().map(|e| e.data).collect();
    assert_eq!(order, [2, 3, 5, 4, 1]);
}

#[test]
fn tracker_ties_prefer_the_smaller_tie_break() {
    let mut tracker = HeapSetTracker::new(10);
    for tie in [4, 2, 9, 1] {
        tracker.insert(entry(50, 1, 50, 1800, tie));
    }
    let order: Vec<_> = tracker.into_sorted().into_iter().map(|e| e.data).collect();
    assert_eq!(order, [1, 2, 4, 9]);
}

#[test]
fn tracker_keeps_only_the_best_at_capacity_and_reports_insertion() {
    let mut tracker = HeapSetTracker::new(3);
    assert!(tracker.insert(entry(10, 0, 10, 0, 1)));
    assert!(tracker.insert(entry(30, 0, 30, 0, 2)));
    assert!(tracker.insert(entry(20, 0, 20, 0, 3)));
    // Full: inserts replace the worst and report false, like DIM's trim.
    assert!(!tracker.insert(entry(25, 0, 25, 0, 4)));
    assert!(!tracker.insert(entry(5, 0, 5, 0, 5)));
    let kept: Vec<_> = tracker.into_sorted().into_iter().map(|e| e.data).collect();
    assert_eq!(kept, [2, 4, 3]);
}

#[test]
fn tracker_could_insert_admits_totals_that_tie_the_worst() {
    let mut tracker = HeapSetTracker::new(2);
    assert!(tracker.could_insert(0));
    tracker.insert(entry(10, 0, 10, 0, 1));
    assert!(tracker.could_insert(0));
    tracker.insert(entry(20, 0, 20, 0, 2));
    assert!(!tracker.could_insert(9));
    // DIM rejects a tie (set-tracker.ts:46-48); Ghostlink admits it so the retained
    // sets do not depend on visiting order.
    assert!(tracker.could_insert(10));
    assert!(tracker.could_insert(11));
}

#[test]
fn stat_mix_packs_only_enabled_stats_in_priority_order() {
    let all = ranges(|_| (0, 200));
    assert_eq!(
        encode_stat_mix(&[1, 2, 3, 4, 5, 6], &all),
        0x0102_0304_0506u64
    );
    let ignoring = ranges(|i| (0, if i == 1 || i == 4 { 0 } else { 200 }));
    assert_eq!(
        encode_stat_mix(&[1, 2, 3, 4, 5, 6], &ignoring),
        0x01_03_04_06u64
    );
    // Values clamp to 8 bits and the encoding keeps lexical ordering.
    assert_eq!(encode_stat_mix(&[300, 0, 0, 0, 0, 0], &all), 255 << 40);
    assert!(
        encode_stat_mix(&[10, 0, 0, 0, 0, 0], &all)
            > encode_stat_mix(&[9, 200, 200, 200, 200, 200], &all)
    );
    assert_eq!(encode_stat_mix(&[1, 2, 3, 4, 5, 6], &ranges(|_| (0, 0))), 0);
}
