use destiny_core::{
    build::*,
    build_analysis::*,
    domain::{ClassType, ItemSlot},
    *,
};

fn definition(hash: i64, bucket: i64) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(bucket)),
        name: "Same exotic".into(),
        tier: Some("Exotic".into()),
        item_type: None,
        element: None,
        slot: ItemSlot::from_equipment_bucket(BucketHash::new(bucket)),
        class_type: Some(ClassType::Hunter),
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: Default::default(),
        non_transferrable: Some(false),
        is_instance_item: Some(true),
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: None,
        stack_unique_label: None,
    }
}
fn item(id: &str, hash: i64, location: ItemLocation) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(hash),
        location,
        quantity: 1,
        equipped: true,
        locked: false,
        power: None,
        perks: vec![],
        plugs: vec![],
        stats: vec![],
        available_plugs: vec![],
        energy: None,
        gear_tier: None,
        sockets: Vec::new(),
        transfer_status: Some(0),
    }
}
fn spec(id: Option<&str>, hash: i64) -> BuildItem {
    BuildItem {
        instance_id: id.map(ItemInstanceId::new),
        hash: ItemHash::new(hash),
        amount: Some(1),
        socket_overrides: vec![],
        crafted_date: None,
    }
}
fn analyze(
    build: &BuildDefinition,
    items: &[ItemInstance],
    defs: &[ItemDefinition],
) -> BuildAnalysis {
    analyze_build(build, &CharacterId::new("1"), items, defs, &[], &[], &[])
}
fn set_catalog(hash: u32) -> armor_catalog::EquipableSetDefinition {
    armor_catalog::EquipableSetDefinition {
        hash,
        name: "Fixture set".into(),
        set_items: vec![],
        set_perks: [2, 4]
            .into_iter()
            .map(|required_set_count| armor_catalog::SetPerk {
                required_set_count,
                sandbox_perk_hash: required_set_count as u32,
                name: None,
                icon: None,
            })
            .collect(),
    }
}
fn set_status(
    build: &BuildDefinition,
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    sets: &[armor_catalog::EquipableSetDefinition],
) -> destiny_core::build_sets::EquippedSetBonuses {
    let analysis = analyze(build, inventory, definitions);
    destiny_core::build_sets::equipped_set_bonuses(&analysis.items, definitions, sets)
}
#[test]
fn native_set_counts_build_equipped_not_inventory_equipped_and_activates_thresholds() {
    let mut d = definition(10, 3448274439);
    d.armor.equipable_item_set_hash = Some(42);
    let mut inventory: Vec<_> = ["1", "2", "3", "4", "5"]
        .into_iter()
        .map(|id| item(id, 10, ItemLocation::Vault))
        .collect();
    inventory[0].equipped = false;
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("1"), 10), spec(Some("2"), 10)],
        unequipped: vec![spec(Some("5"), 10)],
        ..Default::default()
    };
    let sets = [set_catalog(42)];
    let first = set_status(&build, &inventory, &[d.clone()], &sets);
    assert_eq!(first.evaluation, "evaluated");
    assert_eq!(first.sets[0].equipped_count, 2);
    let perks = first.sets[0].perks.as_ref().unwrap();
    assert_eq!(perks[0].active, Some(true));
    assert_eq!(perks[1].active, Some(false));
    build
        .equipped
        .extend([spec(Some("3"), 10), spec(Some("4"), 10)]);
    assert_eq!(
        set_status(&build, &inventory, &[d], &sets).sets[0]
            .perks
            .as_ref()
            .unwrap()[1]
            .active,
        Some(true)
    );
}
#[test]
fn set_unknowns_preserve_hash_counts_and_null_activation_without_substitution() {
    let mut d = definition(10, 3448274439);
    d.armor.equipable_item_set_hash = Some(42);
    let inventory = vec![
        item("1", 10, ItemLocation::Vault),
        item("2", 10, ItemLocation::Vault),
    ];
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("1"), 10), spec(Some("2"), 10)],
        ..Default::default()
    };
    let unknown = set_status(&build, &inventory, &[d.clone()], &[]);
    assert_eq!(unknown.evaluation, "partial");
    assert_eq!(unknown.sets[0].hash, 42);
    assert_eq!(unknown.sets[0].name, None);
    assert_eq!(unknown.sets[0].perks, None);
    build.equipped.push(spec(Some("404"), 10));
    let partial = set_status(&build, &inventory, &[d], &[set_catalog(42)]);
    assert_eq!(partial.reasons, vec!["equipped_armor_unresolved"]);
    let perks = partial.sets[0].perks.as_ref().unwrap();
    assert_eq!(perks[0].active, Some(true));
    assert_eq!(perks[1].active, None);
}
#[test]
fn event_selector_visibility_enabled_native_precedence_and_first_socket_follow_dim() {
    let mut d = definition(10, 3448274439);
    let mut plug = definition(139044974, 0);
    plug.armor.plug_category_hash = Some(1313063513);
    let mut inventory = item("1", 10, ItemLocation::Vault);
    inventory.sockets.push(IndexedSocket {
        socket_index: 0,
        plug_hash: Some(plug.hash),
        enabled: true,
        visible: true,
        reusable_plugs: None,
        plug_objectives: None,
    });
    let build = BuildDefinition {
        equipped: vec![spec(Some("1"), 10)],
        ..Default::default()
    };
    for (visible, enabled, contributes) in [
        (true, true, true),
        (true, false, false),
        (false, true, false),
        (false, false, false),
    ] {
        inventory.sockets[0].visible = visible;
        inventory.sockets[0].enabled = enabled;
        let status = set_status(
            &build,
            &[inventory.clone()],
            &[d.clone(), plug.clone()],
            &[set_catalog(2132906400)],
        );
        assert_eq!(status.evaluation, "evaluated");
        assert_eq!(status.sets.len(), usize::from(contributes));
        if contributes {
            assert_eq!(status.sets[0].hash, 2132906400);
        }
    }
    inventory.sockets[0].visible = true;
    inventory.sockets[0].enabled = false;
    let mut second = inventory.sockets[0].clone();
    second.socket_index = 1;
    second.enabled = true;
    inventory.sockets.push(second);
    assert!(
        set_status(
            &build,
            &[inventory.clone()],
            &[d.clone(), plug.clone()],
            &[]
        )
        .sets
        .is_empty()
    );
    d.armor.equipable_item_set_hash = Some(42);
    assert_eq!(
        set_status(&build, &[inventory.clone()], &[d], &[set_catalog(42)]).sets[0].hash,
        42
    );
    let unknown = set_status(&build, &[inventory], &[definition(10, 3448274439)], &[]);
    assert_eq!(
        unknown.reasons,
        vec!["set_selector_plug_definition_unavailable"]
    );
}

