use destiny_core::{armor_catalog::*, armor_model::*, *};

#[test]
fn tuning_uses_visible_socket_and_raw_membership_not_selectable_flags() {
    let definition = ArmorDefinition {
        plug_category_hash: Some(3481777685),
        ..Default::default()
    };
    let mut sockets = vec![domain::IndexedSocket {
        plug_objectives: None,
        reusable_plugs: None,
        socket_index: 7,
        plug_hash: Some(ItemHash::new(3122197216)),
        enabled: false,
        visible: true,
    }];
    let tuning = armor_tuning(&sockets, |_| Some(&definition)).unwrap();
    assert_eq!(tuning.socket_index, 7);
    assert_eq!(tuning.plug_hash, 3122197216);
    assert_eq!(tuning.tuned_stat_hash, None);
    sockets[0].reusable_plugs = Some(vec![domain::ReusablePlug {
        plug_hash: ItemHash::new(309000506),
        can_insert: false,
        enabled: false,
    }]);
    assert_eq!(
        armor_tuning(&sockets, |_| Some(&definition))
            .unwrap()
            .tuned_stat_hash,
        Some(1735777505)
    );
    sockets[0]
        .reusable_plugs
        .as_mut()
        .unwrap()
        .push(domain::ReusablePlug {
            plug_hash: ItemHash::new(311164277),
            can_insert: true,
            enabled: true,
        });
    assert_eq!(
        armor_tuning(&sockets, |_| Some(&definition))
            .unwrap()
            .tuned_stat_hash,
        None
    );
    sockets[0].visible = false;
    assert!(armor_tuning(&sockets, |_| Some(&definition)).is_none());
    sockets[0].visible = true;
    assert!(armor_tuning(&sockets, |_| None).is_none());
}

#[test]
fn tuning_hash_mapping_matches_all_dim_directions() {
    let directions = [
        (
            1735777505,
            [309000506, 455024236, 1672416975, 1922571986, 4116389173],
        ),
        (
            4244567218,
            [311164277, 534630542, 4020349587, 4164883102, 4210715468],
        ),
        (
            1943323491,
            [323635379, 957763733, 1510949672, 1879022254, 4030660414],
        ),
        (
            392767087,
            [388618952, 2125798995, 3310526732, 3681082702, 4088823605],
        ),
        (
            144602215,
            [673231129, 2244422610, 3554800389, 3946669007, 4026414261],
        ),
        (
            2996146975,
            [691392383, 891771298, 1918710127, 3121760799, 3284443097],
        ),
    ];
    let raw = |hash| domain::ReusablePlug {
        plug_hash: ItemHash::new(hash),
        can_insert: false,
        enabled: false,
    };
    assert_eq!(tuned_stat(&[]), None);
    assert_eq!(tuned_stat(&[raw(3122197216)]), None);
    for (stat, hashes) in directions {
        for hash in hashes {
            assert_eq!(
                tuned_stat(&[raw(hash), raw(hash), raw(3122197216)]),
                Some(stat)
            );
        }
    }
}

#[test]
fn fragment_capacity_sums_inserted_aspects_without_enabled_or_visibility_gate() {
    let item = ArmorDefinition {
        socket_categories: vec![DefinitionSocketCategory {
            socket_category_hash: 2047681910,
            socket_indexes: vec![3, 7],
        }],
        ..Default::default()
    };
    let aspect = ArmorDefinition {
        aspect_capacity: Some(2),
        ..Default::default()
    };
    let sockets: Vec<_> = [3, 7]
        .into_iter()
        .map(|socket_index| domain::IndexedSocket {
            plug_objectives: None,
            reusable_plugs: None,
            socket_index,
            plug_hash: Some(ItemHash::new(99)),
            enabled: false,
            visible: false,
        })
        .collect();
    assert_eq!(
        subclass_fragment_capacity(&item, Some(&sockets), |_| Some(&aspect)),
        Some(4)
    );
    assert_eq!(
        subclass_fragment_capacity(&item, Some(&sockets), |_| None),
        None
    );
    assert_eq!(
        subclass_fragment_capacity(&item, None, |_| Some(&aspect)),
        None
    );
    let empty: Vec<domain::IndexedSocket> = sockets
        .iter()
        .cloned()
        .map(|mut s| {
            s.plug_hash = None;
            s
        })
        .collect();
    assert_eq!(
        subclass_fragment_capacity(&item, Some(&empty), |_| None),
        Some(0)
    );
    assert_eq!(
        subclass_fragment_capacity(&item, Some(&sockets[..1]), |_| Some(&aspect)),
        None
    );
    let no_capacity = ArmorDefinition::default();
    assert_eq!(
        subclass_fragment_capacity(&item, Some(&sockets), |_| Some(&no_capacity)),
        Some(0)
    );
}

