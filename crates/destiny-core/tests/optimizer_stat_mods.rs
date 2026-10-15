//! BE-008b slot-independent mod assignment, optimal stat mods and displayed stat
//! ranges, mirroring DIM 29c122e loadout-builder/process-worker/process-utils.ts and
//! process-utils.test.ts. DIM's tests need its test manifest, so these use synthetic
//! items and mod costs (see each test for the cost model it relies on).
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::optimizer::*;
use std::rc::Rc;

const TAG: u32 = 1764679361;
const OTHER_TAG: u32 = 1269555732;

fn item(index: u32, energy: i32, tag: Option<u32>) -> ProcessItem {
    ProcessItem {
        id: index.to_string(),
        hash: index,
        name: format!("Item {index}"),
        is_exotic: false,
        is_artifice: false,
        remaining_energy_capacity: energy,
        power: 1500,
        stats: [0; 6],
        compatible_activity_mod: tag,
        set_bonus: None,
        intrinsic_perks: None,
        has_set_bonus_mod_socket: false,
        included_tuning_mod: None,
        tuning_variants: None,
    }
}
fn five(energy: i32) -> Vec<ProcessItem> {
    (1..=5).map(|i| item(i, energy, None)).collect()
}
fn with_energy(items: &[ProcessItem], energies: &[i32], artifice: usize) -> Vec<ProcessItem> {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| ProcessItem {
            remaining_energy_capacity: energies[i],
            is_artifice: i < artifice,
            ..item.clone()
        })
        .collect()
}
fn repeat(m: &ProcessMod, n: usize) -> Vec<ProcessMod> {
    vec![m.clone(); n]
}
fn general(hash: u32, cost: i32) -> ProcessMod {
    ProcessMod {
        hash,
        energy_cost: cost,
        tag: None,
    }
}
fn activity(hash: u32, cost: i32, tag: u32) -> ProcessMod {
    ProcessMod {
        hash,
        energy_cost: cost,
        tag: Some(tag),
    }
}

/// Majors/minors by stat order index: hash `1000 + 10 * index + {1 minor, 2 major}`,
/// artifice `2000 + index`. `costs(index)` is `(minor, major)`.
fn auto_data(order: &[u32; 6], costs: impl Fn(usize) -> (i32, i32)) -> AutoModData {
    let mut data = AutoModData::default();
    for (i, stat) in order.iter().enumerate() {
        let (minor, major) = costs(i);
        let base = 1000 + 10 * i as u32;
        data.general_mods.insert(
            *stat,
            GeneralAutoMods {
                minor: AutoMod {
                    hash: base + 1,
                    cost: minor,
                },
                major: AutoMod {
                    hash: base + 2,
                    cost: major,
                },
            },
        );
        data.artifice_mods.insert(*stat, 2000 + i as u32);
    }
    data
}
/// Majors cost 3 and minors 1 for every stat.
fn standard() -> AutoModData {
    auto_data(&ARMOR_STATS, |_| (1, 3))
}
fn session(
    data: &AutoModData,
    generals: &[ProcessMod],
    activities: &[ProcessMod],
) -> LoSessionInfo {
    LoSessionInfo::new(data, generals, activities, true, &ARMOR_STATS)
}
fn ranges(min: u16, max: u16) -> Vec<DesiredStatRange> {
    ARMOR_STATS
        .iter()
        .map(|stat_hash| DesiredStatRange {
            stat_hash: *stat_hash,
            min_stat: min,
            max_stat: max,
        })
        .collect()
}

