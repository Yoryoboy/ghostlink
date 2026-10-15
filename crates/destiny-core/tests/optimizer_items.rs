//! BE-008a ProcessItem mapping, mirroring DIM 29c122e loadout-builder/process/mappers.ts
//! (and mappers.test.ts), loadout-utils.ts:356-405 and armor-upgrade-utils.ts:12-50.
//! DIM's own tests need its test manifest, so these use synthetic items.
use destiny_core::InvestmentStat;
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::build::{AssumeArmorMasterwork as Mw, StatConstraint};
use destiny_core::mod_assignment::AssignmentMod;
use destiny_core::optimizer::*;

const WEAPONS: u32 = ARMOR_STATS[0];
const HEALTH: u32 = ARMOR_STATS[1];
const CLASS: u32 = ARMOR_STATS[2];
const GRENADE: u32 = ARMOR_STATS[3];
const SUPER: u32 = ARMOR_STATS[4];
const MELEE: u32 = ARMOR_STATS[5];
// armor_model::tuning_mod_stat: directional tuners, one per stat.
const TUNE_GRENADE: u32 = 309000506;
const TUNE_MELEE: u32 = 311164277;
const TUNE_WEAPONS: u32 = 691392383;
const TUNE_HEALTH: u32 = 388618952;
const BALANCED: u32 = 3122197216;

fn stat(stat_type_hash: u32, value: i32) -> InvestmentStat {
    InvestmentStat {
        stat_type_hash,
        value,
        is_conditionally_active: false,
    }
}
fn plug(hash: u32, enabled: bool, stats: Option<Vec<InvestmentStat>>) -> TuningPlug {
    TuningPlug {
        hash,
        enabled,
        investment_stats: stats,
    }
}
/// +5 to `up`, -5 to `down`.
fn directional(hash: u32, up: u32, down: u32) -> TuningPlug {
    plug(hash, true, Some(vec![stat(up, 5), stat(down, -5)]))
}
fn balanced() -> TuningPlug {
    let conditional = |s| InvestmentStat {
        is_conditionally_active: true,
        ..stat(s, 1)
    };
    plug(
        BALANCED,
        true,
        Some(ARMOR_STATS.iter().map(|s| conditional(*s)).collect()),
    )
}
fn armor() -> ArmorCandidate {
    ArmorCandidate {
        id: "1".into(),
        hash: 100,
        name: "Helm".into(),
        bucket: 3448274439,
        energy_capacity: Some(10),
        base_stats: [10, 20, 30, 0, 0, 0],
        ..Default::default()
    }
}
fn ranges(max: [u16; 6]) -> Vec<DesiredStatRange> {
    ARMOR_STATS
        .iter()
        .zip(max)
        .map(|(h, m)| DesiredStatRange {
            stat_hash: *h,
            min_stat: 0,
            max_stat: m,
        })
        .collect()
}
fn rules(assume: Mw) -> EnergyRules {
    EnergyRules {
        assume_armor_masterwork: assume,
        min_item_energy: MIN_LO_ITEM_ENERGY,
    }
}
fn map(
    item: &ArmorCandidate,
    r: EnergyRules,
    mods: &[AssignmentMod],
    max: [u16; 6],
    auto: bool,
) -> Vec<ProcessItem> {
    map_armor_to_process_items(item, &r, mods, &ranges(max), auto)
}

#[test]
fn constants_match_dim_types() {
    assert_eq!(
        (
            MIN_LO_ITEM_ENERGY,
            MAX_STAT,
            ARTIFICE_STAT_BOOST,
            MINOR_STAT_BOOST,
            MAJOR_STAT_BOOST
        ),
        (9, 200, 3, 5, 10)
    );
    assert_eq!((TUNING_STAT_BOOST, BALANCED_TUNING_STAT_BOOST), (5, 1));
    assert_eq!(
        (
            GENERAL_SOCKET_PLUG_SET,
            ARTIFICE_SOCKET_PLUG_SET,
            TUNING_SOCKET_PLUG_SET
        ),
        (731468111, 4285066582, 1155052024)
    );
}

