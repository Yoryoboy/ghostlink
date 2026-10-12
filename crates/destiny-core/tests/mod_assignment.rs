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
    // Once eager membership succeeds, independent assignment remains unknown.
    i[0].sockets[0]
        .reusable_plugs
        .as_mut()
        .unwrap()
        .push(domain::ReusablePlug {
            plug_hash: ItemHash::new(42),
            enabled: true,
            can_insert: true,
        });
    assert_eq!(
        analyze_fixture(&b, &i, &d, &c).reason,
        Some("dim_independent_mod_assignment_not_implemented")
    );
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