// Answers are permutations of multisets: 5!/(3!2!) = 10 for [1,2,1,2,1], and so on.
#[test]
fn process_mod_permutations_count_distinct_cost_and_tag_arrangements() {
    for (unique, expected) in [(1, 1), (2, 10), (3, 30), (4, 60), (5, 120)] {
        let mods: Vec<_> = (0..5).map(|i| general(7, i % unique)).collect();
        assert_eq!(generate_process_mod_permutations(&mods).len(), expected);
    }
    // Different hashes with the same cost and tag are the same permutation.
    let same = [general(1, 2), general(2, 2), general(3, 2)];
    assert_eq!(generate_process_mod_permutations(&same).len(), 10);
    // The tag distinguishes mods of equal cost.
    let tagged = [activity(1, 2, TAG), activity(2, 2, OTHER_TAG)];
    assert_eq!(generate_process_mod_permutations(&tagged).len(), 20);
    // No mods: a single all-empty permutation.
    assert_eq!(generate_process_mod_permutations(&[]), vec![vec![None; 5]]);
}

#[test]
fn process_mod_permutations_start_with_the_mods_in_order_padded_with_none() {
    let mods = [general(1, 1), general(2, 2)];
    let permutations = generate_process_mod_permutations(&mods);
    assert_eq!(
        permutations[0],
        vec![Some(general(1, 1)), Some(general(2, 2)), None, None, None]
    );
    assert!(permutations.iter().all(|p| p.len() == 5));
}

fn can_take(generals: &[ProcessMod], activities: &[ProcessMod], items: &[ProcessItem]) -> bool {
    let empty = AutoModData::default();
    let info = LoSessionInfo::new(&empty, generals, activities, false, &ARMOR_STATS);
    pick_and_assign_slot_independent_mods(
        &info,
        &mut ModAssignmentStatistics::default(),
        items,
        Some(&[0; 6]),
        0,
    )
    .is_some()
}

#[test]
fn slot_independent_mods_fit_when_there_is_energy_and_slots() {
    let items = five(10);
    let g = general(1, 3);
    assert!(can_take(&[], &[], &items));
    assert!(can_take(
        &repeat(&g, 5),
        &[],
        &with_energy(&items, &[3; 5], 0)
    ));
    assert!(!can_take(
        &repeat(&g, 5),
        &[],
        &with_energy(&items, &[3, 3, 3, 3, 2], 0)
    ));
    for i in 0..5 {
        let mut energies = [0; 5];
        energies[i] = 3;
        assert!(
            can_take(
                std::slice::from_ref(&g),
                &[],
                &with_energy(&items, &energies, 0)
            ),
            "{i}"
        );
    }
}

#[test]
fn activity_mods_need_a_matching_item_with_enough_energy() {
    let a = activity(9, 2, TAG);
    let tagged =
        |tag: u32| -> Vec<ProcessItem> { (1..=5).map(|i| item(i, 2, Some(tag))).collect() };
    assert!(can_take(&[], &repeat(&a, 5), &tagged(TAG)));
    assert!(!can_take(&[], &repeat(&a, 5), &tagged(OTHER_TAG)));
    for index in 0..5 {
        let items: Vec<_> = (0..5)
            .map(|i| item(i as u32, 2, (i == index).then_some(TAG)))
            .collect();
        assert!(can_take(&[], std::slice::from_ref(&a), &items), "{index}");
    }
    // Not enough energy in the one matching item.
    let mut items = five(10);
    items[4] = item(5, 1, Some(TAG));
    assert!(!can_take(&[], std::slice::from_ref(&a), &items));
}

#[test]
fn general_activity_and_combined_costs_must_fit_together() {
    let mut items = five(0);
    items[4] = item(5, 6, Some(TAG));
    assert!(can_take(&[general(1, 3)], &[activity(2, 3, TAG)], &items));
    items[4] = item(5, 5, Some(TAG));
    assert!(!can_take(&[general(1, 3)], &[activity(2, 3, TAG)], &items));
    // Total energy suffices (the other items have plenty) but the activity mod
    // only fits its matching item.
    let mut items = five(10);
    items[4] = item(5, 1, Some(TAG));
    assert!(!can_take(&[general(1, 6)], &[activity(2, 5, TAG)], &items));
    assert!(!can_take(&[general(1, 5)], &[activity(2, 6, TAG)], &items));
}

