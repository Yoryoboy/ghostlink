//! DIM loadout-analyzer/utils.ts:16-60 (`mergeStrictUpgradeStatConstraints`) and the
//! analyzer budget (BE-008d).
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::build::StatConstraint;
use destiny_core::build_stats::font_bonuses;
use destiny_core::mod_assignment::{ARMOR_BUCKETS, AssignmentMod};
use destiny_core::optimizer::{
    AnalyzerBudget, AnalyzerInputs, ArmorCandidate, DesiredStatRange, ResolvedStatConstraint,
    desired_stat_ranges, merge_strict_upgrade_stat_constraints, resolve_stat_constraints,
    run_analyzer,
};
use std::time::Duration;

fn constraint(index: usize, min: Option<u16>, max: Option<u16>) -> StatConstraint {
    StatConstraint {
        stat_hash: ARMOR_STATS[index],
        min_stat: min,
        max_stat: max,
    }
}

/// The analyzer's "existing loadout stats" constraints: minimum is the assumed stat.
fn existing(
    constraints: &[ResolvedStatConstraint],
    stats: [i32; 6],
) -> Vec<ResolvedStatConstraint> {
    constraints
        .iter()
        .map(|c| ResolvedStatConstraint {
            min_stat: stats[ARMOR_STATS.iter().position(|h| *h == c.stat_hash).unwrap()] as u16,
            max_stat: 200,
            ..*c
        })
        .collect()
}

#[test]
fn resolved_constraints_keep_priority_order_and_mark_ignored_stats() {
    let resolved = resolve_stat_constraints(&[
        constraint(3, Some(60), None),
        constraint(0, None, Some(100)),
    ]);
    let order: Vec<_> = resolved.iter().map(|c| c.stat_hash).collect();
    assert_eq!(
        order,
        vec![
            ARMOR_STATS[3],
            ARMOR_STATS[0],
            ARMOR_STATS[1],
            ARMOR_STATS[2],
            ARMOR_STATS[4],
            ARMOR_STATS[5]
        ]
    );
    assert_eq!(
        (
            resolved[0].min_stat,
            resolved[0].max_stat,
            resolved[0].ignored
        ),
        (60, 200, false)
    );
    assert_eq!(
        (
            resolved[1].min_stat,
            resolved[1].max_stat,
            resolved[1].ignored
        ),
        (0, 100, false)
    );
    assert!(resolved[2].ignored);
    // The existing range mapping folds "ignored" into max 0 and is unchanged.
    let ranges = desired_stat_ranges(&[
        constraint(3, Some(60), None),
        constraint(0, None, Some(100)),
    ]);
    assert_eq!(
        ranges[2],
        DesiredStatRange {
            stat_hash: ARMOR_STATS[1],
            min_stat: 0,
            max_stat: 0
        }
    );
}

#[test]
fn merge_raises_minimums_to_the_existing_stats_without_implying_an_upgrade() {
    // utils.ts:36-46: saved minimum 30 <= existing 50, so the merged minimum is 50 and
    // the merged set is not automatically a strict upgrade.
    let params = resolve_stat_constraints(&[constraint(0, Some(30), None)]);
    let current = existing(&params, [50, 0, 0, 0, 0, 0]);
    let (ranges, implies) = merge_strict_upgrade_stat_constraints(Some(&current), &params);
    assert!(!implies);
    assert_eq!(
        ranges[0],
        DesiredStatRange {
            stat_hash: ARMOR_STATS[0],
            min_stat: 50,
            max_stat: 200
        }
    );
    // Ignored stats are 0/0 whatever the existing stat is (utils.ts:32-33).
    assert_eq!(
        ranges[1],
        DesiredStatRange {
            stat_hash: ARMOR_STATS[1],
            min_stat: 0,
            max_stat: 0
        }
    );
}

