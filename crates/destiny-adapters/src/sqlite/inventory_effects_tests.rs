//! Contract tests for acknowledged local effects, independent of action dispatch.
use super::SqliteStore;
use destiny_core::*;

fn stack_input() -> InventorySnapshotInput {
    let mut seed = input();
    seed.profile_currencies_coverage = ProfileCurrenciesCoverage::Available;
    seed.stacks = vec![
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(20),
            25,
            StackContainer::Postmaster("character".into()),
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            8,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            6,
            StackContainer::ProfileCurrencies,
        )
        .unwrap(),
    ];
    seed
}
fn stack_effect() -> InventoryEffect {
    InventoryEffect::PullStack {
        item_hash: ItemHash::new(7),
        owner: "character".into(),
        quantity: 25,
        destination_bucket: BucketHash::new(10),
        max_stack_size: 10,
    }
}

#[test]
fn acknowledged_stack_effects_persist_reopen_and_preserve_metadata() {
    let uri = "file:acknowledged-stack-effects-reopen?mode=memory&cache=shared";
    let _keeper = rusqlite::Connection::open(uri).unwrap();
    let mut store = SqliteStore::initialize(
        rusqlite::Connection::open(uri).unwrap(),
        super::JournalPolicy::Memory,
    )
    .unwrap();
    let minted = ProfileMintedAt::new(1_800_000_000, 987654321).unwrap();
    let before = store
        .replace_current_minted(stack_input(), Some(minted))
        .unwrap();
    store.invalidate_freshness().unwrap();
    let mut batch = effects();
    batch.push(stack_effect());
    let after = store
        .apply_acknowledged_effects(&membership(), &batch)
        .unwrap();
    let mut expected = before.clone();
    expected.items = apply_inventory_effects(&before, &effects())
        .unwrap()
        .snapshot
        .items;
    expected.stacks = vec![
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            10,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            10,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            10,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            3,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(10),
            6,
            StackContainer::ProfileCurrencies,
        )
        .unwrap(),
    ];
    expected.stacks.sort();
    expected.metadata.revision = SyncRevision::new(before.metadata.revision.value() + 1);
    assert_eq!(after, expected);
    assert_eq!(store.load_current().unwrap(), Some(expected.clone()));
    drop(store);
    let reopened = SqliteStore::initialize(
        rusqlite::Connection::open(uri).unwrap(),
        super::JournalPolicy::Memory,
    )
    .unwrap();
    assert_eq!(reopened.load_current().unwrap(), Some(expected));
    assert_eq!(reopened.profile_minted_at().unwrap(), Some(minted));
    assert!(reopened.freshness_invalidated().unwrap());
}

#[test]
fn postmaster_query_reads_effected_stacks_without_refresh() {
    use destiny_app::test_support::QueuedProfileGateway;
    use destiny_app::{ProfileSyncService, QueryService};
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.replace_current(stack_input()).unwrap();
    store
        .apply_acknowledged_effects(&membership(), &[stack_effect()])
        .unwrap();
    let mut query = QueryService::new(ProfileSyncService::new(QueuedProfileGateway::new(), store));
    let result = query
        .postmaster(
            None,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(200),
        )
        .unwrap();
    assert_eq!(result.characters.len(), 1);
    assert!(result.characters[0].stacks.is_empty());
    assert_eq!(
        result.profile_currencies_coverage,
        ProfileCurrenciesCoverage::Available
    );
}

#[test]
fn acknowledged_stack_effects_roll_back_on_reducer_and_storage_error() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let before = store.replace_current(stack_input()).unwrap();
    let writes = store.connection().total_changes();
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &[stack_effect(), stack_effect()])
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderValidation
    );
    assert_eq!(store.load_current().unwrap(), Some(before.clone()));
    assert_eq!(store.connection().total_changes(), writes);
    store.connection().execute_batch("CREATE TEMP TRIGGER fail_stack_revision BEFORE UPDATE OF revision ON sync_metadata BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &[stack_effect()])
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.load_current().unwrap(), Some(before));
}

fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(1), MembershipId::new("123")).unwrap()
}

fn input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(1),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(200),
        source: SourceMarker::Profile,
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        stacks: vec![],
        plug_set_entries: vec![],
        characters: vec![Character {
            id: "character".into(),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: ["target", "previous"]
            .into_iter()
            .map(|id| ItemInstance {
                instance_id: id.into(),
                item_hash: ItemHash::new(7),
                location: if id == "target" {
                    ItemLocation::Vault
                } else {
                    ItemLocation::Character("character".into())
                },
                equipped: id == "previous",
                locked: false,
                quantity: 1,
                power: None,
                perks: vec![],
                plugs: vec![],
                stats: vec![],
                energy: None,
                gear_tier: None,
                sockets: Vec::new(),
                transfer_status: None,
                available_plugs: vec![],
            })
            .collect(),
    }
}

fn effects() -> Vec<InventoryEffect> {
    vec![
        InventoryEffect::Relocate {
            item: "target".into(),
            to: ItemLocation::Character("character".into()),
        },
        InventoryEffect::SetLocked {
            item: "target".into(),
            locked: true,
        },
        InventoryEffect::Equip {
            item: "target".into(),
            character: "character".into(),
            unequip: vec!["previous".into()],
        },
    ]
}

