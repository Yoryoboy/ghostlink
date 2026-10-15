use destiny_core::{armor_catalog::*, build::*, build_analysis::*, mod_assignment::*, *};

fn socket(index: u8, size: Option<usize>, plugs: &[u32]) -> AssignmentSocket {
    AssignmentSocket {
        index,
        plug_set_size: size,
        fits: plugs.to_vec(),
    }
}
fn mod_(hash: u32, cost: i32) -> AssignmentMod {
    AssignmentMod { hash, cost }
}

#[test]
fn dim_bucket_specific_table() {
    // DIM mod-assignment-utils.ts:576-675: stable socket-size ordering,
    // stable fewest-compatible-sockets ordering, energy and exclusions.
    #[rustfmt::skip] // Keep each behavior-level fixture on one row.
    let cases = [
        ("picky first", vec![socket(0, Some(3), &[1, 2]), socket(1, Some(1), &[1])], vec![mod_(1, 2), mod_(2, 2)], 4, vec![2, 1], vec![]),
        ("energy order is not optimized", vec![socket(0, None, &[1, 2]), socket(1, None, &[1, 2])], vec![mod_(1, 4), mod_(2, 2)], 4, vec![1], vec![2]),
        ("duplicate mods", vec![socket(0, None, &[1]), socket(1, None, &[1])], vec![mod_(1, 1), mod_(1, 1), mod_(1, 1)], 10, vec![1, 1], vec![1]),
        ("no matching socket", vec![socket(0, None, &[1])], vec![mod_(2, 0)], 10, vec![], vec![2]),
        ("finisher exclusion", vec![socket(0, None, &[1125986156, 1170405455]), socket(1, None, &[1125986156, 1170405455])], vec![mod_(1125986156, 1), mod_(1170405455, 1)], 10, vec![1125986156], vec![1170405455]),
        ("fastball excludes duplicate", vec![socket(0, None, &[2649291407]), socket(1, None, &[2649291407])], vec![mod_(2649291407, 1), mod_(2649291407, 1)], 10, vec![2649291407], vec![2649291407]),
        ("zero cost at zero capacity", vec![socket(0, None, &[1])], vec![mod_(1, 0)], 0, vec![1], vec![]),
        ("failed energy does not consume socket", vec![socket(0, None, &[1, 2])], vec![mod_(1, 9), mod_(2, 2)], 2, vec![2], vec![1]),
    ];
    for (name, sockets, mods, capacity, assigned, unassigned) in cases {
        let result = assign_bucket_specific_mods(&sockets, &mods, capacity);
        assert_eq!(result.assigned, assigned, "{name}");
        assert_eq!(result.unassigned, unassigned, "{name}");
    }
}

