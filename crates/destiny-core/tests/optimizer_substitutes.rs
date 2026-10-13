//! Substitutes search for a stored build (BE-008e): DIM's Loadout Optimizer ranges, limit,
//! deterministic order and budget.
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::build::StatConstraint;
use destiny_core::mod_assignment::ARMOR_BUCKETS;
use destiny_core::optimizer::{
    AnalyzerBudget, AnalyzerInputs, ArmorCandidate, DEFAULT_SUBSTITUTES, MAX_SUBSTITUTES,
    ResolvedStatConstraint, SUBSTITUTES_TIME_LIMIT, resolve_stat_constraints, run_substitutes,
};
use std::time::Duration;

fn constraint(index: usize, min: Option<u16>, max: Option<u16>) -> StatConstraint {
    StatConstraint {
        stat_hash: ARMOR_STATS[index],
        min_stat: min,
        max_stat: max,
    }
}
fn piece(id: &str, bucket: usize, stats: [i32; 6]) -> ArmorCandidate {
    ArmorCandidate {
        id: id.into(),
        hash: 100 + bucket as u32,
        name: format!("piece {bucket}"),
        bucket: ARMOR_BUCKETS[bucket],
        power: Some(1900),
        energy_capacity: Some(9),
        base_stats: stats,
        ..Default::default()
    }
}
fn inputs<'a>(
    candidates: &'a [ArmorCandidate],
    ids: &'a [String],
    constraints: &'a [ResolvedStatConstraint],
) -> AnalyzerInputs<'a> {
    AnalyzerInputs {
        candidates,
        loadout_ids: ids,
        query_matches: None,
        locked_exotic: None,
        energy_rules: Default::default(),
        saved_set_bonuses: Default::default(),
        loadout_set_bonuses: Vec::new(),
        bucket_mods: Default::default(),
        general_mods: Vec::new(),
        activity_mods: Vec::new(),
        mod_stat_totals: [0; 6],
        auto_mod_options: Default::default(),
        auto_stat_mods: false,
        constraints,
        assumed_stats: [0; 6],
        budget: AnalyzerBudget::substitutes(),
        threads: 4,
    }
}
fn constraints_for_all(min: [u16; 6], max: [u16; 6]) -> Vec<ResolvedStatConstraint> {
    let constraints: Vec<_> = (0..6)
        .map(|i| constraint(i, Some(min[i]), Some(max[i])))
        .collect();
    resolve_stat_constraints(&constraints)
}

#[test]
fn the_defaults_are_ten_sets_two_hundred_at_most_and_a_ten_second_budget() {
    assert_eq!(DEFAULT_SUBSTITUTES, 10);
    assert_eq!(MAX_SUBSTITUTES, 200);
    assert_eq!(SUBSTITUTES_TIME_LIMIT, Duration::from_secs(10));
    assert_eq!(
        AnalyzerBudget::substitutes().time_limit,
        SUBSTITUTES_TIME_LIMIT
    );
}

#[test]
fn sets_are_ranked_best_first_and_cut_to_the_limit() {
    // Two pieces per bucket that trade stat 0 for stat 1, so neither is strictly worse.
    let mut candidates = Vec::new();
    for b in 0..5 {
        candidates.push(piece(&format!("A{b}"), b, [10, 20, 0, 0, 0, 0]));
        candidates.push(piece(&format!("B{b}"), b, [20, 10, 0, 0, 0, 0]));
    }
    let constraints = constraints_for_all([0; 6], [200; 6]);
    let ids: Vec<String> = (0..5).map(|b| format!("A{b}")).collect();
    let outcome = run_substitutes(&inputs(&candidates, &ids, &constraints), 3).unwrap();
    assert_eq!(outcome.sets.len(), 3);
    assert_eq!(
        outcome.sets[0].armor,
        ["B0", "B1", "B2", "B3", "B4"].map(String::from)
    );
    assert_eq!(outcome.sets[0].stats[0], 100);
    assert_eq!(outcome.sets[0].armor_stats[0], 100);
    // The next best swaps one B for an A (90), in item-id order for the tie.
    assert_eq!(outcome.sets[1].stats[0], 90);
    assert_eq!(outcome.sets[2].stats[0], 90);
    assert_eq!(
        outcome.sets[1].armor,
        ["A0", "B1", "B2", "B3", "B4"].map(String::from)
    );
    assert_eq!(outcome.stat_order[0], ARMOR_STATS[0]);
    assert!(!outcome.info.truncated);
    assert_eq!(outcome.info.combos_total, Some(32));
    // A limit above the maximum never returns more than the tracker keeps.
    let all = run_substitutes(&inputs(&candidates, &ids, &constraints), 10_000).unwrap();
    assert_eq!(all.sets.len(), 32);
}