#[test]
fn remaining_energy_follows_dim_capacity_rule() {
    // (assume, capacity, tier5, expected assumed capacity)
    let cases = [
        (Mw::None, Some(2), false, 9),
        (Mw::None, Some(9), false, 9),
        (Mw::None, Some(10), false, 10),
        (Mw::All, Some(2), false, 10),
        (Mw::All, Some(2), true, 11),
        (Mw::All, Some(10), true, 11),
        (Mw::Legendary, Some(2), false, 10),
        (Mw::ArtificeExotic, Some(2), false, 10),
        (Mw::All, None, false, 0),
        (Mw::None, None, false, 0),
    ];
    for (assume, capacity, tier5, expected) in cases {
        let item = ArmorCandidate {
            energy_capacity: capacity,
            tier5,
            ..armor()
        };
        let mapped = map(&item, rules(assume), &[], [200; 6], false);
        assert_eq!(
            mapped[0].remaining_energy_capacity, expected,
            "{assume:?} {capacity:?} {tier5}"
        );
    }
}

#[test]
fn legendary_assumption_does_not_upgrade_exotics_and_bucket_mods_cost_energy() {
    let exotic = ArmorCandidate {
        energy_capacity: Some(2),
        is_exotic: true,
        ..armor()
    };
    assert_eq!(
        map(&exotic, rules(Mw::Legendary), &[], [200; 6], false)[0].remaining_energy_capacity,
        9
    );
    let mods = [
        AssignmentMod { hash: 1, cost: 4 },
        AssignmentMod { hash: 2, cost: 3 },
    ];
    let mapped = map(&armor(), rules(Mw::All), &mods, [200; 6], false);
    assert_eq!(mapped[0].remaining_energy_capacity, 3);
}

#[test]
fn assumed_masterwork_stats_follow_dim() {
    let armor3 = |mw: [i32; 6]| ArmorCandidate {
        masterwork: Masterwork::Armor3 { plug_stats: mw },
        ..armor()
    };
    // (item, assume, expected stats)
    let cases = [
        // Legacy: +2 each only at capacity >= 10 (assumed or real).
        (armor(), Mw::None, [12, 22, 32, 2, 2, 2]),
        (
            ArmorCandidate {
                energy_capacity: Some(2),
                ..armor()
            },
            Mw::None,
            [10, 20, 30, 0, 0, 0],
        ),
        (
            ArmorCandidate {
                energy_capacity: Some(2),
                ..armor()
            },
            Mw::All,
            [12, 22, 32, 2, 2, 2],
        ),
        // Armor 3: assumed masterwork gives 5 to base-zero stats only.
        (armor3([0; 6]), Mw::All, [10, 20, 30, 5, 5, 5]),
        // Not assumed: current masterwork plug stats on base-zero stats.
        (armor3([0, 0, 0, 3, 2, 1]), Mw::None, [10, 20, 30, 3, 2, 1]),
        (armor3([4, 4, 4, 3, 2, 1]), Mw::None, [10, 20, 30, 3, 2, 1]),
        (armor3([0; 6]), Mw::None, [10, 20, 30, 0, 0, 0]),
    ];
    for (item, assume, expected) in cases {
        let mapped = map(&item, rules(assume), &[], [200; 6], false);
        assert_eq!(mapped[0].stats, expected, "{assume:?}");
    }
}

#[test]
fn artifice_assumption_covers_exotics_without_armor3() {
    let exotic = ArmorCandidate {
        is_exotic: true,
        ..armor()
    };
    let armor3 = ArmorCandidate {
        masterwork: Masterwork::Armor3 { plug_stats: [0; 6] },
        ..exotic.clone()
    };
    let no_energy = ArmorCandidate {
        energy_capacity: None,
        ..exotic.clone()
    };
    let real = ArmorCandidate {
        is_artifice: true,
        ..armor()
    };
    let cases = [
        (&exotic, Mw::ArtificeExotic, true),
        (&exotic, Mw::All, false),
        (&armor3, Mw::ArtificeExotic, false),
        (&no_energy, Mw::ArtificeExotic, false),
        (&real, Mw::None, true),
    ];
    for (item, assume, expected) in cases {
        assert_eq!(
            map(item, rules(assume), &[], [200; 6], false)[0].is_artifice,
            expected,
            "{assume:?}"
        );
    }
}