fn fixture(
    cost: i32,
    capacity: u32,
) -> (
    BuildDefinition,
    Vec<ItemInstance>,
    Vec<ItemDefinition>,
    Vec<ArmorCatalogRecord>,
) {
    let base = ItemDefinition {
        hash: ItemHash::new(1),
        bucket_hash: Some(BucketHash::new(3448274439)),
        name: "Armor".into(),
        tier: Some("Legendary".into()),
        item_type: Some("Armor".into()),
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: Default::default(),
        non_transferrable: Some(false),
        is_instance_item: Some(true),
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: None,
        stack_unique_label: None,
    };
    let mut plug = base.clone();
    plug.hash = ItemHash::new(42);
    plug.bucket_hash = None;
    plug.armor.plug_category_hash = Some(2912171003);
    plug.armor.energy_cost = Some(PlugEnergyCost {
        energy_cost: cost,
        energy_type: 1,
        energy_type_hash: 1,
    });
    let mut defs = vec![plug];
    let mut items = vec![];
    let mut build = BuildDefinition {
        parameters: Some(BuildParameters {
            mods: Some(vec![ItemHash::new(42)]),
            ..Default::default()
        }),
        ..Default::default()
    };
    for (index, bucket) in ARMOR_BUCKETS.into_iter().enumerate() {
        let mut d = base.clone();
        d.hash = ItemHash::new(index as i64 + 1);
        d.bucket_hash = Some(BucketHash::new(bucket));
        d.armor.socket_categories = vec![DefinitionSocketCategory {
            socket_category_hash: 590099826,
            socket_indexes: vec![0],
        }];
        d.armor.socket_entries = vec![DefinitionSocketEntry {
            socket_type_hash: 99,
            plug_sources: Some(2),
            ..Default::default()
        }];
        let item = ItemInstance {
            instance_id: ItemInstanceId::new((index + 1).to_string()),
            item_hash: d.hash,
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            perks: vec![],
            plugs: vec![],
            stats: vec![],
            available_plugs: vec![],
            energy: Some(ItemEnergy {
                energy_capacity: capacity,
                energy_used: capacity,
                ..Default::default()
            }),
            gear_tier: Some(5),
            transfer_status: Some(0),
            sockets: vec![IndexedSocket {
                socket_index: 0,
                plug_hash: Some(ItemHash::new(42)),
                enabled: false,
                visible: true,
                plug_objectives: None,
                reusable_plugs: Some(vec![domain::ReusablePlug {
                    plug_hash: ItemHash::new(42),
                    enabled: false,
                    can_insert: false,
                }]),
            }],
        };
        build.equipped.push(BuildItem {
            instance_id: Some(item.instance_id.clone()),
            hash: d.hash,
            amount: None,
            socket_overrides: vec![],
            crafted_date: None,
        });
        items.push(item);
        defs.push(d);
    }
    let catalogs = vec![
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash: 99,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        }),
        ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
            hash: 100,
            category_style: 3,
            name: "Mod".into(),
        }),
    ];
    (build, items, defs, catalogs)
}
fn analyze_fixture(
    build: &BuildDefinition,
    items: &[ItemInstance],
    defs: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
) -> ModFitAnalysis {
    let resolved = analyze_build(build, &CharacterId::new("1"), items, defs, &[], &[], &[]);
    analyze_bucket_mods(build, &resolved.items, items, defs, catalogs)
}

#[test]
fn missing_armor_mod_categories_remain_unknown() {
    let (b, i, mut d, c) = fixture(3, 10);
    for armor in &mut d[1..] {
        armor.armor.socket_categories.clear();
    }
    let result = analyze_fixture(&b, &i, &d, &c);
    assert_eq!(result.evaluation, "not_evaluated");
    assert_eq!(
        result.reason,
        Some("armor_mod_socket_categories_unavailable")
    );
    assert_eq!(result.item_mod_assignments, None);
    assert_eq!(result.upgrade_costs, None);
}

#[test]
fn analyzer_general_activity_are_ranked() {
    for (category, cost, current, unassigned, upgrades) in [
        (2487827355, 3, 10, vec![], false),
        (2487827355, 9, 7, vec![], true),
        (2487827355, 12, 11, vec![42], false),
        (13646368, 3, 10, vec![42], false),
    ] {
        let (b, i, mut d, c) = fixture(cost, current);
        d[0].armor.plug_category_hash = Some(category);
        let result = analyze_fixture(&b, &i, &d, &c);
        assert_eq!(result.evaluation, "evaluated");
        assert_eq!(result.unassigned_mods, Some(unassigned));
        assert_eq!(result.needs_upgrades_for_mods, Some(upgrades));
    }
}

