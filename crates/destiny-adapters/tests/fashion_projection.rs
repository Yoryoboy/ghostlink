//! Migration 35: fashion classification of plug definitions (`itemType`, `itemSubType`,
//! `itemCategoryHashes`). Fakes and in-memory SQLite only.
use destiny_adapters::{manifest::decode_inventory_items, sqlite::SqliteStore};
use destiny_core::*;

fn input(plug: bool, categories: &str, sub_type: &str) -> DefinitionReplacementInput {
    let plug = if plug {
        r#","plug":{"plugCategoryHash":7}"#
    } else {
        ""
    };
    let json = format!(
        r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Item"}},"itemType":2,"itemSubType":{sub_type},"itemCategoryHashes":{categories}{plug}}}}}"#
    );
    DefinitionReplacementInput {
        definitions: destiny_app::manifest::normalize_manifest(
            decode_inventory_items(DefinitionVersion::new("v1"), json.as_bytes()).unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}
fn stored(store: &SqliteStore) -> ArmorDefinition {
    let Resolution::Unique(definition) = store.lookup_definition(ItemHash::new(1)).unwrap() else {
        panic!("one definition")
    };
    definition.armor
}

#[test]
fn plug_classification_decodes_persists_and_roundtrips_in_order() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let data = input(true, "[41,20,5]", "20");
    let armor = &data.definitions.definitions[0].armor;
    assert_eq!(armor.item_category_hashes, [41, 20, 5]);
    assert_eq!(
        (armor.item_type_code, armor.item_sub_type_code),
        (Some(2), Some(20))
    );
    store.replace_definitions(data).unwrap();
    let armor = stored(&store);
    assert_eq!(armor.item_category_hashes, [41, 20, 5]);
    assert_eq!(
        (armor.item_type_code, armor.item_sub_type_code),
        (Some(2), Some(20))
    );
    // Other projected plug data is untouched.
    assert_eq!(armor.plug_category_hash, Some(7));
}

#[test]
fn non_plug_definitions_and_malformed_values_project_nothing() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let data = input(false, "[41]", "20");
    assert_eq!(data.definitions.definitions[0].armor.item_type_code, None);
    store.replace_definitions(data).unwrap();
    assert_eq!(stored(&store), ArmorDefinition::default());
    // A sub type outside 0..=255 is unobserved, not a corrupted row.
    let data = input(true, r#"["x"]"#, "300");
    let armor = &data.definitions.definitions[0].armor;
    assert!(armor.item_category_hashes.is_empty());
    assert_eq!(armor.item_sub_type_code, None);
}

#[test]
fn too_many_category_hashes_are_refused() {
    let many = format!(
        "[{}]",
        (0..65).map(|i| i.to_string()).collect::<Vec<_>>().join(",")
    );
    let json = format!(
        r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Item"}},"itemCategoryHashes":{many},"plug":{{}}}}}}"#
    );
    assert!(decode_inventory_items(DefinitionVersion::new("v1"), json.as_bytes()).is_err());
}

#[test]
fn downgrade_is_guarded_by_data_and_upgrade_refreshes_freshness_once() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store
        .replace_definitions(input(true, "[41]", "20"))
        .unwrap();
    assert!(store.migrate_to(34).is_err());
    assert_eq!(store.schema_version().unwrap(), 35);
    store.replace_definitions(input(false, "[]", "0")).unwrap();
    store.migrate_to(34).unwrap();
    // The old schema cannot represent the data and refuses it without writing.
    assert!(
        store
            .replace_definitions(input(true, "[41]", "20"))
            .is_err()
    );
    store.replace_definitions(input(false, "[]", "0")).unwrap();
    // Upgrading invalidates Manifest freshness once.
    store.migrate_to(35).unwrap();
    assert!(
        DefinitionRepository::current_metadata(&store)
            .unwrap()
            .is_none()
    );
    store
        .replace_definitions(input(true, "[41]", "20"))
        .unwrap();
    store.migrate_to(35).unwrap();
    assert!(
        DefinitionRepository::current_metadata(&store)
            .unwrap()
            .is_some()
    );
}

#[test]
fn sqlite_and_fake_agree_on_the_projection() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::new(
        DefinitionVersion::new("v1"),
        vec![],
    );
    for data in [
        input(true, "[41]", "20"),
        input(true, "[]", "21"),
        input(true, "[41]", "20"),
    ] {
        let real = store.replace_definitions(data.clone()).unwrap();
        assert_eq!(real, fake.replace_definitions(data).unwrap());
        assert_eq!(
            store.lookup_definition(ItemHash::new(1)).unwrap(),
            fake.lookup_definition(ItemHash::new(1)).unwrap()
        );
    }
}
