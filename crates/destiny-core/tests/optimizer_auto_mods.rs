//! BE-008b auto-mod map and `chooseAutoMods`, mirroring DIM 29c122e
//! loadout-builder/process/mappers.ts:201-249 (getAutoMods) and
//! process-worker/auto-stat-mod-utils.ts (and its test). DIM's own tests need its
//! test manifest, so these use synthetic plug definitions.
use destiny_core::armor_catalog::{ArmorCatalogRecord, PlugSetDefinition, PlugSetItem};
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::optimizer::*;
use destiny_core::{
    ArmorDefinition, BucketHash, DefinitionVersion, InvestmentStat, ItemDefinition, ItemHash,
    PlugEnergyCost,
};
use std::sync::Arc;

const GENERAL_CATEGORY: u32 = 2487827355;

fn plug(hash: u32, stats: &[(u32, i32)], cost: Option<i32>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(i64::from(hash)),
        bucket_hash: Some(BucketHash::new(1)),
        name: format!("Plug {hash}"),
        tier: None,
        item_type: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            plug_category_hash: Some(GENERAL_CATEGORY),
            investment_stats: stats
                .iter()
                .map(|(stat_type_hash, value)| InvestmentStat {
                    stat_type_hash: *stat_type_hash,
                    value: *value,
                    is_conditionally_active: false,
                })
                .collect(),
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
    }
}
fn plug_set(hash: u32, plugs: &[u32]) -> ArmorCatalogRecord {
    ArmorCatalogRecord::PlugSet(PlugSetDefinition {
        hash,
        reusable_plug_items: plugs
            .iter()
            .map(|plug_item_hash| PlugSetItem {
                plug_item_hash: *plug_item_hash,
                currently_can_roll: true,
            })
            .collect(),
    })
}

/// Hash scheme: general `1000 + 10 * stat + {1 minor cost 2, 2 major cost 4,
/// 3 cheap minor cost 1, 4 cheap major cost 3}`, artifice `2000 + stat`.
fn definitions(with_cheap: bool) -> (Vec<ItemDefinition>, Vec<ArmorCatalogRecord>) {
    let mut defs = Vec::new();
    let (mut general, mut artifice) = (Vec::new(), Vec::new());
    for (i, stat) in ARMOR_STATS.iter().enumerate() {
        let base = 1000 + 10 * i as u32;
        defs.push(plug(base + 1, &[(*stat, 5)], Some(2)));
        defs.push(plug(base + 2, &[(*stat, 10)], Some(4)));
        general.extend([base + 1, base + 2]);
        if with_cheap {
            defs.push(plug(base + 3, &[(*stat, 5)], Some(1)));
            defs.push(plug(base + 4, &[(*stat, 10)], Some(3)));
            general.extend([base + 3, base + 4]);
        }
        defs.push(plug(2000 + i as u32, &[(*stat, 3)], Some(0)));
        artifice.push(2000 + i as u32);
    }
    (
        defs,
        vec![
            plug_set(GENERAL_SOCKET_PLUG_SET, &general),
            plug_set(ARTIFICE_SOCKET_PLUG_SET, &artifice),
        ],
    )
}

#[test]
fn auto_mod_map_picks_full_cost_variants_without_unlocks() {
    let (defs, catalogs) = definitions(true);
    let data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    for (i, stat) in ARMOR_STATS.iter().enumerate() {
        let base = 1000 + 10 * i as u32;
        let general = &data.general_mods[stat];
        assert_eq!(
            general.minor,
            AutoMod {
                hash: base + 1,
                cost: 2
            }
        );
        assert_eq!(
            general.major,
            AutoMod {
                hash: base + 2,
                cost: 4
            }
        );
        assert_eq!(data.artifice_mods[stat], 2000 + i as u32);
    }
}

#[test]
fn auto_mod_map_prefers_the_unlocked_cheaper_variant_per_value() {
    let (defs, catalogs) = definitions(true);
    // Only the cheap minor of the first stat is unlocked; the major keeps its full cost.
    let data = auto_mod_data(&defs, &catalogs, &[1003]).unwrap();
    let first = &data.general_mods[&ARMOR_STATS[0]];
    assert_eq!(
        first.minor,
        AutoMod {
            hash: 1003,
            cost: 1
        }
    );
    assert_eq!(
        first.major,
        AutoMod {
            hash: 1002,
            cost: 4
        }
    );
    assert_eq!(data.general_mods[&ARMOR_STATS[1]].minor.hash, 1011);
    // Unlocking the full-cost variants (the larger of each pair) changes nothing.
    let data = auto_mod_data(&defs, &catalogs, &[1001, 1002]).unwrap();
    assert_eq!(data.general_mods[&ARMOR_STATS[0]].minor.hash, 1001);
    assert_eq!(data.general_mods[&ARMOR_STATS[0]].major.hash, 1002);
}