#[test]
fn slot_independent_mods_without_needed_stats_only_check_general_fit() {
    let data = standard();
    let info = session(&data, &[general(1, 3)], &[]);
    let mut stats = ModAssignmentStatistics::default();
    let items = five(10);
    let picks = pick_and_assign_slot_independent_mods(&info, &mut stats, &items, None, 0);
    assert_eq!(picks, Some(vec![]));
    // Enough total energy but no item holds the mod: the assignment itself fails.
    let low = with_energy(&items, &[1; 5], 0);
    assert!(pick_and_assign_slot_independent_mods(&info, &mut stats, &low, None, 0).is_none());
    assert_eq!(stats.final_assignment.mods_assignment_failed, 1);
    assert_eq!(stats.early_mods_check.times_failed, 0);
    // Too little total energy fails the early check before any assignment.
    let tiny = with_energy(&items, &[1, 1, 0, 0, 0], 0);
    assert!(pick_and_assign_slot_independent_mods(&info, &mut stats, &tiny, None, 0).is_none());
    assert_eq!(stats.early_mods_check.times_failed, 1);
    assert_eq!(stats.early_mods_check.times_checked, 3);
    assert_eq!(stats.final_assignment.mod_assignment_attempted, 2);
    assert_eq!(stats.final_assignment.mods_assignment_failed, 1);
    assert_eq!(stats.final_assignment.auto_mods_assignment_failed, 0);
}

#[test]
fn activity_tag_counts_fail_the_early_check() {
    let data = standard();
    let info = session(&data, &[], &[activity(1, 1, TAG), activity(2, 1, TAG)]);
    let mut stats = ModAssignmentStatistics::default();
    let mut items = five(10);
    items[0].compatible_activity_mod = Some(TAG);
    assert!(
        pick_and_assign_slot_independent_mods(&info, &mut stats, &items, Some(&[0; 6]), 0)
            .is_none()
    );
    assert_eq!(stats.early_mods_check.times_failed, 1);
    assert_eq!(stats.final_assignment.mod_assignment_attempted, 0);
    items[3].compatible_activity_mod = Some(TAG);
    assert!(
        pick_and_assign_slot_independent_mods(&info, &mut stats, &items, Some(&[0; 6]), 0)
            .is_some()
    );
}

/// Worked example of process-utils.test.ts (see optimizer_auto_mods.rs): the cost-1
/// activity mod must take a tagged item for the stat mods to fit afterwards.
#[test]
fn auto_mods_are_picked_after_assigning_the_activity_mod() {
    let data = auto_data(&ARMOR_STATS, |i| (if i == 4 { 1 } else { 2 }, 3));
    let user = [general(7, 3), general(8, 4)];
    let info = session(&data, &user, &[activity(9, 1, TAG)]);
    let mut items = five(0);
    for (i, (energy, tag)) in [
        (3, None),
        (4, Some(TAG)),
        (1, None),
        (3, Some(TAG)),
        (4, None),
    ]
    .into_iter()
    .enumerate()
    {
        items[i] = item(i as u32 + 1, energy, tag);
    }
    let mut stats = ModAssignmentStatistics::default();
    let needs = [4, 0, 10, 12, 4, 0];
    let picks = pick_and_assign_slot_independent_mods(&info, &mut stats, &items, Some(&needs), 4)
        .expect("solvable");
    let hashes: Vec<_> = picks.iter().map(|p| p.mod_hashes.clone()).collect();
    assert_eq!(
        hashes,
        [vec![2000, 2000], vec![1022], vec![1032, 2003], vec![1041]]
    );
    assert_eq!(stats.final_assignment.auto_mods_assignment_failed, 0);
    // The 4-energy piece loses its energy: the activity mod still fits the other tagged
    // piece, but the 3 energy left over cannot pay for the stat mods.
    let mut short = items.clone();
    short[1].remaining_energy_capacity = 0;
    assert!(
        pick_and_assign_slot_independent_mods(&info, &mut stats, &short, Some(&needs), 4).is_none()
    );
    // The activity mod fit somewhere, so the failure is attributed to the auto mods.
    assert_eq!(stats.final_assignment.auto_mods_assignment_failed, 1);
    // Higher stats than any pick can reach.
    let higher = [4, 0, 10, 12, 4, 30];
    assert!(
        pick_and_assign_slot_independent_mods(&info, &mut stats, &items, Some(&higher), 4)
            .is_none()
    );
}

