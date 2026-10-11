//! Focused contract tests for the transactional SQLite definition repository.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars.

use super::{LATEST_SCHEMA_VERSION, SqliteStore};
use destiny_core::{
    AppError, BucketHash, Dataset, DefinitionReplacementInput, DefinitionRepository,
    DefinitionVersion, ErrorCategory, ItemDefinition, ItemHash, ManifestDefinitions, Resolution,
    SourceMarker, Timestamp,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

#[test]
fn compact_search_missing_rows_and_table_fail_closed() {
    use destiny_core::{DefinitionSearchQuery, SearchRepository};
    let mut store = SqliteStore::open_in_memory().unwrap();
    let input = replacement("v1", base_manifest().definitions);
    store.replace_definitions(input.clone()).unwrap();
    store
        .connection()
        .execute("DELETE FROM definition_search", [])
        .unwrap();
    let query = DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: 10,
    };
    assert_eq!(
        store.search_definitions(&query).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    assert!(store.name_catalog("perk").is_err());
    assert_eq!(
        store
            .replace_definitions(input.clone())
            .unwrap_err()
            .category(),
        ErrorCategory::LocalStorage
    );
    // A changed publication atomically replaces the incomplete projection.
    let mut changed = input;
    changed.definitions.definitions[0].name.push_str(" changed");
    store.replace_definitions(changed).unwrap();
    assert!(!store.search_definitions(&query).unwrap().is_empty());
    store.migrate_to(21).unwrap();
    assert_eq!(
        store.search_definitions(&query).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
}

#[test]
fn compact_search_failed_backfill_rolls_back_schema_and_version() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(21).unwrap();
    store
        .replace_definitions(replacement("v1", base_manifest().definitions))
        .unwrap();
    store
        .connection()
        .execute_batch(
            "PRAGMA ignore_check_constraints = ON; UPDATE definitions SET slot = 'corrupt';",
        )
        .unwrap();
    assert_eq!(
        store.migrate_to(22).unwrap_err().category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(store.schema_version().unwrap(), 21);
    assert!(
        store
            .connection()
            .prepare("SELECT * FROM definition_search")
            .is_err()
    );
}

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-definitions-{}-{sequence}.db",
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

#[test]
fn qualified_postmaster_metadata_is_refused_before_refresh_or_replacement() {
    use destiny_core::ports::WorldDefinitionRepository;
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(17).unwrap();
    let prior = replacement("v1", base_manifest().definitions);
    let world = bridge_world();
    let metadata = store
        .publish_manifest(prior.clone(), world.clone())
        .unwrap();
    store.connection().execute_batch(
        "INSERT INTO active_profile(singleton,membership_id,membership_type) VALUES (1,'123','3');
         INSERT INTO items VALUES ('instance',10,'vault',NULL,NULL,1,0,0,NULL);
         INSERT INTO item_perks VALUES ('instance',0,7,1,1);
         INSERT INTO sync_metadata VALUES ('profile_inventory','100','profile','4','v1');"
    ).unwrap();
    let before = bridge_rows(&store);
    let changes = store.connection().total_changes();
    // Test each field independently, including values truthiness would lose.
    for known in 0..11 {
        for path in 0..3 {
            for changed_catalog in [false, true] {
                let mut next = prior.clone();
                next.checked_at = Timestamp::new(999);
                let definition = &mut next.definitions.definitions[0];
                if changed_catalog {
                    definition.name = "Replacement".into();
                    definition.tags.clear();
                }
                match known {
                    0 => definition.does_postmaster_pull_have_side_effects = Some(false),
                    1 => definition.does_postmaster_pull_have_side_effects = Some(true),
                    2 => definition.is_instance_item = Some(false),
                    3 => definition.is_instance_item = Some(true),
                    4 => definition.max_stack_size = Some(i32::MIN),
                    5 => definition.max_stack_size = Some(-1),
                    6 => definition.max_stack_size = Some(0),
                    7 => definition.max_stack_size = Some(1),
                    8 => definition.max_stack_size = Some(i32::MAX),
                    9 => definition.stack_unique_label = Some(String::new()),
                    _ => definition.stack_unique_label = Some(" Exact Label ".into()),
                }
                let result = match path {
                    0 => store.replace_definitions(next),
                    1 => store.replace_definitions_with_descriptions(
                        next,
                        world.item_descriptions.clone(),
                    ),
                    _ => store.publish_manifest(next, world.clone()),
                };
                assert_eq!(result, Err(storage_error()));
                assert_eq!(
                    store.connection().total_changes(),
                    changes,
                    "no writes, even freshness-only"
                );
                assert_eq!(bridge_rows(&store), before, "all committed tables intact");
                assert_eq!(store.current_metadata().unwrap(), Some(metadata.clone()));
                assert_eq!(
                    store.lookup_definition(ItemHash::new(10)).unwrap(),
                    Resolution::Unique(prior.definitions.definitions[0].clone())
                );
            }
        }
    }
    assert_eq!(store.schema_version().unwrap(), 17);
}

#[test]
fn non_transferrable_fake_sqlite_roundtrip_and_revision_identity() {
    let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::new(
        DefinitionVersion::new("v1"),
        vec![],
    );
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut next = replacement("v1", base_manifest().definitions);
    let mut previous = None;
    for flag in [None, Some(false), Some(true), None] {
        next.definitions.definitions[0].non_transferrable = flag;
        let real = store.replace_definitions(next.clone()).unwrap();
        assert_eq!(real, fake.replace_definitions(next.clone()).unwrap());
        if let Some(revision) = previous {
            assert_eq!(real.revision.value(), revision + 1);
        }
        previous = Some(real.revision.value());
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)).unwrap(),
            Resolution::Unique(next.definitions.definitions[0].clone())
        );
        assert_eq!(
            store.replace_definitions(next.clone()).unwrap().revision,
            real.revision
        );
    }
}

