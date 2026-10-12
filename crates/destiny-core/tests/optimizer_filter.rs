//! BE-008a pre-filter, mirroring DIM 29c122e loadout-builder/item-filter.ts and
//! item-filter.test.ts, plus utils/stats.ts computeStatDupeLower and
//! utils/stats-set.test.ts. DIM's tests rely on its test manifest, so the same
//! scenarios run over synthetic items.
use destiny_core::armor_model::ARMOR_STATS;
use destiny_core::build::AssumeArmorMasterwork as Mw;
use destiny_core::mod_assignment::{AssignmentMod, AssignmentSocket};
use destiny_core::optimizer::*;
use std::collections::BTreeMap;

const HELMET: i64 = 3448274439;
const GAUNTLETS: i64 = 3551918588;
const CHEST: i64 = 14239492;
const LEGS: i64 = 20886954;
const CLASS_ITEM: i64 = 1585787867;
const TUNE_GRENADE: u32 = 309000506;

fn item(id: &str, bucket: i64) -> ArmorCandidate {
    ArmorCandidate {
        id: id.into(),
        hash: bucket as u32 + 1,
        name: format!("n{bucket}"),
        bucket,
        power: Some(1900),
        energy_capacity: Some(10),
        base_stats: [10, 20, 30, 40, 5, 5],
        ..Default::default()
    }
}
fn exotic(id: &str, bucket: i64, hash: u32) -> ArmorCandidate {
    ArmorCandidate {
        is_exotic: true,
        hash,
        name: format!("ex{hash}"),
        ..item(id, bucket)
    }
}
fn ids(items: &ItemsByBucket, bucket: i64) -> Vec<&str> {
    items[&bucket].iter().map(|i| i.id.as_str()).collect()
}
fn run(
    params: &FilterParams,
    search: &dyn Fn(&ArmorCandidate) -> bool,
) -> (ItemsByBucket, FilterInfo) {
    filter_items(params, search).unwrap()
}
fn all(_: &ArmorCandidate) -> bool {
    true
}
/// Items with distinct stats so the strictly-worse pass keeps all of them.
fn spread(id: &str, bucket: i64, k: i32) -> ArmorCandidate {
    ArmorCandidate {
        base_stats: [10 + k, 40 - k, 20, 20, 20, 20],
        ..item(id, bucket)
    }
}
fn locked(hash: u32, bucket: i64, name: &str) -> Option<LockedExotic> {
    Some(LockedExotic::Item {
        hash,
        definition: Some(ExoticDefinition {
            bucket,
            name: name.into(),
        }),
    })
}

#[test]
fn nothing_filtered_when_no_filters() {
    let items = vec![
        spread("a", HELMET, 0),
        spread("b", HELMET, 1),
        spread("c", CHEST, 0),
    ];
    let (out, info) = run(
        &FilterParams {
            items: &items,
            ..Default::default()
        },
        &all,
    );
    assert_eq!(ids(&out, HELMET), ["a", "b"]);
    assert_eq!(ids(&out, CHEST), ["c"]);
    assert!(out[&GAUNTLETS].is_empty() && out[&LEGS].is_empty() && out[&CLASS_ITEM].is_empty());
    let h = &info.per_bucket[&HELMET];
    assert_eq!(
        (h.total_considered, h.final_valid, h.removed_strictly_worse),
        (2, 2, 0)
    );
    assert!(!info.search_query_effective && !info.exotic_does_not_exist);
}

#[test]
fn unassigned_mods_or_no_items_give_empty_results() {
    let items = vec![spread("a", HELMET, 0)];
    let (out, info) = run(
        &FilterParams {
            items: &items,
            has_unassigned_mods: true,
            ..Default::default()
        },
        &all,
    );
    assert!(out.values().all(Vec::is_empty));
    assert_eq!(info.per_bucket[&HELMET].total_considered, 0);
    let (out, _) = run(&FilterParams::default(), &all);
    assert!(out.values().all(Vec::is_empty));
}