fn finding(result: &BuildAnalysis, kind: FindingKind) -> &BuildFinding {
    result.findings.iter().find(|f| f.kind == kind).unwrap()
}
#[test]
fn incomplete_crafting_objectives_do_not_claim_a_known_missing_date() {
    let mut d = definition(10, 1498876634);
    d.armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 3583996951,
        socket_indexes: vec![0],
    }];
    let objectives = vec![
        ObjectiveDefinition::new(1, String::new(), 0)
            .unwrap()
            .with_ui_style(Some(2)),
        ObjectiveDefinition::new(3, String::new(), 0)
            .unwrap()
            .with_ui_style(Some(4)),
    ];
    let mut inventory = item("2", 10, ItemLocation::Vault);
    inventory.sockets = vec![IndexedSocket {
        socket_index: 0,
        plug_hash: Some(ItemHash::new(100)),
        enabled: true,
        visible: true,
        reusable_plugs: None,
        plug_objectives: Some(vec![(1, Some(1)), (3, Some(123))]),
    }];
    let mut wanted = spec(Some("404"), 10);
    wanted.crafted_date = Some(123);
    let build = BuildDefinition {
        equipped: vec![wanted],
        ..Default::default()
    };
    let result = analyze_build(
        &build,
        &CharacterId::new("1"),
        &[inventory],
        &[d],
        &[],
        &[],
        &objectives,
    );
    assert_eq!(
        result.items[0].resolution_reason,
        Some("crafted_dates_unavailable")
    );
}

