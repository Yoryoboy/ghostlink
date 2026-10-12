use destiny_adapters::{manifest::decode_inventory_items, sqlite::SqliteStore};
use destiny_core::*;

fn input(label: Option<&str>) -> DefinitionReplacementInput {
    let block = label.map_or("null".to_string(), |s| {
        format!(r#"{{"uniqueLabel":"{s}","equipableItemSetHash":9}}"#)
    });
    let json = format!(
        r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Item"}},"equippingBlock":{block}}}}}"#
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
#[test]
fn unique_labels_decode_persist_and_guard_downgrade() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let data = input(Some("exotic_weapon"));
    assert_eq!(
        data.definitions.definitions[0]
            .armor
            .unique_label
            .as_deref(),
        Some("exotic_weapon")
    );
    store.replace_definitions(data).unwrap();
    let Resolution::Unique(definition) = store.lookup_definition(ItemHash::new(1)).unwrap() else {
        panic!()
    };
    assert_eq!(
        definition.armor.unique_label.as_deref(),
        Some("exotic_weapon")
    );
    assert_eq!(definition.armor.equipable_item_set_hash, Some(9));
    assert!(store.migrate_to(32).is_err());
    store.replace_definitions(input(None)).unwrap();
    let Resolution::Unique(definition) = store.lookup_definition(ItemHash::new(1)).unwrap() else {
        panic!()
    };
    assert_eq!(definition.armor.unique_label, None);
    store.migrate_to(32).unwrap();
}
#[test]
fn labels_have_fake_parity_revision_identity_and_upgrade_refreshes_once() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::new(
        DefinitionVersion::new("v1"),
        vec![],
    );
    store.migrate_to(32).unwrap();
    store.replace_definitions(input(None)).unwrap();
    store.migrate_to(33).unwrap();
    assert!(
        DefinitionRepository::current_metadata(&store)
            .unwrap()
            .is_none()
    );
    let mut previous = None;
    for label in [None, Some("exotic_weapon"), Some(""), None] {
        let real = store.replace_definitions(input(label)).unwrap();
        assert_eq!(real, fake.replace_definitions(input(label)).unwrap());
        if let Some(revision) = previous {
            assert_eq!(real.revision.value(), revision + 1);
        }
        previous = Some(real.revision.value());
        assert_eq!(store.replace_definitions(input(label)).unwrap(), real);
        assert_eq!(
            store.lookup_definition(ItemHash::new(1)).unwrap(),
            fake.lookup_definition(ItemHash::new(1)).unwrap()
        );
        store.migrate_to(33).unwrap();
        assert_eq!(
            DefinitionRepository::current_metadata(&store).unwrap(),
            Some(real)
        );
    }
}
#[test]
fn empty_unique_label_is_known_unconstrained_and_roundtrips() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.replace_definitions(input(Some(""))).unwrap();
    let Resolution::Unique(definition) = store.lookup_definition(ItemHash::new(1)).unwrap() else {
        panic!()
    };
    assert_eq!(definition.armor.unique_label.as_deref(), Some(""));
}