fn investment(hash: u32, value: i32, conditional: bool) -> InvestmentStat {
    InvestmentStat {
        stat_type_hash: hash,
        value,
        is_conditionally_active: conditional,
    }
}
fn plug(hash: u32, definition: &ArmorDefinition) -> ModelPlug<'_> {
    ModelPlug {
        hash,
        enabled: true,
        definition: Some(definition),
    }
}
fn group() -> StatGroupDefinition {
    StatGroupDefinition {
        hash: 1,
        maximum_value: Some(100),
        scaled_stats: ARMOR_STATS
            .iter()
            .map(|&hash| ScaledStat {
                stat_hash: hash,
                maximum_value: 100,
                display_as_numeric: false,
                display_interpolation: vec![
                    InterpolationPoint {
                        value: 0,
                        weight: 0,
                    },
                    InterpolationPoint {
                        value: 100,
                        weight: 100,
                    },
                ],
            })
            .collect(),
    }
}
#[test]
fn interpolation_matches_dim_piecewise_and_bankers_rounding() {
    // DIM inventory/store/stats.ts:614-651: clamp input, linear segments, ties to even.
    let mut display = group().scaled_stats[0].clone();
    display.stat_hash = 1;
    display.display_interpolation[1].weight = 5;
    assert_eq!(interpolate_stat(50, &display), Some(2));
    assert_eq!(interpolate_stat(70, &display), Some(4));
    assert_eq!(interpolate_stat(150, &display), Some(5));
    assert_eq!(interpolate_stat(-10, &display), Some(0));
    assert_eq!(interpolate_stat(-30, &display), Some(0));
    display.stat_hash = 3871231066; // Magazine uses Math.round instead.
    assert_eq!(interpolate_stat(50, &display), Some(3));
    display.stat_hash = ARMOR_STATS[0];
    assert_eq!(interpolate_stat(150, &display), Some(150)); // armor bypasses clamp
}
#[test]
fn intrinsic_base_precedes_conditional_masterwork_and_mods() {
    // DIM stats.ts:318-390; stats-conditional.ts:155-160; stats-custom.ts:52-72.
    let item = ArmorDefinition {
        investment_stats: vec![investment(ARMOR_STATS[0], 10, true)],
        ..Default::default()
    };
    let intrinsic = ArmorDefinition {
        plug_category_hash: Some(ARMOR_STATS_PLUG),
        investment_stats: vec![investment(ARMOR_STATS[1], 20, false)],
        ..Default::default()
    };
    let mw = ArmorDefinition {
        plug_category_hash: Some(ARMOR3_MASTERWORK_PLUG),
        investment_stats: ARMOR_STATS
            .iter()
            .map(|&hash| investment(hash, 5, true))
            .collect(),
        ..Default::default()
    };
    let modification = ArmorDefinition {
        investment_stats: vec![investment(ARMOR_STATS[2], 10, false)],
        ..Default::default()
    };
    let plugs = [plug(1, &mw), plug(2, &intrinsic), plug(3, &modification)];
    let result = build_armor_stats(Some(&item), Some(&group()), Some(&plugs), true).unwrap();
    assert_eq!(result.total_base, 30);
    assert_eq!(result.stats[0].base, 10);
    assert_eq!(result.stats[0].current, 10);
    assert_eq!(result.stats[1].base, 20);
    assert_eq!(result.stats[2].current, 15);
    assert_eq!(result.stats[2].base_masterworked, Some(5));
    assert_eq!(armor3_masterwork_tier(Some(&plugs), Some(&result)), Some(5));
}
#[test]
fn unknown_inputs_do_not_become_zero() {
    let definition = ArmorDefinition::default();
    assert!(build_armor_stats(None, Some(&group()), Some(&[]), true).is_none());
    assert!(build_armor_stats(Some(&definition), None, Some(&[]), true).is_none());
    assert!(build_armor_stats(Some(&definition), Some(&group()), None, true).is_none());
    let plugs = [ModelPlug {
        hash: 1,
        enabled: true,
        definition: None,
    }];
    assert!(build_armor_stats(Some(&definition), Some(&group()), Some(&plugs), true).is_none());
    assert_eq!(armor3_masterwork_tier(Some(&plugs), None), None);
}
#[test]
fn disabled_unknown_plug_is_ignored_and_unknown_condition_is_active_like_dim() {
    // DIM stats-conditional.ts:52-55,112: unrecognized conditional has no activation rule.
    let definition = ArmorDefinition::default();
    let conditional = ArmorDefinition {
        investment_stats: vec![investment(ARMOR_STATS[0], 7, true)],
        ..Default::default()
    };
    let plugs = [
        ModelPlug {
            hash: 1,
            enabled: false,
            definition: None,
        },
        plug(2, &conditional),
    ];
    let result = build_armor_stats(Some(&definition), Some(&group()), Some(&plugs), false).unwrap();
    assert_eq!(result.stats[0].base, 0);
    assert_eq!(result.stats[0].current, 7);
    assert_eq!(result.stats[0].base_masterworked, None);
}
#[test]
fn masterwork_without_active_projected_stats_has_no_tier() {
    let item = ArmorDefinition {
        investment_stats: ARMOR_STATS
            .iter()
            .map(|&hash| investment(hash, 10, false))
            .collect(),
        ..Default::default()
    };
    let mut mw = ArmorDefinition {
        plug_category_hash: Some(ARMOR3_MASTERWORK_PLUG),
        investment_stats: ARMOR_STATS
            .iter()
            .map(|&hash| investment(hash, 5, true))
            .collect(),
        ..Default::default()
    };
    let plugs = [plug(1, &mw)];
    let stats = build_armor_stats(Some(&item), Some(&group()), Some(&plugs), true).unwrap();
    assert_eq!(armor3_masterwork_tier(Some(&plugs), Some(&stats)), None);
    // A projected zero-valued unconditional energy stat still makes plug stats nonempty.
    mw.investment_stats.push(investment(1, 0, false));
    assert_eq!(
        armor3_masterwork_tier(Some(&[plug(1, &mw)]), Some(&stats)),
        Some(5)
    );
    assert_eq!(armor3_masterwork_tier(Some(&[plug(1, &mw)]), None), Some(5));
}

