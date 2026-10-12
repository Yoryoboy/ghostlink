//! Vault evidence (BE-007a2b): bucket `itemCount` and the Vault vendor's
//! `acceptedItems`, with shapes taken from the live Manifest measurements.
use destiny_adapters::{
    manifest::{decode_armor_catalog, decode_inventory_items},
    sqlite::SqliteStore,
};
use destiny_app::{
    manifest::normalize_manifest,
    test_support::{InMemoryDefinitionRepository, synthetic_world},
};
use destiny_core::{
    DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
    armor_catalog::*,
    ports::{DefinitionRepository, WorldDefinitionRepository},
};

const BUCKETS: &[u8] = br#"{"138197802":{"hash":138197802,"itemCount":1300,"category":1,"location":2,"displayProperties":{"name":"General"}},"1469714392":{"hash":1469714392,"itemCount":10,"category":3,"location":1},"9":{"hash":9,"redacted":true}}"#;
const VENDORS: &[u8] = br#"{"1":{"hash":1,"displayProperties":{"name":"Other"},"acceptedItems":[{"acceptedInventoryBucketHash":7,"destinationInventoryBucketHash":8}],"groups":[{"x":[1,2,3]}]},"1037843411":{"hash":1037843411,"displayProperties":{"name":"Vault"},"acceptedItems":[{"acceptedInventoryBucketHash":1469714392,"destinationInventoryBucketHash":138197802},{"acceptedInventoryBucketHash":3313201758,"destinationInventoryBucketHash":138197802},{"acceptedInventoryBucketHash":138197802,"destinationInventoryBucketHash":0}],"itemList":[{"a":1}]},"2":{"hash":2,"redacted":true}}"#;