#[test]
fn auto_mod_map_with_a_single_variant_uses_it_even_when_unlocked() {
    let (defs, catalogs) = definitions(false);
    let data = auto_mod_data(&defs, &catalogs, &[1001, 1002]).unwrap();
    assert_eq!(data.general_mods[&ARMOR_STATS[0]].minor.hash, 1001);
    assert_eq!(data.general_mods[&ARMOR_STATS[0]].major.hash, 1002);
}

#[test]
fn auto_mod_map_requires_both_major_and_minor() {
    let (mut defs, catalogs) = definitions(false);
    let at = defs.iter().position(|d| d.hash.value() == 1002).unwrap();
    defs[at].armor.investment_stats.clear();
    let data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    assert!(!data.general_mods.contains_key(&ARMOR_STATS[0]));
    assert!(data.general_mods.contains_key(&ARMOR_STATS[1]));
    // The artifice mod does not depend on the general pair.
    assert_eq!(data.artifice_mods[&ARMOR_STATS[0]], 2000);
}

#[test]
fn auto_mod_map_only_takes_zero_cost_artifice_mods_with_the_artifice_boost() {
    let (mut defs, catalogs) = definitions(false);
    // Stat 0: the artifice mod costs energy. Stat 1: it gives +4 instead of +3.
    let find = |defs: &mut Vec<ItemDefinition>, hash: i64| {
        defs.iter_mut()
            .position(|d| d.hash.value() == hash)
            .unwrap()
    };
    let at = find(&mut defs, 2000);
    defs[at].armor.energy_cost.as_mut().unwrap().energy_cost = 1;
    let at = find(&mut defs, 2001);
    defs[at].armor.investment_stats[0].value = 4;
    let data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    assert!(!data.artifice_mods.contains_key(&ARMOR_STATS[0]));
    assert!(!data.artifice_mods.contains_key(&ARMOR_STATS[1]));
    assert_eq!(data.artifice_mods[&ARMOR_STATS[2]], 2002);
    // No energy cost block at all counts as free.
    let (mut defs, catalogs) = definitions(false);
    let at = find(&mut defs, 2000);
    defs[at].armor.energy_cost = None;
    let data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    assert_eq!(data.artifice_mods[&ARMOR_STATS[0]], 2000);
}

#[test]
fn auto_mod_map_skips_plugs_without_stats_or_plug_category() {
    let (mut defs, catalogs) = definitions(false);
    let at = defs.iter().position(|d| d.hash.value() == 1001).unwrap();
    defs[at].armor.investment_stats.clear();
    let at = defs.iter().position(|d| d.hash.value() == 1012).unwrap();
    defs[at].armor.plug_category_hash = None;
    let data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    assert!(!data.general_mods.contains_key(&ARMOR_STATS[0]));
    assert!(!data.general_mods.contains_key(&ARMOR_STATS[1]));
    assert!(data.general_mods.contains_key(&ARMOR_STATS[2]));
}

#[test]
fn auto_mod_map_reports_missing_inputs_instead_of_assuming_empty() {
    let (defs, catalogs) = definitions(false);
    assert_eq!(
        auto_mod_data(&defs, &catalogs[..1], &[]).unwrap_err(),
        "auto_mod_plug_set_unavailable"
    );
    let missing: Vec<_> = defs
        .iter()
        .filter(|d| d.hash.value() != 2003)
        .cloned()
        .collect();
    // A plug-set entry without a definition is skipped like DIM (sockets.ts:515-523).
    assert!(auto_mod_data(&missing, &catalogs, &[]).is_ok());
}

fn data(with_artifice: bool) -> AutoModData {
    let (defs, catalogs) = definitions(false);
    let mut data = auto_mod_data(&defs, &catalogs, &[]).unwrap();
    if !with_artifice {
        data.artifice_mods.clear();
    }
    data
}
fn session(data: &AutoModData, user_generals: usize) -> LoSessionInfo {
    let mods: Vec<_> = (0..user_generals)
        .map(|i| ProcessMod {
            hash: 7 + i as u32,
            energy_cost: 2,
            tag: None,
        })
        .collect();
    LoSessionInfo::new(data, &mods, &[], true, &ARMOR_STATS)
}

