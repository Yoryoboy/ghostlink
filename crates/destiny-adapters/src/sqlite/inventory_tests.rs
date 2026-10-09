//! Focused contract tests for the transactional SQLite inventory repository.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars.

use super::SqliteStore;
use destiny_core::{
    AppError, Character, DefinitionReplacementInput, DefinitionRepository, DefinitionVersion,
    ErrorCategory, InventoryRepository, InventorySnapshotInput, ItemDefinition, ItemHash,
    ItemInstance, ItemLocation, ManifestDefinitions, MembershipId, MembershipType, PerkHash,
    PerkSummary, Resolution, SourceMarker, Timestamp,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn publication_diff_preserves_untouched_rows() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let rows = Some(vec![]);
    let mut input = full_input();
    input.stacks.push(
        destiny_core::StackObservation::new(
            ItemHash::new(7),
            destiny_core::BucketHash::new(9),
            3,
            destiny_core::StackContainer::ProfileInventory,
        )
        .unwrap(),
    );
    input.plug_set_entries.push(destiny_core::PlugSetEntry {
        scope: destiny_core::PlugSetScope::Profile,
        plug_hash: ItemHash::new(77),
        can_insert: true,
        enabled: false,
    });
    store
        .replace_current_profile(input.clone(), None, rows.clone())
        .unwrap();
    // Fail on any attempted rewrite, including DELETE/INSERT of the singleton
    // whose rowid would otherwise stay the same.
    store.connection().execute_batch(
        "CREATE TEMP TRIGGER keep_saved BEFORE DELETE ON saved_loadouts BEGIN SELECT RAISE(ABORT, 'saved rewrite'); END;
         CREATE TEMP TRIGGER keep_character BEFORE UPDATE ON characters BEGIN SELECT RAISE(ABORT, 'character rewrite'); END;
         CREATE TEMP TRIGGER keep_item BEFORE DELETE ON items WHEN OLD.instance_id = 'instance-2' BEGIN SELECT RAISE(ABORT, 'item rewrite'); END;
         CREATE TEMP TRIGGER keep_stack_delete BEFORE DELETE ON stacks BEGIN SELECT RAISE(ABORT, 'stack rewrite'); END;
         CREATE TEMP TRIGGER keep_stack_update BEFORE UPDATE ON stacks BEGIN SELECT RAISE(ABORT, 'stack rewrite'); END;
         CREATE TEMP TRIGGER keep_option_delete BEFORE DELETE ON subclass_plug_options BEGIN SELECT RAISE(ABORT, 'option rewrite'); END;
         CREATE TEMP TRIGGER keep_option_update BEFORE UPDATE ON subclass_plug_options BEGIN SELECT RAISE(ABORT, 'option rewrite'); END;",
    ).unwrap();
    let before = store.connection().total_changes();
    store
        .replace_current_profile(input.clone(), None, rows.clone())
        .expect("unchanged loadouts must not be rewritten");
    assert_eq!(store.connection().total_changes() - before, 2);
    let mut changed = input;
    changed.items[0].locked = false;
    store
        .replace_current_profile(changed, None, rows)
        .expect("unchanged characters and items must not be rewritten");
}

#[test]
fn corrupt_saved_loadouts_roll_back_publication_diff() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let before = store.replace_current(full_input()).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO saved_loadouts(singleton, normalized_loadouts) VALUES(1, 'corrupt')",
            [],
        )
        .unwrap();
    store.invalidate_freshness().unwrap();
    let mut changed = full_input();
    changed.items[0].locked = false;
    assert_eq!(
        store.replace_current(changed).unwrap_err(),
        AppError::new(ErrorCategory::LocalStorage)
    );
    assert_eq!(store.load_current().unwrap(), Some(before));
    assert!(store.freshness_invalidated().unwrap());
}

#[test]
fn publication_sequence_matches_fresh_publication() {
    use destiny_core::*;
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    let mut loadouts = None;
    let mut previous: Option<InventorySnapshot> = None;
    let mut revision = 0;
    for step in 0..20 {
        match step {
            1 => {
                let mut item = input.items[0].clone();
                item.instance_id = "added-item".into();
                input.items.push(item);
            }
            2 => input.items[0].location = ItemLocation::Character("character-1".into()),
            3 => input.items[0].location = ItemLocation::Postmaster("character-1".into()),
            4 => input.items[0].location = ItemLocation::Vault,
            5 => input.items[0].locked = false,
            6 => {
                input.items[0].plugs = vec![PlugSummary {
                    plug_hash: ItemHash::new(77),
                    enabled: true,
                }];
                input.items[0].available_plugs = vec![AvailablePlug {
                    socket_index: 1,
                    plug_hash: ItemHash::new(78),
                }];
                input.items[0].stats = vec![StatSummary {
                    stat_hash: 12,
                    value: 34,
                }];
                input.items[0].perks.reverse();
            }
            7 => {
                let stack = StackObservation::new(
                    ItemHash::new(7),
                    BucketHash::new(9),
                    3,
                    StackContainer::CharacterInventory("character-1".into()),
                )
                .unwrap();
                input.stacks = vec![stack.clone(), stack];
            }
            8 => {
                input.stacks.pop();
            }
            9 => {
                let mut character = input.characters[0].clone();
                character.id = "123".into();
                input.characters.push(character);
                input.items[0].location = ItemLocation::Character("123".into());
                input.stacks.push(
                    StackObservation::new(
                        ItemHash::new(8),
                        BucketHash::new(10),
                        4,
                        StackContainer::Postmaster("123".into()),
                    )
                    .unwrap(),
                );
                input.plug_set_entries.push(PlugSetEntry {
                    scope: PlugSetScope::Character("123".into()),
                    plug_hash: ItemHash::new(77),
                    can_insert: true,
                    enabled: false,
                });
            }
            10 => input.characters[0].name = Some("Changed".into()),
            11 => {
                input.items[0].location = ItemLocation::Vault;
                input.stacks.retain(|s| s.item_hash() != ItemHash::new(8));
                input.characters.retain(|c| c.id.as_str() != "123");
                input.plug_set_entries.clear();
            }
            12 => input
                .items
                .retain(|i| i.instance_id.as_str() != "added-item"),
            13 => {
                loadouts = Some(vec![CharacterLoadouts {
                    character_id: "123".into(),
                    loadouts: vec![Loadout {
                        slot_index: 0,
                        name_hash: 1,
                        color_hash: 2,
                        icon_hash: 3,
                        items: vec![LoadoutItem {
                            instance_id: "456".into(),
                            plugs: vec![LoadoutPlug {
                                socket_index: 0,
                                plug_hash: ItemHash::new(77),
                            }],
                        }],
                    }],
                }])
            }
            14 => {} // Unchanged saved loadouts and inventory.
            15 => loadouts = Some(vec![]),
            16 => loadouts = None,
            17 => input.stacks.clear(),
            18 => input.profile_currencies_coverage = ProfileCurrenciesCoverage::Available,
            19 => input.membership_id = "another-membership".into(),
            _ => {}
        }
        input.checked_at = Timestamp::new(200 + step);
        let changed = previous
            .as_ref()
            .is_none_or(|old| !InventorySnapshotInput::with_metadata(old).content_eq(&input));
        revision += u64::from(changed);
        let minted = (step % 2 == 0).then(|| ProfileMintedAt::new(100, 2).unwrap());
        store.invalidate_freshness().unwrap();
        let actual = store
            .replace_current_profile(input.clone(), minted, loadouts.clone())
            .unwrap();
        let mut fresh = SqliteStore::open_in_memory().unwrap();
        let mut expected = fresh
            .replace_current_profile(input.clone(), minted, loadouts.clone())
            .unwrap();
        expected.metadata.revision = SyncRevision::new(revision);
        assert_eq!(actual, expected, "step {step}");
        assert_eq!(store.load_current().unwrap(), Some(expected));
        assert_eq!(
            store.saved_loadouts().unwrap(),
            fresh.saved_loadouts().unwrap()
        );
        assert_eq!(store.profile_minted_at().unwrap(), minted);
        assert!(!store.freshness_invalidated().unwrap());
        previous = Some(actual);
    }
}

