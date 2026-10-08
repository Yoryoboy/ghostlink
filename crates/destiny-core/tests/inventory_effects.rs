use destiny_core::*;

fn stack(bucket: i64, quantity: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(42),
        BucketHash::new(bucket),
        quantity,
        container,
    )
    .unwrap()
}
fn stack_effect(quantity: u32, max_stack_size: i32) -> InventoryEffect {
    InventoryEffect::PullStack {
        item_hash: ItemHash::new(42),
        owner: owner(),
        quantity,
        destination_bucket: BucketHash::new(10),
        max_stack_size,
    }
}
fn stack_snapshot(mut stacks: Vec<StackObservation>) -> InventorySnapshot {
    stacks.sort();
    InventorySnapshot {
        stacks,
        ..snapshot()
    }
}

#[test]
fn stack_pull_decrements_smallest_eligible_and_replay_pulls_again() {
    let before = stack_snapshot(vec![
        stack(20, 2, StackContainer::Postmaster(owner())),
        stack(20, 8, StackContainer::Postmaster(owner())),
        stack(20, 8, StackContainer::Postmaster(owner())),
        stack(19, 8, StackContainer::Postmaster(owner())),
        stack(20, 12, StackContainer::Postmaster(owner())),
    ]);
    let result = apply_inventory_effects(&before, &[stack_effect(3, 10)]).unwrap();
    assert!(result.changed);
    let expected = stack_snapshot(vec![
        stack(20, 2, StackContainer::Postmaster(owner())),
        stack(20, 8, StackContainer::Postmaster(owner())),
        stack(20, 8, StackContainer::Postmaster(owner())),
        stack(19, 5, StackContainer::Postmaster(owner())),
        stack(20, 12, StackContainer::Postmaster(owner())),
        stack(10, 3, StackContainer::ProfileInventory),
    ]);
    assert_eq!(result.snapshot, expected);
    let replay = apply_inventory_effects(&expected, &[stack_effect(3, 10)]).unwrap();
    assert!(replay.changed);
    assert!(
        replay
            .snapshot
            .stacks
            .contains(&stack(19, 2, StackContainer::Postmaster(owner())))
    );
    assert_eq!(before.stacks.len(), 5);
}

#[test]
fn stack_pull_removes_source_fills_partials_and_splits_remainder() {
    for quantity in [25, 30] {
        let before = stack_snapshot(vec![
            stack(20, quantity, StackContainer::Postmaster(owner())),
            stack(10, 8, StackContainer::ProfileInventory),
            stack(10, 4, StackContainer::ProfileInventory),
            stack(11, 3, StackContainer::ProfileInventory),
            stack(10, 7, StackContainer::ProfileCurrencies),
            stack(10, 2, StackContainer::CharacterInventory(owner())),
            stack(20, 9, StackContainer::Postmaster(CharacterId::new("b"))),
        ]);
        let mut expected = before.clone();
        expected.stacks = vec![
            stack(10, 10, StackContainer::ProfileInventory),
            stack(10, 10, StackContainer::ProfileInventory),
            stack(10, 10, StackContainer::ProfileInventory),
            stack(11, 3, StackContainer::ProfileInventory),
            stack(10, 7, StackContainer::ProfileCurrencies),
            stack(10, 2, StackContainer::CharacterInventory(owner())),
            stack(20, 9, StackContainer::Postmaster(CharacterId::new("b"))),
        ];
        let mut remainder = quantity - 18;
        while remainder > 0 {
            expected.stacks.push(stack(
                10,
                remainder.min(10),
                StackContainer::ProfileInventory,
            ));
            remainder -= remainder.min(10);
        }
        expected.stacks.sort();
        assert_eq!(
            apply_inventory_effects(&before, &[stack_effect(quantity as u32, 10)])
                .unwrap()
                .snapshot,
            expected
        );
    }
}

