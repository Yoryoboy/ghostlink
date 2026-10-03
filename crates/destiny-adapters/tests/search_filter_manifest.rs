use destiny_adapters::{manifest::decode_inventory_items, sqlite::SqliteStore};
use destiny_app::manifest::normalize_manifest;
use destiny_core::domain::{ClassType, DamageType, ItemSlot};
use destiny_core::{
    DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery, DefinitionVersion,
    Resolution, SearchRepository, SourceMarker, Timestamp,
};

#[test]
fn search_filter_manifest_attributes_survive_decode_normalization_and_storage() {
    let damage = [
        None,
        Some(DamageType::Kinetic),
        Some(DamageType::Arc),
        Some(DamageType::Solar),
        Some(DamageType::Void),
        None,
        Some(DamageType::Stasis),
        Some(DamageType::Strand),
        None,
    ];
    let classes = [
        Some(ClassType::Titan),
        Some(ClassType::Hunter),
        Some(ClassType::Warlock),
        Some(ClassType::Any),
        None,
    ];
    let slots: [(u32, ItemSlot); 8] = [
        (1498876634, ItemSlot::Kinetic),
        (2465295065, ItemSlot::Energy),
        (953998645, ItemSlot::Power),
        (3448274439, ItemSlot::Helmet),
        (3551918588, ItemSlot::Gauntlets),
        (14239492, ItemSlot::Chest),
        (20886954, ItemSlot::Legs),
        (1585787867, ItemSlot::ClassItem),
    ];
    for (index, expected_damage) in damage.into_iter().enumerate() {
        let (bucket, expected_slot) = slots[index % slots.len()];
        for (class, expected_class) in classes.into_iter().enumerate() {
            let payload = format!(
                r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Test"}},"defaultDamageType":{index},"classType":{class},"inventory":{{"bucketTypeHash":{bucket}}}}}}}"#
            );
            let acquisition =
                decode_inventory_items(DefinitionVersion::new("v1"), payload.as_bytes()).unwrap();
            assert_eq!(acquisition.definitions[0].damage_type, expected_damage);
            assert_eq!(acquisition.definitions[0].slot, Some(expected_slot));
            assert_eq!(acquisition.definitions[0].class_type, expected_class);
            let definitions = normalize_manifest(acquisition).unwrap();
            let expected = definitions.definitions[0].clone();
            assert_eq!(expected.slot, Some(expected_slot));
            assert_eq!(expected.class_type, expected_class);
            assert_eq!(
                expected.element.as_deref().map(str::parse::<DamageType>),
                expected_damage.map(Ok)
            );
            let mut store = SqliteStore::open_in_memory().unwrap();
            let input = DefinitionReplacementInput {
                definitions,
                checked_at: Timestamp::new(1),
                source: SourceMarker::Manifest,
            };
            let metadata = store.replace_definitions(input.clone()).unwrap();
            assert_eq!(
                store.lookup_definition(expected.hash).unwrap(),
                Resolution::Unique(expected.clone())
            );
            assert_eq!(
                store
                    .search_definitions(&DefinitionSearchQuery {
                        tier: None,
                        element: expected_damage,
                        slot: Some(expected_slot),
                        class_type: expected_class,
                        text: None,
                        item_type: None,
                        tags: vec![],
                        limit: 10
                    })
                    .unwrap(),
                vec![expected]
            );
            assert_eq!(
                store.replace_definitions(input).unwrap().revision,
                metadata.revision
            );
        }
    }
}

#[test]
fn search_filter_manifest_missing_malformed_and_unknown_attributes_are_optional() {
    for fields in [
        "",
        r#", "classType":999,"defaultDamageType":999,"inventory":{"bucketTypeHash":42}"#,
        r#", "classType":null,"defaultDamageType":"bad","inventory":{"bucketTypeHash":null}"#,
        r#", "classType":-1,"defaultDamageType":-1"#,
    ] {
        let payload =
            format!(r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Test"}}{fields}}}}}"#);
        let acquisition =
            decode_inventory_items(DefinitionVersion::new("v1"), payload.as_bytes()).unwrap();
        let candidate = &acquisition.definitions[0];
        assert_eq!(candidate.damage_type, None);
        assert_eq!(candidate.slot, None);
        assert_eq!(candidate.class_type, None);
    }
}