#[test]
fn saved_loadouts_publication_and_reopen() {
    let database = TempDatabase::new();
    let mut store = SqliteStore::open(database.path()).unwrap();
    let legacy = store.replace_current(full_input()).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 20).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 21).unwrap();
    assert_eq!(store.load_current().unwrap(), Some(legacy));
    assert_eq!(store.saved_loadouts().unwrap(), None);
    let rows = vec![destiny_core::CharacterLoadouts {
        character_id: destiny_core::CharacterId::new("123"),
        loadouts: vec![destiny_core::Loadout {
            slot_index: 3,
            name_hash: 10,
            color_hash: 20,
            icon_hash: 30,
            items: vec![destiny_core::LoadoutItem {
                instance_id: destiny_core::ItemInstanceId::new("456"),
                plugs: vec![destiny_core::LoadoutPlug {
                    socket_index: 2,
                    plug_hash: ItemHash::new(42),
                }],
            }],
        }],
    }];
    store
        .replace_current_profile(full_input(), None, Some(rows.clone()))
        .unwrap();
    assert_eq!(store.saved_loadouts().unwrap(), Some(rows.clone()));
    store.invalidate_freshness().unwrap();
    drop(store);
    let mut reopened = SqliteStore::open(database.path()).unwrap();
    assert_eq!(reopened.saved_loadouts().unwrap(), Some(rows.clone()));
    let mut invalid = full_input();
    invalid.source = SourceMarker::Manifest;
    assert!(
        reopened
            .replace_current_profile(invalid, None, Some(vec![]))
            .is_err()
    );
    assert_eq!(reopened.saved_loadouts().unwrap(), Some(rows.clone()));
    assert!(super::migrations::migrate_to(reopened.connection_mut(), 20).is_err());
    let before = reopened.load_current().unwrap();
    reopened.connection().execute_batch("CREATE TRIGGER reject_saved BEFORE INSERT ON saved_loadouts BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    let mut changed = full_input();
    changed.checked_at = Timestamp::new(9999);
    assert!(
        reopened
            .replace_current_profile(changed, None, Some(vec![]))
            .is_err()
    );
    assert_eq!(reopened.load_current().unwrap(), before);
    assert_eq!(reopened.saved_loadouts().unwrap(), Some(rows));
    reopened
        .connection()
        .execute_batch("DROP TRIGGER reject_saved;")
        .unwrap();
    reopened
        .replace_current_profile(full_input(), None, Some(vec![]))
        .unwrap();
    assert_eq!(reopened.saved_loadouts().unwrap(), Some(vec![]));
}

#[test]
fn minted_publication_migration_and_reopen() {
    let database = TempDatabase::new();
    let mut store = SqliteStore::open(database.path()).unwrap();
    let legacy = store.replace_current(full_input()).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 19).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 21).unwrap();
    assert_eq!(store.profile_minted_at().unwrap(), None);
    assert_eq!(store.load_current().unwrap(), Some(legacy.clone()));
    let minted = destiny_core::ProfileMintedAt::new(1_800_000_000, 123456789).unwrap();
    let published = store
        .replace_current_minted(full_input(), Some(minted))
        .unwrap();
    assert_eq!(published.metadata.revision, legacy.metadata.revision);
    assert_eq!(store.profile_minted_at().unwrap(), Some(minted));
    store.invalidate_freshness().unwrap();
    drop(store);
    let mut reopened = SqliteStore::open(database.path()).unwrap();
    assert_eq!(reopened.load_current().unwrap(), Some(published));
    assert_eq!(reopened.profile_minted_at().unwrap(), Some(minted));
    assert!(reopened.freshness_invalidated().unwrap());
    let mut invalid = full_input();
    invalid.source = SourceMarker::Manifest;
    assert!(reopened.replace_current_minted(invalid, None).is_err());
    assert_eq!(reopened.profile_minted_at().unwrap(), Some(minted));
    assert!(super::migrations::migrate_to(reopened.connection_mut(), 19).is_err());
    super::migrations::migrate_to(reopened.connection_mut(), 21).unwrap();
    reopened.replace_current(full_input()).unwrap();
    assert_eq!(reopened.profile_minted_at().unwrap(), None);
}

#[test]
fn minted_write_failure_rolls_back_publication_and_corrupt_instants_fail_closed() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let minted = destiny_core::ProfileMintedAt::new(-1, 999_999_999).unwrap();
    let before = store
        .replace_current_minted(full_input(), Some(minted))
        .unwrap();
    store.invalidate_freshness().unwrap();
    store.connection().execute_batch("CREATE TRIGGER reject_minted BEFORE UPDATE OF profile_minted_at ON sync_metadata BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    let mut changed = full_input();
    changed.items.clear();
    assert_eq!(
        store
            .replace_current_minted(changed, None)
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.load_current().unwrap(), Some(before));
    assert_eq!(store.profile_minted_at().unwrap(), Some(minted));
    assert!(store.freshness_invalidated().unwrap());
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_minted;")
        .unwrap();
    for raw in [
        "not-an-instant",
        "1:1000000000",
        "01:0",
        "1:00",
        "9223372036854775808:0",
        "1:-1",
    ] {
        store.connection().execute("UPDATE sync_metadata SET profile_minted_at = ?1 WHERE dataset = 'profile_inventory'", [raw]).unwrap();
        assert_eq!(
            store.profile_minted_at().unwrap_err().category(),
            ErrorCategory::LocalStorage
        );
    }
}

#[test]
fn invalidation_keeps_acquisition_metadata() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let before = store.replace_current(full_input()).unwrap();
    store.invalidate_freshness().unwrap();
    assert_eq!(store.load_current().unwrap().unwrap(), before);
    assert!(store.freshness_invalidated().unwrap());
    store.replace_current(full_input()).unwrap();
    assert!(!store.freshness_invalidated().unwrap());
}

#[test]
fn invalidation_survives_reopen_and_v19_preserves_existing_data() {
    let database = TempDatabase::new();
    let mut store = SqliteStore::open(database.path()).unwrap();
    let before = store.replace_current(full_input()).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 18).unwrap();
    super::migrations::migrate_to(store.connection_mut(), 19).unwrap();
    assert_eq!(store.load_current().unwrap().unwrap(), before);
    assert!(!store.freshness_invalidated().unwrap());
    store.invalidate_freshness().unwrap();
    assert!(super::migrations::migrate_to(store.connection_mut(), 18).is_err());
    drop(store);
    let mut reopened = SqliteStore::open(database.path()).unwrap();
    assert!(reopened.freshness_invalidated().unwrap());
    assert_eq!(reopened.load_current().unwrap().unwrap(), before);
    reopened.replace_current(full_input()).unwrap();
    assert!(!reopened.freshness_invalidated().unwrap());
}

#[test]
fn stack_snapshot_sqlite_roundtrips_duplicates_and_revision() {
    use destiny_core::{BucketHash, StackContainer, StackObservation};
    let mut store = SqliteStore::open_in_memory().unwrap();
    let original = store.replace_current(full_input()).unwrap();
    assert!(original.stacks.is_empty());
    let before = store.connection().total_changes();
    let schema = store.schema_version().unwrap();
    let mut replacement = InventorySnapshotInput::with_metadata(&original);
    // Stacks coexist with the existing instance projection.
    replacement.checked_at = Timestamp::new(original.metadata.checked_at.seconds() + 10);
    replacement.definition_version = DefinitionVersion::new("replacement");
    replacement.stacks.push(
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(8),
            2,
            StackContainer::ProfileInventory,
        )
        .unwrap(),
    );
    replacement.stacks.push(replacement.stacks[0].clone());
    let committed = store.replace_current(replacement.clone()).unwrap();
    assert_eq!(committed.stacks.len(), 2);
    assert_eq!(store.load_current().unwrap(), Some(committed.clone()));
    assert!(store.connection().total_changes() > before);
    assert_eq!(store.schema_version().unwrap(), schema);
    replacement.stacks.reverse();
    let same = store.replace_current(replacement.clone()).unwrap();
    assert_eq!(same.metadata.revision, committed.metadata.revision);
    replacement.stacks.pop();
    let changed = store.replace_current(replacement).unwrap();
    assert_eq!(
        changed.metadata.revision.value(),
        committed.metadata.revision.value() + 1
    );

    let mut empty_store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.profile_currencies_coverage = destiny_core::ProfileCurrenciesCoverage::Available;
    input.stacks.push(
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(8),
            2,
            StackContainer::ProfileCurrencies,
        )
        .unwrap(),
    );
    let first = empty_store.replace_current(input).unwrap();
    assert_eq!(first.stacks.len(), 1);
    assert_eq!(empty_store.load_current().unwrap(), Some(first));
}

