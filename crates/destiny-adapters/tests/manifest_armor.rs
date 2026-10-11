//! Synthetic, API-shaped fixtures: hashes are test identities, not game facts.
use destiny_adapters::{manifest::decode_inventory_items, sqlite::SqliteStore};
use destiny_app::{manifest::normalize_manifest, test_support::InMemoryDefinitionRepository};
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;

const ITEMS: &[u8] = br#"{
 "1":{"hash":1,"displayProperties":{"name":"Synthetic Armor 3.0 helmet"},
 "inventory":{"bucketTypeHash":3448274439},
 "investmentStats":[{"statTypeHash":10,"value":24,"isConditionallyActive":false},
 {"statTypeHash":11,"value":5,"isConditionallyActive":true}],
 "stats":{"statGroupHash":100},"equippingBlock":{"equipableItemSetHash":200},
 "sockets":{"socketEntries":[{"socketTypeHash":300,"singleInitialItemHash":301},
 {"socketTypeHash":302,"singleInitialItemHash":0,"reusablePlugSetHash":303},{"socketTypeHash":304,"singleInitialItemHash":0,"randomizedPlugSetHash":305}],
 "socketCategories":[{"socketCategoryHash":306,"socketIndexes":[0,1,2]}]}},
 "2":{"hash":2,"displayProperties":{"name":"Synthetic aspect"},
 "plug":{"plugCategoryIdentifier":"test.aspect","plugCategoryHash":400,"energyCapacity":{"capacityValue":2}}},
 "3":{"hash":3,"displayProperties":{"name":"Synthetic mod"},
 "investmentStats":[{"statTypeHash":10,"value":-5,"isConditionallyActive":false}],
 "plug":{"plugCategoryIdentifier":"test.mod","plugCategoryHash":401,
 "energyCost":{"energyCost":3,"energyType":0,"energyTypeHash":402}}}
}"#;

fn replacement() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: normalize_manifest(
            decode_inventory_items(DefinitionVersion::new("v1"), ITEMS).unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}

#[test]
fn projects_manifest_armor_investment_and_plug_metadata() {
    let items = replacement().definitions.definitions;
    let helmet = &items[0].armor;
    assert_eq!(helmet.investment_stats.len(), 2);
    assert_eq!(helmet.investment_stats[0].value, 24);
    assert!(helmet.investment_stats[1].is_conditionally_active);
    assert_eq!(helmet.stat_group_hash, Some(100));
    assert_eq!(helmet.equipable_item_set_hash, Some(200));
    assert_eq!(items[1].armor.aspect_capacity, Some(2));
    assert_eq!(items[1].armor.plug_category_hash, Some(400));
    assert_eq!(items[1].plug_category.as_deref(), Some("test.aspect"));
    assert_eq!(items[2].armor.investment_stats[0].value, -5);
    assert_eq!(
        items[2].armor.energy_cost,
        Some(PlugEnergyCost {
            energy_cost: 3,
            energy_type: 0,
            energy_type_hash: 402
        })
    );
}

#[test]
fn sqlite_fake_roundtrip_and_revision_parity_for_each_armor_field() {
    let input = replacement();
    let mut sqlite = SqliteStore::open_in_memory().unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    assert_eq!(
        sqlite.replace_definitions(input.clone()),
        fake.replace_definitions(input.clone())
    );
    for item in &input.definitions.definitions {
        assert_eq!(
            sqlite.lookup_definition(item.hash).unwrap(),
            Resolution::Unique(item.clone())
        );
        assert_eq!(
            sqlite.lookup_definition(item.hash),
            fake.lookup_definition(item.hash)
        );
    }
    assert_eq!(
        sqlite.replace_definitions(input.clone()),
        fake.replace_definitions(input.clone())
    );
    for field in 0..6 {
        let mut changed = input.clone();
        let armor = &mut changed.definitions.definitions[0].armor;
        match field {
            0 => armor.investment_stats.push(InvestmentStat {
                stat_type_hash: 12,
                value: -1,
                is_conditionally_active: true,
            }),
            1 => armor.stat_group_hash = Some(101),
            2 => armor.plug_category_hash = Some(0),
            3 => {
                armor.energy_cost = Some(PlugEnergyCost {
                    energy_cost: 0,
                    energy_type: 0,
                    energy_type_hash: 0,
                })
            }
            4 => armor.aspect_capacity = Some(0),
            _ => armor.equipable_item_set_hash = Some(201),
        }
        assert_eq!(
            sqlite.replace_definitions(changed.clone()),
            fake.replace_definitions(changed.clone())
        );
        assert_eq!(
            sqlite.lookup_definition(ItemHash::new(1)).unwrap(),
            Resolution::Unique(changed.definitions.definitions[0].clone())
        );
    }
}