fn items() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: normalize_manifest(
            decode_inventory_items(
                DefinitionVersion::new("v1"),
                br#"{"111":{"hash":111,"displayProperties":{"name":"Item"}}}"#,
            )
            .unwrap(),
        )
        .unwrap(),
        checked_at: Timestamp::new(1),
        source: SourceMarker::Manifest,
    }
}
fn world() -> destiny_core::WorldDefinitionsAcquisition {
    let mut world = synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs =
        decode_armor_catalog(ArmorCatalogKind::InventoryBucket, BUCKETS).unwrap();
    world
        .armor_catalogs
        .extend(decode_armor_catalog(ArmorCatalogKind::VaultVendor, VENDORS).unwrap());
    world
}
#[test]
fn buckets_keep_item_count_and_skip_redacted() {
    let records = decode_armor_catalog(ArmorCatalogKind::InventoryBucket, BUCKETS).unwrap();
    assert_eq!(
        records,
        vec![
            ArmorCatalogRecord::InventoryBucket(InventoryBucketDefinition {
                hash: 138197802,
                item_count: 1300
            }),
            ArmorCatalogRecord::InventoryBucket(InventoryBucketDefinition {
                hash: 1469714392,
                item_count: 10
            }),
        ]
    );
    for bad in [
        r#"{"01":{"hash":1,"itemCount":1}}"#,
        r#"{"1":{"hash":2,"itemCount":1}}"#,
        r#"{"1":{"hash":1}}"#,
        r#"{"1":{"hash":1,"itemCount":-1}}"#,
        r#"{"1":{"hash":1,"itemCount":4294967296}}"#,
        r#"{"1":{"hash":1,"itemCount":1},"1":{"hash":1,"itemCount":1}}"#,
    ] {
        assert!(decode_armor_catalog(ArmorCatalogKind::InventoryBucket, bad.as_bytes()).is_err());
    }
    let mut padded = BUCKETS.to_vec();
    padded.resize(4 * 1024 * 1024 + 1, b' ');
    assert!(decode_armor_catalog(ArmorCatalogKind::InventoryBucket, &padded).is_err());
}
#[test]
fn only_the_vault_vendor_is_retained_in_order() {
    let records = decode_armor_catalog(ArmorCatalogKind::VaultVendor, VENDORS).unwrap();
    assert_eq!(
        records,
        vec![ArmorCatalogRecord::VaultVendor(VaultVendorDefinition {
            hash: VAULT_VENDOR_HASH,
            accepted_items: vec![
                VaultAcceptedItem {
                    accepted_bucket: 1469714392,
                    destination_bucket: 138197802
                },
                VaultAcceptedItem {
                    accepted_bucket: 3313201758,
                    destination_bucket: 138197802
                },
                VaultAcceptedItem {
                    accepted_bucket: 138197802,
                    destination_bucket: 0
                },
            ]
        })]
    );
    assert_eq!(VAULT_VENDOR_HASH, 1037843411);
    // A table without the Vault vendor is legitimately empty (typed skip downstream).
    assert!(
        decode_armor_catalog(ArmorCatalogKind::VaultVendor, br#"{"1":{"hash":1}}"#)
            .unwrap()
            .is_empty()
    );
    // Structure outside the Vault vendor is skipped, but keys stay canonical and unique.
    for bad in [
        r#"{"01":{"hash":1}}"#,
        r#"{"1":{"hash":1},"1":{"hash":1}}"#,
        r#"{"1037843411":{"hash":1037843411}}"#,
        r#"{"1037843411":{"hash":2,"acceptedItems":[]}}"#,
        r#"{"1037843411":{"hash":1037843411,"acceptedItems":[{"acceptedInventoryBucketHash":-1,"destinationInventoryBucketHash":1}]}}"#,
    ] {
        assert!(decode_armor_catalog(ArmorCatalogKind::VaultVendor, bad.as_bytes()).is_err());
    }
    let over = format!(
        r#"{{"1037843411":{{"hash":1037843411,"acceptedItems":[{}]}}}}"#,
        vec![
            r#"{"acceptedInventoryBucketHash":1,"destinationInventoryBucketHash":2}"#;
            MAX_VAULT_ACCEPTED_ITEMS + 1
        ]
        .join(",")
    );
    assert!(decode_armor_catalog(ArmorCatalogKind::VaultVendor, over.as_bytes()).is_err());
    assert_eq!(ArmorCatalogKind::VaultVendor.max_records(), 20_000);
    assert_eq!(ArmorCatalogKind::InventoryBucket.max_records(), 10_000);
    // 20,001 vendor keys exceed the record ceiling even though none are retained.
    let many = format!(
        "{{{}}}",
        (1..=20_001)
            .map(|i| format!(r#""{i}":{{"hash":{i}}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(decode_armor_catalog(ArmorCatalogKind::VaultVendor, many.as_bytes()).is_err());
}
#[test]
fn vault_evidence_has_fake_sqlite_revision_parity() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(34).unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    let mut world = world();
    let mut prior = store.publish_manifest(items(), world.clone()).unwrap();
    assert_eq!(
        fake.publish_manifest(items(), world.clone()).unwrap(),
        prior
    );
    for field in 0..3 {
        match (field, &mut world.armor_catalogs[..]) {
            (0, [ArmorCatalogRecord::InventoryBucket(b), ..]) => b.item_count = 1301,
            (1, [.., ArmorCatalogRecord::VaultVendor(v)]) => v.accepted_items.reverse(),
            _ => {
                world.armor_catalogs.pop();
            }
        }
        let next = store.publish_manifest(items(), world.clone()).unwrap();
        assert!(next.revision > prior.revision);
        assert_eq!(fake.publish_manifest(items(), world.clone()).unwrap(), next);
        for kind in [
            ArmorCatalogKind::InventoryBucket,
            ArmorCatalogKind::VaultVendor,
        ] {
            assert_eq!(
                store.armor_catalog(kind, None),
                fake.armor_catalog(kind, None)
            );
        }
        assert_eq!(
            store.publish_manifest(items(), world.clone()).unwrap(),
            next
        );
        prior = next;
    }
    assert!(
        store
            .armor_catalog(ArmorCatalogKind::VaultVendor, None)
            .unwrap()
            .is_empty()
    );
    // Guarded downgrade: populated evidence refuses, empty evidence allows it.
    let mut populated = SqliteStore::open_in_memory().unwrap();
    populated.publish_manifest(items(), self::world()).unwrap();
    assert!(populated.migrate_to(33).is_err());
    assert_eq!(populated.schema_version().unwrap(), 34);
    populated
        .publish_manifest(items(), synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    populated.migrate_to(33).unwrap();
}
#[test]
fn migration_34_refuses_unrepresentable_publication_and_refreshes_once() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(33).unwrap();
    store
        .publish_manifest(items(), synthetic_world(DefinitionVersion::new("v1")))
        .unwrap();
    store.migrate_to(34).unwrap();
    assert!(store.current_metadata().unwrap().is_none());
    store.publish_manifest(items(), world()).unwrap();
    store.migrate_to(34).unwrap();
    assert!(store.current_metadata().unwrap().is_some());
    let mut old = SqliteStore::open_in_memory().unwrap();
    old.migrate_to(33).unwrap();
    assert!(old.publish_manifest(items(), world()).is_err());
}