#[test]
fn pinned_item_wins_over_everything() {
    let items = vec![spread("a", HELMET, 0), exotic("x", HELMET, 9)];
    let params = FilterParams {
        items: &items,
        pinned: BTreeMap::from([(HELMET, spread("p", HELMET, 3))]),
        locked_exotic: Some(LockedExotic::None),
        ..Default::default()
    };
    let (out, info) = run(&params, &|i| i.id != "p");
    assert_eq!(ids(&out, HELMET), ["p"]); // search is ignored: it would remove everything
    assert_eq!(info.per_bucket[&HELMET].total_considered, 1);
}

#[test]
fn excluded_items_are_removed() {
    let items = vec![spread("a", HELMET, 0), spread("b", HELMET, 1)];
    let params = FilterParams {
        items: &items,
        excluded: BTreeMap::from([(HELMET, vec!["a".to_string()])]),
        ..Default::default()
    };
    assert_eq!(ids(&run(&params, &all).0, HELMET), ["b"]);
}

#[test]
fn exotic_locks() {
    let items = vec![
        spread("h", HELMET, 0),
        exotic("hx", HELMET, 9),
        exotic("hy", HELMET, 10),
        spread("c", CHEST, 0),
        exotic("cx", CHEST, 11),
    ];
    let go = |lock| {
        let params = FilterParams {
            items: &items,
            locked_exotic: lock,
            ..Default::default()
        };
        run(&params, &all)
    };
    // Any exotic / unset: nothing removed.
    for lock in [None, Some(LockedExotic::Any)] {
        let (out, _) = go(lock);
        assert_eq!((out[&HELMET].len(), out[&CHEST].len()), (3, 2));
    }
    // No exotic: exotics gone from every bucket.
    let (out, _) = go(Some(LockedExotic::None));
    assert_eq!(
        (ids(&out, HELMET), ids(&out, CHEST)),
        (vec!["h"], vec!["c"])
    );
    // A specific exotic: only it in its bucket, no other exotics elsewhere.
    let (out, info) = go(locked(9, HELMET, "ex9"));
    assert_eq!(
        (ids(&out, HELMET), ids(&out, CHEST)),
        (vec!["hx"], vec!["c"])
    );
    assert!(!info.exotic_does_not_exist);
    // Missing exotic is reported and leaves the bucket empty.
    let (out, info) = go(locked(77, HELMET, "nobody"));
    assert!(out[&HELMET].is_empty() && info.exotic_does_not_exist);
    // Same name, other hash, also counts (DIM matches `item.name`).
    let (out, _) = go(locked(77, HELMET, "ex10"));
    assert_eq!(ids(&out, HELMET), ["hy"]);
    // Unknown definition: DIM has no def, so nothing is exotic-filtered.
    let params = FilterParams {
        items: &items,
        locked_exotic: Some(LockedExotic::Item {
            hash: 9,
            definition: None,
        }),
        ..Default::default()
    };
    assert_eq!(run(&params, &all).0[&HELMET].len(), 3);
}

#[test]
fn bucket_specific_mods_must_fit_energy_and_sockets() {
    let socket = AssignmentSocket {
        index: 0,
        plug_set_size: Some(3),
        fits: vec![1, 2],
    };
    let with_socket = |id: &str, cap: u32| ArmorCandidate {
        energy_capacity: Some(cap),
        bucket_sockets: Some(vec![socket.clone()]),
        ..spread(id, CHEST, cap as i32)
    };
    let items = vec![with_socket("low", 3), with_socket("ok", 7)];
    let mods = BTreeMap::from([(CHEST, vec![AssignmentMod { hash: 1, cost: 5 }])]);
    let mut params = FilterParams {
        items: &items,
        bucket_mods: mods.clone(),
        ..Default::default()
    };
    // Default rules: capacity is max(actual, 9), so both fit a 5 cost.
    assert_eq!(ids(&run(&params, &all).0, CHEST), ["low", "ok"]);
    // A 9-cost mod needs 9: still fits at the default minimum energy of 9.
    params.bucket_mods = BTreeMap::from([(CHEST, vec![AssignmentMod { hash: 1, cost: 10 }])]);
    let (out, info) = run(&params, &all);
    assert!(out[&CHEST].is_empty());
    assert_eq!(info.per_bucket[&CHEST].cant_fit_mods, 2);
    // Assumed masterwork lifts both to 10.
    params.energy_rules = EnergyRules {
        assume_armor_masterwork: Mw::All,
        ..Default::default()
    };
    assert_eq!(ids(&run(&params, &all).0, CHEST), ["low", "ok"]);
    // A mod no socket accepts removes the item.
    params.bucket_mods = BTreeMap::from([(CHEST, vec![AssignmentMod { hash: 99, cost: 1 }])]);
    assert!(run(&params, &all).0[&CHEST].is_empty());
}