fn final_stats(
    set_stats: [i32; 6],
    energies: [i32; 5],
    artifice: usize,
    desired: &[DesiredStatRange],
) -> [i32; 6] {
    let order: [u32; 6] = std::array::from_fn(|i| desired[i].stat_hash);
    let info = LoSessionInfo::new(&standard(), &[], &[], true, &order);
    let items = with_energy(&five(10), &energies, artifice);
    let picked = pick_optimal_stat_mods(&info, &items, &set_stats, desired, None, None).unwrap();
    let mut stats = set_stats;
    for (stat, bonus) in stats.iter_mut().zip(picked.bonus_stats) {
        *stat += bonus;
    }
    stats
}

/// process-utils.test.ts tierCases, with majors costing 3 and minors 1 (artifice free).
#[test]
fn optimal_mods_reach_the_best_stats_in_priority_order() {
    let default = ranges(30, 80);
    let by_stat = |order: [usize; 6], maxes: [u16; 6]| -> Vec<DesiredStatRange> {
        order
            .iter()
            .zip(maxes)
            .map(|(i, max_stat)| DesiredStatRange {
                stat_hash: ARMOR_STATS[*i],
                min_stat: 0,
                max_stat,
            })
            .collect()
    };
    type Case = ([i32; 6], [i32; 5], usize, [i32; 6], Vec<DesiredStatRange>);
    let cases: Vec<Case> = vec![
        // Two cheap majors: one to Health (70 -> 80) and one to Grenade (40 -> 50).
        (
            [80, 70, 80, 40, 30, 30],
            [0, 3, 0, 3, 0],
            0,
            [80, 80, 80, 50, 30, 30],
            default.clone(),
        ),
        // Artifice plus a cheap minor combine on the first stat (all to the first stat).
        (
            [63, 70, 59, 35, 30, 30],
            [2, 0, 0, 0, 0],
            3,
            [77, 70, 59, 35, 30, 30],
            default.clone(),
        ),
        (
            [68, 66, 30, 30, 30, 30],
            [0; 5],
            4,
            [80, 66, 30, 30, 30, 30],
            default.clone(),
        ),
        // Hit the minimum of the fifth stat first, then spend what is left.
        (
            [68, 66, 30, 30, 11, 30],
            [2, 2, 0, 0, 0],
            4,
            [71, 66, 30, 30, 30, 30],
            default.clone(),
        ),
        // Negative stats are not clamped before the minimum is computed.
        (
            [30, 61, 30, 30, 30, -14],
            [5; 5],
            5,
            [50, 61, 30, 30, 30, 31],
            default.clone(),
        ),
        // Regression in DIM: two +10 mods go to the fourth stat, not the fifth.
        (
            [82, 44, 73, 39, 40, 50],
            [10; 5],
            5,
            [82, 80, 82, 59, 40, 50],
            by_stat([3, 5, 2, 4, 0, 1], [82, 82, 82, 200, 200, 200]),
        ),
    ];
    for (stats, energies, artifice, expected, desired) in cases {
        assert_eq!(
            final_stats(stats, energies, artifice, &desired),
            expected,
            "{stats:?} {energies:?} {artifice}"
        );
    }
}

#[test]
fn optimal_mods_report_exact_stats_for_dim_cases() {
    let desired = ranges(30, 80);
    assert_eq!(
        final_stats([18, 80, 80, 26, 80, 30], [0, 0, 0, 3, 0], 4, &desired),
        [34, 80, 80, 32, 80, 30]
    );
    // Same problem with reordered stats: the same total, spent differently.
    assert_eq!(
        final_stats([26, 80, 80, 18, 80, 30], [0, 0, 0, 3, 0], 4, &desired),
        [36, 80, 80, 30, 80, 30]
    );
}