#[test]
fn analyzer_energy_upgrades_table() {
    for (cost, current, assumption, fits, upgrades) in [
        (7, 7, None, true, false),
        (9, 7, None, true, true),
        (10, 7, None, false, false),
        (10, 7, Some(AssumeArmorMasterwork::All), true, true),
        (11, 10, None, true, true),
        (12, 11, None, false, false),
        (1, 0, None, true, false), // Bucket-specific uses max(current,1).
    ] {
        let (mut b, i, d, c) = fixture(cost, current);
        b.parameters.as_mut().unwrap().assume_armor_masterwork = assumption;
        let result = analyze_fixture(&b, &i, &d, &c);
        assert_eq!(result.evaluation, "evaluated", "{cost}/{current}");
        assert_eq!(
            result.unassigned_mods.as_ref().unwrap().is_empty(),
            fits,
            "{cost}/{current}"
        );
        assert_eq!(
            result.needs_upgrades_for_mods,
            Some(upgrades),
            "{cost}/{current}"
        );
    }
}

#[test]
fn analyzer_unknown_and_negative_cases() {
    let (b, mut i, mut d, c) = fixture(3, 10);
    i[0].sockets[0].visible = false;
    let result = analyze_fixture(&b, &i, &d, &c);
    assert_eq!(result.invalid_mods, Some(vec![])); // Categorization ignores visibility.
    assert_eq!(result.unassigned_mods, Some(vec![42]));
    i[0].sockets[0].visible = true;
    i[0].sockets[0].reusable_plugs = None;
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).reason,
        Some("raw_reusable_plugs_not_projected")
    );
    i[0].sockets[0].reusable_plugs = Some(vec![]);
    // Another owned slot admits this mod, but helmet assignment cannot place it.
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).unassigned_mods,
        Some(vec![42])
    );
    for item in &mut i {
        item.sockets[0].reusable_plugs = Some(vec![]);
    }
    assert_eq!(analyze_fixture(&b, &i, &d, &c).invalid_mods, Some(vec![42]));
    d[0].armor.plug_category_hash = Some(3481777685);
    assert_eq!(analyze_fixture(&b, &i, &d, &c).invalid_mods, Some(vec![42]));
    // Once eager membership succeeds, free tuning assignment is evaluated.
    i[0].sockets[0]
        .reusable_plugs
        .as_mut()
        .unwrap()
        .push(domain::ReusablePlug {
            plug_hash: ItemHash::new(42),
            enabled: true,
            can_insert: true,
        });
    assert_eq!(analyze_fixture(&b, &i, &d, &c).reason, None);
    let mut seasonal = b.clone();
    seasonal.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(14520248)]);
    assert_eq!(
        analyze_fixture(&seasonal, &i, &d, &c).reason,
        Some("deprecated_mod_placeholder_definition_unavailable")
    );
    seasonal.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(2318667184)]);
    assert_eq!(
        analyze_fixture(&seasonal, &i, &d, &c).reason,
        Some("deprecated_mod_placeholder_definition_unavailable")
    );
}

#[test]
fn dim_free_mod_assignment_table() {
    // DIM mod-assignment-utils.ts:359-420: free slots are consumed once,
    // current artifice placement wins, and tuning ignores enabled flags.
    for (category, requested, expected) in [
        (3773173029, 1, vec![]),
        (3773173029, 6, vec![42]),
        (3481777685, 1, vec![]),
        (3481777685, 6, vec![42]),
    ] {
        let (mut b, i, mut d, c) = fixture(0, 10);
        d[0].armor.plug_category_hash = Some(category);
        b.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(42); requested]);
        let result = analyze_fixture(&b, &i, &d, &c);
        assert_eq!(result.evaluation, "evaluated", "{category}/{requested}");
        assert_eq!(result.unassigned_mods, Some(expected));
        assert_eq!(result.needs_upgrades_for_mods, Some(false));
    }
}