#[test]
fn plain_fields_are_carried_over() {
    let item = ArmorCandidate {
        power: Some(1950),
        activity_slot: Some(13646368),
        set_bonus: Some(7),
        has_set_bonus_mod_socket: true,
        intrinsic_perks: vec![11, 12],
        ..armor()
    };
    let p = &map(&item, rules(Mw::None), &[], [200; 6], true)[0];
    assert_eq!((p.id.as_str(), p.hash, p.power), ("1", 100, 1950));
    assert_eq!(
        (p.compatible_activity_mod, p.set_bonus),
        (Some(13646368), Some(7))
    );
    assert!(p.has_set_bonus_mod_socket);
    assert_eq!(p.intrinsic_perks, Some(vec![11, 12]));
    assert_eq!(
        (p.included_tuning_mod, p.tuning_variants.as_ref()),
        (None, None)
    );
    assert_eq!(
        map(&armor(), rules(Mw::None), &[], [200; 6], true)[0].intrinsic_perks,
        None
    );
}

fn tunable(exotic: bool, plugs: Vec<TuningPlug>) -> ArmorCandidate {
    ArmorCandidate {
        is_exotic: exotic,
        tuning: Some(plugs),
        ..armor()
    }
}

#[test]
fn legendary_expands_one_item_per_tuning_mod() {
    let item = tunable(
        false,
        vec![directional(TUNE_GRENADE, GRENADE, WEAPONS), balanced()],
    );
    let mapped = map(
        &item,
        rules(Mw::None),
        &[],
        [100, 200, 200, 200, 200, 200],
        true,
    );
    assert_eq!(mapped.len(), 2);
    assert_eq!(mapped[0].included_tuning_mod, Some(TUNE_GRENADE));
    assert_eq!(mapped[0].stats, [12 - 5, 22, 32, 2 + 5, 2, 2]);
    // Balanced Tuning only touches base-zero stats (the three secondary ones).
    assert_eq!(mapped[1].included_tuning_mod, Some(BALANCED));
    assert_eq!(mapped[1].stats, [12, 22, 32, 3, 3, 3]);
    assert!(mapped.iter().all(|m| m.tuning_variants.is_none()));
}

#[test]
fn exotic_keeps_a_single_item_with_variants() {
    let item = tunable(
        true,
        vec![directional(TUNE_GRENADE, GRENADE, WEAPONS), balanced()],
    );
    let mapped = map(
        &item,
        rules(Mw::None),
        &[],
        [100, 200, 200, 200, 200, 200],
        true,
    );
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].included_tuning_mod, None);
    let variants = mapped[0].tuning_variants.as_ref().unwrap();
    assert_eq!(
        variants.iter().map(|v| v.mod_hash).collect::<Vec<_>>(),
        [TUNE_GRENADE, BALANCED]
    );
    assert_eq!(variants[0].stats, [7, 22, 32, 7, 2, 2]);
    // No variants (e.g. all disabled) leaves the plain exotic.
    let none = tunable(
        true,
        vec![plug(TUNE_GRENADE, false, Some(vec![stat(GRENADE, 5)]))],
    );
    let mapped = map(&none, rules(Mw::None), &[], [200; 6], true);
    assert_eq!(
        (mapped.len(), mapped[0].tuning_variants.as_ref()),
        (1, None)
    );
}

#[test]
fn unusable_tuning_plugs_and_modes_do_not_expand() {
    let skipped = vec![
        plug(
            TUNE_GRENADE,
            false,
            Some(vec![stat(GRENADE, 5), stat(WEAPONS, -5)]),
        ), // disabled
        plug(
            2325217837,
            true,
            Some(vec![stat(GRENADE, 5), stat(WEAPONS, -5)]),
        ), // empty plug hash
        plug(TUNE_MELEE, true, None), // not in the socket's plug set / not pluggable
        plug(TUNE_HEALTH, true, Some(vec![])), // no investment stats
    ];
    let mapped = map(
        &tunable(false, skipped.clone()),
        rules(Mw::None),
        &[],
        [200; 6],
        true,
    );
    assert_eq!(mapped.len(), 0);
    // auto stat mods off, or an empty socket list, keep the plain item.
    let plain = map(
        &tunable(false, skipped),
        rules(Mw::None),
        &[],
        [200; 6],
        false,
    );
    assert_eq!((plain.len(), plain[0].included_tuning_mod), (1, None));
    let empty = map(
        &tunable(false, vec![]),
        rules(Mw::None),
        &[],
        [200; 6],
        true,
    );
    assert_eq!((empty.len(), empty[0].included_tuning_mod), (1, None));
}