#[test]
fn optimal_mods_return_the_mod_hashes_and_bonus_per_stat() {
    let info = session(&standard(), &[], &[]);
    let items = with_energy(&five(10), &[3, 0, 0, 0, 0], 1);
    let picked = pick_optimal_stat_mods(
        &info,
        &items,
        &[80, 60, 30, 30, 30, 30],
        &ranges(30, 80),
        None,
        None,
    )
    .unwrap();
    // Only one item has energy, so one general mod: a major plus the artifice mod is the
    // largest Health boost (+13); a second major would not fit.
    assert_eq!(picked.bonus_stats, [0, 13, 0, 0, 0, 0]);
    assert_eq!(picked.mods, [1012, 2001]);
}

/// The cost model: Health, Class and Super are "expensive" (cost 2), the others cost 1,
/// minor and major alike. All items have 1 or 2 energy, the activity mods cost 1, 1, 1, 2
/// and the exotic piece cannot take any, so only energy-1 slots remain.
#[test]
fn only_stats_whose_mods_actually_fit_are_reported() {
    let order = [
        ARMOR_STATS[1],
        ARMOR_STATS[2],
        ARMOR_STATS[5],
        ARMOR_STATS[3],
        ARMOR_STATS[4],
        ARMOR_STATS[0],
    ];
    let data = auto_data(&order, |i| {
        if matches!(i, 0 | 1 | 4) {
            (2, 2)
        } else {
            (1, 1)
        }
    });
    let activities = [
        activity(1, 1, TAG),
        activity(2, 1, TAG),
        activity(3, 1, TAG),
        activity(4, 2, TAG),
    ];
    let info = LoSessionInfo::new(&data, &[], &activities, true, &order);
    let mut items = vec![
        item(1, 2, Some(TAG)),
        item(2, 1, None),
        item(3, 1, Some(TAG)),
        item(4, 1, Some(TAG)),
        item(5, 2, Some(TAG)),
    ];
    items[1].is_exotic = true;
    let mut stats = ModAssignmentStatistics::default();
    assert!(
        pick_and_assign_slot_independent_mods(&info, &mut stats, &items, Some(&[0; 6]), 0)
            .is_some()
    );
    let desired: Vec<_> = order
        .iter()
        .map(|stat_hash| DesiredStatRange {
            stat_hash: *stat_hash,
            min_stat: 0,
            max_stat: 200,
        })
        .collect();
    let set = [55, 55, 55, 50, 50, 50];
    let picked = pick_optimal_stat_mods(&info, &items, &set, &desired, None, None).unwrap();
    // Two cheap slots with one energy left each: two +10 mods on the first cheap stat.
    assert_eq!(picked.bonus_stats, [0, 0, 20, 0, 0, 0]);
    assert_eq!(picked.mods, [1022, 1022]);
    let mut max_stats = [MinMaxStat::default(); 6];
    let improved = update_max_stats(&info, &items, &set, 0, &desired, &mut max_stats, None);
    assert!(improved);
    assert_eq!(
        max_stats.map(|stat| stat.max_stat),
        [55, 55, 75, 70, 50, 70]
    );
}