#[test]
fn qualified_metadata_v18_restart_and_content_identity() {
    let database = TempDatabase::new();
    let mut next = replacement("v1", base_manifest().definitions);
    let definition = &mut next.definitions.definitions[0];
    definition.does_postmaster_pull_have_side_effects = Some(false);
    definition.is_instance_item = Some(true);
    definition.max_stack_size = Some(i32::MIN);
    definition.stack_unique_label = Some(" \0\n Exact ".into());
    let metadata = {
        let mut store = SqliteStore::open(database.path()).unwrap();
        store.replace_definitions(next.clone()).unwrap()
    };
    let mut store = SqliteStore::open(database.path()).unwrap();
    assert_eq!(
        store.schema_version().unwrap(),
        super::LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).unwrap(),
        Resolution::Unique(next.definitions.definitions[0].clone())
    );
    next.checked_at = Timestamp::new(999);
    let refreshed = store.replace_definitions(next.clone()).unwrap();
    assert_eq!(refreshed.revision, metadata.revision);
    assert_eq!(refreshed.checked_at, next.checked_at);
    next.definitions.definitions[0].does_postmaster_pull_have_side_effects = Some(true);
    let changed = store.replace_definitions(next).unwrap();
    assert_eq!(changed.revision.value(), refreshed.revision.value() + 1);
}

#[test]
fn qualified_metadata_v18_all_publication_paths_and_identity() {
    use destiny_core::ports::WorldDefinitionRepository;
    for path in 0..3 {
        for changed_catalog in [false, true] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let world = bridge_world();
            let mut next = replacement("v1", base_manifest().definitions);
            let publish = |store: &mut SqliteStore, next: DefinitionReplacementInput| match path {
                0 => store.replace_definitions(next),
                1 => store
                    .replace_definitions_with_descriptions(next, world.item_descriptions.clone()),
                _ => store.publish_manifest(next, world.clone()),
            };
            let mut metadata = publish(&mut store, next.clone()).unwrap();
            store
                .connection()
                .execute_batch(
                    "INSERT INTO active_profile VALUES(1,'123','3');
                 INSERT INTO items(instance_id,item_hash,location_kind,location_character_id,location_other,quantity,equipped,locked,power) VALUES('instance',10,'vault',NULL,NULL,1,0,0,NULL);
                 INSERT INTO item_perks VALUES('instance',0,7,1,1);
                 INSERT INTO item_plugs VALUES('instance',0,8);
                 INSERT INTO item_stats VALUES('instance',9,-1);
                 INSERT INTO item_available_plugs VALUES('instance',0,0,10);
                 INSERT INTO stacks VALUES(0,'profile_inventory',NULL,8,10,2);
                 INSERT INTO stacks VALUES(1,'profile_inventory',NULL,8,10,2);
                 INSERT INTO stacks VALUES(2,'profile_currencies',NULL,9,10,0);
                 INSERT INTO inventory_coverage VALUES(1,'available');
                 INSERT INTO sync_metadata (dataset,checked_at,source,revision,definition_version,invalidated) VALUES('profile_inventory','100','profile','4','v1',0);",
                )
                .unwrap();
            let inventory = |store: &SqliteStore| {
                bridge_rows(store)
                    .into_iter()
                    .filter(|(table, _)| {
                        [
                            "active_profile",
                            "items",
                            "item_perks",
                            "item_plugs",
                            "item_stats",
                            "item_available_plugs",
                            "stacks",
                            "inventory_coverage",
                        ]
                        .contains(&table.as_str())
                    })
                    .collect::<Vec<_>>()
            };
            let before = inventory(&store);
            // Each successive value changes exactly one metadata field. The
            // final unknown record also proves known -> unknown is content.
            for case in 0..13 {
                let definition = &mut next.definitions.definitions[0];
                if changed_catalog {
                    definition.name = format!("Replacement {case}");
                }
                match case {
                    0 => definition.does_postmaster_pull_have_side_effects = Some(false),
                    1 => definition.does_postmaster_pull_have_side_effects = Some(true),
                    2 => definition.is_instance_item = Some(false),
                    3 => definition.is_instance_item = Some(true),
                    4 => definition.max_stack_size = Some(i32::MIN),
                    5 => definition.max_stack_size = Some(-1),
                    6 => definition.max_stack_size = Some(0),
                    7 => definition.max_stack_size = Some(1),
                    8 => definition.max_stack_size = Some(i32::MAX),
                    9 => definition.stack_unique_label = Some(String::new()),
                    10 => definition.stack_unique_label = Some(" Exact Label ".into()),
                    11 => definition.stack_unique_label = Some("Exact Label".into()),
                    _ => {
                        definition.does_postmaster_pull_have_side_effects = None;
                        definition.is_instance_item = None;
                        definition.max_stack_size = None;
                        definition.stack_unique_label = None;
                    }
                }
                next.checked_at = Timestamp::new(300 + case);
                let changed = publish(&mut store, next.clone()).unwrap();
                assert_eq!(changed.revision.value(), metadata.revision.value() + 1);
                assert_eq!(
                    store.lookup_definition(ItemHash::new(10)).unwrap(),
                    Resolution::Unique(next.definitions.definitions[0].clone())
                );
                assert_eq!(
                    inventory(&store),
                    before,
                    "metadata does not relabel or erase observations"
                );
                assert_eq!(
                    store
                        .connection()
                        .query_row(
                            "SELECT revision FROM sync_metadata WHERE dataset='profile_inventory'",
                            [],
                            |row| row.get::<_, String>(0)
                        )
                        .unwrap(),
                    "4"
                );
                let catalog_before = bridge_rows(&store);
                next.checked_at = Timestamp::new(500 + case);
                metadata = publish(&mut store, next.clone()).unwrap();
                assert_eq!(metadata.revision, changed.revision);
                assert_eq!(metadata.checked_at, next.checked_at);
                let without_freshness = |rows: Vec<(String, Vec<Vec<rusqlite::types::Value>>)>| {
                    rows.into_iter()
                        .filter(|(table, _)| table != "sync_metadata")
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    without_freshness(bridge_rows(&store)),
                    without_freshness(catalog_before)
                );
            }
        }
    }
}