#[test]
fn dump_stat_keep_rules() {
    // Tuners: +grenade/-weapons, +grenade/-health, +grenade/-melee, +grenade/-class.
    let plugs = || {
        vec![
            directional(TUNE_WEAPONS, GRENADE, WEAPONS),
            directional(TUNE_HEALTH, GRENADE, HEALTH),
            directional(TUNE_MELEE, GRENADE, MELEE),
            directional(323635379, GRENADE, CLASS),
        ]
    };
    let kept = |max: [u16; 6]| {
        map(&tunable(false, plugs()), rules(Mw::None), &[], max, true)
            .iter()
            .map(|m| m.included_tuning_mod.unwrap())
            .collect::<Vec<_>>()
    };
    // Default dump stat is the last ignored (max 0) stat: melee. Others kept only
    // when 0 < max <= 175.
    assert_eq!(kept([200, 200, 200, 200, 200, 0]), [TUNE_MELEE]);
    assert_eq!(
        kept([175, 176, 200, 200, 200, 0]),
        [TUNE_WEAPONS, TUNE_MELEE]
    );
    // Two ignored stats: the last one wins (class); ignored non-default stats are not kept.
    assert_eq!(kept([200, 0, 0, 200, 200, 200]), [323635379]);
    // Nothing ignored: default dump is the lowest maximum, later stats win ties.
    assert_eq!(kept([200, 200, 200, 200, 200, 190]), [TUNE_MELEE]);
    assert_eq!(kept([200, 200, 200, 200, 190, 190]), [TUNE_MELEE]);
    // Balanced Tuning (no negative stat) is always kept.
    let with_balanced = tunable(
        false,
        vec![balanced(), directional(TUNE_WEAPONS, GRENADE, WEAPONS)],
    );
    let mapped = map(
        &with_balanced,
        rules(Mw::None),
        &[],
        [200, 200, 200, 200, 200, 0],
        true,
    );
    assert_eq!(
        mapped
            .iter()
            .map(|m| m.included_tuning_mod)
            .collect::<Vec<_>>(),
        [Some(BALANCED)]
    );
}

#[test]
fn tuned_stats_are_clamped_to_max_stat() {
    let item = ArmorCandidate {
        base_stats: [198, 0, 0, 0, 0, 0],
        ..tunable(false, vec![directional(TUNE_WEAPONS, WEAPONS, HEALTH)])
    };
    let mapped = map(
        &item,
        rules(Mw::None),
        &[],
        [200, 100, 200, 200, 200, 0],
        true,
    );
    assert_eq!(mapped[0].stats[0], 200);
}

#[test]
fn desired_ranges_fill_ignored_stats_in_priority_order() {
    // DIM loadout-params.ts:14-34 (ignored stats keep in-game order after the
    // constrained ones) and loadout-analyzer/utils.ts:32-33 (ignored => 0/0).
    let c = |stat_hash, min_stat, max_stat| StatConstraint {
        stat_hash,
        min_stat,
        max_stat,
    };
    let out = desired_stat_ranges(&[c(CLASS, Some(30), Some(100)), c(WEAPONS, None, None)]);
    let got: Vec<_> = out
        .iter()
        .map(|r| (r.stat_hash, r.min_stat, r.max_stat))
        .collect();
    assert_eq!(
        got,
        [
            (CLASS, 30, 100),
            (WEAPONS, 0, 200),
            (HEALTH, 0, 0),
            (GRENADE, 0, 0),
            (SUPER, 0, 0),
            (MELEE, 0, 0),
        ]
    );
}