#[test]
fn optimal_mods_edge_cases() {
    // No energy at all: a defined, empty result even for an impossible minimum.
    let info = session(&standard(), &[], &[]);
    let items = five(0);
    let picked =
        pick_optimal_stat_mods(&info, &items, &[0; 6], &ranges(100, 100), None, None).unwrap();
    assert_eq!(picked.bonus_stats, [0; 6]);
    assert!(picked.mods.is_empty());
    // Auto stat mods off: nothing to pick even with plenty of energy.
    let off = LoSessionInfo::new(&standard(), &[], &[], false, &ARMOR_STATS);
    let rich = five(10);
    let picked = pick_optimal_stat_mods(
        &off,
        &rich,
        &[65, 42, 60, 76, 60, 87],
        &ranges(0, 200),
        None,
        None,
    )
    .unwrap();
    assert_eq!(picked.bonus_stats, [0; 6]);
    assert!(picked.mods.is_empty());
    // No valid activity mod assignment: no result at all.
    let blocked = session(&standard(), &[], &[activity(1, 1, TAG)]);
    assert!(
        pick_optimal_stat_mods(&blocked, &rich, &[50; 6], &ranges(0, 200), None, None).is_none()
    );
}

#[test]
fn ignored_stats_and_the_maximum_cap_added_points() {
    let info = session(&standard(), &[], &[]);
    let items = five(10);
    // An ignored stat (max 0) gets nothing; the cap keeps stats from exceeding their maximum.
    let mut desired = ranges(0, 200);
    desired[0].max_stat = 0;
    desired[1].max_stat = 62;
    let picked = pick_optimal_stat_mods(
        &info,
        &items,
        &[40, 50, 50, 50, 50, 50],
        &desired,
        None,
        None,
    )
    .unwrap();
    assert_eq!(picked.bonus_stats[0], 0);
    assert!(picked.bonus_stats[1] <= 12 && picked.bonus_stats[1] > 0);
}

#[test]
fn greedy_pick_returns_nothing_without_energy_or_artifice() {
    let info = session(&standard(), &[], &[]);
    let max_added = [50; 6];
    let none = greedy_pick_stat_mods(&info, &mut [0; 6], &max_added, 0, &[[0; 5]], 0);
    assert!(none.is_empty());
    // The needed minimums are unreachable: nothing is picked at all.
    let none = greedy_pick_stat_mods(
        &info,
        &mut [90, 0, 0, 0, 0, 0],
        &max_added,
        5,
        &[[3; 5]],
        15,
    );
    assert!(none.is_empty());
}

fn range_case(
    stats: [i32; 6],
    mut maxes: [i32; 6],
    min: u16,
    max: impl Fn(usize) -> u16,
) -> (bool, [i32; 6]) {
    let info = session(&standard(), &[], &[]);
    let desired: Vec<_> = ARMOR_STATS
        .iter()
        .enumerate()
        .map(|(i, stat_hash)| DesiredStatRange {
            stat_hash: *stat_hash,
            min_stat: min,
            max_stat: max(i),
        })
        .collect();
    let mut ranges = maxes.map(|max_stat| MinMaxStat {
        min_stat: 0,
        max_stat,
    });
    let found = update_max_stats(&info, &five(10), &stats, 0, &desired, &mut ranges, None);
    maxes = ranges.map(|r| r.max_stat);
    (found, maxes)
}

/// process-utils.test.ts "process-utils updateMaxStats".
#[test]
fn update_max_stats_tracks_minimums_improvements_and_the_cap() {
    // Raised to the minimum, but every other stat needs +10 majors that use all five
    // slots, so nothing is left to improve.
    let (found, maxes) = range_case([50; 6], [55; 6], 60, |_| 100);
    assert!(!found);
    assert_eq!(maxes[0], 60);
    // The value beats the running maximum, and five majors reach 135.
    let (found, maxes) = range_case(
        [85, 50, 50, 50, 50, 50],
        [70, 50, 50, 50, 50, 50],
        30,
        |i| {
            if i == 0 { 100 } else { 70 }
        },
    );
    assert!(found);
    assert_eq!(maxes[0], 135);
    // A stat already at the maximum is skipped.
    let (found, maxes) = range_case(
        [180, 50, 50, 50, 50, 50],
        [200, 50, 50, 50, 50, 50],
        30,
        |_| 200,
    );
    assert!(found);
    assert_eq!(maxes[0], 200);
}