#[test]
fn qualified_metadata_v18_failed_publication_and_corruption_are_atomic() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let prior = replacement("v1", base_manifest().definitions);
    store.replace_definitions(prior.clone()).unwrap();
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER fail_metadata BEFORE INSERT ON definitions
         WHEN NEW.hash=20 AND NEW.is_instance_item=0
         BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    let before = bridge_rows(&store);
    let mut next = prior.clone();
    next.definitions.definitions[0].max_stack_size = Some(i32::MAX);
    next.definitions.definitions[1].is_instance_item = Some(false);
    assert_eq!(store.replace_definitions(next), Err(storage_error()));
    assert_eq!(
        bridge_rows(&store),
        before,
        "rollback even after first new row inserted"
    );

    // Constraint bypass models corrupt on-disk values; both single-record and
    // catalog identity reads must fail rather than refresh or reinterpret them.
    store
        .connection()
        .pragma_update(None, "ignore_check_constraints", 1)
        .unwrap();
    for (column, value) in [
        ("does_postmaster_pull_have_side_effects", -1_i64),
        ("does_postmaster_pull_have_side_effects", 2),
        ("is_instance_item", -1),
        ("is_instance_item", 2),
        ("max_stack_size", i64::from(i32::MIN) - 1),
        ("max_stack_size", i64::from(i32::MAX) + 1),
    ] {
        store
            .connection()
            .execute(
                &format!("UPDATE definitions SET {column}=?1 WHERE hash=10"),
                [value],
            )
            .unwrap();
        let corrupt = bridge_rows(&store);
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)),
            Err(storage_error())
        );
        assert_eq!(
            store.replace_definitions(prior.clone()),
            Err(storage_error())
        );
        assert_eq!(bridge_rows(&store), corrupt);
        store
            .connection()
            .execute(
                &format!("UPDATE definitions SET {column}=NULL WHERE hash=10"),
                [],
            )
            .unwrap();
    }
}

