use destiny_core::{armor_model::ARMOR_STATS, build::StatConstraint, build_stats::*};

#[test]
fn analyzer_minima_only_last_duplicate_and_unknown_hash_ignored() {
    let values = [20, 30, 40, 50, 60, 70];
    let c = |hash, min, max| StatConstraint {
        stat_hash: hash,
        min_stat: Some(min),
        max_stat: Some(max),
    };
    assert!(satisfies_minima(
        &values,
        &[
            c(ARMOR_STATS[0], 100, 200),
            c(ARMOR_STATS[0], 20, 10),
            c(123, 200, 200)
        ]
    ));
    assert!(!satisfies_minima(&values, &[c(ARMOR_STATS[1], 31, 200)]));
    assert!(satisfies_minima(&[-10; 6], &[c(ARMOR_STATS[0], 0, 0)]));
}

#[test]
fn conditional_tuning_needs_assigned_base_zero_and_merges_first_flag() {
    use destiny_core::{ArmorDefinition, InvestmentStat};
    let mut plug = ArmorDefinition {
        investment_stats: vec![
            InvestmentStat {
                stat_type_hash: ARMOR_STATS[0],
                value: 1,
                is_conditionally_active: true,
            },
            InvestmentStat {
                stat_type_hash: ARMOR_STATS[0],
                value: 2,
                is_conditionally_active: false,
            },
        ],
        ..Default::default()
    };
    let mut values = [0; 6];
    add_plug_stats(&mut values, 3122197216, &plug, 0, Some(&[0; 6])).unwrap();
    assert_eq!(values[0], 3);
    add_plug_stats(&mut values, 3122197216, &plug, 0, Some(&[10; 6])).unwrap();
    add_plug_stats(&mut values, 3122197216, &plug, 0, None).unwrap();
    assert_eq!(values[0], 3);
    plug.investment_stats[0].is_conditionally_active = false;
    add_plug_stats(&mut values, 3122197216, &plug, 0, None).unwrap();
    assert_eq!(values[0], 6);
}

#[test]
fn subclass_class_conditionals_and_unknown_conditions_match_dim() {
    use destiny_core::{ArmorDefinition, InvestmentStat};
    let plug = ArmorDefinition {
        investment_stats: ARMOR_STATS
            .iter()
            .map(|hash| InvestmentStat {
                stat_type_hash: *hash,
                value: -10,
                is_conditionally_active: true,
            })
            .collect(),
        ..Default::default()
    };
    for hash in [2272984671, 1727069360] {
        for (class, index) in [(0, 1), (1, 0), (2, 2)] {
            let mut totals = [0; 6];
            add_plug_stats(&mut totals, hash, &plug, class, None).unwrap();
            assert_eq!(totals[index], -10);
            assert_eq!(totals[..3].iter().sum::<i32>(), -10);
            assert_eq!(totals[3..], [-10; 3]);
        }
    }
    let mut totals = [0; 6];
    add_plug_stats(&mut totals, 123, &plug, 0, None).unwrap();
    assert_eq!(totals, [-10; 6]);
    add_plug_stats(&mut totals, 3511092054, &plug, 0, None).unwrap();
    assert_eq!(totals, [-10; 6]);
}
