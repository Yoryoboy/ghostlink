//! API-shaped synthetic identities, not assertions about live game content.
use destiny_adapters::{
    manifest::{decode_armor_catalog, decode_inventory_items},
    sqlite::SqliteStore,
};
use destiny_app::{
    manifest::normalize_manifest,
    test_support::{InMemoryDefinitionRepository, QueuedManifestGateway, synthetic_world},
};
use destiny_core::{
    DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
    armor_catalog::*,
    ports::{DefinitionRepository, WorldDefinitionRepository},
};

const GROUP: &[u8] = br#"{"123":{"hash":123,"scaledStats":[{"statHash":2996146975,"maximumValue":200,"displayAsNumeric":false,"displayInterpolation":[{"value":0,"weight":0},{"value":100,"weight":100}]}],"maximumValue":999,"displayProperties":{"name":"ignored"}}}"#;
const SET: &[u8] = br#"{"456":{"hash":456,"displayProperties":{"name":"Synthetic renewal","description":"ignored"},"setItems":[111,222],"setPerks":[{"requiredSetCount":2,"sandboxPerkHash":333},{"requiredSetCount":4,"sandboxPerkHash":444}]}}"#;
const ITEM: &[u8] = br#"{"111":{"hash":111,"displayProperties":{"name":"Synthetic helmet"}}}"#;

fn catalogs() -> Vec<ArmorCatalogRecord> {
    let mut records = decode_armor_catalog(ArmorCatalogKind::StatGroup, GROUP).unwrap();
    records.extend(decode_armor_catalog(ArmorCatalogKind::EquipableSet, SET).unwrap());
    records
}
fn items() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: normalize_manifest(
            decode_inventory_items(DefinitionVersion::new("v1"), ITEM).unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}

#[test]
fn stat_group_and_equipable_set_projection() {
    let records = catalogs();
    let ArmorCatalogRecord::StatGroup(group) = &records[0] else {
        panic!()
    };
    assert_eq!(group.maximum_value, Some(999));
    assert_eq!(group.scaled_stats[0].maximum_value, 200);
    assert_eq!(group.scaled_stats[0].display_interpolation[1].weight, 100);
    let ArmorCatalogRecord::EquipableSet(set) = &records[1] else {
        panic!()
    };
    assert_eq!(set.name, "Synthetic renewal");
    assert_eq!(set.set_items, [111, 222]);
    assert_eq!(set.set_perks[1].required_set_count, 4);
}

#[test]
fn catalog_rejects_aliases_duplicates_mismatch_and_unbounded_arrays() {
    for payload in [
        r#"{"01":{"hash":1,"scaledStats":[]}}"#,
        r#"{"1":{"hash":2,"scaledStats":[]}}"#,
        r#"{"1":{"hash":1,"scaledStats":[]},"1":{"hash":1,"scaledStats":[]}}"#,
        r#"{"1":{"hash":1,"scaledStats":"bad"}}"#,
    ] {
        assert!(decode_armor_catalog(ArmorCatalogKind::StatGroup, payload.as_bytes()).is_err());
    }
    let payload = format!(
        r#"{{"1":{{"hash":1,"displayProperties":{{"name":"Set"}},"setItems":[{}],"setPerks":[]}}}}"#,
        vec!["1"; 1025].join(",")
    );
    assert!(decode_armor_catalog(ArmorCatalogKind::EquipableSet, payload.as_bytes()).is_err());
    assert!(
        decode_armor_catalog(ArmorCatalogKind::StatGroup, b"{}")
            .unwrap()
            .is_empty()
    );
    assert!(
        decode_armor_catalog(
            ArmorCatalogKind::StatGroup,
            br#"{"1":{"hash":1,"redacted":true}}"#
        )
        .unwrap()
        .is_empty()
    );
    let mut padded = GROUP.to_vec();
    padded.resize(16 * 1024 * 1024 + 1, b' ');
    assert!(decode_armor_catalog(ArmorCatalogKind::StatGroup, &padded).is_err());
}

#[test]
fn fake_sqlite_parity_revision_identity_and_batch_reads() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = catalogs();
    let mut revision = store.publish_manifest(items(), world.clone()).unwrap();
    assert_eq!(
        fake.publish_manifest(items(), world.clone()).unwrap(),
        revision
    );
    for field in 0..11 {
        match &mut world.armor_catalogs[0] {
            ArmorCatalogRecord::StatGroup(group) => match field {
                0 => group.scaled_stats[0].stat_hash += 1,
                1 => group.scaled_stats[0].maximum_value += 1,
                2 => group.scaled_stats[0].display_as_numeric = true,
                3 => group.scaled_stats[0].display_interpolation[0].value = -1,
                4 => group.scaled_stats[0].display_interpolation[0].weight = -2,
                10 => group.maximum_value = Some(200),
                _ => {}
            },
            _ => panic!(),
        }
        match &mut world.armor_catalogs[1] {
            ArmorCatalogRecord::EquipableSet(set) => match field {
                5 => set.name.push('!'),
                6 => set.set_items.push(u32::MAX),
                7 => set.set_perks[0].required_set_count += 1,
                8 => set.set_perks[0].sandbox_perk_hash += 1,
                9 => set.set_items.reverse(),
                _ => {}
            },
            _ => panic!(),
        }
        let next = store.publish_manifest(items(), world.clone()).unwrap();
        assert!(next.revision > revision.revision);
        assert_eq!(fake.publish_manifest(items(), world.clone()).unwrap(), next);
        world.armor_catalogs.reverse();
        assert_eq!(
            store.publish_manifest(items(), world.clone()).unwrap(),
            next
        );
        assert_eq!(fake.publish_manifest(items(), world.clone()).unwrap(), next);
        world.armor_catalogs.reverse();
        for kind in ArmorCatalogKind::ALL {
            assert_eq!(
                store.armor_catalog(kind, None),
                fake.armor_catalog(kind, None)
            );
            let hash = if kind == ArmorCatalogKind::StatGroup {
                123
            } else {
                456
            };
            assert_eq!(
                store.armor_catalog_batch(kind, &[hash, hash, 0]),
                fake.armor_catalog_batch(kind, &[hash, hash, 0])
            );
        }
        revision = next;
    }
    let mut duplicate = world.clone();
    duplicate
        .armor_catalogs
        .push(duplicate.armor_catalogs[0].clone());
    assert!(store.publish_manifest(items(), duplicate.clone()).is_err());
    assert!(fake.publish_manifest(items(), duplicate).is_err());
    assert_eq!(store.current_metadata().unwrap(), Some(revision.clone()));
    assert!(store.migrate_to(27).is_err());
    // Empty socket catalogs permit 29 -> 28; populated armor catalogs guard 28 -> 27.
    assert_eq!(store.schema_version().unwrap(), 28);
    // The successful 29 down step invalidates freshness before the guarded 28 step.
    assert_eq!(store.current_metadata().unwrap(), None);
}

#[test]
fn migration_28_forces_same_version_refresh_once() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(27).unwrap();
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = catalogs();
    assert!(store.publish_manifest(items(), world).is_err());
    store
        .publish_manifest(items(), synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    store.migrate_to(28).unwrap();
    assert!(store.current_metadata().unwrap().is_none());
    let mut gateway = QueuedManifestGateway::new();
    for _ in 0..2 {
        gateway.queue(Ok(decode_inventory_items(
            DefinitionVersion::new("v1"),
            ITEM,
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
