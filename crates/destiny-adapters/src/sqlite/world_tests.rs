use super::{SqliteStore, migrations};
use destiny_app::test_support::InMemoryDefinitionRepository;
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;

fn world(version: &str, name: &str) -> WorldDefinitionsAcquisition {
    WorldDefinitionsAcquisition {
        version: DefinitionVersion::new(version),
        objectives: WorldDefinitionTable {
            definitions: vec![ObjectiveDefinition::new(5, name.into(), 1).unwrap()],
            skipped_count: 5,
        },
        activity_types: WorldDefinitionTable {
            definitions: vec![ActivityTypeDefinition::new(4, name.into(), "".into()).unwrap()],
            skipped_count: 6,
        },
        milestones: WorldDefinitionTable {
            definitions: vec![
                MilestoneDefinition::new(1, name.into(), "description".into(), 3).unwrap(),
            ],
            skipped_count: 2,
        },
        activities: WorldDefinitionTable {
            definitions: vec![ActivityDefinition::new(2, name.into(), "".into(), 4, 2000).unwrap()],
            skipped_count: 3,
        },
        modifiers: WorldDefinitionTable {
            definitions: vec![ActivityModifierDefinition::new(3, name.into(), "".into()).unwrap()],
            skipped_count: 4,
        },
    }
}

fn items(version: &str, name: &str) -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions: vec![ItemDefinition {
                hash: ItemHash::new(10),
                bucket_hash: None,
                name: name.into(),
                item_type: None,
                tier: None,
                element: None,
                tags: vec![],
                version: DefinitionVersion::new(version),
            }],
        },
        checked_at: Timestamp::new(100),
        source: SourceMarker::Manifest,
    }
}

#[test]
fn same_version_without_world_catalog_downloads_then_skips_with_fake_sqlite_parity() {
    use destiny_app::test_support::QueuedManifestGateway;
    use destiny_core::ports::DefinitionCatalogRepository;

    fn exercise<R: DefinitionCatalogRepository + WorldDefinitionRepository>(repository: R) {
        assert!(repository.world_definition_counts().unwrap().contains(&0));
        let mut gateway = QueuedManifestGateway::new();
        for _ in 0..2 {
            gateway.queue(Ok(ManifestAcquisition {
                version: DefinitionVersion::new("v1"),
                definitions: vec![ManifestDefinitionCandidate {
                    hash: ItemHash::new(10),
                    bucket_hash: None,
                    name: "published".into(),
                    item_type: None,
                    tier: None,
                    element: None,
                    tags: vec![],
                    version: DefinitionVersion::new("v1"),
                }],
                skipped_count: 0,
            }));
        }
        let mut service = destiny_app::ManifestService::new(gateway, repository);
        assert!(matches!(
            service.sync(Timestamp::new(200)).unwrap(),
            destiny_app::ManifestSyncReport::Updated {
                world_counts: [1, 1, 1, 1, 1],
                ..
            }
        ));
        assert_eq!(service.gateway().component_calls(), 1);
        assert_eq!(service.gateway().world_calls(), 1);
        assert!(service.repository().lookup_milestone(1).unwrap().is_some());
        assert!(service.repository().lookup_activity(2).unwrap().is_some());
        assert!(
            service
                .repository()
                .lookup_activity_modifier(3)
                .unwrap()
                .is_some()
        );
        let Resolution::Unique(item) = service
            .repository()
            .lookup_definition(ItemHash::new(10))
            .unwrap()
        else {
            panic!("item missing")
        };
        assert_eq!(item.name, "published");
        let metadata = service.repository().current_metadata().unwrap();
        assert!(matches!(
            service.sync(Timestamp::new(300)).unwrap(),
            destiny_app::ManifestSyncReport::Unchanged {
                world_counts: [1, 1, 1, 1, 1],
                ..
            }
        ));
        assert_eq!(service.gateway().component_calls(), 1);
        assert_eq!(service.gateway().world_calls(), 1);
        assert_eq!(service.repository().current_metadata().unwrap(), metadata);
    }

    let mut sqlite = SqliteStore::open_in_memory().unwrap();
    sqlite.migrate_to(2).unwrap();
    sqlite.replace_definitions(items("v1", "prior")).unwrap();
    sqlite.migrate_to(4).unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    fake.replace_definitions(items("v1", "prior")).unwrap();
    exercise(sqlite);
    exercise(fake);
    for table in [
        "milestone_definitions",
        "activity_definitions",
        "activity_modifier_definitions",
        "objective_definitions",
        "activity_type_definitions",
    ] {
        let mut sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite
            .publish_manifest(items("v1", "prior"), world("v1", "prior"))
            .unwrap();
        sqlite
            .connection()
            .execute(&format!("DELETE FROM {table}"), [])
            .unwrap();
        exercise(sqlite);
    }
}