#[test]
fn missing_socket_projection_is_an_error_not_a_guess() {
    let items = vec![spread("a", CHEST, 0)];
    let params = FilterParams {
        items: &items,
        bucket_mods: BTreeMap::from([(CHEST, vec![AssignmentMod { hash: 1, cost: 1 }])]),
        ..Default::default()
    };
    assert_eq!(
        filter_items(&params, &all).unwrap_err(),
        FilterError::BucketSocketsUnavailable {
            item_id: "a".into()
        }
    );
    // No mods for that bucket: sockets are never consulted.
    let params = FilterParams {
        items: &items,
        ..Default::default()
    };
    assert!(filter_items(&params, &all).is_ok());
}

#[test]
fn search_filter_rules() {
    let items = vec![
        spread("a", HELMET, 0),
        spread("b", HELMET, 1),
        exotic("x", HELMET, 9),
        spread("c", CHEST, 0),
        exotic("cx", CHEST, 10),
    ];
    let go = |lock, f: &dyn Fn(&ArmorCandidate) -> bool| {
        let params = FilterParams {
            items: &items,
            locked_exotic: lock,
            ..Default::default()
        };
        run(&params, f)
    };
    // Applies to the buckets where something matches.
    let (out, info) = go(None, &|i| i.id == "a" || i.id == "cx");
    assert_eq!(ids(&out, HELMET), ["a"]);
    assert!(info.search_query_effective);
    assert_eq!(info.per_bucket[&HELMET].removed_by_search_filter, 2);
    // Chest only matches the exotic: ignore the search there (no double exotics).
    assert_eq!(ids(&out, CHEST), ["c", "cx"]);
    // Infeasible search is ignored everywhere.
    let (out, info) = go(None, &|_| false);
    assert_eq!((out[&HELMET].len(), out[&CHEST].len()), (3, 2));
    assert!(!info.search_query_effective);
    // Exotic-only matches are kept when the bucket has no legendaries left to fall back to.
    let (out, _) = go(locked(9, HELMET, "ex9"), &|i| i.is_exotic);
    assert_eq!(ids(&out, HELMET), ["x"]);
    // Any exotic with no exotic satisfying the search: exotics skip the search.
    let (out, _) = go(Some(LockedExotic::Any), &|i| i.id == "a" || i.id == "c");
    assert_eq!(
        (ids(&out, HELMET), ids(&out, CHEST)),
        (vec!["a", "x"], vec!["c", "cx"])
    );
    // Any exotic with a satisfiable search: exotics are filtered like the rest.
    let (out, _) = go(Some(LockedExotic::Any), &|i| {
        i.id == "a" || i.id == "x" || i.id == "c"
    });
    assert_eq!(
        (ids(&out, HELMET), ids(&out, CHEST)),
        (vec!["a", "x"], vec!["c"])
    );
}