#[test]
fn qualified_metadata_v18_migration_preserves_all_rows_and_guards_downgrade() {
    use destiny_core::ports::WorldDefinitionRepository;
    use rusqlite::types::Value;
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(17).unwrap();
    let prior = replacement("v1", base_manifest().definitions);
    store
        .publish_manifest(prior.clone(), bridge_world())
        .unwrap();
    store
        .connection()
        .execute_batch(
            "INSERT INTO active_profile VALUES(1,'123','3');
         INSERT INTO characters(character_id,class_name) VALUES('owner','Hunter');
         INSERT INTO items VALUES('instance',10,'postmaster','owner',NULL,1,0,0,NULL);
         INSERT INTO item_perks VALUES('instance',0,7,1,1);
         INSERT INTO item_plugs VALUES('instance',0,8);
         INSERT INTO item_stats VALUES('instance',9,-1);
         INSERT INTO item_available_plugs VALUES('instance',0,0,10);
         INSERT INTO stacks VALUES(0,'postmaster','owner',8,10,2);
         INSERT INTO stacks VALUES(1,'postmaster','owner',8,10,2);
         INSERT INTO stacks VALUES(2,'profile_currencies',NULL,9,10,0);
         INSERT INTO inventory_coverage VALUES(1,'available');
         INSERT INTO sync_metadata VALUES('profile_inventory','100','profile','4','v1');",
        )
        .unwrap();
    let schema = |store: &SqliteStore| {
        store
            .connection()
            .prepare("SELECT type,name,sql FROM sqlite_schema ORDER BY type,name")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let check_fks = |store: &SqliteStore| {
        assert_eq!(
            store
                .connection()
                .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            !store
                .connection()
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    };
    let legacy_rows = bridge_rows(&store);
    let legacy_schema = schema(&store);
    check_fks(&store);
    store.migrate_to(18).unwrap();
    assert_eq!(store.schema_version().unwrap(), 18);
    check_fks(&store);
    let mut migrated = bridge_rows(&store);
    for (table, rows) in &mut migrated {
        if table == "definitions" {
            for row in rows {
                let fields = row.split_off(row.len() - 4);
                assert_eq!(fields, vec![Value::Null; 4]);
            }
        }
    }
    assert_eq!(migrated, legacy_rows);
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).unwrap(),
        Resolution::Unique(prior.definitions.definitions[0].clone())
    );
    // No schema objects except the additive definition declaration change.
    let other_schema = |rows: Vec<(String, String, Option<String>)>| {
        rows.into_iter()
            .filter(|(_, name, _)| name != "definitions")
            .collect::<Vec<_>>()
    };
    assert_eq!(
        other_schema(schema(&store)),
        other_schema(legacy_schema.clone())
    );
    for (column, value) in [
        ("does_postmaster_pull_have_side_effects", Value::Integer(0)),
        ("does_postmaster_pull_have_side_effects", Value::Integer(1)),
        ("is_instance_item", Value::Integer(0)),
        ("is_instance_item", Value::Integer(1)),
        ("max_stack_size", Value::Integer(i64::from(i32::MIN))),
        ("max_stack_size", Value::Integer(-1)),
        ("max_stack_size", Value::Integer(0)),
        ("max_stack_size", Value::Integer(i64::from(i32::MAX))),
        ("stack_unique_label", Value::Text(String::new())),
        ("stack_unique_label", Value::Text(" \0\n ".into())),
    ] {
        store
            .connection()
            .execute(
                &format!("UPDATE definitions SET {column}=?1 WHERE hash=10"),
                [value],
            )
            .unwrap();
        let rows_before = bridge_rows(&store);
        let schema_before = schema(&store);
        assert_eq!(store.migrate_to(17), Err(storage_error()));
        assert_eq!(store.schema_version().unwrap(), 18);
        assert_eq!(bridge_rows(&store), rows_before);
        assert_eq!(schema(&store), schema_before);
        check_fks(&store);
        store
            .connection()
            .execute(
                &format!("UPDATE definitions SET {column}=NULL WHERE hash=10"),
                [],
            )
            .unwrap();
    }
    store.migrate_to(17).unwrap();
    assert_eq!(store.schema_version().unwrap(), 17);
    assert_eq!(bridge_rows(&store), legacy_rows);
    assert_eq!(schema(&store), legacy_schema);
    check_fks(&store);
    // The v17 coverage/zero guard remains authoritative after a safe v18 down.
    assert_eq!(store.migrate_to(16), Err(storage_error()));
    assert_eq!(store.schema_version().unwrap(), 17);
    assert_eq!(bridge_rows(&store), legacy_rows);
    store.migrate_to(18).unwrap();
    check_fks(&store);
}

fn bridge_world() -> destiny_core::WorldDefinitionsAcquisition {
    use destiny_core::*;
    fn table<T>(definition: T) -> WorldDefinitionTable<T> {
        WorldDefinitionTable {
            definitions: vec![definition],
            skipped_count: 0,
        }
    }
    WorldDefinitionsAcquisition {
        version: DefinitionVersion::new("v1"),
        item_descriptions: vec![
            ItemDescription::new(ItemHash::new(10), "Original tooltip".into()).unwrap(),
        ],
        milestones: table(
            MilestoneDefinition::new(1, "Milestone".into(), "Description".into(), 3).unwrap(),
        ),
        activities: table(
            ActivityDefinition::new(2, "Activity".into(), "".into(), 4, 2000).unwrap(),
        ),
        modifiers: table(ActivityModifierDefinition::new(3, "Modifier".into(), "".into()).unwrap()),
        objectives: table(ObjectiveDefinition::new(5, "Objective".into(), 1).unwrap()),
        activity_types: table(ActivityTypeDefinition::new(4, "Type".into(), "".into()).unwrap()),
        stats: table(StatDefinition::new(6, "Stat".into()).unwrap()),
        loadout_names: WorldDefinitionTable {
            definitions: vec![],
            skipped_count: 0,
        },
    }
}