#[test]
fn update_max_stats_without_mod_capacity_only_tracks_the_sets() {
    let off = LoSessionInfo::new(&standard(), &[], &[], false, &ARMOR_STATS);
    let desired = ranges(0, 200);
    let mut max_stats = [MinMaxStat::default(); 6];
    let found = update_max_stats(
        &off,
        &five(10),
        &[10, 20, 30, 40, 50, 60],
        0,
        &desired,
        &mut max_stats,
        None,
    );
    assert!(found);
    assert_eq!(max_stats.map(|s| s.max_stat), [10, 20, 30, 40, 50, 60]);
    // A fixed range (min == max) cannot be improved.
    let fixed = ranges(50, 50);
    let mut max_stats = [MinMaxStat::default(); 6];
    assert!(!update_max_stats(
        &off,
        &five(10),
        &[50; 6],
        0,
        &fixed,
        &mut max_stats,
        None
    ));
    assert_eq!(max_stats.map(|s| s.max_stat), [50; 6]);
}

#[test]
fn update_max_stats_uses_artifice_mods_and_memoizes_the_boost_search() {
    let info = session(&standard(), &[], &[]);
    let desired = ranges(0, 200);
    let items = with_energy(&five(10), &[0; 5], 0);
    let mut max_stats = [MinMaxStat::default(); 6];
    // Without energy only artifice mods help: 2 mods add 6 points to any one stat.
    assert!(update_max_stats(
        &info,
        &items,
        &[50; 6],
        2,
        &desired,
        &mut max_stats,
        None
    ));
    assert_eq!(max_stats.map(|s| s.max_stat), [56; 6]);
    assert!(info.max_boost_memo_size() > 0);
    // The same energy profile is asked again for other values: still exact.
    let mut again = [MinMaxStat::default(); 6];
    update_max_stats(&info, &items, &[60; 6], 2, &desired, &mut again, None);
    assert_eq!(again.map(|s| s.max_stat), [66; 6]);
}

#[test]
fn set_energy_cache_is_shared_between_range_and_pick_calls() {
    let info = session(&standard(), &[], &[]);
    let desired = ranges(0, 200);
    let items = five(10);
    let mut cache = SetEnergyCache::default();
    let mut max_stats = [MinMaxStat::default(); 6];
    update_max_stats(
        &info,
        &items,
        &[50; 6],
        0,
        &desired,
        &mut max_stats,
        Some(&mut cache),
    );
    let first = cache.result.clone().expect("computed once for the set");
    pick_optimal_stat_mods(&info, &items, &[50; 6], &desired, Some(0), Some(&mut cache)).unwrap();
    assert!(Rc::ptr_eq(&first, cache.result.as_ref().unwrap()));
    assert_eq!(first.set_energy, 50);
    assert_eq!(first.remaining_energies_per_assignment, vec![[10; 5]]);
}

#[test]
fn process_mod_tags_only_activity_mods() {
    use destiny_core::{
        ArmorDefinition, BucketHash, DefinitionVersion, ItemDefinition, ItemHash, PlugEnergyCost,
    };
    let definition = |category: Option<u32>, cost: Option<i32>| ItemDefinition {
        hash: ItemHash::new(77),
        bucket_hash: Some(BucketHash::new(1)),
        name: "Mod".into(),
        tier: None,
        item_type: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            plug_category_hash: category,
            energy_cost: cost.map(|energy_cost| PlugEnergyCost {
                energy_cost,
                energy_type: 0,
                energy_type_hash: 0,
            }),
            ..Default::default()
        },
        non_transferrable: None,
        is_instance_item: None,
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: None,
        stack_unique_label: None,
    };
    // 13646368 is an activity (specialty socket) plug category, 2487827355 the general one.
    assert_eq!(
        process_mod(&definition(Some(13646368), Some(3))),
        activity(77, 3, 13646368)
    );
    assert_eq!(
        process_mod(&definition(Some(2487827355), Some(4))),
        general(77, 4)
    );
    assert_eq!(process_mod(&definition(None, None)), general(77, 0));
}