fn schema(store: &SqliteStore) -> Vec<(String, String)> {
    store
        .connection()
        .prepare("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL ORDER BY name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn populated_v3_round_trip_restores_exact_schema_and_data() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(3).unwrap();
    store
        .connection()
        .execute_batch(
            "INSERT INTO definitions VALUES (10, NULL, 'item', NULL, NULL, NULL, 'v3');
        INSERT INTO milestone_definitions VALUES (1, 'milestone', 'description', 3, 'v3');
        INSERT INTO activity_definitions VALUES (2, 'activity', '', 4, -10, 'v3');
        INSERT INTO activity_modifier_definitions VALUES (3, 'modifier', '', 'v3');
        INSERT INTO sync_metadata VALUES ('manifest_definitions', '100', 'manifest', '2', 'v3');",
        )
        .unwrap();
    let before = schema(&store);
    let metadata = store.current_metadata().unwrap();
    let milestone = store.lookup_milestone(1).unwrap();
    let activity = store.lookup_activity(2).unwrap();
    let modifier = store.lookup_activity_modifier(3).unwrap();
    let item = store.lookup_definition(ItemHash::new(10)).unwrap();
    store.migrate_to(4).unwrap();
    assert_eq!(store.world_definition_counts().unwrap(), [1, 1, 1, 0, 0]);
    assert_eq!(store.lookup_milestone(1).unwrap(), milestone);
    assert_eq!(store.lookup_activity(2).unwrap(), activity);
    assert_eq!(store.lookup_activity_modifier(3).unwrap(), modifier);
    assert_eq!(store.lookup_definition(ItemHash::new(10)).unwrap(), item);
    assert_eq!(store.current_metadata().unwrap(), metadata);
    store
        .connection()
        .execute_batch(
            "INSERT INTO objective_definitions VALUES (5, '', -1, 'v3');
        INSERT INTO activity_type_definitions VALUES (4, 'type', '', 'v3');",
        )
        .unwrap();
    store.migrate_to(3).unwrap();
    assert_eq!(schema(&store), before);
    assert_eq!(store.lookup_milestone(1).unwrap(), milestone);
    assert_eq!(store.lookup_activity(2).unwrap(), activity);
    assert_eq!(store.lookup_activity_modifier(3).unwrap(), modifier);
    assert_eq!(store.lookup_definition(ItemHash::new(10)).unwrap(), item);
    assert_eq!(store.current_metadata().unwrap(), metadata);
    store
        .connection()
        .pragma_update(None, "user_version", 5)
        .unwrap();
    assert!(store.migrate_to(4).is_err());
    assert_eq!(schema(&store), before);
}

#[test]
fn populated_v2_round_trip_restores_exact_schema_and_data() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(2).unwrap();
    store.replace_definitions(items("v2", "prior")).unwrap();
    store
        .connection()
        .execute_batch(
            "INSERT INTO active_profile VALUES (1, 'membership', '3');
         INSERT INTO characters VALUES ('character', NULL, NULL, NULL, NULL, 2000);
         INSERT INTO items VALUES ('instance', 10, 'character', 'character', NULL, 1, 1, 0, 2000);
         INSERT INTO item_perks VALUES ('instance', 0, 4294967295, 1, 0);
         INSERT INTO sync_metadata VALUES ('profile_inventory', '100', 'profile', '1', 'v2');",
        )
        .unwrap();
    let before = schema(&store);
    let metadata = store.current_metadata().unwrap();
    store.migrate_to(3).unwrap();
    assert_eq!(store.schema_version().unwrap(), 3);
    assert!(store.lookup_milestone(1).unwrap().is_none());
    store.migrate_to(2).unwrap();
    assert_eq!(schema(&store), before);
    for table in ["active_profile", "characters", "items", "item_perks"] {
        let count: i64 = store
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
    assert_eq!(store.current_metadata().unwrap(), metadata);
    assert!(matches!(
        store.lookup_definition(ItemHash::new(10)).unwrap(),
        Resolution::Unique(_)
    ));
    store
        .connection()
        .pragma_update(None, "user_version", 5)
        .unwrap();
    assert!(migrations::migrate_to(store.connection_mut(), 3).is_err());
}

#[test]
fn combined_publish_rolls_back_every_table_and_metadata_on_mid_publish_failure() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let prior = store
        .publish_manifest(items("v1", "prior"), world("v1", "prior"))
        .unwrap();
    store.connection().execute_batch("CREATE TRIGGER fail_world BEFORE INSERT ON activity_type_definitions BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(
        store
            .publish_manifest(items("v2", "next"), world("v2", "next"))
            .is_err()
    );
    assert_eq!(store.current_metadata().unwrap(), Some(prior));
    assert_eq!(store.lookup_milestone(1).unwrap().unwrap().name(), "prior");
    assert_eq!(store.lookup_activity(2).unwrap().unwrap().name(), "prior");
    assert_eq!(
        store.lookup_activity_modifier(3).unwrap().unwrap().name(),
        "prior"
    );
    assert_eq!(
        store
            .lookup_objective(5)
            .unwrap()
            .unwrap()
            .progress_description(),
        "prior"
    );
    assert_eq!(
        store.lookup_activity_type(4).unwrap().unwrap().name(),
        "prior"
    );
    let Resolution::Unique(item) = store.lookup_definition(ItemHash::new(10)).unwrap() else {
        panic!("missing item")
    };
    assert_eq!(item.name, "prior");
}