#[test]
fn artifice_exotic_assumption_requires_legacy_armor_and_raw_tuning_stays_unknown() {
    let (mut b, mut i, mut d, c) = fixture(0, 10);
    d[0].armor.plug_category_hash = Some(ARTIFICE_CATEGORY);
    // Hidden artifice sockets still eagerly admit mods, but don't prove artifice.
    for item in &mut i {
        item.sockets[0].visible = false;
    }
    d[1].tier = Some("Exotic".into());
    b.parameters.as_mut().unwrap().assume_armor_masterwork =
        Some(AssumeArmorMasterwork::ArtificeExotic);
    let result = analyze_fixture(&b, &i, &d, &c);
    assert_eq!(result.unassigned_mods, Some(vec![]));
    assert_eq!(result.needs_upgrades_for_mods, Some(true));
    let mut enhancement = d[0].clone();
    enhancement.hash = ItemHash::new(720825311);
    enhancement.armor.insertion_material_requirement_hash = Some(301);
    d.push(enhancement);
    let mut costs = c.clone();
    costs.push(ArmorCatalogRecord::MaterialRequirementSet(
        MaterialRequirementSetDefinition {
            hash: 301,
            materials: vec![
                MaterialRequirement {
                    item_hash: RANKED_MATERIALS[0],
                    count: 1,
                },
                MaterialRequirement {
                    item_hash: RANKED_MATERIALS[0],
                    count: 9,
                },
            ],
        },
    ));
    assert_eq!(
        analyze_fixture(&b, &i, &d, &costs).upgrade_costs,
        Some([1, 0, 0, 0, 0])
    );

    let mut masterwork = d[0].clone();
    masterwork.hash = ItemHash::new(99);
    masterwork.armor.plug_category_hash = Some(armor_model::ARMOR3_MASTERWORK_PLUG);
    d.push(masterwork);
    let mut socket = i[0].sockets[0].clone();
    socket.socket_index = 1;
    socket.plug_hash = Some(ItemHash::new(99));
    socket.visible = false; // Armor 3 detection ignores visibility and enabled.
    i[0].sockets.push(socket);
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).unassigned_mods,
        Some(vec![42])
    );

    d[0].armor.plug_category_hash = Some(TUNING_CATEGORY);
    i[0].sockets[0].visible = true;
    i[0].sockets[0].reusable_plugs = None;
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).reason,
        Some("raw_reusable_plugs_not_projected")
    );
}

#[test]
fn ranked_categories_are_valid_and_other_categories_are_invalid() {
    for (category, expected_reason, expected_invalid) in [
        (2487827355, None, Some(vec![])),
        (13646368, None, Some(vec![])),
        (123, None, Some(vec![42])),
    ] {
        let (b, i, mut d, c) = fixture(0, 10);
        d[0].armor.plug_category_hash = Some(category);
        let result = analyze_fixture(&b, &i, &d, &c);
        assert_eq!(result.reason, expected_reason);
        assert_eq!(result.invalid_mods, expected_invalid);
    }
}

#[test]
fn seasonal_bucket_mods_resolve_to_normal_without_unlocks() {
    let (mut b, mut i, mut d, c) = fixture(3, 10);
    let normal = 14520248;
    d[0].hash = ItemHash::new(normal);
    b.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(2318667184)]);
    for item in &mut i {
        item.sockets[0].plug_hash = Some(ItemHash::new(normal));
        item.sockets[0].reusable_plugs.as_mut().unwrap()[0].plug_hash = ItemHash::new(normal);
    }
    let result = analyze_fixture(&b, &i, &d, &c);
    assert_eq!(result.evaluation, "evaluated");
    assert_eq!(result.invalid_mods, Some(vec![]));
    assert_eq!(result.unassigned_mods, Some(vec![]));
}