// Capture every row of the synthetic in-memory database, including inventory,
// tags, descriptions, world tables and both datasets' freshness/revision rows.
fn bridge_rows(store: &SqliteStore) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    let tables: Vec<String> = store.connection().prepare(
        "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    ).unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    tables
        .into_iter()
        .map(|table| {
            let mut statement = store
                .connection()
                .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"))
                .unwrap();
            let columns = statement.column_count();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<Result<Vec<rusqlite::types::Value>, _>>()
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

fn storage_error() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

fn item(hash: i64, name: &str) -> ItemDefinition {
    ItemDefinition {
        non_transferrable: None,
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
        plug_category: None,
        tags: Vec::new(),
        version: DefinitionVersion::new("v1"),
    }
}

fn versioned(mut definition: ItemDefinition, version: &str) -> ItemDefinition {
    definition.version = DefinitionVersion::new(version);
    definition
}

fn full_definition(hash: i64, tags: &[&str]) -> ItemDefinition {
    ItemDefinition {
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        bucket_hash: Some(BucketHash::new(1_498_876_634)),
        name: "Suros Regime".into(),
        item_type: Some("Auto Rifle".into()),
        tier: Some("Exotic".into()),
        element: Some("Solar".into()),
        slot: None,
        class_type: None,
        plug_category: None,
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        version: DefinitionVersion::new("manifest-2026-09-21"),
    }
}

fn replacement(version: &str, definitions: Vec<ItemDefinition>) -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions,
        },
        checked_at: Timestamp::new(200),
        source: SourceMarker::Manifest,
    }
}

fn base_manifest() -> ManifestDefinitions {
    ManifestDefinitions {
        version: DefinitionVersion::new("v1"),
        definitions: vec![
            ItemDefinition {
                non_transferrable: None,
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: ItemHash::new(10),
                bucket_hash: Some(BucketHash::new(100)),
                name: "Alpha".into(),
                item_type: Some("Auto Rifle".into()),
                tier: Some("Legendary".into()),
                element: Some("Void".into()),
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec!["first".into(), "second".into()],
                version: DefinitionVersion::new("v1"),
            },
            ItemDefinition {
                non_transferrable: None,
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: ItemHash::new(20),
                bucket_hash: None,
                name: "Beta".into(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: Vec::new(),
                version: DefinitionVersion::new("v1"),
            },
        ],
    }
}

fn definition_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM definitions", [], |row| row.get(0))
        .expect("count definitions")
}

fn tag_count(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM definition_tags", [], |row| row.get(0))
        .expect("count definition tags")
}

#[test]
fn plug_category_round_trips_and_changes_revision() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut definition = item(1, "Trait");
    definition.plug_category = Some("frames".into());
    store
        .replace_definitions(replacement("v1", vec![definition.clone()]))
        .unwrap();
    assert_eq!(
        store.lookup_definition(definition.hash).unwrap(),
        Resolution::Unique(definition.clone())
    );
    assert_eq!(
        store
            .replace_definitions(replacement("v1", vec![definition.clone()]))
            .unwrap()
            .revision
            .value(),
        1
    );
    definition.plug_category = Some("origins".into());
    assert_eq!(
        store
            .replace_definitions(replacement("v1", vec![definition]))
            .unwrap()
            .revision
            .value(),
        2
    );
}

#[test]
fn plug_category_invalid_publication_rolls_back() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let prior = item(1, "Trait");
    store
        .replace_definitions(replacement("v1", vec![prior.clone()]))
        .unwrap();
    for category in ["".to_owned(), "x".repeat(257), "frames\0".to_owned()] {
        let mut invalid = prior.clone();
        invalid.plug_category = Some(category);
        assert!(
            store
                .replace_definitions(replacement("v1", vec![invalid]))
                .is_err()
        );
        assert_eq!(
            store.lookup_definition(prior.hash).unwrap(),
            Resolution::Unique(prior.clone())
        );
    }
}

#[test]
fn empty_store_reads_match_fake_semantics() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    assert_eq!(store.current_metadata().expect("read metadata"), None);
    assert_eq!(
        store.current_definition_version(),
        Err(storage_error()),
        "an empty store reports local storage, matching the fake"
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(7)).expect("lookup"),
        Resolution::Missing
    );
}

#[test]
fn first_publication_assigns_revision_one_and_round_trips_exactly() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let alpha = full_definition(-42, &["weapon", "exotic", "search term"]);
    let beta = versioned(item(2_522_695, "Beta"), "manifest-2026-09-21");

    let metadata = store
        .replace_definitions(replacement(
            "manifest-2026-09-21",
            vec![alpha.clone(), beta.clone()],
        ))
        .expect("publish the first complete set");

    assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
    assert_eq!(metadata.source, SourceMarker::Manifest);
    assert_eq!(metadata.revision.value(), 1);
    assert_eq!(metadata.checked_at, Timestamp::new(200));
    assert_eq!(metadata.definition_version.as_str(), "manifest-2026-09-21");

    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(metadata)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "manifest-2026-09-21"
    );
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup alpha"),
        Resolution::Unique(alpha.clone()),
        "every normalized field and ordered tag must round-trip exactly"
    );
    assert_eq!(
        store.lookup_definition(beta.hash).expect("lookup beta"),
        Resolution::Unique(beta.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(9_999))
            .expect("lookup"),
        Resolution::Missing
    );
}