#[test]
fn set_bonus_requirements_with_a_locked_exotic() {
    let in_set = |id: &str, bucket, set| ArmorCandidate {
        set_bonus: Some(set),
        ..spread(id, bucket, 0)
    };
    let items = vec![
        in_set("s1", HELMET, 1),
        in_set("s2", GAUNTLETS, 2),
        ArmorCandidate {
            set_bonus: Some(3),
            ..spread("other", GAUNTLETS, 3)
        },
        ArmorCandidate {
            has_set_bonus_mod_socket: true,
            ..spread("sock", GAUNTLETS, 1)
        },
        spread("none", GAUNTLETS, 2),
        exotic("cx", CHEST, 9),
        spread("c", CHEST, 0),
    ];
    let go = |bonuses: &[(u32, u32)], lock| {
        let params = FilterParams {
            items: &items,
            set_bonuses: bonuses.iter().copied().collect(),
            locked_exotic: lock,
            ..Default::default()
        };
        run(&params, &all).0
    };
    let out = go(&[(1, 2), (2, 2)], locked(9, CHEST, "ex9"));
    assert_eq!(ids(&out, GAUNTLETS), ["s2", "sock"]);
    assert_eq!(ids(&out, CHEST), ["cx"]); // the exotic's own bucket is not set-filtered
    // Fewer than four pieces, or no locked exotic: no set filtering.
    assert_eq!(
        go(&[(1, 2), (2, 1)], locked(9, CHEST, "ex9"))[&GAUNTLETS].len(),
        4
    );
    assert_eq!(go(&[(1, 2), (2, 2)], None)[&GAUNTLETS].len(), 4);
}