#[test]
fn unlocked_reduced_bucket_mods_fit_with_their_actual_cost_and_duplicates() {
    let (mut b, mut i, mut d, c) = fixture(9, 10);
    let mut reduced = d[0].clone();
    d[0].hash = ItemHash::new(14520248);
    reduced.hash = ItemHash::new(2318667184);
    reduced.armor.energy_cost.as_mut().unwrap().energy_cost = 3;
    d.push(reduced);
    b.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(14520248); 2]);
    for item in &mut i {
        item.sockets[0].plug_hash = Some(ItemHash::new(14520248));
        item.sockets[0].reusable_plugs.as_mut().unwrap()[0].plug_hash = ItemHash::new(2318667184);
    }
    let resolved = analyze_build(&b, &CharacterId::new("1"), &i, &d, &[], &[], &[]);
    let result = analyze_bucket_mods_with_unlocks(&b, &resolved.items, &i, &d, &c, &[2318667184]);
    assert_eq!(result.evaluation, "evaluated");
    assert_eq!(result.invalid_mods, Some(vec![]));
    assert_eq!(result.unassigned_mods, Some(vec![2318667184]));
    assert_eq!(
        b.parameters.unwrap().mods.unwrap(),
        vec![ItemHash::new(14520248); 2]
    );
}

#[test]
fn deprecated_missing_or_nonpluggable_mods_use_dim_placeholder() {
    let (mut b, i, mut d, c) = fixture(3, 10);
    let mut placeholder = d[0].clone();
    placeholder.hash = ItemHash::new(DEPRECATED_MOD_PLACEHOLDER.into());
    d.push(placeholder);
    // Both an absent definition and an existing non-pluggable definition resolve
    // to the placeholder. It is invalid because no inventory socket admits it.
    b.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(999), ItemHash::new(1)]);
    let result = analyze_fixture(&b, &i, &d, &c);
    assert_eq!(result.evaluation, "evaluated");
    assert_eq!(
        result.invalid_mods,
        Some(vec![DEPRECATED_MOD_PLACEHOLDER; 2])
    );
    assert_eq!(result.unassigned_mods, Some(vec![]));
}

#[test]
fn five_resolved_entries_match_dim_full_set_gate_even_with_repeated_bucket() {
    // R3: DIM analysis.ts:454 checks length, not full-set bucket membership.
    let (b, i, mut d, c) = fixture(3, 10);
    d[2].bucket_hash = d[1].bucket_hash;
    assert_eq!(analyze_fixture(&b, &i, &d, &c).evaluation, "evaluated");
}

#[test]
fn expensive_variant_refit_seasonal_table() {
    for (normal_cost, expected) in [(9, Some(false)), (12, Some(true))] {
        let (mut b, mut i, mut d, c) = fixture(normal_cost, 10);
        d[0].hash = ItemHash::new(14520248);
        let mut reduced = d[0].clone();
        reduced.hash = ItemHash::new(2318667184);
        reduced.armor.energy_cost.as_mut().unwrap().energy_cost = 3;
        d.push(reduced);
        b.parameters.as_mut().unwrap().mods = Some(vec![ItemHash::new(14520248)]);
        for item in &mut i {
            item.sockets[0].plug_hash = Some(ItemHash::new(14520248));
            item.sockets[0].reusable_plugs = Some(
                vec![14520248, 2318667184]
                    .into_iter()
                    .map(|hash| domain::ReusablePlug {
                        plug_hash: ItemHash::new(hash),
                        enabled: false,
                        can_insert: false,
                    })
                    .collect(),
            );
        }
        let resolved = analyze_build(&b, &CharacterId::new("1"), &i, &d, &[], &[], &[]);
        let result =
            analyze_bucket_mods_with_unlocks(&b, &resolved.items, &i, &d, &c, &[2318667184]);
        assert_eq!(result.uses_seasonal_mods, expected);
        assert_eq!(
            result.item_mod_assignments.as_ref().unwrap()[0].1,
            vec![2318667184]
        );
        assert_eq!(result.unassigned_mods, Some(vec![]));
        d.remove(0);
        for item in &mut i {
            item.sockets[0].plug_hash = Some(ItemHash::new(2318667184));
        }
        let resolved = analyze_build(&b, &CharacterId::new("1"), &i, &d, &[], &[], &[]);
        let unknown =
            analyze_bucket_mods_with_unlocks(&b, &resolved.items, &i, &d, &c, &[2318667184]);
        assert_eq!(unknown.evaluation, "evaluated");
        assert_eq!(unknown.uses_seasonal_mods, None);
        assert_eq!(
            unknown.reason,
            Some("expensive_variant_fit_inputs_unavailable")
        );
    }
}