/// DIM auto-stat-mod-utils.test.ts "different ways of hitting target stats" snapshots,
/// for the fourth stat: picks per reachable value.
#[test]
fn ways_of_hitting_a_stat_match_the_dim_snapshots() {
    let cases: [(usize, bool, Vec<usize>); 5] = [
        (5, true, vec![1; 15]),
        (
            4,
            true,
            vec![
                2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1,
            ],
        ),
        (
            2,
            false,
            vec![
                1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1,
                1, 1,
            ],
        ),
        (
            0,
            false,
            vec![
                1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
                3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            ],
        ),
        (
            0,
            true,
            vec![
                2, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 6, 8, 8, 8, 8, 8, 10, 10, 10, 10, 10, 11,
                11, 11, 11, 11, 11, 11, 11, 11, 11, 10, 10, 10, 10, 10, 8, 8, 8, 8, 8, 6, 6, 6, 6,
                6, 4, 4, 4, 4, 4, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1,
            ],
        ),
    ];
    for (user_generals, artifice, expected) in cases {
        let info = session(&data(artifice), user_generals);
        let table = &info.auto_mod_options[3];
        // Keys are contiguous from 1 to the maximum reachable value.
        assert_eq!(table.len(), expected.len());
        let counts: Vec<_> = (1..=expected.len() as i32)
            .map(|v| table[&v].len())
            .collect();
        assert_eq!(
            counts, expected,
            "{user_generals} user generals, artifice {artifice}"
        );
    }
}

#[test]
fn picks_prefer_artifice_then_more_general_mods() {
    let info = session(&data(true), 0);
    let shapes: Vec<_> = info.auto_mod_options[3][&5]
        .iter()
        .map(|p| (p.num_artifice_mods, p.num_general_mods, p.exact_stat_points))
        .collect();
    assert_eq!(shapes, [(2, 0, 6), (0, 1, 5)]);
    let minor = &info.auto_mod_options[3][&5][1];
    assert_eq!(minor.mod_hashes, [1031]);
    assert_eq!(minor.general_mods_costs, [2]);
    assert_eq!(minor.mod_energy_cost, 2);
    assert_eq!(minor.target_stat_index, 3);
    // 13 points, ordered by artifice count then general count (the DIM snapshot has 6).
    let shapes: Vec<_> = info.auto_mod_options[3][&13]
        .iter()
        .map(|p| (p.num_artifice_mods, p.num_general_mods))
        .collect();
    assert_eq!(shapes, [(5, 0), (3, 1), (1, 2), (1, 1), (0, 3), (0, 2)]);
    // Mod hashes list majors, then minors, then artifice mods.
    let major = &info.auto_mod_options[3][&13][3];
    assert_eq!(major.mod_hashes, [1032, 2003]);
    assert_eq!(major.general_mods_costs, [4]);
}

#[test]
fn stats_without_mods_get_empty_caches() {
    let mut without = data(true);
    without.general_mods.remove(&ARMOR_STATS[3]);
    assert_eq!(session(&without, 0).auto_mod_options[3].len(), 15);
    without.artifice_mods.clear();
    assert!(session(&without, 0).auto_mod_options[3].is_empty());
    // Auto stat mods off leaves no general slots, only artifice picks.
    let off = LoSessionInfo::new(&data(true), &[], &[], false, &ARMOR_STATS);
    assert_eq!(off.num_available_general_mods, 0);
    assert_eq!(off.auto_mod_options[3].len(), 15);
}

/// DIM auto-stat-mod-utils.test.ts "memoizes across equal inputs and distinguishes
/// different ones".
#[test]
fn choose_auto_mods_memoizes_equal_inputs_only() {
    let data = data(true);
    let info = session(&data, 0);
    let energies = [[10, 7, 4, 3, 0]];
    let pick = choose_auto_mods(&info, &[10, 0, 0, 5, 0, 0], 2, &energies, 24).unwrap();
    assert_eq!(info.auto_mods_memo_size(), 1);
    let repeated = choose_auto_mods(&info, &[10, 0, 0, 5, 0, 0], 2, &energies, 24).unwrap();
    assert_eq!(info.auto_mods_memo_size(), 1);
    assert!(pick.iter().zip(&repeated).all(|(a, b)| Arc::ptr_eq(a, b)));
    let different = choose_auto_mods(&info, &[15, 0, 0, 5, 0, 0], 2, &energies, 24).unwrap();
    assert_eq!(info.auto_mods_memo_size(), 2);
    assert!(different[0].exact_stat_points >= 15 - 4);
    // A fresh, un-memoized session agrees with the memoized results.
    let fresh = session(&data, 0);
    let again = choose_auto_mods(&fresh, &[10, 0, 0, 5, 0, 0], 2, &energies, 24).unwrap();
    assert_eq!(
        again.iter().map(|p| (**p).clone()).collect::<Vec<_>>(),
        pick.iter().map(|p| (**p).clone()).collect::<Vec<_>>()
    );
}

