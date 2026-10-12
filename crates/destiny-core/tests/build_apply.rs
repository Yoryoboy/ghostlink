use destiny_core::{
    build::*,
    build_analysis::ResolvedBuildItem,
    build_apply::*,
    domain::{ClassType, ItemSlot},
    *,
};
fn def(hash: i64, bucket: i64, label: &str) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(bucket)),
        name: "Item".into(),
        tier: Some("Legendary".into()),
        item_type: Some("Weapon".into()),
        element: None,
        slot: ItemSlot::from_equipment_bucket(BucketHash::new(bucket)),
        class_type: Some(ClassType::Any),
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            unique_label: Some(label.into()),
            ..Default::default()
        },
        non_transferrable: Some(false),
        is_instance_item: Some(true),
        does_postmaster_pull_have_side_effects: None,
        max_stack_size: Some(1),
        stack_unique_label: None,
    }
}
fn item(id: &str, hash: i64, location: ItemLocation, equipped: bool) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(hash),
        location,
        equipped,
        locked: false,
        quantity: 1,
        power: Some(100),
        transfer_status: Some(0),
        energy: None,
        gear_tier: None,
        sockets: vec![],
        available_plugs: vec![],
        perks: vec![],
        plugs: vec![],
        stats: vec![],
    }
}
fn selected(item: Option<&ItemInstance>, equip: bool) -> ResolvedBuildItem {
    ResolvedBuildItem {
        spec: BuildItem {
            instance_id: item.map(|i| i.instance_id.clone()),
            hash: item.map_or(ItemHash::new(99), |i| i.item_hash),
            amount: Some(1),
            socket_overrides: vec![],
            crafted_date: None,
        },
        should_equip: equip,
        inventory: item.cloned(),
        class_compatible: Some(true),
        resolution_reason: Some("not_owned"),
    }
}
fn chars() -> Vec<Character> {
    ["1", "2", "3"]
        .map(|id| Character {
            id: CharacterId::new(id),
            name: None,
            class: Some("Hunter".into()),
            race: None,
            gender: None,
            light: None,
            date_last_played: Some(Timestamp::new(id.parse().unwrap())),
        })
        .to_vec()
}
const BUCKETS: [i64; 8] = [
    1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
];
fn vault_of(capacity: usize) -> VaultEvidence {
    let buckets = BUCKETS.map(BucketHash::new);
    VaultEvidence::single(&buckets, BucketHash::new(138197802), capacity)
}
fn plan_with(
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    context: &ApplyContext,
    slots: usize,
) -> BuildApplyPlan {
    let policy = EquipmentCapacityPolicy::new(
        DefinitionVersion::new("v1"),
        "fixture",
        BUCKETS.map(|b| (BucketHash::new(b), slots)).to_vec(),
    )
    .unwrap();
    plan_build_items(
        &CharacterId::new("1"),
        selected,
        inventory,
        defs,
        &chars(),
        &policy,
        context,
    )
}
fn plan(
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    vault: Option<usize>,
    slots: usize,
) -> BuildApplyPlan {
    let evidence = vault.map(vault_of);
    let context = ApplyContext {
        vault: evidence.as_ref(),
        ..Default::default()
    };
    plan_with(selected, inventory, defs, &context, slots)
}
#[test]
fn source_dequip_precedes_vault_transfers_and_grouped_equips() {
    let defs = [
        def(10, 1498876634, "exotic_weapon"),
        def(11, 1498876634, ""),
        def(20, 2465295065, ""),
    ];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Character(CharacterId::new("2")),
            true,
        ),
        item(
            "11",
            11,
            ItemLocation::Character(CharacterId::new("2")),
            false,
        ),
        item("20", 20, ItemLocation::Vault, false),
    ];
    let before = items.clone();
    let p = plan(
        &[
            selected(Some(&items[0]), true),
            selected(Some(&items[2]), true),
        ],
        &items,
        &defs,
        Some(4),
        10,
    );
    assert!(p.skipped.is_empty());
    assert_eq!(p.steps.len(), 5);
    assert!(
        matches!(&p.steps[0].action, BuildAction::Equip { items, character } if items == &[ItemInstanceId::new("11")] && character.as_str()=="2")
    );
    // A lone source dequip is handled inline with the move (loadout-apply.ts:461).
    assert_eq!(p.steps[0].phase, ApplyPhase::Move);
    assert!(
        matches!(&p.steps[4].action, BuildAction::Equip { items, character } if items == &[ItemInstanceId::new("10"),ItemInstanceId::new("20")] && character.as_str()=="1")
    );
    assert_eq!(items, before);
}
#[test]
fn vault_space_displaces_to_least_recent_character_before_transfer() {
    let defs = [def(10, 1498876634, ""), def(11, 2465295065, "")];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Character(CharacterId::new("2")),
            false,
        ),
        item("11", 11, ItemLocation::Vault, false),
    ];
    let p = plan(
        &[selected(Some(&items[0]), false)],
        &items,
        &defs,
        Some(1),
        10,
    );
    assert!(p.skipped.is_empty());
    assert!(
        matches!(&p.steps[0].action,BuildAction::Move { item, to, .. } if item.as_str()=="11" && *to==ItemLocation::Character(CharacterId::new("1")))
    );
    assert_eq!(p.steps.len(), 3);
}
#[test]
fn unique_labels_not_rarity_drive_conflict_replacement() {
    let defs = [
        def(10, 1498876634, "limited"),
        def(20, 2465295065, "limited"),
        def(21, 2465295065, ""),
    ];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Character(CharacterId::new("1")),
            false,
        ),
        item(
            "20",
            20,
            ItemLocation::Character(CharacterId::new("1")),
            true,
        ),
        item(
            "21",
            21,
            ItemLocation::Character(CharacterId::new("1")),
            false,
        ),
    ];
    let p = plan(&[selected(Some(&items[0]), true)], &items, &defs, None, 10);
    assert!(p.skipped.is_empty());
    assert!(
        matches!(&p.steps[0].action,BuildAction::Equip { items, .. } if items==&[ItemInstanceId::new("10"),ItemInstanceId::new("21")])
    );
}
#[test]
fn unresolved_and_unknown_vault_capacity_are_typed_without_substitution() {
    let defs = [def(10, 1498876634, "")];
    let items = [item(
        "10",
        10,
        ItemLocation::Character(CharacterId::new("2")),
        false,
    )];
    let p = plan(
        &[selected(None, true), selected(Some(&items[0]), true)],
        &items,
        &defs,
        None,
        10,
    );
    assert!(p.steps.is_empty());
    assert_eq!(
        p.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [
            ApplySkipReason::Unresolved,
            ApplySkipReason::VaultCapacityUnavailable
        ]
    );
    assert!(p.not_applied.is_empty() && p.socket_skipped.is_empty());
}
#[test]
fn requested_items_are_never_chosen_as_replacements_and_failure_is_partial() {
    let defs = [
        def(10, 1498876634, ""),
        def(11, 1498876634, ""),
        def(20, 2465295065, ""),
    ];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Character(CharacterId::new("2")),
            true,
        ),
        item("11", 11, ItemLocation::Vault, false),
        item(
            "20",
            20,
            ItemLocation::Character(CharacterId::new("1")),
            false,
        ),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), true),
            selected(Some(&items[1]), false),
            selected(Some(&items[2]), true),
        ],
        &items,
        &defs,
        Some(4),
        10,
    );
    assert_eq!(p.skipped[0].reason, ApplySkipReason::NoReplacement);
    assert!(
        p.steps
            .iter()
            .all(|s| !matches!(&s.action,BuildAction::Move { item,.. } if item.as_str()=="10"))
    );
    assert!(
        matches!(&p.steps.last().unwrap().action,BuildAction::Equip { items,.. } if items==&[ItemInstanceId::new("20")])
    );
}
fn at(c: &str) -> ItemLocation {
    ItemLocation::Character(CharacterId::new(c))
}
fn describe(p: &BuildApplyPlan) -> Vec<String> {
    let loc = |l: &ItemLocation| match l {
        ItemLocation::Vault => "V".to_string(),
        ItemLocation::Character(c) => c.as_str().to_string(),
        ItemLocation::Postmaster(c) => format!("P{}", c.as_str()),
        ItemLocation::Other(o) => o.clone(),
    };
    p.steps
        .iter()
        .map(|s| {
            let phase = match s.phase {
                ApplyPhase::Dequip => "dequip",
                ApplyPhase::Move => "move",
                ApplyPhase::Equip => "equip",
                ApplyPhase::Sockets => "sockets",
                ApplyPhase::ClearSpace => "clear",
            };
            match &s.action {
                BuildAction::Move { item, from, to } => {
                    format!("{phase}: {} {}>{}", item.as_str(), loc(from), loc(to))
                }
                BuildAction::PullFromPostmaster { item, character } => {
                    format!("{phase}: pull {} {}", item.as_str(), character.as_str())
                }
                BuildAction::Equip { items, character } => format!(
                    "{phase}: equip {} on {}",
                    items
                        .iter()
                        .map(|i| i.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                    character.as_str()
                ),
                BuildAction::InsertPlug {
                    item,
                    socket_index,
                    plug,
                    ..
                } => format!(
                    "{phase}: plug {} {socket_index} {}",
                    item.as_str(),
                    plug.value()
                ),
            }
        })
        .collect()
}
fn safe(mut d: ItemDefinition) -> ItemDefinition {
    d.does_postmaster_pull_have_side_effects = Some(false);
    d
}
#[test]
fn several_dequips_per_owner_group_replacements_before_one_equip() {
    let defs = [
        def(10, 1498876634, ""),
        def(11, 1498876634, ""),
        def(20, 2465295065, ""),
        def(21, 2465295065, ""),
    ];
    let items = [
        item("10", 10, at("2"), true),
        item("20", 20, at("2"), true),
        item("11", 11, ItemLocation::Vault, false),
        item("21", 21, ItemLocation::Vault, false),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), true),
            selected(Some(&items[1]), true),
        ],
        &items,
        &defs,
        Some(10),
        10,
    );
    assert!(p.skipped.is_empty());
    assert_eq!(
        describe(&p)[..3],
        [
            "dequip: 11 V>2",
            "dequip: 21 V>2",
            "dequip: equip 11,21 on 2"
        ]
    );
    assert_eq!(describe(&p).last().unwrap(), "equip: equip 10,20 on 1");
}
#[test]
fn a_single_equip_runs_inline_with_its_move() {
    let defs = [def(10, 1498876634, ""), def(20, 2465295065, "")];
    let items = [
        item("10", 10, ItemLocation::Vault, false),
        item("20", 20, ItemLocation::Vault, false),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), true),
            selected(Some(&items[1]), false),
        ],
        &items,
        &defs,
        Some(10),
        10,
    );
    assert_eq!(
        describe(&p),
        ["move: 10 V>1", "move: equip 10 on 1", "move: 20 V>1"]
    );
}
#[test]
fn displacement_prefers_items_outside_the_owners_saved_loadouts() {
    let defs = [
        def(10, 1498876634, ""),
        def(11, 1498876634, ""),
        def(12, 1498876634, ""),
    ];
    let items = [
        item("11", 11, at("1"), false),
        item("12", 12, at("1"), false),
        item("10", 10, ItemLocation::Vault, false),
    ];
    let vault = vault_of(5);
    let membership = [(CharacterId::new("1"), ItemInstanceId::new("11"))];
    let context = ApplyContext {
        vault: Some(&vault),
        loadouts: Some(&membership),
        ..Default::default()
    };
    let p = plan_with(
        &[selected(Some(&items[2]), false)],
        &items,
        &defs,
        &context,
        2,
    );
    assert_eq!(describe(&p), ["move: 12 1>V", "move: 10 V>1"]);
    assert!(p.unknown.is_empty());
    let unknown = plan_with(
        &[selected(Some(&items[2]), false)],
        &items,
        &defs,
        &Default::default(),
        2,
    );
    assert_eq!(
        unknown.unknown,
        ["vault_capacity", "in_game_loadout_membership"]
    );
}
#[test]
fn postmaster_items_are_pulled_to_their_owner_under_the_pull_policy() {
    let defs = [safe(def(10, 1498876634, "")), safe(def(20, 2465295065, ""))];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Postmaster(CharacterId::new("1")),
            false,
        ),
        item(
            "20",
            20,
            ItemLocation::Postmaster(CharacterId::new("2")),
            false,
        ),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), false),
            selected(Some(&items[1]), false),
        ],
        &items,
        &defs,
        Some(10),
        10,
    );
    assert_eq!(
        describe(&p),
        [
            "move: pull 10 1",
            "move: pull 20 2",
            "move: 20 2>V",
            "move: 20 V>1"
        ]
    );
    let unsafe_defs = [def(10, 1498876634, ""), {
        let mut d = def(20, 2465295065, "");
        d.does_postmaster_pull_have_side_effects = Some(true);
        d
    }];
    let p = plan(
        &[
            selected(Some(&items[0]), false),
            selected(Some(&items[1]), false),
        ],
        &items,
        &unsafe_defs,
        Some(10),
        10,
    );
    assert!(p.steps.is_empty());
    assert_eq!(
        p.skipped.iter().map(|s| s.reason).collect::<Vec<_>>(),
        [
            ApplySkipReason::PostmasterSideEffectsUnknown,
            ApplySkipReason::PostmasterSideEffects
        ]
    );
}
#[test]
fn stackable_definitions_are_a_typed_skip() {
    let mut stack = def(10, 1498876634, "");
    stack.max_stack_size = Some(100);
    let items = [item("10", 10, ItemLocation::Vault, false)];
    let p = plan(
        &[selected(Some(&items[0]), false)],
        &items,
        &[stack],
        Some(10),
        10,
    );
    assert!(p.steps.is_empty());
    assert_eq!(p.skipped[0].reason, ApplySkipReason::StackUnsupported);
}
#[test]
fn clear_space_moves_unlisted_unequipped_items_only_for_selected_categories() {
    let defs = [
        def(10, 1498876634, ""),
        def(11, 1498876634, ""),
        def(12, 1498876634, ""),
        def(30, 3448274439, ""),
    ];
    let items = [
        item("10", 10, at("1"), false),
        item("11", 11, at("1"), false),
        item("12", 12, at("1"), true),
        item("30", 30, at("1"), false),
    ];
    let vault = vault_of(10);
    let weapons = ApplyContext {
        vault: Some(&vault),
        clear_weapons: true,
        ..Default::default()
    };
    let selection = [selected(Some(&items[0]), false)];
    let p = plan_with(&selection, &items, &defs, &weapons, 10);
    assert_eq!(describe(&p), ["clear: 11 1>V"]);
    assert!(p.clear_skipped.is_empty());
    let none = plan_with(
        &selection,
        &items,
        &defs,
        &ApplyContext {
            vault: Some(&vault),
            ..Default::default()
        },
        10,
    );
    assert!(none.steps.is_empty());
    let full = vault_of(0);
    let blocked = plan_with(
        &selection,
        &items,
        &defs,
        &ApplyContext {
            vault: Some(&full),
            clear_weapons: true,
            ..Default::default()
        },
        10,
    );
    assert!(blocked.steps.is_empty());
    assert_eq!(
        blocked.clear_skipped,
        [(ItemInstanceId::new("11"), ApplySkipReason::NoSpace)]
    );
}
#[test]
fn vault_capacity_is_per_corresponding_vault_bucket() {
    let defs = [def(10, 1498876634, ""), def(11, 2465295065, "")];
    let items = [
        item("10", 10, at("2"), false),
        item("11", 11, ItemLocation::Vault, false),
    ];
    let separate = VaultEvidence {
        mapping: vec![
            (BucketHash::new(1498876634), BucketHash::new(1)),
            (BucketHash::new(2465295065), BucketHash::new(2)),
        ],
        capacity: vec![(BucketHash::new(1), 1), (BucketHash::new(2), 1)],
    };
    let context = ApplyContext {
        vault: Some(&separate),
        ..Default::default()
    };
    let p = plan_with(
        &[selected(Some(&items[0]), false)],
        &items,
        &defs,
        &context,
        10,
    );
    assert!(p.skipped.is_empty());
    assert_eq!(describe(&p), ["move: 10 2>V", "move: 10 V>1"]);
    let unmapped = VaultEvidence {
        mapping: vec![(BucketHash::new(2465295065), BucketHash::new(2))],
        capacity: vec![(BucketHash::new(2), 1)],
    };
    let context = ApplyContext {
        vault: Some(&unmapped),
        ..Default::default()
    };
    let p = plan_with(
        &[selected(Some(&items[0]), false)],
        &items,
        &defs,
        &context,
        10,
    );
    assert_eq!(
        p.skipped[0].reason,
        ApplySkipReason::VaultCapacityUnavailable
    );
}
#[test]
fn occupants_of_other_known_buckets_do_not_block_character_space() {
    // A subclass-like item (unsupported bucket) occupies the character but is
    // not in the destination bucket; a bucketless definition stays unknown.
    let defs = [def(10, 1498876634, ""), def(50, 3284755031, "")];
    let items = [
        item("10", 10, ItemLocation::Vault, false),
        item("50", 50, at("1"), false),
    ];
    let p = plan(
        &[selected(Some(&items[0]), false)],
        &items,
        &defs,
        Some(5),
        1,
    );
    assert!(p.skipped.is_empty());
    assert_eq!(describe(&p), ["move: 10 V>1"]);
    let mut bucketless = def(50, 3284755031, "");
    bucketless.bucket_hash = None;
    let p = plan(
        &[selected(Some(&items[0]), false)],
        &items,
        &[defs[0].clone(), bucketless],
        Some(5),
        1,
    );
    assert_eq!(p.skipped[0].reason, ApplySkipReason::OccupancyUnknown);
}
#[test]
fn vault_evidence_follows_dim_accepted_items_and_bucket_capacity() {
    use destiny_core::armor_catalog::{
        InventoryBucketDefinition, VAULT_VENDOR_HASH, VaultAcceptedItem, VaultVendorDefinition,
    };
    let accepted = |a, d| VaultAcceptedItem {
        accepted_bucket: a,
        destination_bucket: d,
    };
    let bucket = |hash, item_count| InventoryBucketDefinition { hash, item_count };
    let vendor = VaultVendorDefinition {
        hash: VAULT_VENDOR_HASH,
        accepted_items: vec![
            accepted(1469714392, 138197802),
            accepted(3313201758, 138197802),
            // Later entries override earlier ones (`d2-buckets.ts:59-61`).
            accepted(1498876634, 7),
            accepted(1498876634, 138197802),
            // Destination 0 is falsy in DIM: no Vault bucket (`d2-buckets.ts:62-64`).
            accepted(138197802, 0),
            accepted(2465295065, 138197802),
            accepted(2465295065, 0),
            // Destination without a bucket definition: no Vault bucket.
            accepted(953998645, 999),
        ],
    };
    let evidence =
        VaultEvidence::from_catalogs(&vendor, &[bucket(138197802, 1300), bucket(1469714392, 10)])
            .unwrap();
    let general = BucketHash::new(138197802);
    assert_eq!(
        evidence.mapping,
        [
            (BucketHash::new(1469714392), general),
            (BucketHash::new(1498876634), general),
            (BucketHash::new(3313201758), general),
        ]
    );
    assert_eq!(evidence.capacity, [(general, 1300)]);
    // Nothing usable is missing evidence, never an empty mapping.
    assert_eq!(VaultEvidence::from_catalogs(&vendor, &[bucket(1, 5)]), None);
}
#[test]
fn steps_record_the_earlier_steps_they_depend_on() {
    // DIM runs a lone dequip inline with the move (loadout-apply.ts:437-513), so the
    // move and its second Vault leg cannot succeed unless that preparation did.
    let defs = [
        def(10, 1498876634, "exotic_weapon"),
        def(11, 1498876634, ""),
        def(20, 2465295065, ""),
    ];
    let items = [
        item(
            "10",
            10,
            ItemLocation::Character(CharacterId::new("2")),
            true,
        ),
        item(
            "11",
            11,
            ItemLocation::Character(CharacterId::new("2")),
            false,
        ),
        item("20", 20, ItemLocation::Vault, false),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), true),
            selected(Some(&items[2]), true),
        ],
        &items,
        &defs,
        Some(4),
        10,
    );
    let needs: Vec<_> = p.steps.iter().map(|s| s.needs.clone()).collect();
    assert_eq!(needs, [vec![], vec![0], vec![0, 1], vec![], vec![]]);
}
#[test]
fn grouped_dequip_equip_needs_every_replacement_and_later_moves_need_it() {
    // Several dequips per owner share one equip (loadout-apply.ts:437-513); a failed
    // replacement move fails the group, and the dequipped items' moves wait for it.
    let defs = [
        def(10, 1498876634, ""),
        def(11, 1498876634, ""),
        def(20, 2465295065, ""),
        def(21, 2465295065, ""),
    ];
    let on2 = ItemLocation::Character(CharacterId::new("2"));
    let items = [
        item("10", 10, on2.clone(), true),
        item("20", 20, on2.clone(), true),
        item("11", 11, ItemLocation::Vault, false),
        item("21", 21, ItemLocation::Vault, false),
    ];
    let p = plan(
        &[
            selected(Some(&items[0]), false),
            selected(Some(&items[1]), false),
        ],
        &items,
        &defs,
        Some(4),
        10,
    );
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    let group = p
        .steps
        .iter()
        .position(|s| matches!(&s.action, BuildAction::Equip { items, .. } if items.len() == 2))
        .expect("one grouped equip");
    assert_eq!(p.steps[group].needs, (0..group).collect::<Vec<_>>());
    for step in &p.steps[group + 1..] {
        assert!(step.needs.contains(&group), "{:?}", step);
    }
}