#[test]
fn migration_26_guards_populated_armor_and_legacy_publication() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), 27);
    store.migrate_to(25).unwrap();
    store
        .replace_definitions(replacement())
        .expect_err("legacy schema cannot discard armor");
    assert!(store.current_metadata().unwrap().is_none());
    store.migrate_to(26).unwrap();
    let mut input = replacement();
    for item in &mut input.definitions.definitions {
        item.armor.socket_entries.clear();
        item.armor.socket_categories.clear();
    }
    store.replace_definitions(input.clone()).unwrap();
    store
        .migrate_to(25)
        .expect_err("downgrade must preserve armor");
    assert_eq!(store.schema_version().unwrap(), 26);
    assert_eq!(
        store.lookup_definition(ItemHash::new(1)).unwrap(),
        Resolution::Unique(input.definitions.definitions[0].clone())
    );
}

#[test]
fn armor_projection_rejects_malformed_and_over_limit_stats() {
    for field in [
        serde_json::json!([{"statTypeHash":10,"value":"bad","isConditionallyActive":false}]),
        serde_json::json!([{"statTypeHash":-1,"value":1,"isConditionallyActive":false}]),
        serde_json::json!(vec![
            serde_json::json!({"statTypeHash":10,"value":1,"isConditionallyActive":false});
            65
        ]),
    ] {
        let raw = serde_json::json!({"1":{"hash":1,"displayProperties":{"name":"Item"},"investmentStats":field}});
        assert!(
            decode_inventory_items(
                DefinitionVersion::new("v1"),
                &serde_json::to_vec(&raw).unwrap()
            )
            .is_err()
        );
    }
    let mut input = replacement();
    input.definitions.definitions[0].armor.investment_stats = vec![
        InvestmentStat {
            stat_type_hash: 1,
            value: 1,
            is_conditionally_active: false
        };
        65
    ];
    let mut sqlite = SqliteStore::open_in_memory().unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    assert!(sqlite.replace_definitions(input.clone()).is_err());
    assert!(fake.replace_definitions(input).is_err());
}

#[test]
fn missing_metadata_stays_unknown_and_investment_bound_roundtrips() {
    let raw = br#"{"1":{"hash":1,"displayProperties":{"name":"Unknown"}}}"#;
    let mut input = DefinitionReplacementInput {
        definitions: normalize_manifest(
            decode_inventory_items(DefinitionVersion::new("v1"), raw).unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    };
    assert_eq!(
        input.definitions.definitions[0].armor,
        ArmorDefinition::default()
    );
    input.definitions.definitions[0].armor.investment_stats = vec![
        InvestmentStat {
            stat_type_hash: u32::MAX,
            value: i32::MIN,
            is_conditionally_active: false,
        };
        64
    ];
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.replace_definitions(input.clone()).unwrap();
    assert_eq!(
        store.lookup_definition(ItemHash::new(1)).unwrap(),
        Resolution::Unique(input.definitions.definitions[0].clone())
    );
}

#[test]
fn each_migration_child_independently_guards_downgrade() {
    for field in 0..3 {
        let mut input = replacement();
        for item in &mut input.definitions.definitions {
            item.armor = Default::default();
        }
        let armor = &mut input.definitions.definitions[0].armor;
        match field {
            0 => armor.aspect_capacity = Some(0),
            1 => armor.investment_stats.push(InvestmentStat {
                stat_type_hash: 0,
                value: 0,
                is_conditionally_active: false,
            }),
            _ => {
                armor.energy_cost = Some(PlugEnergyCost {
                    energy_cost: 0,
                    energy_type: 0,
                    energy_type_hash: 0,
                })
            }
        }
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.migrate_to(26).unwrap();
        let metadata = store.replace_definitions(input.clone()).unwrap();
        assert!(store.migrate_to(25).is_err());
        assert_eq!(store.current_metadata().unwrap(), Some(metadata));
        assert_eq!(
            store.lookup_definition(ItemHash::new(1)).unwrap(),
            Resolution::Unique(input.definitions.definitions[0].clone())
        );
    }
}

#[test]
fn migration_forces_same_version_refresh_once() {
    use destiny_app::{
        ManifestService,
        test_support::{QueuedManifestGateway, synthetic_world},
    };
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(25).unwrap();
    let mut prior = replacement();
    for item in &mut prior.definitions.definitions {
        item.armor = Default::default();
    }
    store
        .publish_manifest(prior, synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    assert!(store.current_metadata().unwrap().is_some());
    store.migrate_to(27).unwrap();
    assert!(store.current_metadata().unwrap().is_none());
    let mut gateway = QueuedManifestGateway::new();
    for _ in 0..2 {
        gateway.queue(Ok(decode_inventory_items(
            DefinitionVersion::new("v1"),
            ITEMS,
        )
        .unwrap()));
    }
    let mut service = ManifestService::new(gateway, store);
    assert!(matches!(
        service.sync(Timestamp::new(2)).unwrap(),
        destiny_app::ManifestSyncReport::Updated { .. }
    ));
    assert!(matches!(
        service.sync(Timestamp::new(3)).unwrap(),
        destiny_app::ManifestSyncReport::Unchanged { .. }
    ));
    assert_eq!(service.gateway().component_calls(), 1);
    assert_eq!(service.gateway().world_calls(), 1);
    assert_eq!(
        service
            .repository()
            .lookup_definition(ItemHash::new(1))
            .unwrap(),
        Resolution::Unique(replacement().definitions.definitions[0].clone())
    );
}