#[test]
fn postmaster_and_stacks_roundtrip_filter_and_atomic_safety() {
    use destiny_core::{
        BucketHash, InstanceSearchQuery, LocationFilter, SearchRepository, StackContainer,
        StackObservation,
    };
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.items[2].location = ItemLocation::Postmaster(input.characters[0].id.clone());
    input.profile_currencies_coverage = destiny_core::ProfileCurrenciesCoverage::Available;
    for container in [
        StackContainer::ProfileInventory,
        StackContainer::ProfileCurrencies,
        StackContainer::CharacterInventory(input.characters[0].id.clone()),
        StackContainer::Postmaster(input.characters[0].id.clone()),
    ] {
        input.stacks.push(
            StackObservation::new(ItemHash::new(7), BucketHash::new(8), 2, container).unwrap(),
        );
    }
    let original = store.replace_current(input.clone()).unwrap();
    assert_eq!(store.load_current().unwrap(), Some(original.clone()));
    let mut permuted = input.clone();
    permuted.stacks.reverse();
    permuted.items.reverse();
    permuted.characters.reverse();
    assert_eq!(store.replace_current(permuted).unwrap(), original);
    let query = InstanceSearchQuery {
        offset: 0,
        sort: None,
        location: None,
        equipped: None,
        include_available: false,
        item_hashes: destiny_core::ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        stats: vec![],
        character_id: None,
        definitions: None,
        min_power: None,
        limit: 10,
    };
    let hits = store.search_instances(&query).unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[2].location, input.items[2].location);
    let mut carried = query.clone();
    carried.character_id = Some(input.characters[0].id.clone());
    assert_eq!(store.search_instances(&carried).unwrap().len(), 1);
    carried.character_id = None;
    carried.location = Some(LocationFilter::Character);
    assert_eq!(store.search_instances(&carried).unwrap().len(), 1);

    let mut invalid = input.clone();
    invalid.items[2].equipped = true;
    assert_eq!(
        store.replace_current(invalid).unwrap_err().category(),
        ErrorCategory::ProviderValidation
    );
    let mut invalid = input.clone();
    invalid.stacks[0] = StackObservation::new(
        ItemHash::new(7),
        BucketHash::new(8),
        2,
        StackContainer::Postmaster("missing".into()),
    )
    .unwrap();
    assert_eq!(
        store.replace_current(invalid).unwrap_err().category(),
        ErrorCategory::ProviderValidation
    );
    let mut invalid = input.clone();
    invalid.characters.clear();
    assert_eq!(
        store.replace_current(invalid).unwrap_err().category(),
        ErrorCategory::ProviderValidation
    );
    assert_eq!(store.load_current().unwrap(), Some(original.clone()));

    // Force a storage failure after deleting/reinserting projection rows.
    store.connection().execute_batch(
        "CREATE TEMP TRIGGER refuse_stack BEFORE INSERT ON stacks BEGIN SELECT RAISE(ABORT, 'test'); END;"
    ).unwrap();
    input.stacks[0] = StackObservation::new(
        ItemHash::new(7),
        BucketHash::new(8),
        3,
        StackContainer::ProfileInventory,
    )
    .unwrap();
    assert_eq!(
        store.replace_current(input.clone()).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.load_current().unwrap(), Some(original.clone()));
    store
        .connection()
        .execute_batch("DROP TRIGGER refuse_stack")
        .unwrap();
    let changed = store.replace_current(input).unwrap();
    assert_eq!(
        changed.metadata.revision.value(),
        original.metadata.revision.value() + 1
    );
    // Empty replacement deletes owner-bound observations before their characters.
    let mut empty = InventorySnapshotInput::with_metadata(&changed);
    empty.stacks.clear();
    empty.items.clear();
    empty.characters.clear();
    store.replace_current(empty).unwrap();
}

#[test]
fn stack_readback_fails_closed_for_invalid_fields_owners_and_orphans() {
    use destiny_core::{BucketHash, StackContainer, StackObservation};
    for corruption in [
        "UPDATE stacks SET quantity=0",
        "UPDATE stacks SET quantity=2147483648",
        "UPDATE stacks SET item_hash=-1",
        "UPDATE stacks SET bucket_hash=4294967296",
        "UPDATE stacks SET container='postmaster',owner='missing'",
        "UPDATE stacks SET container='postmaster',owner=NULL",
        "UPDATE stacks SET ordinal=2",
        "DELETE FROM sync_metadata; DELETE FROM items; DELETE FROM characters; DELETE FROM active_profile",
    ] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut input = full_input();
        input.stacks.push(
            StackObservation::new(
                ItemHash::new(7),
                BucketHash::new(8),
                2,
                StackContainer::ProfileInventory,
            )
            .unwrap(),
        );
        store.replace_current(input).unwrap();
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints=ON; PRAGMA foreign_keys=OFF;")
            .unwrap();
        store.connection().execute_batch(corruption).unwrap();
        assert_eq!(
            store.load_current().unwrap_err().category(),
            ErrorCategory::LocalStorage,
            "{corruption}"
        );
    }
}

#[test]
fn configured_label_to_manifest_version_preserves_snapshots_and_revision_identity() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let schema = store.schema_version().unwrap();
    let mut input = full_input();
    input.definition_version = DefinitionVersion::new("old-configured-label");
    let old = store.replace_current(input.clone()).unwrap();
    store.replace_definitions(manifest_replacement()).unwrap();
    assert_eq!(store.load_current().unwrap(), Some(old.clone()));
    input.definition_version = store.current_definition_version().unwrap();
    input.checked_at = Timestamp::new(input.checked_at.seconds() + 1);
    let changed = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        changed.metadata.revision.value(),
        old.metadata.revision.value() + 1
    );
    for _ in 0..3 {
        let same = store.replace_current(input.clone()).unwrap();
        assert_eq!(same.metadata.revision, changed.metadata.revision);
    }
    assert_eq!(store.schema_version().unwrap(), schema);
}

#[test]
fn plugs_round_trip_search_and_revision_identity() {
    use destiny_core::{PlugSummary, SearchRepository};
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.items[0].plugs = [0, 6001, 6001, i64::from(u32::MAX)]
        .into_iter()
        .map(|hash| PlugSummary {
            plug_hash: ItemHash::new(hash),
            enabled: true,
        })
        .collect();
    let first = store.replace_current(input.clone()).unwrap();
    assert_eq!(store.load_current().unwrap().unwrap(), first);
    assert_eq!(
        store.lookup_instance(&input.items[0].instance_id).unwrap(),
        Resolution::Unique(first.items[0].clone())
    );
    let same = store.replace_current(input.clone()).unwrap();
    assert_eq!(same.metadata.revision, first.metadata.revision);
    input.items[0].plugs[1].plug_hash = ItemHash::new(6002);
    let changed = store.replace_current(input).unwrap();
    assert_eq!(
        changed.metadata.revision.value(),
        first.metadata.revision.value() + 1
    );
}

#[test]
fn sync_filters_plugs_before_sqlite_publication_and_keeps_unknown_without_manifest() {
    use destiny_app::{ProfileSyncService, test_support::QueuedProfileGateway};
    use destiny_core::{DestinyMembership, FreshnessPolicy, PlugSummary, ProfileSyncReason};
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap();
    let mut gateway = QueuedProfileGateway::new();
    for shader in [61, 64, 61] {
        let mut input = full_input();
        input.membership_id = membership.membership_id().clone();
        input.membership_type = membership.membership_type();
        input.items[0].plugs = [60, shader, 62]
            .into_iter()
            .map(|hash| PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        gateway.queue_membership(Ok(membership.clone()));
        gateway.queue_inventory(Ok(input));
    }
    let store = SqliteStore::open_in_memory().unwrap();
    let mut service = ProfileSyncService::new_with_definitions(gateway, store);
    let policy = FreshnessPolicy { ttl_seconds: 300 };
    service
        .sync(ProfileSyncReason::Forced, policy, Timestamp::new(200))
        .unwrap();
    assert_eq!(
        service.repository().load_current().unwrap().unwrap().items[0]
            .plugs
            .len(),
        3
    );
    let definitions = [(60, "frames"), (61, "shader"), (64, "shader")]
        .into_iter()
        .map(|(hash, category)| ItemDefinition {
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: "plug".into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: Some(category.into()),
            tags: vec![],
            version: DefinitionVersion::new("v1"),
        })
        .collect();
    service
        .repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions,
            },
            checked_at: Timestamp::new(200),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    service
        .sync(ProfileSyncReason::Forced, policy, Timestamp::new(200))
        .unwrap();
    let snapshot = service.repository().load_current().unwrap().unwrap();
    assert_eq!(
        snapshot.items[0]
            .plugs
            .iter()
            .map(|plug| plug.plug_hash.value())
            .collect::<Vec<_>>(),
        [60, 62]
    );
    let cosmetic_only = service
        .sync(ProfileSyncReason::Forced, policy, Timestamp::new(200))
        .unwrap();
    assert_eq!(cosmetic_only.revision(), snapshot.metadata.revision);
}