#[test]
fn merge_flags_a_saved_minimum_above_the_existing_stat_as_a_strict_upgrade() {
    let params = resolve_stat_constraints(&[constraint(0, Some(80), None)]);
    let current = existing(&params, [50, 0, 0, 0, 0, 0]);
    let (ranges, implies) = merge_strict_upgrade_stat_constraints(Some(&current), &params);
    assert!(implies);
    assert_eq!(ranges[0].min_stat, 80);
}

#[test]
fn merge_caps_the_existing_minimum_at_the_saved_maximum() {
    // utils.ts:42: effectiveMinStat = min(maxStat, existingMinStat).
    let params = resolve_stat_constraints(&[constraint(0, Some(0), Some(40))]);
    let current = existing(&params, [70, 0, 0, 0, 0, 0]);
    let (ranges, implies) = merge_strict_upgrade_stat_constraints(Some(&current), &params);
    assert!(!implies);
    assert_eq!(
        ranges[0],
        DesiredStatRange {
            stat_hash: ARMOR_STATS[0],
            min_stat: 40,
            max_stat: 40
        }
    );
}

#[test]
fn merge_without_existing_stats_maps_the_parameters_only() {
    let params = resolve_stat_constraints(&[constraint(2, Some(20), Some(90))]);
    let (ranges, implies) = merge_strict_upgrade_stat_constraints(None, &params);
    assert!(!implies);
    assert_eq!(
        ranges[0],
        DesiredStatRange {
            stat_hash: ARMOR_STATS[2],
            min_stat: 20,
            max_stat: 90
        }
    );
}

#[test]
fn font_bonuses_follow_the_cost_pair_table_and_cap_at_three_mods() {
    // build_stats.rs `total_stats`: [0, 20, 40, 50] per stat by Font count.
    let fonts = [
        4046357305, 686455429, 1193713026, 1781551382, 1130820873, 633101315,
    ];
    assert_eq!(font_bonuses(&[]), [0; 6]);
    assert_eq!(font_bonuses(&[fonts[1]]), [0, 20, 0, 0, 0, 0]);
    assert_eq!(font_bonuses(&[fonts[0]; 5]), [50, 0, 0, 0, 0, 0]);
}

#[test]
fn default_budget_is_two_seconds_with_a_combo_cap() {
    let budget = AnalyzerBudget::default();
    assert_eq!(budget.time_limit, std::time::Duration::from_secs(2));
    assert!(budget.max_combos > 0);
}

/// A piece with energy 9, so the default rules add no masterwork bonus to its stats (power
/// is set because the strictly-worse pre-filter ignores armor without it).
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
fn full_set(extra: Vec<ArmorCandidate>) -> Vec<ArmorCandidate> {
    let mut all: Vec<_> = (0..5)
        .map(|b| piece(&format!("L{b}"), b, [10; 6]))
        .collect();
    all.extend(extra);
    all
}
fn all_stats() -> Vec<ResolvedStatConstraint> {
    let constraints: Vec<_> = (0..6).map(|i| constraint(i, Some(0), None)).collect();
    resolve_stat_constraints(&constraints)
}
fn analyzer<'a>(
    candidates: &'a [ArmorCandidate],
    ids: &'a [String],
    constraints: &'a [ResolvedStatConstraint],
    query: Option<&'a dyn Fn(&ArmorCandidate) -> bool>,
) -> AnalyzerInputs<'a> {
    AnalyzerInputs {
        candidates,
        loadout_ids: ids,
        query_matches: query,
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
        assumed_stats: [50; 6],
        budget: AnalyzerBudget::default(),
        threads: 4,
    }
}
fn loadout_ids() -> Vec<String> {
    (0..5).map(|b| format!("L{b}")).collect()
}