#[test]
fn identical_replacement_retains_revision() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let input = replacement("v1", vec![item(10, "Alpha"), item(20, "Beta")]);

    let first = store
        .replace_definitions(input.clone())
        .expect("publish the first set");
    let second = store
        .replace_definitions(input)
        .expect("republish the same set");

    assert_eq!(first, second);
    assert_eq!(second.revision.value(), 1);
    assert_eq!(definition_count(&store), 2);
}

#[test]
fn definition_order_is_not_content_but_tag_order_is() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["a".into(), "b".into()];
    let beta = item(20, "Beta");

    let first = store
        .replace_definitions(replacement("v1", vec![alpha.clone(), beta.clone()]))
        .expect("publish the first set");
    assert_eq!(first.revision.value(), 1);

    // The schema stores no definition ordinal and reads are ordered by primary
    // key, so identity is the normalized set, not the input order.
    let reordered = store
        .replace_definitions(replacement("v1", vec![beta.clone(), alpha.clone()]))
        .expect("republish the same set in a different order");
    assert_eq!(reordered.revision.value(), 1);

    // Tags carry an ordinal and round-trip in order, so reordering them is a
    // normalized content change.
    let mut retagged = alpha.clone();
    retagged.tags = vec!["b".into(), "a".into()];
    let changed = store
        .replace_definitions(replacement("v1", vec![retagged, beta]))
        .expect("republish with reordered tags");
    assert_eq!(changed.revision.value(), 2);

    match store
        .lookup_definition(ItemHash::new(10))
        .expect("lookup alpha")
    {
        Resolution::Unique(definition) => {
            assert_eq!(definition.tags, vec!["b".to_owned(), "a".to_owned()]);
        }
        other => panic!("expected a unique definition, got {other:?}"),
    }
}

#[test]
fn later_checked_at_retains_revision_and_refreshes_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let base = replacement("v1", vec![item(10, "Alpha")]);

    let first = store.replace_definitions(base.clone()).expect("publish");
    assert_eq!(first.revision.value(), 1);
    assert_eq!(first.checked_at, Timestamp::new(200));

    let mut refreshed = base;
    refreshed.checked_at = Timestamp::new(9_000);
    let second = store.replace_definitions(refreshed).expect("refresh");

    assert_eq!(
        second.revision.value(),
        1,
        "freshness alone must not advance"
    );
    assert_eq!(second.checked_at, Timestamp::new(9_000));
    assert_eq!(second.source, SourceMarker::Manifest);
    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(second)
    );
}

#[test]
fn changing_each_normalized_field_advances_exactly_once() {
    type Mutation = (&'static str, fn(&mut ManifestDefinitions));

    let mutations: [Mutation; 11] = [
        ("bucket_hash", |set| {
            set.definitions[0].bucket_hash = Some(BucketHash::new(101));
        }),
        ("name", |set| {
            set.definitions[0].name = "Alpha Changed".into();
        }),
        ("item_type", |set| {
            set.definitions[0].item_type = None;
        }),
        ("tier", |set| {
            set.definitions[0].tier = None;
        }),
        ("element", |set| {
            set.definitions[0].element = None;
        }),
        ("tag", |set| {
            set.definitions[0].tags[0] = "renamed".into();
        }),
        ("tag order", |set| {
            set.definitions[0].tags.swap(0, 1);
        }),
        ("definition hash", |set| {
            set.definitions[0].hash = ItemHash::new(11);
        }),
        ("added definition", |set| {
            set.definitions.push(item(30, "Gamma"));
        }),
        ("removed definition", |set| {
            set.definitions.pop();
        }),
        ("definition version", |set| {
            set.version = DefinitionVersion::new("v2");
            for definition in &mut set.definitions {
                definition.version = DefinitionVersion::new("v2");
            }
        }),
    ];

    for (field, mutate) in mutations {
        let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
        store
            .replace_definitions(replacement("v1", base_manifest().definitions))
            .expect("publish the base set");

        let mut changed = base_manifest();
        mutate(&mut changed);
        let metadata = store
            .replace_definitions(DefinitionReplacementInput {
                definitions: changed,
                checked_at: Timestamp::new(200),
                source: SourceMarker::Manifest,
            })
            .expect("changed content publishes");

        assert_eq!(
            metadata.revision.value(),
            2,
            "changing {field} must advance the revision exactly once"
        );
        assert_eq!(metadata.checked_at, Timestamp::new(200));
    }
}

#[test]
fn invalid_replacements_are_provider_validation_and_preserve_state() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let committed = store
        .replace_definitions(replacement("v1", vec![item(10, "Alpha")]))
        .expect("seed a committed set");
    assert_eq!(committed.revision.value(), 1);

    let mut wrong_source = replacement("v2", vec![versioned(item(99, "Gamma"), "v2")]);
    wrong_source.source = SourceMarker::Profile;
    let empty = replacement("v2", Vec::new());
    let duplicate = replacement(
        "v2",
        vec![
            versioned(item(20, "Delta"), "v2"),
            versioned(item(20, "Epsilon"), "v2"),
        ],
    );
    let mixed = replacement("v2", vec![item(30, "Zeta")]);

    let candidates = [
        ("invalid source", wrong_source),
        ("empty set", empty),
        ("duplicate hashes", duplicate),
        ("mixed version", mixed),
    ];

    for (label, candidate) in candidates {
        assert_eq!(
            store.replace_definitions(candidate),
            Err(AppError::new(ErrorCategory::ProviderValidation)),
            "{label} must be rejected as provider validation"
        );
        assert_eq!(
            store.current_metadata().expect("read metadata"),
            Some(committed.clone()),
            "{label} must not change metadata"
        );
        assert_eq!(
            store
                .current_definition_version()
                .expect("read version")
                .as_str(),
            "v1",
            "{label} must not change the version"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)).expect("lookup"),
            Resolution::Unique(item(10, "Alpha")),
            "{label} must preserve the committed catalog"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(20)).expect("lookup"),
            Resolution::Missing,
            "{label} must not add rows"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(99)).expect("lookup"),
            Resolution::Missing
        );
    }
}