#[test]
fn malformed_plug_ordinals_fail_closed_on_inventory_and_search_reads() {
    use destiny_core::{PlugSummary, SearchRepository};
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.items[0].plugs = vec![PlugSummary {
        plug_hash: ItemHash::new(60),
        enabled: true,
    }];
    store.replace_current(input.clone()).unwrap();
    store
        .connection()
        .execute("UPDATE item_plugs SET ordinal=1", [])
        .unwrap();
    assert_eq!(
        store.load_current().unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(
        store
            .lookup_instance(&input.items[0].instance_id)
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
}

#[test]
fn plug_storage_rejects_invalid_hash_disabled_and_overflow() {
    use destiny_core::PlugSummary;
    for plugs in [
        vec![PlugSummary {
            plug_hash: ItemHash::new(-1),
            enabled: true,
        }],
        vec![PlugSummary {
            plug_hash: ItemHash::new(1),
            enabled: false,
        }],
        vec![
            PlugSummary {
                plug_hash: ItemHash::new(1),
                enabled: true
            };
            33
        ],
    ] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let seed = store.replace_current(full_input()).unwrap();
        let mut input = full_input();
        input.items[0].plugs = plugs;
        assert!(store.replace_current(input).is_err());
        assert_eq!(store.load_current().unwrap().unwrap(), seed);
    }
}

fn option(scope: destiny_core::PlugSetScope, hash: i64) -> destiny_core::PlugSetEntry {
    destiny_core::PlugSetEntry {
        scope,
        plug_hash: ItemHash::new(hash),
        can_insert: true,
        enabled: false,
    }
}

#[test]
fn subclass_options_roundtrip_revision_and_atomic_replace() {
    use destiny_core::PlugSetScope;
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.characters[0].id = "1".into();
    input.items.iter_mut().for_each(|item| {
        if matches!(item.location, ItemLocation::Character(ref id) if id.as_str() == "character-1")
        {
            item.location = ItemLocation::Character("1".into());
        }
    });
    input.plug_set_entries = vec![
        option(PlugSetScope::Character("1".into()), 42),
        option(PlugSetScope::Profile, 42),
    ];
    let first = store.replace_current(input.clone()).unwrap();
    assert_eq!(store.load_current().unwrap().unwrap(), first);
    input.plug_set_entries.reverse();
    assert_eq!(
        store
            .replace_current(input.clone())
            .unwrap()
            .metadata
            .revision,
        first.metadata.revision
    );
    input.plug_set_entries[0].enabled = true;
    let changed = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        changed.metadata.revision.value(),
        first.metadata.revision.value() + 1
    );
    store.connection().execute_batch("CREATE TRIGGER reject_options BEFORE DELETE ON subclass_plug_options BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    input.plug_set_entries.pop();
    input.items.clear();
    input.checked_at = Timestamp::new(300);
    assert_eq!(
        store.replace_current(input).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.load_current().unwrap().unwrap(), changed);
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_options;")
        .unwrap();
    let empty = store.replace_current(full_input()).unwrap();
    assert!(empty.plug_set_entries.is_empty());
    assert_eq!(store.load_current().unwrap().unwrap(), empty);
}

#[test]
fn sync_publishes_only_named_known_subclass_options() {
    use destiny_app::{ProfileSyncService, test_support::QueuedProfileGateway};
    use destiny_core::{DestinyMembership, FreshnessPolicy, PlugSetScope, ProfileSyncReason};
    let mut input = full_input();
    input.membership_id = MembershipId::new("123");
    input.membership_type = MembershipType::new(3);
    input.plug_set_entries = (60..65)
        .map(|hash| option(PlugSetScope::Profile, hash))
        .collect();
    let membership =
        DestinyMembership::new(input.membership_type, input.membership_id.clone()).unwrap();
    let mut gateway = QueuedProfileGateway::new();
    gateway.queue_membership(Ok(membership));
    gateway.queue_inventory(Ok(input));
    let mut store = SqliteStore::open_in_memory().unwrap();
    let definitions = [
        (60, "hunter.arc.supers", "Arc Staff"),
        (61, "shader", "Cosmetic"),
        (62, "hunter.arc.supers", "Empty Super Socket"),
        (63, "frames", "Other"),
    ]
    .into_iter()
    .map(|(hash, category, name)| ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.into(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: Some(category.into()),
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    })
    .collect();
    store
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions,
            },
            checked_at: Timestamp::new(200),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    let mut service = ProfileSyncService::new_with_definitions(gateway, store);
    service
        .sync(
            ProfileSyncReason::Forced,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(200),
        )
        .unwrap();
    assert_eq!(
        service
            .repository()
            .load_current()
            .unwrap()
            .unwrap()
            .plug_set_entries,
        vec![option(PlugSetScope::Profile, 60)]
    );
}