#[test]
fn crafted_date_fallback_preserves_id_priority_and_dim_last_match_without_hash_gate() {
    let mut d = definition(10, 1498876634);
    d.armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 3583996951,
        socket_indexes: vec![0],
    }];
    let mut other = d.clone();
    other.hash = ItemHash::new(20);
    let objectives: Vec<_> = (1..=3)
        .map(|hash| {
            ObjectiveDefinition::new(hash, String::new(), 0)
                .unwrap()
                .with_ui_style(Some(hash + 1))
        })
        .collect();
    let mut items = vec![
        item("2", 10, ItemLocation::Vault),
        item("3", 20, ItemLocation::Vault),
    ];
    for item in &mut items {
        item.sockets = vec![IndexedSocket {
            socket_index: 0,
            plug_hash: Some(ItemHash::new(100)),
            enabled: false,
            visible: false,
            reusable_plugs: None,
            plug_objectives: Some(vec![(1, Some(1)), (2, Some(0)), (3, Some(123))]),
        }];
    }
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("404"), 10)],
        ..Default::default()
    };
    build.equipped[0].crafted_date = Some(123);
    let run = |build: &BuildDefinition| {
        analyze_build(
            build,
            &CharacterId::new("1"),
            &items,
            &[d.clone(), other.clone()],
            &[],
            &[],
            &objectives,
        )
    };
    assert_eq!(
        run(&build).items[0]
            .inventory
            .as_ref()
            .unwrap()
            .instance_id
            .as_str(),
        "3"
    );
    build.equipped[0].instance_id = Some(ItemInstanceId::new("2"));
    assert_eq!(
        run(&build).items[0]
            .inventory
            .as_ref()
            .unwrap()
            .instance_id
            .as_str(),
        "2"
    );
    build.equipped[0].instance_id = None;
    build.equipped[0].crafted_date = Some(0);
    assert_eq!(run(&build).resolved, 0);
    build.equipped[0].crafted_date = Some(124);
    assert_eq!(run(&build).resolved, 0);
    assert!(
        !run(&build)
            .not_evaluated
            .contains(&"crafted_dates_unavailable")
    );
}