#[test]
fn fake_and_sqlite_share_world_publication_and_lookup_contract() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let mut fake = InMemoryDefinitionRepository::default();
    for name in ["first", "first", "changed"] {
        assert_eq!(
            store
                .publish_manifest(items("v1", name), world("v1", name))
                .unwrap(),
            fake.publish_manifest(items("v1", name), world("v1", name))
                .unwrap()
        );
        for hash in [1, 2, 3, 4, 5, u32::MAX] {
            assert_eq!(
                store.lookup_objective(hash).unwrap(),
                fake.lookup_objective(hash).unwrap()
            );
            assert_eq!(
                store.lookup_activity_type(hash).unwrap(),
                fake.lookup_activity_type(hash).unwrap()
            );
            assert_eq!(
                store.lookup_milestone(hash).unwrap(),
                fake.lookup_milestone(hash).unwrap()
            );
            assert_eq!(
                store.lookup_activity(hash).unwrap(),
                fake.lookup_activity(hash).unwrap()
            );
            assert_eq!(
                store.lookup_activity_modifier(hash).unwrap(),
                fake.lookup_activity_modifier(hash).unwrap()
            );
        }
    }
    // A world-only change advances exactly once; skip counts and set order do not.
    for name in ["world-only", "world-only"] {
        assert_eq!(
            store
                .publish_manifest(items("v1", "changed"), world("v1", name))
                .unwrap(),
            fake.publish_manifest(items("v1", "changed"), world("v1", name))
                .unwrap()
        );
    }
    assert_eq!(
        store.current_metadata().unwrap().unwrap().revision.value(),
        3
    );
    let prior = store.current_metadata().unwrap();
    let mut duplicate = world("v1", "bad");
    duplicate
        .activities
        .definitions
        .push(duplicate.activities.definitions[0].clone());
    assert_eq!(
        store.publish_manifest(items("v1", "bad"), duplicate.clone()),
        fake.publish_manifest(items("v1", "bad"), duplicate)
    );
    assert_eq!(store.current_metadata().unwrap(), prior);
    assert_eq!(fake.current_metadata().unwrap(), prior);
    for mut invalid in [world("other-version", "bad"), world("v1", "bad")] {
        invalid.modifiers.definitions.clear();
        assert_eq!(
            store.publish_manifest(items("v1", "bad"), invalid.clone()),
            fake.publish_manifest(items("v1", "bad"), invalid)
        );
        assert_eq!(store.current_metadata().unwrap(), prior);
        assert_eq!(fake.current_metadata().unwrap(), prior);
    }
    // Legacy item-only updates clear world rows rather than retaining stale versions.
    assert_eq!(
        store
            .replace_definitions(items("v2", "items-only"))
            .unwrap(),
        fake.replace_definitions(items("v2", "items-only")).unwrap()
    );
    assert_eq!(store.world_definition_counts().unwrap(), [0; 5]);
    assert_eq!(fake.world_definition_counts().unwrap(), [0; 5]);
}