#[test]
fn choose_auto_mods_does_not_memoize_multiple_vectors_or_negative_budgets() {
    let info = session(&data(true), 0);
    let vectors = [[10, 7, 4, 3, 0], [9, 9, 9, 0, 0]];
    assert!(choose_auto_mods(&info, &[10, 0, 0, 5, 0, 0], 2, &vectors, 24).is_some());
    assert_eq!(info.auto_mods_memo_size(), 0);
    // A negative budget rejects even a zero-cost (artifice-only) pick.
    let single = [[10, 7, 4, 3, 0]];
    assert!(choose_auto_mods(&info, &[6, 0, 0, 0, 0, 0], 2, &single, -1).is_none());
    assert_eq!(info.auto_mods_memo_size(), 0);
    assert_eq!(
        choose_auto_mods(&info, &[6, 0, 0, 0, 0, 0], 2, &single, 0)
            .unwrap()
            .len(),
        1
    );
    // Unreachable needs are memoized as failures and stay failures.
    assert!(choose_auto_mods(&info, &[200, 0, 0, 0, 0, 0], 2, &single, 24).is_none());
    assert!(choose_auto_mods(&info, &[200, 0, 0, 0, 0, 0], 2, &single, 24).is_none());
    assert_eq!(info.auto_mods_memo_size(), 2);
}

#[test]
fn choose_auto_mods_checks_energy_budget_per_item_fit_and_artifice() {
    let info = session(&data(true), 0);
    let ten = [10, 0, 0, 0, 0, 0];
    // A major (cost 4) fits an item with 4 energy; with 3 neither it nor two minors fit.
    assert!(choose_auto_mods(&info, &ten, 0, &[[4, 0, 0, 0, 0]], 4).is_some());
    assert!(choose_auto_mods(&info, &ten, 0, &[[3, 0, 0, 0, 0]], 3).is_none());
    // Two minors (cost 2 each) fit [3, 3] by item, and the budget must cover both.
    assert!(choose_auto_mods(&info, &ten, 0, &[[3, 3, 0, 0, 0]], 4).is_some());
    assert!(choose_auto_mods(&info, &ten, 0, &[[3, 3, 0, 0, 0]], 3).is_none());
    let nine = [9, 0, 0, 0, 0, 0];
    assert!(choose_auto_mods(&info, &nine, 3, &[[0; 5]], 0).is_some());
    assert!(choose_auto_mods(&info, &nine, 2, &[[0; 5]], 0).is_none());
    // No needs: an empty pick list, regardless of energy.
    let none = choose_auto_mods(&info, &[0; 6], 0, &[[0; 5]], 0).unwrap();
    assert!(none.is_empty());
}

/// Port of process-utils.test.ts "the problem is solvable": 2 user general mods
/// (costs 4 and 3), 3 free general slots, 4 artifice mods, energies [3, 4, 1, 3, 4]
/// with the cost-1 activity mod fitting only the 4 and 3 energy items (budget 7).
/// Manifest costs are replaced by the ones the example needs: majors 3, stat 4's minor 1.
#[test]
fn choose_auto_mods_solves_the_dim_worked_example() {
    let mut data = data(true);
    for (stat, general) in data.general_mods.iter_mut() {
        general.major.cost = 3;
        general.minor.cost = if *stat == ARMOR_STATS[4] { 1 } else { 2 };
    }
    let mods = [
        ProcessMod {
            hash: 7,
            energy_cost: 3,
            tag: None,
        },
        ProcessMod {
            hash: 8,
            energy_cost: 4,
            tag: None,
        },
    ];
    let info = LoSessionInfo::new(&data, &mods, &[], true, &ARMOR_STATS);
    assert_eq!(info.num_available_general_mods, 3);
    let needs = [4, 0, 10, 12, 4, 0];
    let picks = choose_auto_mods(&info, &needs, 4, &[[4, 3, 3, 3, 1]], 7).unwrap();
    let hashes: Vec<_> = picks.iter().map(|p| p.mod_hashes.clone()).collect();
    assert_eq!(
        hashes,
        [vec![2000, 2000], vec![1022], vec![1032, 2003], vec![1041]]
    );
    let points: Vec<_> = picks.iter().map(|p| p.exact_stat_points).collect();
    assert_eq!(points, [6, 10, 13, 5]);
    // The solution needs three artifice mods (two for stat 0, one for stat 3), not four.
    assert!(choose_auto_mods(&info, &needs, 3, &[[4, 3, 3, 3, 1]], 7).is_some());
    assert!(choose_auto_mods(&info, &needs, 2, &[[4, 3, 3, 3, 1]], 7).is_none());
    // Higher stats, or no energy at all, cannot be solved.
    assert!(choose_auto_mods(&info, &[4, 0, 10, 12, 4, 20], 4, &[[4, 3, 3, 3, 1]], 7).is_none());
    assert!(choose_auto_mods(&info, &needs, 4, &[[0; 5]], 0).is_none());
}
