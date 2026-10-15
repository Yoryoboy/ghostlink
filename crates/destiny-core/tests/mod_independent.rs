use destiny_core::{mod_assignment::*, *};

#[test]
fn dim_heap_order_and_hash_deduplication() {
    // DIM mod-permutations.ts:3-64. Order matters to the later strict tie-break.
    assert_eq!(generate_mod_permutations(&[]), vec![vec![None; 5]]);
    assert_eq!(
        generate_mod_permutations(&[7]),
        vec![
            vec![Some(7), None, None, None, None],
            vec![None, Some(7), None, None, None],
            vec![None, None, Some(7), None, None],
            vec![None, None, None, Some(7), None],
            vec![None, None, None, None, Some(7)],
        ]
    );
    for (mods, count) in [
        (vec![1, 1], 10),
        (vec![1, 2], 20),
        (vec![1, 2, 3, 4, 5], 120),
        (vec![1; 5], 1),
        (vec![1, 2, 3, 4, 5, 6], 120),
    ] {
        let permutations = generate_mod_permutations(&mods);
        assert_eq!(permutations.len(), count);
        assert_eq!(permutations[0][0], Some(mods[0]));
        if mods.len() > 5 {
            assert!(permutations.iter().all(|p| p[5] == Some(6)));
        }
    }
    let permutations = generate_mod_permutations(&[1, 2, 3, 4, 5]);
    assert_eq!(
        permutations[1],
        vec![Some(2), Some(1), Some(3), Some(4), Some(5)]
    );
    assert_eq!(
        permutations[2],
        vec![Some(3), Some(1), Some(2), Some(4), Some(5)]
    );
}

fn item(slot: usize, exotic: bool, tuning: Option<Option<u32>>) -> FreeModItem {
    FreeModItem {
        bucket: ARMOR_BUCKETS[slot],
        exotic,
        artifice: true,
        tuning,
        current_plugs: vec![],
    }
}

#[test]
fn dim_artifice_prefers_existing_then_nonexotic_then_first() {
    let mut items = vec![
        item(0, true, None),
        item(1, false, None),
        item(2, false, None),
    ];
    items[0].current_plugs = vec![1];
    items[2].current_plugs = vec![2];
    let result = assign_free_mods(&items, &[1, 2, 3, 4], &[]);
    assert_eq!(result.assigned, vec![vec![1], vec![3], vec![2]]);
    assert_eq!(result.unassigned, vec![4]);
    assert_eq!(assign_free_mods(&items, &[9], &[]).assigned[1], vec![9]);
    items[1].artifice = false;
    items[2].artifice = false;
    assert_eq!(assign_free_mods(&items, &[9], &[]).assigned[0], vec![9]);
}

#[test]
fn dim_tuning_canonical_order_exotic_count_and_direction_table() {
    let grenade = 1735777505;
    let melee = 4244567218;
    // Caller order is deliberately reversed. Exotics with no unique direction
    // accept any mod, but are dropped unless request count implies they were tuned.
    let items = vec![
        item(2, false, Some(Some(melee))),
        item(1, false, Some(Some(grenade))),
        item(0, true, Some(None)),
    ];
    #[rustfmt::skip] // Keep the behavior table compact.
    let cases = [
        (vec![309000506, 311164277], vec![vec![311164277], vec![309000506], vec![]], vec![]),
        (vec![3122197216, 311164277], vec![vec![311164277], vec![3122197216], vec![]], vec![]),
        (vec![309000506, 309000506, 311164277], vec![vec![311164277], vec![309000506], vec![309000506]], vec![]),
        (vec![311164277, 311164277], vec![vec![311164277], vec![], vec![]], vec![311164277]),
    ];
    for (mods, assigned, unassigned) in cases {
        let result = assign_free_mods(&items, &[], &mods);
        assert_eq!(result.assigned, assigned);
        assert_eq!(result.unassigned, unassigned);
    }
    // Known empty/conflicting membership means no restriction, not missing input.
    assert_eq!(
        assign_free_mods(&[item(0, false, Some(None))], &[], &[309000506]).unassigned,
        vec![]
    );
}

#[test]
fn dim_specialty_table_uses_visible_plugged_first_not_enabled() {
    let mut definition = ItemDefinition {
        hash: ItemHash::new(1),
        bucket_hash: Some(BucketHash::new(ARMOR_BUCKETS[0])),
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
    let mut plug = definition.clone();
    plug.hash = ItemHash::new(42);
    plug.armor.plug_category_hash = Some(123);
    let mut item = ItemInstance {
        instance_id: ItemInstanceId::new("1"),
        item_hash: definition.hash,
        location: ItemLocation::Vault,
        quantity: 1,
        equipped: false,
        locked: false,
        power: None,
        perks: vec![],
        plugs: vec![],
        stats: vec![],
        available_plugs: vec![],
        energy: None,
        gear_tier: None,
        transfer_status: Some(0),
        sockets: vec![],
    };
    for (socket_type, category) in SPECIALTY_SOCKETS {
        definition.armor.socket_entries = vec![DefinitionSocketEntry {
            socket_type_hash: socket_type,
            ..Default::default()
        }];
        item.sockets = vec![IndexedSocket {
            socket_index: 0,
            plug_hash: Some(plug.hash),
            enabled: false,
            visible: true,
            reusable_plugs: Some(vec![]),
            plug_objectives: None,
        }];
        assert_eq!(
            specialty_category(&item, &definition, &[plug.clone()], &[]),
            Ok(Some(category))
        );
        item.sockets[0].visible = false;
        assert_eq!(
            specialty_category(&item, &definition, &[plug.clone()], &[]),
            Ok(None)
        );
        item.sockets[0].visible = true;
        item.sockets[0].plug_hash = None;
        assert_eq!(
            specialty_category(&item, &definition, &[plug.clone()], &[]),
            Ok(None)
        );
    }
}
