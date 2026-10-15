//! API-shaped synthetic identities, not assertions about live game content.
use destiny_adapters::{manifest::decode_armor_catalog, sqlite::SqliteStore};
use destiny_app::test_support::{InMemoryDefinitionRepository, synthetic_world};
use destiny_core::{
    DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
    armor_catalog::*,
    ports::{DefinitionRepository, WorldDefinitionRepository},
};
const TYPE: &[u8] = br#"{"123":{"hash":123,"socketCategoryHash":456,"alwaysRandomizeSockets":true,"plugWhitelist":[{"categoryHash":789,"categoryIdentifier":"armor.stat.mods"}]}}"#;
const CATEGORY: &[u8] =
    br#"{"456":{"hash":456,"categoryStyle":3,"displayProperties":{"name":"Armor Mods"}}}"#;
const SET: &[u8] = br#"{"789":{"hash":789,"reusablePlugItems":[{"plugItemHash":111,"currentlyCanRoll":true},{"plugItemHash":222,"currentlyCanRoll":false}]}}"#;
fn records() -> Vec<ArmorCatalogRecord> {
    [
        (ArmorCatalogKind::SocketType, TYPE),
        (ArmorCatalogKind::SocketCategory, CATEGORY),
        (ArmorCatalogKind::PlugSet, SET),
    ]
    .into_iter()
    .flat_map(|(kind, bytes)| decode_armor_catalog(kind, bytes).unwrap())
    .collect()
}
fn items() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: destiny_app::manifest::normalize_manifest(
            destiny_adapters::manifest::decode_inventory_items(
                DefinitionVersion::new("v1"),
                br#"{"111":{"hash":111,"displayProperties":{"name":"Helmet"}}}"#,
            )
            .unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}