#[test]
fn instance_identity_never_substitutes_and_preserves_missing_spec() {
    let defs = [definition(10, 3448274439)];
    let items = [item("2", 10, ItemLocation::Vault)];
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("404"), 10)],
        class_type: 1,
        ..Default::default()
    };
    build.equipped[0].crafted_date = Some(123);
    let missing = analyze(&build, &items, &defs);
    assert_eq!(missing.items[0].spec, build.equipped[0]);
    assert!(missing.items[0].inventory.is_none());
    assert_eq!(missing.items[0].resolution_reason, Some("item_not_found"));
    assert_eq!(
        finding(&missing, FindingKind::MissingItems).present,
        Some(true)
    );
    build.equipped[0].instance_id = Some(ItemInstanceId::new("2"));
    let found = analyze(&build, &items, &defs);
    assert_eq!(found.resolved, 1);
    assert_eq!(
        found.items[0].inventory.as_ref().unwrap().location,
        ItemLocation::Vault
    );
    assert_eq!(found.items[0].class_compatible, Some(true));
    assert_eq!(
        finding(&found, FindingKind::NotAFullArmorSet).present,
        Some(true)
    );
    assert!(analyze(&build, &items, &[]).items[0].inventory.is_none());
}
#[test]
fn hash_buckets_prefer_target_then_only_first_transferable_candidate() {
    for bucket in [
        3284755031, 4274335291, 1093090108, 3796357825, 2973005342, 1506418338,
    ] {
        let mut d = definition(10, bucket);
        let mut items = vec![
            item("2", 10, ItemLocation::Character(CharacterId::new("2"))),
            item("3", 10, ItemLocation::Postmaster(CharacterId::new("1"))),
        ];
        items[0].transfer_status = Some(2);
        let build = BuildDefinition {
            equipped: vec![spec(Some("404"), 10)],
            class_type: 1,
            ..Default::default()
        };
        assert_eq!(
            analyze(&build, &items, &[d.clone()]).items[0]
                .inventory
                .as_ref()
                .unwrap()
                .instance_id,
            ItemInstanceId::new("3")
        );
        items.pop();
        assert_eq!(analyze(&build, &items, &[d.clone()]).missing, 1);
        items.push(item("4", 10, ItemLocation::Vault));
        assert_eq!(analyze(&build, &items, &[d.clone()]).missing, 1); // never scan past first
        items[0].transfer_status = Some(1); // equipped is not NotTransferrable
        assert_eq!(analyze(&build, &items, &[d.clone()]).resolved, 1);
        d.non_transferrable = Some(true);
        assert_eq!(analyze(&build, &items, &[d]).missing, 1);
    }
    let mut d = definition(10, 999);
    d.is_instance_item = Some(false);
    let build = BuildDefinition {
        equipped: vec![spec(None, 10)],
        ..Default::default()
    };
    assert_eq!(
        analyze(&build, &[item("1", 10, ItemLocation::Vault)], &[d]).resolved,
        1
    );
}
#[test]
fn exotic_sentinels_name_matching_and_armor_guard_follow_dim() {
    let defs = [definition(10, 3448274439), definition(20, 3448274439)];
    let items = [item("2", 10, ItemLocation::Vault)];
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("2"), 10)],
        class_type: 1,
        ..Default::default()
    };
    for (constraint, mismatch) in [
        (ExoticArmor::None, true),
        (ExoticArmor::Any, false),
        (ExoticArmor::Item(ItemHash::new(20)), false),
    ] {
        build.parameters = Some(BuildParameters {
            exotic_armor_hash: Some(constraint),
            ..Default::default()
        });
        assert_eq!(
            finding(
                &analyze(&build, &items, &defs),
                FindingKind::DoesNotRespectExotic
            )
            .present,
            Some(mismatch)
        );
    }
    build.equipped.clear();
    let result = analyze(&build, &items, &defs);
    assert_eq!(
        finding(&result, FindingKind::NotAFullArmorSet).present,
        Some(false)
    );
    assert_eq!(
        finding(&result, FindingKind::DoesNotRespectExotic).present,
        Some(false)
    );
}
#[test]
fn search_invalid_nonmatching_and_unsupported_are_distinct() {
    let defs = [definition(10, 3448274439)];
    let items = [item("2", 10, ItemLocation::Vault)];
    let mut build = BuildDefinition {
        equipped: vec![spec(Some("2"), 10)],
        class_type: 1,
        ..Default::default()
    };
    for (query, invalid, mismatch) in [
        ("(", true, false),
        ("is:legendary", false, true),
        ("is:exotic", false, false),
    ] {
        build.parameters = Some(BuildParameters {
            query: Some(query.into()),
            ..Default::default()
        });
        let result = analyze(&build, &items, &defs);
        assert_eq!(
            finding(&result, FindingKind::InvalidSearchQuery).present,
            Some(invalid)
        );
        assert_eq!(
            finding(&result, FindingKind::ItemsDoNotMatchSearchQuery).present,
            Some(mismatch)
        );
        assert_eq!(
            finding(&result, FindingKind::ItemsDoNotMatchSearchQuery).evaluation,
            "search_only"
        );
    }
    for query in ["is:crafted", "DIMfuture:value"] {
        build.parameters = Some(BuildParameters {
            query: Some(query.into()),
            ..Default::default()
        });
        assert_eq!(
            finding(
                &analyze(&build, &items, &defs),
                FindingKind::InvalidSearchQuery
            )
            .evaluation,
            "not_evaluated"
        );
    }
    for kind in [
        FindingKind::InvalidMods,
        FindingKind::EmptyFragmentSlots,
        FindingKind::TooManyFragments,
        FindingKind::NeedsArmorUpgrades,
        FindingKind::UsesSeasonalMods,
        FindingKind::BetterStatsAvailable,
        FindingKind::ModsDontFit,
        FindingKind::DoesNotSatisfyStatConstraints,
    ] {
        let result = analyze(&build, &items, &defs);
        assert_eq!(finding(&result, kind).evaluation, "not_evaluated");
        assert!(finding(&result, kind).reason.is_some());
    }
}