#[test]
fn acknowledged_effects_preserve_metadata_and_update_search() {
    for invalidated in [false, true] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let minted = ProfileMintedAt::new(1_800_000_000, 987654321).unwrap();
        let before = store.replace_current_minted(input(), Some(minted)).unwrap();
        if invalidated {
            store.invalidate_freshness().unwrap();
        }
        let after = store
            .apply_acknowledged_effects(&membership(), &effects())
            .unwrap();
        let mut expected = apply_inventory_effects(&before, &effects())
            .unwrap()
            .snapshot;
        expected.metadata.revision = SyncRevision::new(before.metadata.revision.value() + 1);
        assert_eq!(after, expected);
        assert_eq!(store.load_current().unwrap(), Some(after.clone()));
        assert_eq!(store.freshness_invalidated().unwrap(), invalidated);
        assert_eq!(store.profile_minted_at().unwrap(), Some(minted));
        assert_eq!(
            store.lookup_instance(&"target".into()).unwrap(),
            Resolution::Unique(
                after
                    .items
                    .iter()
                    .find(|i| i.instance_id.as_str() == "target")
                    .unwrap()
                    .clone()
            )
        );
        let query = InstanceSearchQuery {
            offset: 0,
            sort: None,
            location: None,
            equipped: Some(true),
            include_available: false,
            item_hashes: ItemHashFilter::Any,
            perk_hash: None,
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: vec![],
            character_id: Some("character".into()),
            definitions: None,
            min_power: None,
            limit: 10,
        };
        let hits = store.search_instances(&query).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].instance_id.as_str(), "target");
        assert!(hits[0].locked);
        let writes = store.connection().total_changes();
        for no_op in [
            vec![],
            vec![InventoryEffect::SetLocked {
                item: "target".into(),
                locked: true,
            }],
            vec![
                InventoryEffect::SetLocked {
                    item: "target".into(),
                    locked: false,
                },
                InventoryEffect::SetLocked {
                    item: "target".into(),
                    locked: true,
                },
            ],
        ] {
            assert_eq!(
                store
                    .apply_acknowledged_effects(&membership(), &no_op)
                    .unwrap(),
                after
            );
            assert_eq!(store.connection().total_changes(), writes);
        }
        // A subsequent profile publication must compare against the effected content.
        assert_eq!(
            store
                .replace_current(InventorySnapshotInput::with_metadata(&after))
                .unwrap()
                .metadata
                .revision,
            after.metadata.revision
        );
    }
}

#[test]
fn acknowledged_effects_fail_closed_without_writes() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &[])
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    let before = store.replace_current(input()).unwrap();
    store.invalidate_freshness().unwrap();
    let writes = store.connection().total_changes();
    let bad = vec![
        effects()[1].clone(),
        InventoryEffect::SetLocked {
            item: "missing".into(),
            locked: true,
        },
    ];
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &bad)
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderValidation
    );
    for wrong in [
        DestinyMembership::new(MembershipType::new(1), MembershipId::new("456")).unwrap(),
        DestinyMembership::new(MembershipType::new(2), MembershipId::new("123")).unwrap(),
    ] {
        assert_eq!(
            store
                .apply_acknowledged_effects(&wrong, &effects())
                .unwrap_err()
                .category(),
            ErrorCategory::LocalStorage
        );
    }
    let dangling = [InventoryEffect::Relocate {
        item: "target".into(),
        to: ItemLocation::Character("missing".into()),
    }];
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &dangling)
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderValidation
    );
    assert_eq!(store.load_current().unwrap(), Some(before));
    assert_eq!(store.connection().total_changes(), writes);
    assert!(store.freshness_invalidated().unwrap());
}

#[test]
fn acknowledged_effects_overflow_and_transaction_rollback() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.replace_current(input()).unwrap();
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = ?1 WHERE dataset = 'profile_inventory'",
            [u64::MAX.to_string()],
        )
        .unwrap();
    let before = store.load_current().unwrap().unwrap();
    let writes = store.connection().total_changes();
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &[])
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &effects())
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.connection().total_changes(), writes);
    assert_eq!(store.load_current().unwrap(), Some(before));

    store
        .connection()
        .execute("UPDATE sync_metadata SET revision = '1'", [])
        .unwrap();
    let before = store.load_current().unwrap();
    // Fail the metadata update after item writes to prove the whole transaction rolls back.
    store.connection().execute_batch("CREATE TEMP TRIGGER fail_revision BEFORE UPDATE OF revision ON sync_metadata BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    assert_eq!(
        store
            .apply_acknowledged_effects(&membership(), &effects())
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.load_current().unwrap(), before);
}

#[test]
fn acknowledged_effects_survive_reopen() {
    // A keeper holds the shared database alive while the repository is reopened.
    // This exercises connection reconstruction without writing test artifacts.
    let uri = "file:acknowledged-effects-reopen?mode=memory&cache=shared";
    let _keeper = rusqlite::Connection::open(uri).unwrap();
    let mut store = SqliteStore::initialize(
        rusqlite::Connection::open(uri).unwrap(),
        super::JournalPolicy::Memory,
    )
    .unwrap();
    store.replace_current(input()).unwrap();
    store.invalidate_freshness().unwrap();
    let expected = store
        .apply_acknowledged_effects(&membership(), &effects())
        .unwrap();
    drop(store);
    let reopened = SqliteStore::initialize(
        rusqlite::Connection::open(uri).unwrap(),
        super::JournalPolicy::Memory,
    )
    .unwrap();
    assert_eq!(reopened.load_current().unwrap(), Some(expected));
    assert!(reopened.freshness_invalidated().unwrap());
}