#[test]
fn overflow_retains_at_the_boundary_and_preserves_state() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let original = replacement("v1", vec![item(10, "Alpha")]);
    store
        .replace_definitions(original.clone())
        .expect("publish the base set");
    store
        .connection_mut()
        .execute(
            "UPDATE sync_metadata SET revision = '18446744073709551615' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("seed the overflow boundary");

    // An unchanged refresh at the boundary retains the revision and refreshes
    // only the committed timestamp.
    let mut refreshed = original.clone();
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store.replace_definitions(refreshed).expect("refresh");
    assert_eq!(retained.revision.value(), u64::MAX);
    assert_eq!(retained.checked_at, Timestamp::new(9_000));

    // A genuine content change at the boundary overflows before any mutation.
    let changed = replacement("v2", vec![versioned(item(10, "Alpha"), "v2")]);
    assert_eq!(store.replace_definitions(changed), Err(storage_error()));

    let metadata = store
        .current_metadata()
        .expect("read metadata")
        .expect("metadata present");
    assert_eq!(metadata.revision.value(), u64::MAX);
    assert_eq!(metadata.checked_at, Timestamp::new(9_000));
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v1"
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(item(10, "Alpha"))
    );
}

#[test]
fn constraint_failure_mid_replacement_rolls_back_catalog_and_metadata() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["kept".into()];
    let committed = store
        .replace_definitions(replacement("v1", vec![alpha, item(20, "Beta")]))
        .expect("seed a committed set");

    // The first definition writes successfully, then a tag that exceeds the
    // schema bound fails the transaction partway through the replacement.
    let overflow_tag = "x".repeat(65);
    let mut alpha_changed = item(10, "Alpha Changed");
    alpha_changed.tags = vec!["kept".into(), overflow_tag];
    let mut beta_changed = item(20, "Beta Changed");
    beta_changed.tags = vec!["ok".into()];
    let failing = replacement(
        "v2",
        vec![
            versioned(alpha_changed, "v2"),
            versioned(beta_changed, "v2"),
        ],
    );

    let error = store
        .replace_definitions(failing)
        .expect_err("a constraint failure must not commit");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(error.to_string(), storage_error().to_string());

    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(committed)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v1"
    );
    let mut expected_alpha = item(10, "Alpha");
    expected_alpha.tags = vec!["kept".into()];
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(expected_alpha)
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(20)).expect("lookup"),
        Resolution::Unique(item(20, "Beta"))
    );
    assert_eq!(definition_count(&store), 2);
    assert_eq!(
        tag_count(&store),
        1,
        "the replacement's tag rows must not survive the rollback"
    );
}

#[test]
fn complete_replacement_removes_stale_definitions_and_tags() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alpha");
    alpha.tags = vec!["a".into(), "b".into()];
    let mut beta = item(20, "Beta");
    beta.tags = vec!["c".into()];
    store
        .replace_definitions(replacement("v1", vec![alpha, beta]))
        .expect("publish the first set");
    assert_eq!(tag_count(&store), 3);

    let gamma = versioned(item(30, "Gamma"), "v2");
    let metadata = store
        .replace_definitions(replacement("v2", vec![gamma.clone()]))
        .expect("publish the replacement");
    assert_eq!(metadata.revision.value(), 2);

    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Missing
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(20)).expect("lookup"),
        Resolution::Missing
    );
    assert_eq!(
        store.lookup_definition(ItemHash::new(30)).expect("lookup"),
        Resolution::Unique(gamma)
    );
    assert_eq!(definition_count(&store), 1);
    assert_eq!(tag_count(&store), 0, "stale tags must be removed");
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "v2"
    );
}