#[test]
fn unresolved_disabled_plug_keeps_assumed_masterwork_unknown() {
    let item = ArmorDefinition::default();
    let plugs = [ModelPlug {
        hash: 1,
        enabled: false,
        definition: None,
    }];
    let stats = build_armor_stats(Some(&item), Some(&group()), Some(&plugs), true).unwrap();
    assert_eq!(stats.total_base, 0);
    assert!(
        stats
            .stats
            .iter()
            .all(|stat| stat.base_masterworked.is_none())
    );
}

#[test]
fn duplicate_plug_stats_preserve_first_condition_and_legacy_bonus() {
    let item = ArmorDefinition {
        investment_stats: vec![investment(ARMOR_STATS[0], 10, false)],
        ..Default::default()
    };
    let mut mw = ArmorDefinition {
        plug_category_hash: Some(ARMOR3_MASTERWORK_PLUG),
        investment_stats: vec![
            investment(ARMOR_STATS[0], 2, false),
            investment(ARMOR_STATS[0], 3, true),
        ],
        ..Default::default()
    };
    let stats =
        build_armor_stats(Some(&item), Some(&group()), Some(&[plug(1, &mw)]), true).unwrap();
    assert_eq!(stats.stats[0].base, 10);
    assert_eq!(stats.stats[0].current, 15);
    mw.investment_stats.reverse();
    let stats =
        build_armor_stats(Some(&item), Some(&group()), Some(&[plug(1, &mw)]), true).unwrap();
    assert_eq!(stats.stats[0].current, 10);
    let stats = build_armor_stats(Some(&item), Some(&group()), Some(&[]), true).unwrap();
    assert_eq!(stats.stats[0].base_masterworked, Some(12));
    let mut disabled = plug(1, &mw);
    disabled.enabled = false;
    let stats = build_armor_stats(Some(&item), Some(&group()), Some(&[disabled]), true).unwrap();
    assert_eq!(stats.stats[0].current, 10);
    assert_eq!(stats.stats[1].base_masterworked, Some(5));
}

#[test]
fn empty_interpolation_and_missing_armor_display_are_unknown() {
    let mut group = group();
    group.scaled_stats[0].display_interpolation.clear();
    assert_eq!(interpolate_stat(20, &group.scaled_stats[0]), Some(20));
    let result = build_armor_stats(
        Some(&ArmorDefinition::default()),
        Some(&group),
        Some(&[]),
        false,
    )
    .unwrap();
    assert_eq!(result.stats[0].maximum_value, Some(100));
    group.maximum_value = None;
    let result = build_armor_stats(
        Some(&ArmorDefinition::default()),
        Some(&group),
        Some(&[]),
        false,
    )
    .unwrap();
    assert_eq!(result.stats[0].maximum_value, None);
    group.scaled_stats.pop();
    assert!(
        build_armor_stats(
            Some(&ArmorDefinition::default()),
            Some(&group),
            Some(&[]),
            false
        )
        .is_none()
    );
}