#[test]
fn subclass_options_validate_storage_and_match_in_memory_twin() {
    use destiny_app::test_support::InMemoryInventoryRepository;
    use destiny_core::PlugSetScope;
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut twin = InMemoryInventoryRepository::new();
    let mut input = full_input();
    input.plug_set_entries = vec![
        option(PlugSetScope::Profile, 42),
        option(PlugSetScope::Profile, 0),
    ];
    let seed = store.replace_current(input.clone()).unwrap();
    assert_eq!(twin.replace_current(input.clone()).unwrap(), seed);
    input.plug_set_entries.reverse();
    input.plug_set_entries[0].can_insert = false;
    input.plug_set_entries[0].enabled = true;
    assert_eq!(
        store.replace_current(input.clone()).unwrap(),
        twin.replace_current(input.clone()).unwrap()
    );
    let prior = store.load_current().unwrap().unwrap();
    for entries in [
        vec![option(PlugSetScope::Profile, -1)],
        vec![option(PlugSetScope::Profile, i64::from(u32::MAX) + 1)],
        vec![option(PlugSetScope::Character("01".into()), 42)],
        vec![option(PlugSetScope::Character("123".into()), 42)],
        vec![option(PlugSetScope::Profile, 42); 2],
    ] {
        input.plug_set_entries = entries;
        assert_eq!(
            store.replace_current(input.clone()).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert!(twin.replace_current(input.clone()).is_err());
        assert_eq!(store.load_current().unwrap().unwrap(), prior);
    }
    for sql in [
        "INSERT INTO subclass_plug_options VALUES ('01', 1, 1, 1)",
        "INSERT INTO subclass_plug_options VALUES ('profile', -1, 1, 1)",
        "INSERT INTO subclass_plug_options VALUES ('profile', 1, 2, 1)",
        "INSERT INTO subclass_plug_options VALUES ('profile', 1, 1, -1)",
    ] {
        assert!(store.connection().execute(sql, []).is_err());
    }
    store
        .connection()
        .execute(
            "INSERT INTO subclass_plug_options VALUES ('123', 1, 1, 1)",
            [],
        )
        .unwrap();
    assert_eq!(
        store.load_current().unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    let orphaned = SqliteStore::open_in_memory().unwrap();
    orphaned
        .connection()
        .execute(
            "INSERT INTO subclass_plug_options VALUES ('profile', 1, 1, 1)",
            [],
        )
        .unwrap();
    assert_eq!(
        orphaned.load_current().unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
}

#[test]
fn migration_v13_preserves_v11_inventory_and_defaults_last_played() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(11).unwrap();
    // Seed using SQL: the v11 schema predates subclass options.
    store
        .connection()
        .execute("INSERT INTO active_profile VALUES (1, '123', '3')", [])
        .unwrap();
    store.connection().execute("INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) VALUES ('profile_inventory', '200', 'profile', '1', 'v1')", []).unwrap();
    store.connection().execute("INSERT INTO characters (character_id, name, class_name, race, gender, light) VALUES ('1', 'Nova', 'Warlock', NULL, NULL, 1800)", []).unwrap();
    store
        .migrate_to(12)
        .expect("upgrade to subclass option schema");
    store.migrate_to(13).expect("upgrade to last-played schema");
    let last_played: Option<String> = store
        .connection()
        .query_row("SELECT date_last_played FROM characters", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(last_played, None);
    // Repository readback uses the current schema, not a historical schema.
    store.migrate_to(super::LATEST_SCHEMA_VERSION).unwrap();
    let snapshot = store.load_current().unwrap().unwrap();
    assert_eq!(snapshot.characters[0].name.as_deref(), Some("Nova"));
    assert!(snapshot.characters[0].date_last_played.is_none());
    assert_eq!(snapshot.metadata.revision.value(), 1);
}

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-inventory-{}-{sequence}.db",
            std::process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut sidecar = self.path.clone().into_os_string();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

fn storage_error() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// A complete projection covering every optional field, all three typed
/// locations, the exact numeric bounds of the schema columns, and an ordered
/// perk list that includes a duplicated hash, both state flags, both `u32` hash
/// bounds, and an empty list.
fn full_input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
        stacks: Vec::new(),
        plug_set_entries: Vec::new(),
        membership_id: MembershipId::new("membership-1"),
        membership_type: MembershipType::new(u32::MAX),
        definition_version: DefinitionVersion::new("definitions-v1"),
        checked_at: Timestamp::new(200),
        source: SourceMarker::Profile,
        characters: vec![
            Character {
                id: "character-1".into(),
                name: Some("Nova".into()),
                class: Some("Warlock".into()),
                race: Some("Awoken".into()),
                gender: Some("Female".into()),
                light: Some(u16::MAX),
                date_last_played: Some(Timestamp::new(u64::MAX)),
            },
            Character {
                id: "character-2".into(),
                name: None,
                class: None,
                race: None,
                gender: None,
                light: Some(0),
                date_last_played: None,
            },
        ],
        items: vec![
            ItemInstance {
                available_plugs: Vec::new(),
                instance_id: "instance-1".into(),
                item_hash: ItemHash::new(-42),
                location: ItemLocation::Vault,
                quantity: u32::MAX,
                equipped: false,
                locked: true,
                power: Some(u16::MAX),
                stats: Vec::new(),
                plugs: Vec::new(),
                perks: vec![
                    PerkSummary {
                        perk_hash: PerkHash::new(0),
                        active: true,
                        visible: true,
                    },
                    PerkSummary {
                        perk_hash: PerkHash::new(u32::MAX),
                        active: false,
                        visible: false,
                    },
                    // A duplicate hash is legal because a provider may summarize
                    // the same perk from more than one source.
                    PerkSummary {
                        perk_hash: PerkHash::new(0),
                        active: false,
                        visible: true,
                    },
                ],
            },
            ItemInstance {
                available_plugs: Vec::new(),
                instance_id: "instance-2".into(),
                item_hash: ItemHash::new(7),
                location: ItemLocation::Character("character-1".into()),
                quantity: 0,
                equipped: true,
                locked: false,
                power: None,
                stats: Vec::new(),
                plugs: Vec::new(),
                perks: vec![PerkSummary {
                    perk_hash: PerkHash::new(42),
                    active: false,
                    visible: true,
                }],
            },
            ItemInstance {
                available_plugs: Vec::new(),
                instance_id: "instance-3".into(),
                item_hash: ItemHash::new(8),
                location: ItemLocation::Other("postmaster".into()),
                quantity: 1,
                equipped: false,
                locked: false,
                power: Some(0),
                stats: Vec::new(),
                plugs: Vec::new(),
                perks: Vec::new(),
            },
        ],
    }
}

/// The canonical characters `full_input` round-trips to, ordered by identifier.
fn expected_characters() -> Vec<Character> {
    vec![
        Character {
            id: "character-1".into(),
            name: Some("Nova".into()),
            class: Some("Warlock".into()),
            race: Some("Awoken".into()),
            gender: Some("Female".into()),
            light: Some(u16::MAX),
            date_last_played: Some(Timestamp::new(u64::MAX)),
        },
        Character {
            id: "character-2".into(),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: Some(0),
            date_last_played: None,
        },
    ]
}

/// The canonical items `full_input` round-trips to, ordered by identifier.
fn expected_items() -> Vec<ItemInstance> {
    full_input().items
}

#[test]
fn available_plugs_corruption_and_failed_writes_fail_closed() {
    for sql in [
        "UPDATE item_available_plugs SET ordinal=1",
        "PRAGMA ignore_check_constraints=ON; UPDATE item_available_plugs SET plug_hash=-1",
        "PRAGMA ignore_check_constraints=ON; UPDATE item_available_plugs SET socket_index=256",
        "PRAGMA foreign_keys=OFF; UPDATE item_available_plugs SET instance_id='orphan'",
    ] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut input = full_input();
        input.items[0].available_plugs = vec![destiny_core::AvailablePlug {
            socket_index: 0,
            plug_hash: ItemHash::new(50),
        }];
        let committed = store.replace_current(input.clone()).unwrap();
        store.connection().execute_batch("CREATE TRIGGER fail_available BEFORE INSERT ON item_available_plugs BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
        input.items[0].available_plugs[0].plug_hash = ItemHash::new(51);
        assert!(store.replace_current(input).is_err());
        assert_eq!(store.load_current().unwrap().unwrap(), committed);
        store
            .connection()
            .execute_batch("DROP TRIGGER fail_available")
            .unwrap();
        store.connection().execute_batch(sql).unwrap();
        assert_eq!(
            store.load_current().unwrap_err().category(),
            destiny_core::ErrorCategory::LocalStorage
        );
    }
}

#[test]
fn stats_round_trip_search_and_revision_identity() {
    use destiny_core::{SearchRepository, StatSummary};
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut input = full_input();
    input.items[0].available_plugs = vec![
        destiny_core::AvailablePlug {
            socket_index: 2,
            plug_hash: ItemHash::new(50),
        },
        destiny_core::AvailablePlug {
            socket_index: 2,
            plug_hash: ItemHash::new(51),
        },
    ];
    input.items[0].stats = vec![
        StatSummary {
            stat_hash: 1,
            value: i32::MIN,
        },
        StatSummary {
            stat_hash: u32::MAX,
            value: i32::MAX,
        },
    ];
    let first = store.replace_current(input.clone()).unwrap();
    let snapshot = store.load_current().unwrap().unwrap();
    assert_eq!(snapshot.items, input.items);
    assert_eq!(
        store
            .search_instances(&destiny_core::InstanceSearchQuery {
                offset: 0,
                sort: None,
                location: None,
                equipped: None,
                include_available: false,
                item_hashes: destiny_core::ItemHashFilter::Any,
                perk_hash: None,
                perk_name: None,
                mod_name: None,
                archetype: None,
                stats: Vec::new(),
                character_id: None,
                definitions: None,
                min_power: None,
                limit: 10
            })
            .unwrap(),
        input.items
    );
    let mut fake = destiny_app::test_support::InMemoryInventoryRepository::default();
    assert_eq!(fake.replace_current(input.clone()).unwrap(), first);
    assert_eq!(
        store.lookup_instance(&input.items[0].instance_id).unwrap(),
        destiny_core::Resolution::Unique(input.items[0].clone())
    );
    input.items[0].stats.reverse();
    assert_eq!(store.replace_current(input.clone()).unwrap(), first);
    assert_eq!(fake.replace_current(input.clone()).unwrap(), first);
    input.items[0].available_plugs.swap(0, 1);
    let reordered = store.replace_current(input.clone()).unwrap();
    assert_ne!(reordered.metadata.revision, first.metadata.revision);
    assert_eq!(fake.replace_current(input.clone()).unwrap(), reordered);
    input.items[0].stats[0].value -= 1;
    let changed = store.replace_current(input.clone()).unwrap();
    assert_ne!(changed.metadata.revision, first.metadata.revision);
    assert_eq!(fake.replace_current(input.clone()).unwrap(), changed);
    let duplicate = input.items[0].stats[0].clone();
    input.items[0].stats.push(duplicate);
    assert!(store.replace_current(input.clone()).is_err());
    assert!(fake.replace_current(input).is_err());
    assert_eq!(store.load_current().unwrap().unwrap(), changed);
    store.connection().execute_batch("CREATE TRIGGER fail_stats BEFORE INSERT ON item_stats BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
    let mut replacement = InventorySnapshotInput::with_metadata(&changed);
    replacement.items[0].stats[0].value += 1;
    assert!(store.replace_current(replacement).is_err());
    assert_eq!(store.load_current().unwrap().unwrap(), changed);
    store
        .connection()
        .execute_batch("DROP TRIGGER fail_stats; PRAGMA ignore_check_constraints = ON;")
        .unwrap();
    store
        .connection()
        .execute(
            "UPDATE item_stats SET value = 2147483648 WHERE stat_hash = 1",
            [],
        )
        .unwrap();
    assert!(store.load_current().is_err());
    assert!(store.lookup_instance(&"instance-1".into()).is_err());
}

#[test]
fn stats_migration_constraints_cascade_and_corruption_checks() {
    use destiny_core::SearchRepository;
    let mut store = seeded_store();
    for sql in [
        "INSERT INTO item_stats VALUES ('instance-1', -1, 0)",
        "INSERT INTO item_stats VALUES ('instance-1', 4294967296, 0)",
        "INSERT INTO item_stats VALUES ('instance-1', 1, 2147483648)",
        "INSERT INTO item_stats VALUES ('instance-1', 1, -2147483649)",
        "INSERT INTO item_stats VALUES ('instance-1', 1, 1.5)",
        "INSERT INTO item_stats VALUES ('missing', 1, 0)",
    ] {
        assert!(store.connection().execute(sql, []).is_err());
    }
    store
        .connection()
        .execute("INSERT INTO item_stats VALUES ('instance-1', 1, 0)", [])
        .unwrap();
    assert!(
        store
            .connection()
            .execute("INSERT INTO item_stats VALUES ('instance-1', 1, 10)", [])
            .is_err()
    );
    store.migrate_to(8).unwrap();
    assert!(
        store
            .connection()
            .prepare("SELECT * FROM item_stats")
            .is_err()
    );
    store.migrate_to(super::LATEST_SCHEMA_VERSION).unwrap();
    assert!(
        store
            .load_current()
            .unwrap()
            .unwrap()
            .items
            .iter()
            .all(|item| item.stats.is_empty())
    );
    for hash in 0..65 {
        store
            .connection()
            .execute(
                "INSERT INTO item_stats VALUES ('instance-1', ?1, 0)",
                [hash],
            )
            .unwrap();
    }
    assert!(store.load_current().is_err());
    assert!(store.lookup_instance(&"instance-1".into()).is_err());
    store
        .connection()
        .execute("DELETE FROM items WHERE instance_id = 'instance-1'", [])
        .unwrap();
    assert_eq!(
        store
            .connection()
            .query_row("SELECT count(*) FROM item_stats", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    store
        .connection()
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    store
        .connection()
        .execute("INSERT INTO item_stats VALUES ('missing', 1, 0)", [])
        .unwrap();
    assert!(store.load_current().is_err());
}

fn seeded_store() -> SqliteStore {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_current(full_input())
        .expect("seed the committed projection");
    store
}

fn character_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM characters", [], |row| row.get(0))
        .expect("count characters")
}

fn item_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .expect("count items")
}

fn perk_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM item_perks", [], |row| row.get(0))
        .expect("count perks")
}

fn metadata_row_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM sync_metadata", [], |row| row.get(0))
        .expect("count metadata rows")
}

/// Enables the in-test constraint bypass so a real write cannot produce the
/// corrupt values the repository's own parse must reject.
fn bypass_constraints(store: &SqliteStore) {
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable the in-test constraint bypass");
}

fn manifest_replacement() -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new("manifest-v1"),
            definitions: vec![ItemDefinition {
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: ItemHash::new(10),
                bucket_hash: None,
                name: "Alpha".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec!["tag".into()],
                version: DefinitionVersion::new("manifest-v1"),
            }],
        },
        checked_at: Timestamp::new(100),
        source: SourceMarker::Manifest,
    }
}