#[test]
fn dim_energy_upgrade_selection_and_cumulative_materials() {
    let (b, mut items, mut defs, mut catalogs) = fixture(5, 3);
    defs[0].armor.plug_category_hash = Some(2487827355);
    items[0].energy.as_mut().unwrap().energy_type = Some(3);
    let tier_socket = items[0].sockets[0].clone();
    items[0].sockets.push(IndexedSocket {
        socket_index: 1,
        ..tier_socket
    });
    defs[1]
        .armor
        .socket_categories
        .push(DefinitionSocketCategory {
            socket_category_hash: 760375309,
            socket_indexes: vec![1],
        });
    defs[1].armor.socket_entries.push(DefinitionSocketEntry {
        reusable_plug_set_hash: Some(201),
        ..Default::default()
    });
    let mut plugs = vec![];
    for (hash, capacity, category, kind, availability, count) in [
        (2001, 3, 400813043, 3, 2, 10),
        (2002, 5, 400813043, 3, 2, 20),
        (2003, 5, 400813043, 3, 2, 999), // First matching tier, not sum.
        (2004, 4, 400813043, 4, 2, 999), // Other energy type.
        (2005, 4, 400813043, 3, 0, 999), // Other availability.
        (2006, 4, 123, 3, 2, 999),       // Other category.
    ] {
        let mut plug = defs[0].clone();
        plug.hash = ItemHash::new(hash);
        plug.armor.plug_category_hash = Some(category);
        plug.armor.aspect_capacity = Some(capacity);
        plug.armor.energy_capacity_type = Some(kind);
        plug.armor.plug_availability = Some(availability);
        plug.armor.insertion_material_requirement_hash = Some(hash as u32);
        defs.push(plug);
        plugs.push(PlugSetItem {
            plug_item_hash: hash as u32,
            currently_can_roll: false,
        });
        catalogs.push(ArmorCatalogRecord::MaterialRequirementSet(
            MaterialRequirementSetDefinition {
                hash: hash as u32,
                materials: vec![
                    MaterialRequirement {
                        item_hash: RANKED_MATERIALS[4],
                        count,
                    },
                    MaterialRequirement {
                        item_hash: RANKED_MATERIALS[4],
                        count: 1,
                    },
                    MaterialRequirement {
                        item_hash: 123,
                        count: 999,
                    },
                ],
            },
        ));
    }
    catalogs.push(ArmorCatalogRecord::PlugSet(PlugSetDefinition {
        hash: 201,
        reusable_plug_items: plugs,
    }));
    let costs = cumulative_energy_costs(&items[0], &defs[1], &defs, &catalogs).unwrap();
    assert_eq!(costs.len(), 12);
    assert_eq!(costs[2], [0; 5]);
    assert_eq!(costs[3], [0, 0, 0, 0, 11]);
    assert_eq!(costs[4], costs[3]);
    assert_eq!(costs[5], [0, 0, 0, 0, 32]);
    assert_eq!(costs[11], costs[5]);
    // The first item supplies DIM's model for every Legendary. Already-paid
    // tiers make the second item cheapest, overriding current-placement ties.
    items[1].energy.as_mut().unwrap().energy_capacity = 5;
    let result = analyze_fixture(&b, &items, &defs, &catalogs);
    assert_eq!(result.evaluation, "evaluated");
    assert_eq!(result.item_mod_assignments.as_ref().unwrap()[1].1, vec![42]);
    assert_eq!(result.upgrade_costs, Some([0; 5]));
    assert_eq!(result.needs_upgrades_for_mods, Some(false));
    // Missing inputs are unknown, not free upgrades. Explicitly absent tier
    // sockets/plug sets, in contrast, have DIM's known empty plug list.
    let without_set: Vec<_> = catalogs
        .iter()
        .filter(|r| !matches!(r, ArmorCatalogRecord::PlugSet(_)))
        .cloned()
        .collect();
    assert_eq!(
        cumulative_energy_costs(&items[0], &defs[1], &defs, &without_set),
        Err("energy_plug_set_unavailable")
    );
    let without_materials: Vec<_> = catalogs
        .iter()
        .filter(|r| !matches!(r, ArmorCatalogRecord::MaterialRequirementSet(_)))
        .cloned()
        .collect();
    assert_eq!(
        analyze_fixture(&b, &items, &defs, &without_materials).reason,
        Some("material_requirement_set_unavailable")
    );
    let mut no_set = defs[1].clone();
    no_set.armor.socket_entries[1].reusable_plug_set_hash = None;
    assert_eq!(
        cumulative_energy_costs(&items[0], &no_set, &defs, &catalogs).unwrap(),
        vec![[0; 5]; 12]
    );
    // GhostTier is a fallback, not a second cost source. Hidden/disabled tier
    // sockets and cannot-roll plugs remain eligible (energy.ts:18-49).
    let mut ghost = defs[1].clone();
    ghost
        .armor
        .socket_categories
        .last_mut()
        .unwrap()
        .socket_category_hash = 446880883;
    items[0].sockets[1].visible = false;
    assert_eq!(
        cumulative_energy_costs(&items[0], &ghost, &defs, &catalogs).unwrap(),
        costs
    );
    no_set
        .armor
        .socket_categories
        .push(ghost.armor.socket_categories.last().unwrap().clone());
    assert_eq!(
        cumulative_energy_costs(&items[0], &no_set, &defs, &catalogs).unwrap(),
        vec![[0; 5]; 12]
    );
    items[0].energy.as_mut().unwrap().energy_type = None;
    assert_eq!(
        cumulative_energy_costs(&items[0], &defs[1], &defs, &catalogs),
        Err("armor_energy_type_unavailable")
    );
}