#[test]
fn world_publication_round_trips_signed_light_levels() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    for light_level in [-10, i32::MIN, i32::MAX] {
        let mut definitions = world("v1", "Raid");
        let activity =
            ActivityDefinition::new(2, "Raid".into(), "".into(), 4, light_level).unwrap();
        definitions.activities.definitions = vec![activity.clone()];
        let metadata = store
            .publish_manifest(items("v1", "Raid"), definitions.clone())
            .unwrap();
        assert_eq!(store.lookup_activity(2).unwrap(), Some(activity));
        assert_eq!(
            store
                .publish_manifest(items("v1", "Raid"), definitions)
                .unwrap(),
            metadata
        );
    }
}

#[test]
fn world_schema_enforces_bounds_and_strict_integer_domains() {
    let store = SqliteStore::open_in_memory().unwrap();
    for sql in [
        "INSERT INTO objective_definitions VALUES (-1, '', 1, 'v1')",
        "INSERT INTO objective_definitions VALUES (4294967296, '', 1, 'v1')",
        "INSERT INTO objective_definitions VALUES (1, '', -2147483649, 'v1')",
        "INSERT INTO objective_definitions VALUES (1, '', 2147483648, 'v1')",
        "INSERT INTO objective_definitions VALUES (1, '', 1.5, 'v1')",
        "INSERT INTO objective_definitions VALUES (1, '', 1, '')",
        "INSERT INTO objective_definitions VALUES (1, replace(hex(zeroblob(513)), '00', 'x'), 1, 'v1')",
        "INSERT INTO activity_type_definitions VALUES (1, '', '', 'v1')",
        "INSERT INTO activity_type_definitions VALUES (1, 'name', zeroblob(2049), 'v1')",
        "INSERT INTO activity_type_definitions VALUES (-1, 'name', '', 'v1')",
        "INSERT INTO activity_type_definitions VALUES (1, replace(hex(zeroblob(257)), '00', 'x'), '', 'v1')",
        "INSERT INTO milestone_definitions VALUES (-1, 'name', '', 3, 'v1')",
        "INSERT INTO milestone_definitions VALUES (4294967296, 'name', '', 3, 'v1')",
        "INSERT INTO milestone_definitions VALUES (1, '', '', 3, 'v1')",
        "INSERT INTO milestone_definitions VALUES (1, 'name', '', 3.5, 'v1')",
        "INSERT INTO activity_definitions VALUES (1, 'name', '', 4294967296, 2000, 'v1')",
        "INSERT INTO activity_definitions VALUES (1, 'name', '', 3, -2147483649, 'v1')",
        "INSERT INTO activity_definitions VALUES (1, 'name', '', 3, 2147483648, 'v1')",
        "INSERT INTO activity_definitions VALUES (1, 'name', '', 3, 3.5, 'v1')",
        "INSERT INTO activity_modifier_definitions VALUES (1, 'name', '', '')",
        "INSERT INTO activity_modifier_definitions VALUES (1, 'name', zeroblob(2049), 'v1')",
    ] {
        assert!(store.connection().execute(sql, []).is_err());
    }
    store.connection().execute("INSERT INTO activity_definitions VALUES (4294967295, 'name', '', 4294967295, 2147483647, 'v1')", []).unwrap();
    store
        .connection()
        .execute(
            "INSERT INTO objective_definitions VALUES (4294967295, '', -2147483648, 'v1')",
            [],
        )
        .unwrap();
    let objective = store.lookup_objective(u32::MAX).unwrap().unwrap();
    assert_eq!(objective.progress_description(), "");
    assert_eq!(objective.completion_value(), i32::MIN);
    let activity = store.lookup_activity(u32::MAX).unwrap().unwrap();
    assert_eq!(activity.light_level(), i32::MAX);
}
