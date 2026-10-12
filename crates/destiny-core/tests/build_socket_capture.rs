use destiny_core::{armor_catalog::*, build_capture::*, domain::ReusablePlug, *};

fn socket(index: u8, hash: u32) -> IndexedSocket {
    IndexedSocket {
        socket_index: index,
        plug_hash: Some(ItemHash::new(i64::from(hash))),
        enabled: false,
        visible: false,
        reusable_plugs: Some(vec![ReusablePlug {
            plug_hash: ItemHash::new(i64::from(hash)),
            can_insert: false,
            enabled: false,
        }]),
        plug_objectives: None,
    }
}

#[test]
fn equipped_defaults_membership_and_fragment_positions_follow_dim() {
    let mut armor = ArmorDefinition {
        socket_entries: vec![
            DefinitionSocketEntry {
                socket_type_hash: 1,
                single_initial_item_hash: 10,
                plug_sources: Some(2),
                ..Default::default()
            };
            5
        ],
        socket_categories: vec![
            DefinitionSocketCategory {
                socket_category_hash: 309722977,
                socket_indexes: vec![0],
            },
            DefinitionSocketCategory {
                socket_category_hash: 2047681910,
                socket_indexes: vec![1],
            },
            DefinitionSocketCategory {
                socket_category_hash: 271461480,
                socket_indexes: vec![2, 3, 4],
            },
        ],
        ..Default::default()
    };
    armor.socket_entries[2].socket_type_hash = 2;
    armor.socket_entries[3].socket_type_hash = 2;
    armor.socket_entries[4].socket_type_hash = 2;
    let mut sockets = vec![
        socket(0, 10),
        socket(1, 20),
        socket(2, 10),
        socket(3, 30),
        socket(4, 40),
    ];
    let types = vec![
        SocketTypeDefinition {
            hash: 1,
            socket_category_hash: 309722977,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        },
        SocketTypeDefinition {
            hash: 2,
            socket_category_hash: 271461480,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        },
    ];
    let result = equipped_socket_overrides(&armor, &sockets, &types, &[], Some(2));
    // Ability defaults are retained, while the empty fragment still consumes capacity.
    assert_eq!(
        result
            .overrides
            .iter()
            .map(|(i, h)| (*i, h.value()))
            .collect::<Vec<_>>(),
        vec![(0, 10), (1, 20), (3, 30)]
    );
    assert!(!result.incomplete);
    sockets[0].reusable_plugs = Some(vec![]);
    assert!(
        !equipped_socket_overrides(&armor, &sockets, &types, &[], Some(2))
            .overrides
            .iter()
            .any(|(i, _)| *i == 0)
    );
    assert!(equipped_socket_overrides(&armor, &sockets, &types, &[], None).incomplete);
}

#[test]
fn default_empty_precedence_and_manifest_membership_ignore_unlock_flags() {
    let armor = ArmorDefinition {
        socket_entries: vec![DefinitionSocketEntry {
            socket_type_hash: 1,
            single_initial_item_hash: 99,
            reusable_plug_set_hash: Some(2),
            plug_sources: Some(2),
            ..Default::default()
        }],
        socket_categories: vec![DefinitionSocketCategory {
            socket_category_hash: 123,
            socket_indexes: vec![0],
        }],
        ..Default::default()
    };
    let types = vec![SocketTypeDefinition {
        hash: 1,
        socket_category_hash: 123,
        always_randomize_sockets: false,
        plug_whitelist: vec![],
    }];
    let sets = vec![PlugSetDefinition {
        hash: 2,
        reusable_plug_items: vec![
            PlugSetItem {
                plug_item_hash: 235531041,
                currently_can_roll: false,
            },
            PlugSetItem {
                plug_item_hash: 99,
                currently_can_roll: false,
            },
        ],
    }];
    assert_eq!(
        equipped_socket_overrides(&armor, &[socket(0, 99)], &types, &sets, Some(0)).overrides,
        vec![(0, ItemHash::new(99))]
    );
    assert!(
        equipped_socket_overrides(&armor, &[socket(0, 235531041)], &types, &sets, Some(0))
            .overrides
            .is_empty()
    );
    let mut only_manifest = socket(0, 99);
    only_manifest.reusable_plugs = Some(vec![]);
    assert_eq!(
        equipped_socket_overrides(&armor, &[only_manifest], &types, &sets, Some(0)).overrides,
        vec![(0, ItemHash::new(99))]
    );
    assert!(equipped_socket_overrides(&armor, &[socket(0, 99)], &[], &sets, Some(0)).incomplete);
    let mut unknown_sources = armor.clone();
    unknown_sources.socket_entries[0].plug_sources = None;
    assert!(
        equipped_socket_overrides(&unknown_sources, &[socket(0, 99)], &types, &sets, Some(0))
            .incomplete
    );
}

#[test]
fn in_game_conversion_keeps_empty_plugs_but_removes_unset() {
    assert_eq!(
        in_game_socket_overrides(&[
            (0, ItemHash::new(0)),
            (1, ItemHash::new(235531041)),
            (2, ItemHash::new(42))
        ]),
        vec![(1, ItemHash::new(235531041)), (2, ItemHash::new(42))]
    );
}