#[test]
fn stable_ties_follow_requested_order_not_cost_or_hash() {
    let sockets = [socket(0, Some(1), &[9, 1])];
    assert_eq!(
        assign_bucket_specific_mods(&sockets, &[mod_(9, 4), mod_(1, 1)], 4).assigned,
        [9]
    );
    assert_eq!(
        assign_bucket_specific_mods(&sockets, &[mod_(1, 1), mod_(9, 4)], 4).assigned,
        [1]
    );
}

#[test]
fn socket_membership_uses_sets_even_when_cannot_roll_and_excludes_masterwork() {
    let (b, mut i, mut d, mut c) = fixture(3, 10);
    d[1].armor.socket_entries[0].reusable_plug_set_hash = Some(200);
    i[0].sockets[0].reusable_plugs = Some(vec![]);
    c.push(ArmorCatalogRecord::PlugSet(PlugSetDefinition {
        hash: 200,
        reusable_plug_items: vec![PlugSetItem {
            plug_item_hash: 42,
            currently_can_roll: false,
        }],
    }));
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).unassigned_mods,
        Some(vec![])
    );
    d[1].armor.socket_entries[0].socket_type_hash = 1843767421;
    // The active socket still validates the mod during categorization, but the
    // assignment helper deliberately excludes this redundant masterwork socket.
    c.push(ArmorCatalogRecord::SocketType(SocketTypeDefinition {
        hash: 1843767421,
        socket_category_hash: 100,
        always_randomize_sockets: false,
        plug_whitelist: vec![],
    }));
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).unassigned_mods,
        Some(vec![42])
    );
}