#[test]
fn a_strictly_better_piece_is_found_and_replaces_the_items_finding() {
    // analysis.ts:350-367: the stat-lower pre-filter drops L0, but better stats win.
    let candidates = full_set(vec![piece("B0", 0, [30, 10, 10, 10, 10, 10])]);
    let (ids, constraints) = (loadout_ids(), all_stats());
    let everything = |_: &ArmorCandidate| true;
    let outcome = run_analyzer(&analyzer(
        &candidates,
        &ids,
        &constraints,
        Some(&everything),
    ))
    .unwrap();
    assert_eq!(outcome.better_stats, Some(true));
    assert_eq!(outcome.items_do_not_match, Some(false));
    assert_eq!(outcome.info.evaluation, "evaluated");
    assert_eq!(outcome.info.candidates, Some([1; 5]));
}

#[test]
fn the_loadout_alone_has_no_better_stats_and_no_excluded_pieces() {
    let candidates = full_set(vec![]);
    let (ids, constraints) = (loadout_ids(), all_stats());
    let outcome = run_analyzer(&analyzer(&candidates, &ids, &constraints, None)).unwrap();
    assert_eq!(
        (outcome.better_stats, outcome.items_do_not_match),
        (Some(false), Some(false))
    );
}

#[test]
fn a_query_that_excludes_a_loadout_piece_raises_the_items_finding() {
    // O0 is worse than L0, so only the query keeps L0 out (analysis.ts:293-307).
    let candidates = full_set(vec![piece("O0", 0, [5; 6])]);
    let (ids, constraints) = (loadout_ids(), all_stats());
    let not_l0 = |c: &ArmorCandidate| c.id != "L0";
    let outcome = run_analyzer(&analyzer(&candidates, &ids, &constraints, Some(&not_l0))).unwrap();
    assert_eq!(outcome.better_stats, Some(false));
    assert_eq!(outcome.items_do_not_match, Some(true));
    // Without a saved query the finding is never raised, whatever the pre-filter drops.
    let outcome = run_analyzer(&analyzer(&candidates, &ids, &constraints, None)).unwrap();
    assert_eq!(outcome.items_do_not_match, Some(false));
}

#[test]
fn an_exhausted_budget_never_guesses_the_findings() {
    // Stat 0 and 1 trade off, so no set reaches 100 in both and nothing can be pruned early.
    let mut extra = Vec::new();
    for bucket in 0..5 {
        for k in 0..12 {
            extra.push(piece(
                &format!("T{bucket}-{k}"),
                bucket,
                [2 * k, 24 - 2 * k, 0, 0, 0, 0],
            ));
        }
    }
    let candidates = full_set(extra);
    let constraints = resolve_stat_constraints(&[
        constraint(0, Some(100), None),
        constraint(1, Some(100), None),
    ]);
    let ids = loadout_ids();
    let not_l0 = |c: &ArmorCandidate| c.id != "L0";
    let mut inputs = analyzer(&candidates, &ids, &constraints, Some(&not_l0));
    inputs.assumed_stats = [100, 100, 0, 0, 0, 0];
    inputs.budget = AnalyzerBudget {
        time_limit: Duration::from_secs(60),
        max_combos: 100_000,
    };
    let outcome = run_analyzer(&inputs).unwrap();
    assert!(outcome.info.truncated);
    assert_eq!(outcome.info.evaluation, "not_evaluated");
    assert_eq!(outcome.info.reason, Some("optimizer_budget_exhausted"));
    assert_eq!(
        (outcome.better_stats, outcome.items_do_not_match),
        (None, None)
    );
    // With room the same search completes and finds nothing better.
    inputs.budget.max_combos = u64::MAX;
    let outcome = run_analyzer(&inputs).unwrap();
    assert!(!outcome.info.truncated);
    assert_eq!(outcome.better_stats, Some(false));
}

#[test]
fn locked_bucket_mods_need_projected_mod_sockets() {
    let candidates = full_set(vec![]);
    let (ids, constraints) = (loadout_ids(), all_stats());
    let mut inputs = analyzer(&candidates, &ids, &constraints, None);
    inputs
        .bucket_mods
        .insert(ARMOR_BUCKETS[0], vec![AssignmentMod { hash: 1, cost: 1 }]);
    assert_eq!(run_analyzer(&inputs), Err("bucket_mod_sockets_unavailable"));
}