#[test]
fn socket_catalog_projection() {
    let rows = records();
    let ArmorCatalogRecord::SocketType(value) = &rows[0] else {
        panic!()
    };
    assert_eq!(value.socket_category_hash, 456);
    assert!(value.always_randomize_sockets);
    assert_eq!(
        value.plug_whitelist[0].category_identifier,
        "armor.stat.mods"
    );
    let ArmorCatalogRecord::SocketCategory(value) = &rows[1] else {
        panic!()
    };
    assert_eq!(value.category_style, 3);
    assert_eq!(value.name, "Armor Mods");
    let ArmorCatalogRecord::PlugSet(value) = &rows[2] else {
        panic!()
    };
    assert_eq!(value.reusable_plug_items[1].plug_item_hash, 222);
    assert!(!value.reusable_plug_items[1].currently_can_roll);
}
#[test]
fn sockets_persist_with_fake_parity_and_revision_identity() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = records();
    let mut prior = store.publish_manifest(items(), world.clone()).unwrap();
    assert_eq!(
        fake.publish_manifest(items(), world.clone()).unwrap(),
        prior
    );
    for field in 0..8 {
        match &mut world.armor_catalogs[0] {
            ArmorCatalogRecord::SocketType(v) => match field {
                0 => v.socket_category_hash += 1,
                1 => v.always_randomize_sockets = false,
                2 => v.plug_whitelist[0].category_hash += 1,
                3 => v.plug_whitelist[0].category_identifier.push('x'),
                _ => {}
            },
            _ => panic!(),
        }
        match &mut world.armor_catalogs[1] {
            ArmorCatalogRecord::SocketCategory(v) => match field {
                4 => v.category_style += 1,
                5 => v.name.push('x'),
                _ => {}
            },
            _ => panic!(),
        }
        match &mut world.armor_catalogs[2] {
            ArmorCatalogRecord::PlugSet(v) => match field {
                6 => v.reusable_plug_items[0].plug_item_hash += 1,
                7 => v.reusable_plug_items[0].currently_can_roll = false,
                _ => {}
            },
            _ => panic!(),
        }
        let next = store.publish_manifest(items(), world.clone()).unwrap();
        assert!(next.revision > prior.revision);
        assert_eq!(fake.publish_manifest(items(), world.clone()).unwrap(), next);
        world.armor_catalogs.reverse();
        assert_eq!(
            store.publish_manifest(items(), world.clone()).unwrap(),
            next
        );
        assert_eq!(fake.publish_manifest(items(), world.clone()).unwrap(), next);
        world.armor_catalogs.reverse();
        for kind in [
            ArmorCatalogKind::SocketType,
            ArmorCatalogKind::SocketCategory,
            ArmorCatalogKind::PlugSet,
        ] {
            let hash = world
                .armor_catalogs
                .iter()
                .find(|r| r.kind() == kind)
                .unwrap()
                .hash();
            assert_eq!(
                store.armor_catalog(kind, None).unwrap(),
                fake.armor_catalog(kind, None).unwrap()
            );
            assert_eq!(
                store.armor_catalog_batch(kind, &[hash, hash, 0]).unwrap(),
                fake.armor_catalog_batch(kind, &[hash, hash, 0]).unwrap()
            );
        }
        prior = next;
    }
    assert!(store.migrate_to(28).is_err());
    assert_eq!(store.schema_version().unwrap(), 29);
    // Successful 30 down step invalidates freshness before the guarded 29 step.
    assert_eq!(store.current_metadata().unwrap(), None);
}
#[test]
fn invalid_socket_catalogs_are_rejected() {
    for (kind, bytes) in [
        (ArmorCatalogKind::SocketType, TYPE),
        (ArmorCatalogKind::SocketCategory, CATEGORY),
        (ArmorCatalogKind::PlugSet, SET),
    ] {
        let bad = String::from_utf8(bytes.to_vec())
            .unwrap()
            .replacen("\"hash\":", "\"hash\":9", 1);
        assert!(decode_armor_catalog(kind, bad.as_bytes()).is_err());
        assert!(decode_armor_catalog(kind, br#"{"1":{"hash":1}}"#).is_err());
        assert!(
            decode_armor_catalog(kind, br#"{"1":{"hash":1,"redacted":true}}"#)
                .unwrap()
                .is_empty()
        );
    }
    let plug = r#"{"plugItemHash":111,"currentlyCanRoll":false}"#;
    // Large shared sets are valid; 1024 (inline socket limit) is not sufficient.
    for (count, valid) in [
        (4096, true),
        (MAX_PLUG_SET_ITEMS, true),
        (MAX_PLUG_SET_ITEMS + 1, false),
    ] {
        let payload = format!(
            r#"{{"1":{{"hash":1,"reusablePlugItems":[{}]}}}}"#,
            vec![plug; count].join(",")
        );
        assert_eq!(
            decode_armor_catalog(ArmorCatalogKind::PlugSet, payload.as_bytes()).is_ok(),
            valid
        );
    }
    let mut rows = records();
    let ArmorCatalogRecord::SocketType(v) = &mut rows[0] else {
        panic!()
    };
    v.plug_whitelist
        .resize(MAX_PLUG_WHITELIST + 1, v.plug_whitelist[0].clone());
    assert!(rows[0].validate().is_err());
}
#[test]
fn per_kind_record_limits_are_inclusive_and_roundtrip() {
    for (kind, limit, fields) in [
        (
            ArmorCatalogKind::PlugSet,
            50_000,
            r#""reusablePlugItems":[]"#,
        ),
        (
            ArmorCatalogKind::SocketType,
            20_000,
            r#""socketCategoryHash":456,"alwaysRandomizeSockets":false,"plugWhitelist":[]"#,
        ),
        (
            ArmorCatalogKind::SocketCategory,
            10_000,
            r#""categoryStyle":3,"displayProperties":{"name":"Mods"}"#,
        ),
        (ArmorCatalogKind::StatGroup, 10_000, r#""scaledStats":[]"#),
        (
            ArmorCatalogKind::EquipableSet,
            10_000,
            r#""displayProperties":{"name":"Set"},"setItems":[],"setPerks":[]"#,
        ),
    ] {
        let payload = |count: u32| {
            let rows: Vec<_> = (0..count)
                .map(|hash| format!(r#""{hash}":{{"hash":{hash},{fields}}}"#))
                .collect();
            format!("{{{}}}", rows.join(","))
        };
        let records = decode_armor_catalog(kind, payload(limit).as_bytes()).unwrap();
        assert_eq!(records.len(), limit as usize);
        assert!(validate_catalogs(&records).is_ok());
        let mut excessive = records.clone();
        assert!(decode_armor_catalog(kind, payload(limit + 1).as_bytes()).is_err());
        let extra = format!(r#"{{"{limit}":{{"hash":{limit},{fields}}}}}"#);
        excessive.extend(decode_armor_catalog(kind, extra.as_bytes()).unwrap());
        assert!(validate_catalogs(&excessive).is_err());
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut world = synthetic_world(DefinitionVersion::new("v1"));
        world.armor_catalogs = records.clone();
        store.publish_manifest(items(), world).unwrap();
        assert_eq!(store.armor_catalog(kind, None).unwrap(), records);
    }
}
#[test]
fn per_table_byte_limits_are_inclusive() {
    for (kind, bytes) in [
        (ArmorCatalogKind::PlugSet, 64 * 1024 * 1024),
        (ArmorCatalogKind::SocketType, 16 * 1024 * 1024),
    ] {
        let mut payload = b"{}".to_vec();
        payload.resize(bytes, b' ');
        assert!(decode_armor_catalog(kind, &payload).unwrap().is_empty());
        payload.push(b' ');
        assert!(decode_armor_catalog(kind, &payload).is_err());
    }
}
#[test]
fn migration_29_refreshes_same_version_once() {
    use destiny_app::test_support::QueuedManifestGateway;
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(28).unwrap();
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = records();
    assert!(store.publish_manifest(items(), world).is_err());
    store
        .publish_manifest(items(), synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    store.migrate_to(29).unwrap();
    assert!(store.current_metadata().unwrap().is_none());
    let mut gateway = QueuedManifestGateway::new();
    for _ in 0..2 {
        gateway.queue(Ok(destiny_adapters::manifest::decode_inventory_items(
            DefinitionVersion::new("v1"),
            br#"{"111":{"hash":111,"displayProperties":{"name":"Helmet"}}}"#,
        )
        .unwrap()));
    }
    let mut service = destiny_app::ManifestService::new(gateway, store);
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
}