#[test]
fn file_close_and_reopen_preserves_exact_reads_and_revision() {
    let temp = TempDatabase::new();
    let alpha = full_definition(-42, &["weapon", "exotic"]);
    let input = replacement("manifest-2026-09-21", vec![alpha.clone()]);

    let committed = {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store
            .replace_definitions(input.clone())
            .expect("publish to the file store")
    };

    let mut store = SqliteStore::open(temp.path()).expect("reopen file store");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(
        store.current_metadata().expect("read metadata"),
        Some(committed)
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read version")
            .as_str(),
        "manifest-2026-09-21"
    );
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup"),
        Resolution::Unique(alpha.clone())
    );

    // The reconstructed committed content equals the original input, so an
    // unchanged refresh after reopen retains the revision.
    let mut refreshed = input;
    refreshed.checked_at = Timestamp::new(9_000);
    let retained = store.replace_definitions(refreshed).expect("refresh");
    assert_eq!(retained.revision.value(), 1);
    assert_eq!(retained.checked_at, Timestamp::new(9_000));
    assert_eq!(
        store.lookup_definition(alpha.hash).expect("lookup"),
        Resolution::Unique(alpha)
    );
}

#[test]
fn reads_and_failures_are_deterministic_and_sanitized() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let mut alpha = item(10, "Alph\u{e1}");
    alpha.tags = vec!["First".into(), "second".into()];
    let mut beta = item(20, "Beta");
    beta.item_type = Some("Auto Rifle".into());
    store
        .replace_definitions(replacement("v1", vec![beta.clone(), alpha.clone()]))
        .expect("publish the set");

    for _ in 0..3 {
        assert_eq!(
            store
                .current_definition_version()
                .expect("read version")
                .as_str(),
            "v1"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(10)).expect("lookup"),
            Resolution::Unique(alpha.clone()),
            "tag order and exact casing must be stable across reads"
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(20)).expect("lookup"),
            Resolution::Unique(beta.clone())
        );
        assert_eq!(
            store.lookup_definition(ItemHash::new(30)).expect("lookup"),
            Resolution::Missing
        );
    }

    // A sentinel placed in an over-long tag must never reach the sanitized error.
    let sentinel = "provider-secret-sentinel";
    let mut bad = alpha.clone();
    bad.tags = vec![format!("{sentinel}{sentinel}{sentinel}{sentinel}")];
    let error = store
        .replace_definitions(replacement("v1", vec![bad]))
        .expect_err("an over-long tag is rejected");
    assert_eq!(error, storage_error());
    for leaked in [
        sentinel,
        "sqlite",
        "constraint",
        "definitions",
        "definition_tags",
        "CHECK",
        "tag",
    ] {
        assert!(
            !error.to_string().contains(leaked),
            "error leaked {leaked}: {error}"
        );
    }
    assert!(!format!("{error:?}").contains(sentinel));
    assert_eq!(
        store.lookup_definition(ItemHash::new(10)).expect("lookup"),
        Resolution::Unique(alpha)
    );

    // No stored column is a generic payload column.
    for table in ["definitions", "definition_tags", "sync_metadata"] {
        let mut statement = store
            .connection()
            .prepare(&format!(
                "SELECT name, type FROM pragma_table_info('{table}')"
            ))
            .expect("prepare column query");
        let columns: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("run column query")
            .map(|row| row.expect("read column row"))
            .collect();
        assert!(!columns.is_empty());
        for (column, column_type) in columns {
            assert!(
                column_type == "TEXT" || column_type == "INTEGER",
                "{table}.{column} must not be a raw payload column"
            );
        }
    }
}

#[test]
fn corrupted_metadata_fails_closed_without_weakening_the_schema() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .replace_definitions(replacement("v1", vec![item(10, "Alpha")]))
        .expect("seed a committed set");

    // `ignore_check_constraints` simulates on-disk corruption for this test
    // only. The committed schema still carries its canonical-decimal CHECK
    // constraints, so a real write cannot produce these values; the
    // repository's own parse must therefore fail closed on them.
    store
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable the in-test constraint bypass");

    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = 'not-a-number' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("write a malformed revision");
    assert_eq!(
        store
            .current_metadata()
            .expect_err("a malformed revision must fail closed"),
        storage_error()
    );

    store
        .connection()
        .execute(
            "UPDATE sync_metadata SET revision = '1', checked_at = '01' \
             WHERE dataset = 'manifest_definitions'",
            [],
        )
        .expect("write a non-canonical timestamp");
    assert_eq!(
        store
            .current_definition_version()
            .expect_err("a non-canonical timestamp must fail closed"),
        storage_error()
    );
}
