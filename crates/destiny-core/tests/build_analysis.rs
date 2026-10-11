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
    analyze_build(build, &CharacterId::new("1"), items, defs, &[], &[])
}
fn finding(result: &BuildAnalysis, kind: FindingKind) -> &BuildFinding {
    result.findings.iter().find(|f| f.kind == kind).unwrap()
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
    assert_eq!(
        missing.items[0].resolution_reason,
        Some("crafted_dates_unavailable")
    );
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