#[test]
#[rustfmt::skip] // Keep the behavior-level totals alongside their compact fixtures.
fn stat_fits_are_independent_and_total_unassigned_mods_with_masterworks() {
    use destiny_core::{armor_model::*, build_stats::*};
    let (mut b, items, mut defs, catalogs) = fixture(10, 7);
    b.parameters.as_mut().unwrap().assume_armor_masterwork = Some(AssumeArmorMasterwork::All);
    defs[0].armor.investment_stats = vec![InvestmentStat {
        stat_type_hash: ARMOR_STATS[0],
        value: 10,
        is_conditionally_active: false,
    }];
    let resolved = analyze_build(&b, &CharacterId::new("1"), &items, &defs, &[], &[], &[]);
    let as_is = stat_mod_assignments(&b, &resolved.items, &defs, &catalogs, &[], false);
    let assumed = stat_mod_assignments(&b, &resolved.items, &defs, &catalogs, &[], true);
    assert_eq!(as_is.unassigned_mods, Some(vec![42]));
    assert_eq!(assumed.unassigned_mods, Some(vec![]));
    let stats = ArmorStats {
        stats: ARMOR_STATS.iter().enumerate().map(|(i, hash)| ArmorStat {
            stat_hash: *hash,
            base: if i < 3 { 10 } else { 0 },
            current: 99, // Must not count currently inserted mods/tuning twice.
            base_masterworked: None,
            maximum_value: None,
        }).collect(),
        total_base: 30,
    };
    let armor: Vec<_> = items.iter().map(|item| StatArmor {
        item,
        definition: defs.iter().find(|d| d.hash == item.item_hash).unwrap(),
        stats: &stats,
        masterwork: Some([0, 0, 0, 2, 2, 2]),
    }).collect();
    assert_eq!(total_stats(&armor, &as_is, &defs, &[], 0, false, None).unwrap(), [60, 50, 50, 10, 10, 10]);
    assert_eq!(total_stats(&armor, &assumed, &defs, &[], 0, false, Some(AssumeArmorMasterwork::All)).unwrap(), [60, 50, 50, 25, 25, 25]);
    let legacy: Vec<_> = armor.iter().map(|a| StatArmor { masterwork: None, ..*a }).collect();
    assert_eq!(total_stats(&legacy, &assumed, &defs, &[], 0, false, Some(AssumeArmorMasterwork::All)).unwrap(), [70, 60, 60, 10, 10, 10]);
    let mut font = defs[0].clone();
    font.hash = ItemHash::new(4046357305);
    font.armor.investment_stats.clear();
    let font_defs = vec![font];
    let mut fonts = as_is.clone();
    fonts.unassigned_mods = Some(vec![4046357305; 4]);
    let empty_assigned = fonts.item_mod_assignments.as_mut().unwrap();
    empty_assigned.iter_mut().for_each(|(_, mods)| mods.clear());
    assert_eq!(total_stats(&armor, &fonts, &font_defs, &[], 0, false, None).unwrap()[0], 50);
    assert_eq!(total_stats(&armor, &fonts, &font_defs, &[], 0, true, None).unwrap()[0], 100);
}

#[test]
fn mixed_tier_rarity_cache_remains_unknown_instead_of_inventing_prices() {
    let (mut b, mut items, defs, catalogs) = fixture(0, 10);
    b.parameters.as_mut().unwrap().mods = Some(vec![]);
    items[0].gear_tier = Some(4);
    items[1].energy.as_mut().unwrap().energy_capacity = 11;
    let result = analyze_fixture(&b, &items, &defs, &catalogs);
    assert_eq!(result.evaluation, "not_evaluated");
    assert_eq!(
        result.reason,
        Some("energy_cost_original_capacity_out_of_range")
    );
    items[0].gear_tier = Some(5);
    assert_eq!(
        analyze_fixture(&b, &items, &defs, &catalogs).evaluation,
        "evaluated"
    );
}