#[test]
fn saved_minimums_and_maximums_both_shape_the_results() {
    let mut candidates = Vec::new();
    for b in 0..5 {
        candidates.push(piece(&format!("A{b}"), b, [10, 20, 0, 0, 0, 0]));
        candidates.push(piece(&format!("B{b}"), b, [20, 10, 0, 0, 0, 0]));
    }
    let ids: Vec<String> = (0..5).map(|b| format!("A{b}")).collect();
    // Minimum 85: only the all-B set and the five sets with one A reach it.
    let min = constraints_for_all([85, 0, 0, 0, 0, 0], [200; 6]);
    let outcome = run_substitutes(&inputs(&candidates, &ids, &min), 200).unwrap();
    assert_eq!(outcome.sets.len(), 6);
    assert!(outcome.sets.iter().all(|s| s.stats[0] >= 90));
    // A set that only just meets every minimum is still listed: substitutes are not the
    // analyzer's strict upgrades.
    let exact = constraints_for_all([100, 50, 0, 0, 0, 0], [200; 6]);
    let outcome = run_substitutes(&inputs(&candidates, &ids, &exact), 200).unwrap();
    assert_eq!(outcome.sets.len(), 1);
    assert_eq!(outcome.sets[0].stats[..2], [100, 50]);
    // A maximum of 60 on stat 0: past it the surplus counts for nothing and costs stat 1,
    // so the sets at 60 lead, the lowest item ids first.
    let max = constraints_for_all([0; 6], [60, 200, 200, 200, 200, 200]);
    let outcome = run_substitutes(&inputs(&candidates, &ids, &max), 3).unwrap();
    assert!(outcome.sets.iter().all(|s| s.stats[0] == 60));
    assert_eq!(
        outcome.sets[0].armor,
        ["A0", "A1", "A2", "A3", "B4"].map(String::from)
    );
}

#[test]
fn the_order_never_depends_on_the_thread_count_or_candidate_order() {
    let mut candidates = Vec::new();
    for b in 0..5 {
        for k in 0..3 {
            candidates.push(piece(&format!("T{b}-{k}"), b, [10, 0, 0, 0, 0, 0]));
        }
    }
    let constraints = constraints_for_all([0; 6], [200; 6]);
    let ids: Vec<String> = (0..5).map(|b| format!("T{b}-0")).collect();
    let mut one = inputs(&candidates, &ids, &constraints);
    one.threads = 1;
    let first = run_substitutes(&one, 10).unwrap().sets;
    let mut reversed: Vec<_> = candidates.iter().rev().cloned().collect();
    let mut many = inputs(&reversed, &ids, &constraints);
    many.threads = 5;
    assert_eq!(run_substitutes(&many, 10).unwrap().sets, first);
    reversed.clear();
    assert_eq!(
        first[0].armor,
        ["T0-0", "T1-0", "T2-0", "T3-0", "T4-0"].map(String::from)
    );
}

#[test]
fn an_exhausted_budget_returns_partial_sets_flagged_truncated() {
    let mut candidates = Vec::new();
    for b in 0..5 {
        for k in 0..16 {
            candidates.push(piece(
                &format!("T{b}-{k}"),
                b,
                [2 * k, 32 - 2 * k, 0, 0, 0, 0],
            ));
        }
    }
    let constraints = constraints_for_all([0; 6], [200; 6]);
    let ids = vec!["T0-0".to_string()];
    let mut limited = inputs(&candidates, &ids, &constraints);
    limited.budget = AnalyzerBudget {
        time_limit: Duration::from_secs(60),
        max_combos: 100_000,
    };
    let outcome = run_substitutes(&limited, 10).unwrap();
    assert!(outcome.info.truncated);
    assert_eq!(outcome.info.evaluation, "evaluated");
    assert!(outcome.info.combos_visited.unwrap() < outcome.info.combos_total.unwrap());
    assert!(!outcome.sets.is_empty());
}