#[test]
fn stack_pull_exact_split_and_canonical_partial_order() {
    let before = stack_snapshot(vec![stack(20, 30, StackContainer::Postmaster(owner()))]);
    let result = apply_inventory_effects(&before, &[stack_effect(30, 10)]).unwrap();
    assert_eq!(
        result.snapshot.stacks,
        vec![stack(10, 10, StackContainer::ProfileInventory); 3]
    );
    let before = stack_snapshot(vec![
        stack(20, 3, StackContainer::Postmaster(owner())),
        stack(10, 8, StackContainer::ProfileInventory),
        stack(10, 4, StackContainer::ProfileInventory),
    ]);
    assert_eq!(
        apply_inventory_effects(&before, &[stack_effect(3, 10)])
            .unwrap()
            .snapshot
            .stacks,
        vec![
            stack(10, 7, StackContainer::ProfileInventory),
            stack(10, 8, StackContainer::ProfileInventory)
        ]
    );
}

#[test]
fn stack_pull_rejects_invalid_amount_capacity_source_and_overflow_atomically() {
    let before = stack_snapshot(vec![stack(20, 10, StackContainer::Postmaster(owner()))]);
    for effect in [
        stack_effect(0, 10),
        stack_effect(1, 0),
        stack_effect(1, -1),
        stack_effect(11, 10),
        stack_effect(u32::MAX, 10),
    ] {
        assert!(
            apply_inventory_effects(
                &before,
                &[
                    InventoryEffect::SetLocked {
                        item: id("item"),
                        locked: true
                    },
                    effect
                ]
            )
            .is_err()
        );
    }
    for container in [
        StackContainer::ProfileInventory,
        StackContainer::Postmaster(CharacterId::new("b")),
    ] {
        assert!(
            apply_inventory_effects(
                &stack_snapshot(vec![stack(20, 10, container)]),
                &[stack_effect(1, 10)]
            )
            .is_err()
        );
    }
    let mut wrong_hash = before.clone();
    wrong_hash.stacks = vec![
        StackObservation::new(
            ItemHash::new(99),
            BucketHash::new(20),
            10,
            StackContainer::Postmaster(owner()),
        )
        .unwrap(),
    ];
    assert!(apply_inventory_effects(&wrong_hash, &[stack_effect(1, 10)]).is_err());
    assert!(!before.items[0].locked);
}

fn id(value: &str) -> ItemInstanceId {
    ItemInstanceId::new(value)
}
fn owner() -> CharacterId {
    CharacterId::new("a")
}
fn carried() -> ItemLocation {
    ItemLocation::Character(owner())
}
fn instance(value: &str, location: ItemLocation, equipped: bool) -> ItemInstance {
    ItemInstance {
        instance_id: id(value),
        item_hash: ItemHash::new(42),
        location,
        quantity: 1,
        equipped,
        locked: false,
        power: Some(2000),
        perks: vec![PerkSummary {
            perk_hash: PerkHash::new(7),
            active: true,
            visible: false,
        }],
        plugs: vec![PlugSummary {
            plug_hash: ItemHash::new(8),
            enabled: true,
        }],
        available_plugs: vec![AvailablePlug {
            socket_index: 2,
            plug_hash: ItemHash::new(9),
        }],
        stats: vec![StatSummary {
            stat_hash: 10,
            value: 50,
        }],
    }
}
fn snapshot() -> InventorySnapshot {
    InventorySnapshot {
        profile_currencies_coverage: ProfileCurrenciesCoverage::Available,
        stacks: vec![
            StackObservation::new(
                ItemHash::new(1),
                BucketHash::new(2),
                3,
                StackContainer::ProfileInventory,
            )
            .unwrap(),
        ],
        plug_set_entries: vec![PlugSetEntry {
            scope: PlugSetScope::Profile,
            plug_hash: ItemHash::new(8),
            can_insert: true,
            enabled: true,
        }],
        metadata: SnapshotMetadata {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(3),
            revision: SyncRevision::new(4),
            definition_version: DefinitionVersion::new("v"),
            checked_at: Timestamp::new(100),
            source: SourceMarker::Profile,
        },
        characters: vec![Character {
            id: owner(),
            name: Some("name".into()),
            class: None,
            race: None,
            gender: None,
            light: Some(2000),
            date_last_played: Some(Timestamp::new(90)),
        }],
        items: vec![
            instance("item", carried(), true),
            instance("old", carried(), true),
            instance("other", ItemLocation::Vault, false),
        ],
    }
}
fn assert_effects(effects: &[InventoryEffect], expected: InventorySnapshot) {
    let before = snapshot();
    let result = apply_inventory_effects(&before, effects).unwrap();
    assert!(result.changed);
    assert_eq!(result.snapshot, expected);
    assert_eq!(before, snapshot(), "input must remain untouched");
    let replay = apply_inventory_effects(&result.snapshot, effects).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.snapshot, result.snapshot);
}