#[test]
fn empty_store_reports_no_projection() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    assert_eq!(store.load_current().expect("read projection"), None);
}

#[test]
fn empty_collections_publish_as_a_complete_projection() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut empty = full_input();
    empty.characters.clear();
    empty.items.clear();

    let returned = store
        .replace_current(empty)
        .expect("an empty but structurally valid projection publishes");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert!(returned.characters.is_empty());
    assert!(returned.items.is_empty());
    assert_eq!(character_count(&store), 0);
    assert_eq!(item_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned),
        "an anchored projection with no children is still a committed projection"
    );
}

#[test]
fn first_publication_assigns_revision_one_and_round_trips_every_field() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let returned = store
        .replace_current(full_input())
        .expect("publish the first complete projection");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert_eq!(returned.metadata.checked_at, Timestamp::new(200));
    assert_eq!(returned.metadata.source, SourceMarker::Profile);
    assert_eq!(returned.metadata.membership_id.as_str(), "membership-1");
    assert_eq!(returned.metadata.membership_type.value(), u32::MAX);
    assert_eq!(
        returned.metadata.definition_version.as_str(),
        "definitions-v1"
    );

    assert_eq!(returned.characters, expected_characters());
    assert_eq!(returned.items, expected_items());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned),
        "the committed projection must equal the returned snapshot exactly"
    );
}

#[test]
fn character_and_item_permutations_retain_revision_and_canonical_order() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let first = store
        .replace_current(full_input())
        .expect("publish the first projection");
    assert_eq!(first.metadata.revision.value(), 1);

    let mut permuted = full_input();
    permuted.characters.reverse();
    permuted.items.reverse();
    let second = store
        .replace_current(permuted)
        .expect("republish in the opposite order");

    assert_eq!(second.metadata.revision.value(), 1);
    assert_eq!(second.characters, expected_characters());
    assert_eq!(second.items, expected_items());
    assert_eq!(store.load_current().expect("read projection"), Some(second));
}

#[test]
fn permuted_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_current(full_input())
        .expect("publish the first projection");

    let mut permuted = full_input();
    permuted.characters.reverse();
    permuted.items.reverse();
    permuted.checked_at = Timestamp::new(9_000);
    let refreshed = store
        .replace_current(permuted)
        .expect("refresh with the same content");

    assert_eq!(refreshed.metadata.revision.value(), 1);
    assert_eq!(refreshed.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(refreshed.characters, expected_characters());
    assert_eq!(refreshed.items, expected_items());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(refreshed)
    );
}

#[test]
fn every_normalized_field_and_location_advances_exactly_once() {
    type Mutation = (&'static str, fn(&mut InventorySnapshotInput));

    // Character index 1 is unreferenced, so renaming or removing it never makes
    // the candidate structurally invalid.
    let mutations: [Mutation; 24] = [
        ("membership_id", |value| {
            value.membership_id = MembershipId::new("membership-2");
        }),
        ("membership_type", |value| {
            value.membership_type = MembershipType::new(2);
        }),
        ("definition_version", |value| {
            value.definition_version = DefinitionVersion::new("definitions-v2");
        }),
        ("character.id", |value| {
            value.characters[1].id = "character-2-changed".into();
        }),
        ("character.name", |value| {
            value.characters[1].name = Some("name-changed".into());
        }),
        ("character.class", |value| {
            value.characters[1].class = Some("class-changed".into());
        }),
        ("character.race", |value| {
            value.characters[1].race = Some("race-changed".into());
        }),
        ("character.gender", |value| {
            value.characters[1].gender = Some("gender-changed".into());
        }),
        ("character.light", |value| {
            value.characters[1].light = Some(1);
        }),
        ("item.instance_id", |value| {
            value.items[0].instance_id = "instance-1-changed".into();
        }),
        ("item.item_hash", |value| {
            value.items[0].item_hash = ItemHash::new(9);
        }),
        ("item.location.vault-to-character", |value| {
            value.items[0].location = ItemLocation::Character("character-1".into());
        }),
        ("item.location.vault-to-other", |value| {
            value.items[0].location = ItemLocation::Other("postmaster".into());
        }),
        ("item.quantity", |value| {
            value.items[0].quantity = 5;
        }),
        ("item.equipped", |value| {
            value.items[0].equipped = true;
        }),
        ("item.locked", |value| {
            value.items[0].locked = false;
        }),
        ("item.power", |value| {
            value.items[0].power = Some(1);
        }),
        ("item.perks.order", |value| {
            value.items[0].perks.reverse();
        }),
        ("item.perks.perk_hash", |value| {
            value.items[0].perks[0].perk_hash = PerkHash::new(1);
        }),
        ("item.perks.active", |value| {
            value.items[0].perks[0].active = false;
        }),
        ("item.perks.visible", |value| {
            value.items[0].perks[0].visible = false;
        }),
        ("item.perks.from-empty", |value| {
            value.items[2].perks.push(PerkSummary {
                perk_hash: PerkHash::new(9),
                active: false,
                visible: true,
            });
        }),
        ("characters.membership", |value| {
            value.characters.pop();
        }),
        ("items.membership", |value| {
            value.items.pop();
        }),
    ];

    for (field, mutate) in mutations {
        let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
        store
            .replace_current(full_input())
            .expect("publish the base projection");
        let mut changed = full_input();
        mutate(&mut changed);

        assert_eq!(
            store
                .replace_current(changed)
                .expect("changed content publishes")
                .metadata
                .revision
                .value(),
            2,
            "changing {field} must advance the revision exactly once"
        );
    }
}

#[test]
fn invalid_source_duplicates_and_dangling_references_preserve_state() {
    let mut store = seeded_store();
    let committed = store.load_current().expect("read projection");

    let mut wrong_source = full_input();
    wrong_source.source = SourceMarker::Manifest;
    wrong_source.membership_id = MembershipId::new("membership-2");

    let mut duplicate_characters = full_input();
    duplicate_characters
        .characters
        .push(duplicate_characters.characters[0].clone());

    let mut duplicate_items = full_input();
    duplicate_items.items.push(duplicate_items.items[0].clone());

    let mut dangling = full_input();
    dangling.items[0].location = ItemLocation::Character("missing-character".into());

    let candidates = [
        ("invalid source", wrong_source),
        ("duplicate character ids", duplicate_characters),
        ("duplicate item ids", duplicate_items),
        ("dangling character location", dangling),
    ];

    for (label, candidate) in candidates {
        assert_eq!(
            store.replace_current(candidate),
            Err(AppError::new(ErrorCategory::ProviderValidation)),
            "{label} must be rejected as provider validation"
        );
        assert_eq!(
            store.load_current().expect("read projection"),
            committed,
            "{label} must preserve the committed projection"
        );
    }
}

#[test]
fn complete_replacement_removes_stale_rows_on_membership_and_set_change() {
    let mut store = seeded_store();
    assert_eq!(character_count(&store), 2);
    assert_eq!(item_count(&store), 3);

    let mut smaller = full_input();
    smaller.membership_id = MembershipId::new("membership-2");
    smaller
        .characters
        .retain(|character| character.id.as_str() == "character-1");
    smaller
        .items
        .retain(|item| item.instance_id.as_str() == "instance-1");

    let replaced = store
        .replace_current(smaller)
        .expect("publish the smaller projection");

    assert_eq!(replaced.metadata.revision.value(), 2);
    assert_eq!(replaced.metadata.membership_id.as_str(), "membership-2");
    assert_eq!(
        character_count(&store),
        1,
        "stale characters must be removed"
    );
    assert_eq!(item_count(&store), 1, "stale items must be removed");
    assert_eq!(
        perk_count(&store),
        3,
        "perks of removed items must be removed"
    );
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(replaced)
    );
}

#[test]
fn empty_perk_lists_round_trip_as_empty() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut input = full_input();
    for item in &mut input.items {
        item.perks.clear();
    }

    let returned = store
        .replace_current(input)
        .expect("a projection with no perks publishes");

    assert_eq!(returned.metadata.revision.value(), 1);
    assert!(returned.items.iter().all(|item| item.perks.is_empty()));
    assert_eq!(perk_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned)
    );
}

#[test]
fn ordered_perks_round_trip_with_duplicates_and_both_hash_bounds() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let returned = store
        .replace_current(full_input())
        .expect("publish the full perk projection");

    assert_eq!(returned.items, expected_items());
    assert_eq!(perk_count(&store), 4);
    assert_eq!(returned.items[0].perks.len(), 3);
    assert_eq!(returned.items[0].perks[0].perk_hash, PerkHash::new(0));
    assert_eq!(
        returned.items[0].perks[1].perk_hash,
        PerkHash::new(u32::MAX)
    );
    assert_eq!(
        returned.items[0].perks[2].perk_hash,
        PerkHash::new(0),
        "a duplicated perk hash must be preserved, not deduplicated"
    );
    assert!(returned.items[2].perks.is_empty());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(returned)
    );
}

#[test]
fn unchanged_exact_perks_retain_the_revision() {
    let mut store = seeded_store();
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);

    let retained = store
        .replace_current(refreshed)
        .expect("refresh the identical perks");

    assert_eq!(retained.metadata.revision.value(), 1);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(retained.items, expected_items());
}

