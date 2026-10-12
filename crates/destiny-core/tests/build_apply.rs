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
fn plan(
    selected: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    vault: Option<usize>,
    slots: usize,
) -> BuildApplyPlan {
    let policy = EquipmentCapacityPolicy::new(
        DefinitionVersion::new("v1"),
        "fixture",
        [
            1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954,
            1585787867,
        ]
        .map(|b| (BucketHash::new(b), slots))
        .to_vec(),
    )
    .unwrap();
    plan_build_items(
        &CharacterId::new("1"),
        selected,
        inventory,
        defs,
        &chars(),
        &policy,
        vault,
    )
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
    assert_eq!(p.steps[0].phase, ApplyPhase::Dequip);
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
    assert_eq!(p.not_applied, ["sockets_mods_subclass_deferred_to_be_007b"]);
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