#[test]
fn relocate_clears_equipped_and_preserves_every_other_field() {
    let effect = InventoryEffect::Relocate {
        item: id("item"),
        to: ItemLocation::Vault,
    };
    let mut expected = snapshot();
    expected.items[0].location = ItemLocation::Vault;
    expected.items[0].equipped = false;
    assert_effects(&[effect], expected);
}
#[test]
fn ordered_transfer_legs_route_through_vault() {
    let effects = [
        InventoryEffect::Relocate {
            item: id("item"),
            to: ItemLocation::Vault,
        },
        InventoryEffect::Relocate {
            item: id("item"),
            to: ItemLocation::Character(CharacterId::new("b")),
        },
    ];
    let mut expected = snapshot();
    expected.items[0].location = ItemLocation::Character(CharacterId::new("b"));
    expected.items[0].equipped = false;
    let result = apply_inventory_effects(&snapshot(), &effects).unwrap();
    assert!(result.changed);
    assert_eq!(result.snapshot, expected);
}
#[test]
fn lock_and_unlock_only_change_lock_state() {
    let mut expected = snapshot();
    expected.items[0].locked = true;
    assert_effects(
        &[InventoryEffect::SetLocked {
            item: id("item"),
            locked: true,
        }],
        expected.clone(),
    );
    let result = apply_inventory_effects(
        &expected,
        &[InventoryEffect::SetLocked {
            item: id("item"),
            locked: false,
        }],
    )
    .unwrap();
    assert!(result.changed);
    assert_eq!(result.snapshot, snapshot());
}
#[test]
fn equip_moves_target_and_clears_all_explicit_subjects() {
    let mut before = snapshot();
    before.items.push(instance("old2", carried(), true));
    let effect = InventoryEffect::Equip {
        item: id("other"),
        character: owner(),
        unequip: vec![id("item"), id("old"), id("old2")],
    };
    let mut expected = before.clone();
    for index in [0, 1, 3] {
        expected.items[index].equipped = false;
    }
    expected.items[2].location = carried();
    expected.items[2].equipped = true;
    let result = apply_inventory_effects(&before, std::slice::from_ref(&effect)).unwrap();
    assert!(result.changed);
    assert_eq!(result.snapshot, expected);
    let replay = apply_inventory_effects(&expected, &[effect]).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.snapshot, expected);
}
#[test]
fn pull_changes_postmaster_to_inventory_and_is_replayable() {
    let mut before = snapshot();
    before.items[0].location = ItemLocation::Postmaster(owner());
    let effect = InventoryEffect::PullFromPostmaster {
        item: id("item"),
        character: owner(),
    };
    let mut expected = before.clone();
    expected.items[0].location = carried();
    expected.items[0].equipped = false;
    let result = apply_inventory_effects(&before, std::slice::from_ref(&effect)).unwrap();
    assert!(result.changed);
    assert_eq!(result.snapshot, expected);
    let replay = apply_inventory_effects(&expected, &[effect]).unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.snapshot, expected);
}
#[test]
fn no_change_includes_empty_effects_and_net_zero_sequence() {
    let before = snapshot();
    for effects in [
        vec![],
        vec![InventoryEffect::SetLocked {
            item: id("item"),
            locked: false,
        }],
        vec![
            InventoryEffect::SetLocked {
                item: id("item"),
                locked: true,
            },
            InventoryEffect::SetLocked {
                item: id("item"),
                locked: false,
            },
        ],
        vec![InventoryEffect::Equip {
            item: id("item"),
            character: owner(),
            unequip: vec![],
        }],
    ] {
        let result = apply_inventory_effects(&before, &effects).unwrap();
        assert!(!result.changed);
        assert_eq!(result.snapshot, before);
    }
}
#[test]
fn unknown_and_duplicate_subjects_are_rejected_for_every_variant() {
    for subject in ["item", "old"] {
        let effects = if subject == "item" {
            vec![
                InventoryEffect::Relocate {
                    item: id(subject),
                    to: ItemLocation::Vault,
                },
                InventoryEffect::SetLocked {
                    item: id(subject),
                    locked: true,
                },
                InventoryEffect::Equip {
                    item: id(subject),
                    character: owner(),
                    unequip: vec![],
                },
                InventoryEffect::PullFromPostmaster {
                    item: id(subject),
                    character: owner(),
                },
            ]
        } else {
            vec![InventoryEffect::Equip {
                item: id("item"),
                character: owner(),
                unequip: vec![id(subject)],
            }]
        };
        for effect in effects {
            let mut missing = snapshot();
            missing.items.retain(|item| item.instance_id != id(subject));
            assert_eq!(
                apply_inventory_effects(&missing, std::slice::from_ref(&effect)),
                Err(InventoryEffectError::UnknownInstance { item: id(subject) })
            );
            let mut duplicate = snapshot();
            duplicate.items.push(
                duplicate
                    .items
                    .iter()
                    .find(|item| item.instance_id == id(subject))
                    .unwrap()
                    .clone(),
            );
            assert_eq!(
                apply_inventory_effects(&duplicate, &[effect]),
                Err(InventoryEffectError::DuplicateInstance { item: id(subject) })
            );
        }
    }
}
#[test]
fn unsupported_relocate_destinations_are_rejected() {
    for to in [
        ItemLocation::other("unknown"),
        ItemLocation::Postmaster(owner()),
    ] {
        assert_eq!(
            apply_inventory_effects(
                &snapshot(),
                &[InventoryEffect::Relocate {
                    item: id("item"),
                    to
                }]
            ),
            Err(InventoryEffectError::UnsupportedDestination)
        );
    }
    // Equip destinations are CharacterId by construction, not arbitrary locations.
}
#[test]
fn pull_rejects_wrong_owner_and_non_postmaster_sources() {
    for location in [
        ItemLocation::Vault,
        ItemLocation::other("unknown"),
        ItemLocation::Postmaster(CharacterId::new("b")),
        ItemLocation::Character(CharacterId::new("b")),
        carried(),
    ] {
        let mut before = snapshot();
        before.items[0].location = location;
        assert_eq!(
            apply_inventory_effects(
                &before,
                &[InventoryEffect::PullFromPostmaster {
                    item: id("item"),
                    character: owner()
                }]
            ),
            Err(InventoryEffectError::InvalidPostmasterSource { item: id("item") })
        );
    }
}
#[test]
fn unequip_requires_equipped_subjects_on_the_character_except_complete_replay() {
    for location in [
        ItemLocation::Vault,
        ItemLocation::Postmaster(owner()),
        ItemLocation::other("unknown"),
        ItemLocation::Character(CharacterId::new("b")),
        carried(),
    ] {
        let mut before = snapshot();
        before.items[1].location = location.clone();
        before.items[1].equipped = location != carried();
        assert_eq!(
            apply_inventory_effects(
                &before,
                &[InventoryEffect::Equip {
                    item: id("other"),
                    character: owner(),
                    unequip: vec![id("old")]
                }]
            ),
            Err(InventoryEffectError::InvalidUnequipSubject { item: id("old") })
        );
    }
    assert_eq!(
        apply_inventory_effects(
            &snapshot(),
            &[InventoryEffect::Equip {
                item: id("item"),
                character: owner(),
                unequip: vec![id("item")]
            }]
        ),
        Err(InventoryEffectError::InvalidUnequipSubject { item: id("item") })
    );
}
#[test]
fn later_failure_does_not_mutate_input_and_unrelated_duplicates_are_preserved() {
    let before = snapshot();
    assert_eq!(
        apply_inventory_effects(
            &before,
            &[
                InventoryEffect::SetLocked {
                    item: id("item"),
                    locked: true
                },
                InventoryEffect::SetLocked {
                    item: id("missing"),
                    locked: true
                }
            ]
        ),
        Err(InventoryEffectError::UnknownInstance {
            item: id("missing")
        })
    );
    assert_eq!(before, snapshot());
    let mut duplicate = before.clone();
    duplicate.items.push(duplicate.items[2].clone());
    let mut expected = duplicate.clone();
    expected.items[0].locked = true;
    assert_eq!(
        apply_inventory_effects(
            &duplicate,
            &[InventoryEffect::SetLocked {
                item: id("item"),
                locked: true
            }]
        )
        .unwrap()
        .snapshot,
        expected
    );
}