#[test]
fn stale_perks_are_removed_when_items_shrink_or_lose_their_perks() {
    let mut store = seeded_store();
    assert_eq!(perk_count(&store), 4);

    let mut shrunk = full_input();
    shrunk.items[0].perks.truncate(1);
    shrunk.items[1].perks.clear();
    let replaced = store
        .replace_current(shrunk)
        .expect("publish the shrunk perk lists");
    assert_eq!(replaced.metadata.revision.value(), 2);
    assert_eq!(perk_count(&store), 1);
    assert_eq!(replaced.items[0].perks.len(), 1);
    assert!(replaced.items[1].perks.is_empty());

    let mut cleared = full_input();
    for item in &mut cleared.items {
        item.perks.clear();
    }
    let replaced = store
        .replace_current(cleared)
        .expect("publish with every perk cleared");
    assert_eq!(replaced.metadata.revision.value(), 3);
    assert_eq!(perk_count(&store), 0);
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(replaced)
    );
}

#[test]
fn perk_write_failure_rolls_back_items_perks_and_metadata() {
    let mut store = seeded_store();
    let committed = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(perk_count(&store), 4);

    // The replacement changes membership, writes the perk-bearing first item and
    // its three perks, and then fails on the second item's over-long unknown
    // location. The whole transaction must roll back to the prior projection.
    let mut failing = full_input();
    failing.membership_id = MembershipId::new("membership-2");
    failing.items[1].location = ItemLocation::Other("x".repeat(257));

    let error = store
        .replace_current(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error, storage_error());
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed),
        "the prior projection must survive the rollback"
    );
    assert_eq!(
        perk_count(&store),
        4,
        "rolled-back perk rows must not survive"
    );
    assert_eq!(item_count(&store), 3);
}

#[test]
fn corrupt_perk_rows_fail_closed() {
    // A non-contiguous ordinal list: deleting the zero ordinal leaves 1 and 2.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "DELETE FROM item_perks WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("drop the first perk ordinal");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-boolean active flag.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET active = 2 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a non-boolean active flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-boolean visible flag.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET visible = -1 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a negative visible flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range perk hash.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET perk_hash = 4294967296 \
             WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write an out-of-range perk hash");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A negative ordinal.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE item_perks SET ordinal = -1 WHERE instance_id = 'instance-1' AND ordinal = 0",
            [],
        )
        .expect("write a negative ordinal");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A perk row that references an unknown item instance, foreign keys off.
    let store = seeded_store();
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys for the corrupt write");
    store
        .connection()
        .execute(
            "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
             VALUES ('ghost-item', 0, 1, 0, 0)",
            [],
        )
        .expect("write an orphan perk row");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn overflow_retains_at_the_boundary_and_preserves_state() {
    let mut store = seeded_store();
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = '18446744073709551615' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("seed the overflow boundary");

    // An unchanged refresh at the boundary retains the revision and refreshes
    // only the committed timestamp.
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store
        .replace_current(refreshed)
        .expect("an unchanged refresh must not overflow");
    assert_eq!(retained.metadata.revision.value(), u64::MAX);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));

    // A genuine content change at the boundary overflows before any write.
    let mut changed = full_input();
    changed.membership_type = MembershipType::new(2);
    assert_eq!(store.replace_current(changed), Err(storage_error()));

    let loaded = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(loaded.metadata.revision.value(), u64::MAX);
    assert_eq!(loaded.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(loaded.characters, retained.characters);
    assert_eq!(loaded.items, retained.items);
}

#[test]
fn constraint_failure_mid_replacement_rolls_back_profile_rows_and_metadata() {
    let mut store = seeded_store();
    let committed = store
        .load_current()
        .expect("read projection")
        .expect("projection present");

    // The membership and both collections change, so the replacement deletes
    // every prior row, upserts the anchor, writes the first character, and then
    // fails on a name that exceeds the schema bound.
    let mut failing = full_input();
    failing.membership_id = MembershipId::new("membership-2");
    failing.items.clear();
    failing.characters[1].name = Some("x".repeat(257));

    let error = store
        .replace_current(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(error, storage_error());

    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed),
        "the prior projection and metadata must survive the rollback"
    );
    assert_eq!(character_count(&store), 2);
    assert_eq!(item_count(&store), 3);
}

#[test]
fn partial_anchor_and_child_rows_fail_closed() {
    // Metadata without the active-profile anchor.
    let store = seeded_store();
    store
        .connection()
        .execute("DELETE FROM active_profile", [])
        .expect("remove the anchor");
    assert_eq!(store.load_current(), Err(storage_error()));

    // The active-profile anchor without metadata.
    let store = seeded_store();
    store
        .connection()
        .execute(
            "DELETE FROM sync_metadata WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("remove the metadata");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Character rows without both anchor and metadata.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute(
            "INSERT INTO characters (character_id, name, class_name, race, gender, light) \
             VALUES ('orphan', 'n', 'c', 'r', 'g', 1)",
            [],
        )
        .expect("insert an orphan character");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Item rows without both anchor and metadata.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute(
            "INSERT INTO items \
             (instance_id, item_hash, location_kind, location_character_id, location_other, \
              quantity, equipped, locked, power) \
             VALUES ('orphan-item', 1, 'vault', NULL, NULL, 1, 0, 0, NULL)",
            [],
        )
        .expect("insert an orphan item");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Perk rows without any item, anchor, or metadata, foreign keys off.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys");
    store
        .connection()
        .execute(
            "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
             VALUES ('ghost-perk', 0, 1, 0, 1)",
            [],
        )
        .expect("insert an orphan perk");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn extra_active_profile_rows_fail_closed() {
    let store = seeded_store();
    bypass_constraints(&store);

    // The schema's primary key makes `singleton = 1` unique in a well-formed
    // store, but a corrupt database could carry an additional row with a
    // different discriminator. The singleton read must fail closed instead of
    // silently ignoring it.
    store
        .connection()
        .execute(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (2, 'intruder', '1')",
            [],
        )
        .expect("insert an extra anchor row under the constraint bypass");
    assert_eq!(store.load_current(), Err(storage_error()));

    // Restore constraint enforcement: the corrupt two-row state still fails
    // closed, and the committed schema again rejects a fabricated anchor row.
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = OFF")
        .expect("restore constraint enforcement");
    assert_eq!(store.load_current(), Err(storage_error()));
    assert!(
        store
            .connection()
            .execute(
                "INSERT INTO active_profile (singleton, membership_id, membership_type) \
                 VALUES (3, 'intruder-2', '1')",
                [],
            )
            .is_err(),
        "the committed schema must reject a fabricated anchor row"
    );
}

#[test]
fn malformed_decimal_source_and_range_values_fail_closed() {
    // A malformed revision.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = 'not-a-number' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a malformed revision");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-canonical timestamp.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET checked_at = '01' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a non-canonical timestamp");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A mismatched provenance marker.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET source = 'manifest' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("write a mismatched source");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range membership type.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE active_profile SET membership_type = '4294967296'",
            [],
        )
        .expect("write an out-of-range membership type");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A non-canonical membership type.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute("UPDATE active_profile SET membership_type = '01'", [])
        .expect("write a non-canonical membership type");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A corrupt singleton discriminator.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute("UPDATE active_profile SET singleton = 0", [])
        .expect("write a corrupt singleton");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn invalid_boolean_location_and_reference_values_fail_closed() {
    // An out-of-range light value.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE characters SET light = 70000 WHERE character_id = 'character-2'",
            [],
        )
        .expect("write an out-of-range light");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range quantity.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET quantity = 4294967296 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an out-of-range quantity");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An out-of-range power value.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET power = 70000 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an out-of-range power");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A boolean that is not exactly 0 or 1.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET equipped = 2 WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a non-boolean flag");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A vault location that also carries a character reference.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET location_character_id = 'character-1' \
             WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a mixed location");
    assert_eq!(store.load_current(), Err(storage_error()));

    // An unknown location kind.
    let store = seeded_store();
    bypass_constraints(&store);
    store
        .connection()
        .execute(
            "UPDATE items SET location_kind = 'future' WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write an unknown location kind");
    assert_eq!(store.load_current(), Err(storage_error()));

    // A character reference with no matching character, with foreign keys off.
    let store = seeded_store();
    store
        .connection()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .expect("disable foreign keys for the corrupt write");
    store
        .connection()
        .execute(
            "UPDATE items \
             SET location_kind = 'character', location_character_id = 'ghost', \
                 location_other = NULL \
             WHERE instance_id = 'instance-1'",
            [],
        )
        .expect("write a dangling reference");
    assert_eq!(store.load_current(), Err(storage_error()));
}

#[test]
fn file_close_and_reopen_preserves_exact_projection_and_revision() {
    let temp = TempDatabase::new();

    let committed = {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store
            .replace_current(full_input())
            .expect("publish to the file store")
    };

    let mut store = SqliteStore::open(temp.path()).expect("reopen file store");
    assert_eq!(
        store.load_current().expect("read projection"),
        Some(committed.clone())
    );

    // The reconstructed committed content equals the original input, so an
    // unchanged refresh after reopen retains the revision.
    let mut refreshed = full_input();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store
        .replace_current(refreshed)
        .expect("refresh after reopen");
    assert_eq!(retained.metadata.revision.value(), 1);
    assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(retained.characters, committed.characters);
    assert_eq!(retained.items, committed.items);
}

