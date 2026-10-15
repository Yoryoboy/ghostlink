//! Synthetic API-shaped fixtures for the material-ranking prerequisites.
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

const PLUG: &[u8] = br#"{"111":{"hash":111,"displayProperties":{"name":"Upgrade"},"plug":{"plugCategoryHash":1,"insertionMaterialRequirementHash":123,"plugAvailability":2,"energyCapacity":{"capacityValue":10,"energyType":3,"energyTypeHash":456}}}}"#;
const MATERIALS: &[u8] = br#"{"123":{"hash":123,"materials":[{"itemHash":3159615086,"count":0,"omitFromRequirements":true},{"itemHash":3159615086,"count":500},{"itemHash":4294967295,"count":4294967295}]},"456":{"hash":456,"materials":[]}}"#;
fn items(bytes: &[u8]) -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: normalize_manifest(
            decode_inventory_items(DefinitionVersion::new("v1"), bytes).unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}
fn world() -> destiny_core::WorldDefinitionsAcquisition {
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs =
        decode_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, MATERIALS).unwrap();
    world
}
#[test]
fn upgrade_plug_projects_ranking_inputs() {
    let data = items(PLUG);
    let armor = &data.definitions.definitions[0].armor;
    assert_eq!(armor.insertion_material_requirement_hash, Some(123));
    assert_eq!(armor.energy_capacity_type, Some(3));
    assert_eq!(armor.plug_availability, Some(2));
    assert_eq!(armor.aspect_capacity, Some(10));
    let absent = items(br#"{"111":{"hash":111,"displayProperties":{"name":"No plug"}}}"#);
    assert_eq!(absent.definitions.definitions[0].armor, Default::default());
}
#[test]
fn material_catalog_keeps_zero_duplicate_and_empty_requirements() {
    let world = world();
    let ArmorCatalogRecord::MaterialRequirementSet(set) = &world.armor_catalogs[0] else {
        panic!()
    };
    assert_eq!(
        set.materials,
        vec![
            MaterialRequirement {
                item_hash: 3159615086,
                count: 0
            },
            MaterialRequirement {
                item_hash: 3159615086,
                count: 500
            },
            MaterialRequirement {
                item_hash: u32::MAX,
                count: u32::MAX
            }
        ]
    );
    assert_eq!(
        ArmorCatalogKind::MaterialRequirementSet.max_records(),
        10_000
    );
    assert!(
        decode_armor_catalog(
            ArmorCatalogKind::MaterialRequirementSet,
            br#"{"1":{"hash":1,"redacted":true}}"#
        )
        .unwrap()
        .is_empty()
    );
    for payload in [
        r#"{"01":{"hash":1,"materials":[]}}"#,
        r#"{"1":{"hash":2,"materials":[]}}"#,
        r#"{"1":{"hash":1}}"#,
        r#"{"1":{"hash":1,"materials":[{"itemHash":1,"count":-1}]}}"#,
        r#"{"1":{"hash":1,"materials":[{"itemHash":1,"count":4294967296}]}}"#,
        r#"{"1":{"hash":1,"materials":[]},"1":{"hash":1,"materials":[]}}"#,
    ] {
        assert!(
            decode_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, payload.as_bytes())
                .is_err()
        );
    }
    let over = format!(
        r#"{{"1":{{"hash":1,"materials":[{}]}}}}"#,
        vec![r#"{"itemHash":1,"count":0}"#; MAX_MATERIAL_REQUIREMENTS + 1].join(",")
    );
    assert!(
        decode_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, over.as_bytes()).is_err()
    );
    let mut padded = MATERIALS.to_vec();
    padded.resize(16 * 1024 * 1024 + 1, b' ');
    assert!(decode_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, &padded).is_err());
}
#[test]
fn material_inputs_and_catalog_have_fake_sqlite_revision_parity() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    // Exercise the v32 downgrade guard without a later migration's freshness reset.
    store.migrate_to(32).unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    let mut input = items(PLUG);
    let mut world = world();
    let mut prior = store
        .publish_manifest(input.clone(), world.clone())
        .unwrap();
    assert_eq!(
        fake.publish_manifest(input.clone(), world.clone()).unwrap(),
        prior
    );
    for field in 0..6 {
        let armor = &mut input.definitions.definitions[0].armor;
        match field {
            0 => armor.insertion_material_requirement_hash = Some(0),
            1 => armor.energy_capacity_type = Some(u32::MAX),
            2 => armor.plug_availability = Some(0),
            _ => {
                let ArmorCatalogRecord::MaterialRequirementSet(set) = &mut world.armor_catalogs[0]
                else {
                    panic!()
                };
                match field {
                    3 => set.materials[0].count = 1,
                    4 => set.materials[0].item_hash = 1,
                    _ => set.materials.reverse(),
                }
            }
        }
        let next = store
            .publish_manifest(input.clone(), world.clone())
            .unwrap();
        assert!(next.revision > prior.revision);
        assert_eq!(
            fake.publish_manifest(input.clone(), world.clone()).unwrap(),
            next
        );
        assert_eq!(
            store.lookup_definition(destiny_core::ItemHash::new(111)),
            fake.lookup_definition(destiny_core::ItemHash::new(111))
        );
        assert_eq!(
            store.armor_catalog(ArmorCatalogKind::MaterialRequirementSet, None),
            fake.armor_catalog(ArmorCatalogKind::MaterialRequirementSet, None)
        );
        assert_eq!(
            store
                .armor_catalog_batch(
                    ArmorCatalogKind::MaterialRequirementSet,
                    &[123, 456, 123, 0]
                )
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store
                .publish_manifest(input.clone(), world.clone())
                .unwrap(),
            next
        );
        prior = next;
    }
    assert!(store.migrate_to(31).is_err());
    assert_eq!(store.schema_version().unwrap(), 32);
    assert_eq!(store.current_metadata().unwrap(), Some(prior));
    // Complete replacement cascades stale rows and removes cost inputs.
    let empty = synthetic_world(DefinitionVersion::new("v1"));
    let sparse = items(br#"{"111":{"hash":111,"displayProperties":{"name":"Sparse"}}}"#);
    store
        .publish_manifest(sparse.clone(), empty.clone())
        .unwrap();
    fake.publish_manifest(sparse, empty).unwrap();
    assert!(
        store
            .armor_catalog(ArmorCatalogKind::MaterialRequirementSet, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.lookup_definition(destiny_core::ItemHash::new(111)),
        fake.lookup_definition(destiny_core::ItemHash::new(111))
    );
    store.migrate_to(31).unwrap();
}
#[test]
fn migration_32_refuses_unrepresentable_publication_and_refreshes_once() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(31).unwrap();
    assert!(
        store
            .publish_manifest(items(PLUG), synthetic_world(DefinitionVersion::new("v1")))
            .is_err()
    );
    let sparse = items(br#"{"111":{"hash":111,"displayProperties":{"name":"Sparse"}}}"#);
    assert!(store.publish_manifest(sparse.clone(), world()).is_err());
    store
        .publish_manifest(sparse, synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    store.migrate_to(32).unwrap();
    assert!(store.current_metadata().unwrap().is_none());
    let mut gateway = QueuedManifestGateway::new();
    gateway.queue_world(Ok(world()));
    for _ in 0..2 {
        gateway.queue(Ok(decode_inventory_items(
            DefinitionVersion::new("v1"),
            PLUG,
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
    assert_eq!(
        service
            .repository()
            .lookup_armor_catalog(ArmorCatalogKind::MaterialRequirementSet, 456)
            .unwrap(),
        Some(ArmorCatalogRecord::MaterialRequirementSet(
            MaterialRequirementSetDefinition {
                hash: 456,
                materials: vec![]
            }
        ))
    );
}