#[test]
fn exotic_class_item_perks() {
    let class_hash = 266021826;
    let with = |id: &str, perks: &[u32]| ArmorCandidate {
        extra_intrinsic_perks: perks.to_vec(),
        ..exotic(id, CLASS_ITEM, class_hash)
    };
    let items = vec![with("a", &[1, 2]), with("b", &[1, 3]), with("c", &[])];
    let go = |lock: Option<LockedExotic>, perks: &[u32]| {
        let params = FilterParams {
            items: &items,
            locked_exotic: lock,
            perks: perks.to_vec(),
            ..Default::default()
        };
        run(&params, &all).0
    };
    let lock = || locked(class_hash, CLASS_ITEM, "ex");
    assert_eq!(ids(&go(lock(), &[1, 2]), CLASS_ITEM), ["a"]);
    assert_eq!(ids(&go(lock(), &[1]), CLASS_ITEM), ["a", "b"]);
    // Perks that do not belong to the class item are ignored.
    assert_eq!(go(lock(), &[999]).get(&CLASS_ITEM).unwrap().len(), 3);
    assert_eq!(go(lock(), &[999, 3]).get(&CLASS_ITEM).unwrap().len(), 1);
    // No perks: untouched. A locked exotic that is not a perk class item: untouched.
    assert_eq!(go(lock(), &[]).get(&CLASS_ITEM).unwrap().len(), 3);
    assert_eq!(
        go(locked(5, CLASS_ITEM, "ex266021826"), &[1, 2])
            .get(&CLASS_ITEM)
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn strictly_worse_items_are_removed() {
    let stats = |id: &str, s: [i32; 6]| ArmorCandidate {
        base_stats: s,
        ..item(id, HELMET)
    };
    let run_ids = |items: Vec<ArmorCandidate>| {
        let (out, info) = run(
            &FilterParams {
                items: &items,
                ..Default::default()
            },
            &all,
        );
        (
            ids(&out, HELMET)
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
            info.per_bucket[&HELMET].removed_strictly_worse,
        )
    };
    // Strictly lower is removed; equal and incomparable are kept.
    let (kept, removed) = run_ids(vec![
        stats("best", [20, 20, 20, 20, 20, 20]),
        stats("worse", [20, 20, 20, 20, 20, 19]),
        stats("equal", [20, 20, 20, 20, 20, 20]),
        stats("mixed", [30, 10, 20, 20, 20, 20]),
    ]);
    assert_eq!(
        (kept, removed),
        (vec!["best".into(), "equal".into(), "mixed".into()], 1)
    );
    // More energy or the required activity slot keeps an otherwise-worse item.
    let low_energy = ArmorCandidate {
        energy_capacity: Some(9),
        ..stats("low", [10; 6])
    };
    let (kept, _) = run_ids(vec![stats("hi", [20; 6]), low_energy]);
    assert_eq!(kept, ["hi"]);
    let more = ArmorCandidate {
        energy_capacity: Some(11),
        ..stats("more", [10; 6])
    };
    let (kept, _) = run_ids(vec![stats("hi", [20; 6]), more]);
    assert_eq!(kept, ["hi", "more"]);
}

#[test]
fn strictly_worse_considers_slots_set_bonuses_and_power() {
    let base = |id: &str, s| ArmorCandidate {
        base_stats: [s; 6],
        ..item(id, HELMET)
    };
    let items = vec![
        base("better", 20),
        ArmorCandidate {
            activity_slot: Some(7),
            ..base("slot", 10)
        },
        ArmorCandidate {
            set_bonus: Some(3),
            ..base("set", 10)
        },
        base("plain", 10),
        ArmorCandidate {
            power: Some(0),
            ..base("nopower", 5)
        },
    ];
    let params = FilterParams {
        items: &items,
        activity_mod_slots: vec![7],
        set_bonuses: BTreeMap::from([(3, 1)]),
        ..Default::default()
    };
    let (out, _) = run(&params, &all);
    // "plain" loses to "better"; slot/set holders survive; zero-power items are never compared.
    assert_eq!(ids(&out, HELMET), ["better", "slot", "set", "nopower"]);
}

#[test]
fn strictly_worse_groups_by_bucket_class_and_exotic_identity() {
    let items = vec![
        ArmorCandidate {
            base_stats: [20; 6],
            ..exotic("e1", HELMET, 1)
        },
        ArmorCandidate {
            base_stats: [10; 6],
            ..exotic("e2", HELMET, 2)
        }, // other exotic hash
        ArmorCandidate {
            base_stats: [10; 6],
            ..exotic("e1b", HELMET, 1)
        }, // same exotic, worse
        ArmorCandidate {
            base_stats: [10; 6],
            ..item("l", HELMET)
        }, // legendary pool
        ArmorCandidate {
            base_stats: [10; 6],
            class_type: 1,
            ..item("hunter", HELMET)
        },
    ];
    let (out, _) = run(
        &FilterParams {
            items: &items,
            ..Default::default()
        },
        &all,
    );
    assert_eq!(ids(&out, HELMET), ["e1", "e2", "l", "hunter"]);
}

#[test]
fn tuning_and_artifice_mixes_prevent_removal() {
    let kept = |items: Vec<ArmorCandidate>| {
        let (out, _) = run(
            &FilterParams {
                items: &items,
                ..Default::default()
            },
            &all,
        );
        out[&HELMET]
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>()
    };
    let flat = |id: &str, v: i32| ArmorCandidate {
        base_stats: [v; 6],
        ..item(id, HELMET)
    };
    let tuner = vec![TuningPlug {
        hash: TUNE_GRENADE,
        enabled: true,
        investment_stats: None,
    }];
    // Without tuning the 21s item removes the 20s one...
    assert_eq!(kept(vec![flat("a", 20), flat("b", 21)]), ["b"]);
    // ...but a grenade tuner can reach grenade 25 (trading 5 from another stat), which 21 cannot beat.
    let tunable = ArmorCandidate {
        tuning: Some(tuner),
        ..flat("a", 20)
    };
    assert_eq!(kept(vec![tunable, flat("b", 21)]), ["a", "b"]);
    // Artifice adds +3 mixes: the plain twin is dominated, the artifice item survives.
    let art = ArmorCandidate {
        is_artifice: true,
        ..flat("art", 20)
    };
    assert_eq!(kept(vec![art, flat("plain", 20)]), ["art"]);
}

#[test]
fn stats_set_port_of_dim_cases() {
    // DIM utils/stats-set.test.ts 'insertAll 1'.
    let mut set = StatsSet::default();
    for s in [[1, 2, 3], [1, 1, 2], [1, 1, 1]] {
        set.insert(s.to_vec());
    }
    let cases: [([i32; 3], bool); 7] = [
        ([1, 2, 3], false),
        ([1, 1, 2], true),
        ([1, 1, 3], true),
        ([1, 2, 2], true),
        ([2, 1, 1], false),
        ([1, 4, 0], false),
        ([0, 0, 0], true),
    ];
    for (stats, expected) in cases {
        assert_eq!(set.do_better_stats_exist(&stats), expected, "{stats:?}");
    }
    assert!(!StatsSet::default().do_better_stats_exist(&[1]));
    assert_eq!(ARMOR_STATS.len(), 6);
}