#[test]
fn manifest_definitions_and_metadata_are_untouched_by_profile_writes() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let manifest = store
        .replace_definitions(manifest_replacement())
        .expect("publish Manifest definitions");
    store
        .replace_current(full_input())
        .expect("publish the profile projection");

    let mut changed = full_input();
    changed.membership_id = MembershipId::new("membership-2");
    store
        .replace_current(changed)
        .expect("replace the profile projection");

    assert_eq!(metadata_row_count(&store), 2);
    assert_eq!(
        store.current_metadata().expect("read Manifest metadata"),
        Some(manifest),
        "the Manifest metadata row must not change"
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read Manifest version")
            .as_str(),
        "manifest-v1"
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(10))
            .expect("lookup definition"),
        Resolution::Unique(manifest_replacement().definitions.definitions[0].clone()),
        "the Manifest catalog must not change"
    );

    // The profile projection itself remains readable and canonical.
    let loaded = store
        .load_current()
        .expect("read projection")
        .expect("projection present");
    assert_eq!(loaded.metadata.revision.value(), 2);
    assert_eq!(loaded.metadata.membership_id.as_str(), "membership-2");
}

#[test]
fn errors_and_debug_never_leak_stored_values() {
    let mut store = seeded_store();
    let committed = store.load_current().expect("read projection");

    let sentinel = "provider-secret-sentinel";
    let mut bad = full_input();
    bad.characters[1].name = Some(sentinel.repeat(20));
    let error = store
        .replace_current(bad)
        .expect_err("an over-long name is rejected");

    assert_eq!(error, storage_error());
    for leaked in [
        sentinel,
        "sqlite",
        "constraint",
        "CHECK",
        "characters",
        "items",
        "active_profile",
        "sync_metadata",
    ] {
        assert!(
            !error.to_string().contains(leaked),
            "error leaked {leaked}: {error}"
        );
    }
    assert!(!format!("{error:?}").contains(sentinel));
    assert_eq!(
        store.load_current().expect("read projection"),
        committed,
        "a rejected replacement must preserve the committed projection"
    );
}

#[test]
fn currency_zero_and_coverage_sqlite_roundtrip() {
    use destiny_core::{
        BucketHash, ProfileCurrenciesCoverage as Coverage, StackContainer, StackObservation,
    };
    let temp = TempDatabase::new();
    let mut store = SqliteStore::open(temp.path()).unwrap();
    let mut input = full_input();
    input.profile_currencies_coverage = Coverage::Available;
    input.stacks.push(
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(8),
            0,
            StackContainer::ProfileCurrencies,
        )
        .unwrap(),
    );
    input.stacks.push(input.stacks[0].clone());
    let snapshot = store.replace_current(input).unwrap();
    assert_eq!(snapshot.profile_currencies_coverage, Coverage::Available);
    assert_eq!(snapshot.stacks.len(), 2);
    assert_eq!(store.load_current().unwrap(), Some(snapshot.clone()));
    drop(store);
    let reopened = SqliteStore::open(temp.path()).unwrap();
    assert_eq!(reopened.load_current().unwrap(), Some(snapshot));
}

#[test]
fn currency_coverage_states_revision_restart_and_rollback() {
    use destiny_core::ProfileCurrenciesCoverage as Coverage;
    let temp = TempDatabase::new();
    let mut last_revision = 0;
    for coverage in [
        Coverage::NotObserved,
        Coverage::Unavailable,
        Coverage::Available,
    ] {
        let snapshot = {
            let mut store = SqliteStore::open(temp.path()).unwrap();
            let mut input = full_input();
            input.profile_currencies_coverage = coverage;
            let snapshot = store.replace_current(input).unwrap();
            assert_eq!(snapshot.metadata.revision.value(), last_revision + 1);
            let mut refreshed = InventorySnapshotInput::with_metadata(&snapshot);
            refreshed.checked_at = Timestamp::new(snapshot.metadata.checked_at.seconds() + 1);
            let same = store.replace_current(refreshed).unwrap();
            assert_eq!(same.metadata.revision, snapshot.metadata.revision);
            last_revision = snapshot.metadata.revision.value();
            same
        };
        let mut store = SqliteStore::open(temp.path()).unwrap();
        assert_eq!(store.load_current().unwrap(), Some(snapshot.clone()));
        store
            .connection()
            .execute_batch(
                "CREATE TRIGGER refuse_metadata BEFORE UPDATE ON sync_metadata
            BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
            )
            .unwrap();
        let mut changed = InventorySnapshotInput::with_metadata(&snapshot);
        changed.profile_currencies_coverage = if coverage == Coverage::Available {
            Coverage::Unavailable
        } else {
            Coverage::Available
        };
        assert_eq!(
            store.replace_current(changed).unwrap_err().category(),
            ErrorCategory::LocalStorage
        );
        assert_eq!(store.load_current().unwrap(), Some(snapshot));
        store
            .connection()
            .execute_batch("DROP TRIGGER refuse_metadata")
            .unwrap();
    }
}

#[test]
fn currency_marker_orphans_corruption_and_contradictions_fail_closed() {
    use destiny_core::ProfileCurrenciesCoverage as Coverage;
    for corrupt in [
        "UPDATE inventory_coverage SET profile_currencies = 'invalid'",
        "UPDATE inventory_coverage SET id = 2",
        "INSERT INTO inventory_coverage VALUES(2,'available')",
        "INSERT INTO stacks VALUES(0,'profile_currencies',NULL,8,7,1)",
    ] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let original = store.replace_current(full_input()).unwrap();
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = ON")
            .unwrap();
        store.connection().execute_batch(corrupt).unwrap();
        store
            .connection()
            .execute_batch("PRAGMA ignore_check_constraints = OFF")
            .unwrap();
        assert_eq!(
            store.load_current().unwrap_err().category(),
            ErrorCategory::LocalStorage,
            "{corrupt}"
        );
        assert_eq!(
            store
                .replace_current(InventorySnapshotInput::with_metadata(&original))
                .unwrap_err()
                .category(),
            ErrorCategory::LocalStorage
        );
    }
    for coverage in ["not_observed", "unavailable", "available"] {
        let mut store = SqliteStore::open_in_memory().unwrap();
        store
            .connection()
            .execute("INSERT INTO inventory_coverage VALUES(1,?1)", [coverage])
            .unwrap();
        assert_eq!(
            store.load_current().unwrap_err().category(),
            ErrorCategory::LocalStorage
        );
        assert_eq!(
            store.replace_current(full_input()).unwrap_err().category(),
            ErrorCategory::LocalStorage
        );
    }
    let mut store = SqliteStore::open_in_memory().unwrap();
    let snapshot = store.replace_current(full_input()).unwrap();
    store
        .connection()
        .execute("DELETE FROM inventory_coverage", [])
        .unwrap();
    assert_eq!(store.load_current().unwrap(), Some(snapshot));
    // Legacy positive currency rows do not imply availability, even after migration.
    store.migrate_to(16).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO stacks VALUES(0,'profile_currencies',NULL,8,7,1)",
            [],
        )
        .unwrap();
    store.migrate_to(17).unwrap();
    assert_eq!(
        store.load_current().unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    let mut available = full_input();
    available.profile_currencies_coverage = Coverage::Available;
    assert_eq!(
        store.replace_current(available).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
}

#[test]
fn currency_coverage_structural_rejection_and_balance_revision() {
    use destiny_core::{
        BucketHash, ProfileCurrenciesCoverage as Coverage, StackContainer, StackObservation,
    };
    let mut store = SqliteStore::open_in_memory().unwrap();
    let original = store.replace_current(full_input()).unwrap();
    let balance = |quantity| {
        StackObservation::new(
            ItemHash::new(7),
            BucketHash::new(8),
            quantity,
            StackContainer::ProfileCurrencies,
        )
        .unwrap()
    };
    for coverage in [Coverage::NotObserved, Coverage::Unavailable] {
        let mut invalid = full_input();
        invalid.profile_currencies_coverage = coverage;
        invalid.stacks.push(balance(0));
        assert_eq!(
            store.replace_current(invalid).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(store.load_current().unwrap(), Some(original.clone()));
    }
    let mut input = full_input();
    input.profile_currencies_coverage = Coverage::Available;
    let empty = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        empty.metadata.revision.value(),
        original.metadata.revision.value() + 1
    );
    input.stacks.push(balance(0));
    let zero = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        zero.metadata.revision.value(),
        empty.metadata.revision.value() + 1
    );
    input.stacks[0] = balance(1);
    let positive = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        positive.metadata.revision.value(),
        zero.metadata.revision.value() + 1
    );
    input.stacks.push(balance(1));
    let duplicate = store.replace_current(input.clone()).unwrap();
    assert_eq!(
        duplicate.metadata.revision.value(),
        positive.metadata.revision.value() + 1
    );
    input.stacks.reverse();
    assert_eq!(
        store.replace_current(input).unwrap().metadata.revision,
        duplicate.metadata.revision
    );
}
