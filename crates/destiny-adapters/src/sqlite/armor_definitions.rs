//! Normalized Manifest-only investment and plug metadata, joined by item hash.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, ArmorDefinition, InvestmentStat, ItemDefinition, PlugEnergyCost};
use rusqlite::{Connection, params};
use std::collections::HashMap;

/// Internal table/column literals only; values always remain bound parameters.
pub(super) fn read_sql(table: &str, columns: &str, ordered: bool, hash: Option<i64>) -> String {
    format!(
        "SELECT {columns} FROM {table}{}{}",
        if hash.is_some() {
            " WHERE definition_hash = ?1"
        } else {
            ""
        },
        if ordered {
            " ORDER BY definition_hash, ordinal"
        } else {
            ""
        }
    )
}

pub(super) fn write(
    connection: &Connection,
    definitions: &[ItemDefinition],
) -> Result<(), AppError> {
    if read_user_version(connection)? < 26 {
        return Ok(()); // The publication preflight already refused populated data.
    }
    let mut armor = connection
        .prepare_cached("INSERT INTO definition_armor VALUES (?1,?2,?3,?4,?5)")
        .map_err(|_| storage_failure())?;
    let mut stats = connection
        .prepare_cached("INSERT INTO definition_investment_stats VALUES (?1,?2,?3,?4,?5)")
        .map_err(|_| storage_failure())?;
    let mut costs = connection
        .prepare_cached("INSERT INTO definition_energy_costs VALUES (?1,?2,?3,?4)")
        .map_err(|_| storage_failure())?;
    for item in definitions {
        let data = &item.armor;
        if data.stat_group_hash.is_some()
            || data.plug_category_hash.is_some()
            || data.aspect_capacity.is_some()
            || data.equipable_item_set_hash.is_some()
        {
            armor
                .execute(params![
                    item.hash.value(),
                    data.stat_group_hash,
                    data.plug_category_hash,
                    data.aspect_capacity,
                    data.equipable_item_set_hash
                ])
                .map_err(|_| storage_failure())?;
        }
        for (ordinal, stat) in data.investment_stats.iter().enumerate() {
            stats
                .execute(params![
                    item.hash.value(),
                    ordinal as i64,
                    stat.stat_type_hash,
                    stat.value,
                    stat.is_conditionally_active
                ])
                .map_err(|_| storage_failure())?;
        }
        if let Some(cost) = &data.energy_cost {
            costs
                .execute(params![
                    item.hash.value(),
                    cost.energy_cost,
                    cost.energy_type,
                    cost.energy_type_hash
                ])
                .map_err(|_| storage_failure())?;
        }
    }
    if read_user_version(connection)? >= 32 {
        let mut statement = connection
            .prepare_cached("INSERT INTO definition_material_inputs VALUES (?1,?2,?3,?4)")
            .map_err(|_| storage_failure())?;
        for item in definitions {
            let data = &item.armor;
            if data.insertion_material_requirement_hash.is_some()
                || data.energy_capacity_type.is_some()
                || data.plug_availability.is_some()
            {
                statement
                    .execute(params![
                        item.hash.value(),
                        data.insertion_material_requirement_hash,
                        data.energy_capacity_type,
                        data.plug_availability
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
    }
    super::definition_sockets::write(connection, definitions)?;
    Ok(())
}

/// Bounded queries, not three queries per item during revision comparison.
/// Legacy schemas preserve unknown metadata; no backfill or inferred values.
pub(super) fn read(
    connection: &Connection,
    hash: Option<i64>,
) -> Result<HashMap<i64, ArmorDefinition>, AppError> {
    let mut result: HashMap<i64, ArmorDefinition> = HashMap::new();
    if read_user_version(connection)? < 26 {
        return Ok(result);
    }
    if read_user_version(connection)? >= 32 {
        let mut statement = connection
            .prepare_cached(&read_sql(
                "definition_material_inputs",
                "definition_hash, requirement_hash, capacity_type, availability",
                false,
                hash,
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(hash), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<u32>>(1)?,
                    row.get::<_, Option<u32>>(2)?,
                    row.get::<_, Option<u32>>(3)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, requirement, capacity, availability) = row.map_err(|_| storage_failure())?;
            if requirement.is_none() && capacity.is_none() && availability.is_none() {
                return Err(storage_failure());
            }
            let data = result.entry(hash).or_default();
            data.insertion_material_requirement_hash = requirement;
            data.energy_capacity_type = capacity;
            data.plug_availability = availability;
        }
    }
    let mut statement = connection.prepare_cached(&read_sql("definition_armor", "definition_hash, stat_group_hash, plug_category_hash, aspect_capacity, equipable_item_set_hash", false, hash)).map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(hash), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                ArmorDefinition {
                    stat_group_hash: row.get(1)?,
                    plug_category_hash: row.get(2)?,
                    aspect_capacity: row.get(3)?,
                    equipable_item_set_hash: row.get(4)?,
                    ..Default::default()
                },
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, armor) = row.map_err(|_| storage_failure())?;
        let data = result.entry(hash).or_default();
        data.stat_group_hash = armor.stat_group_hash;
        data.plug_category_hash = armor.plug_category_hash;
        data.aspect_capacity = armor.aspect_capacity;
        data.equipable_item_set_hash = armor.equipable_item_set_hash;
    }
    let mut statement = connection
        .prepare_cached(&read_sql(
            "definition_investment_stats",
            "definition_hash, ordinal, stat_type_hash, value, conditionally_active",
            true,
            hash,
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(hash), |row| {
            let active = match row.get::<_, i64>(4)? {
                0 => false,
                1 => true,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                InvestmentStat {
                    stat_type_hash: row.get(2)?,
                    value: row.get(3)?,
                    is_conditionally_active: active,
                },
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, ordinal, stat) = row.map_err(|_| storage_failure())?;
        let data = result.entry(hash).or_default();
        if ordinal != data.investment_stats.len() as i64 || !(0..64).contains(&ordinal) {
            return Err(storage_failure());
        }
        data.investment_stats.push(stat);
    }
    let mut statement = connection
        .prepare_cached(&read_sql(
            "definition_energy_costs",
            "definition_hash, energy_cost, energy_type, energy_type_hash",
            false,
            hash,
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(hash), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                PlugEnergyCost {
                    energy_cost: row.get(1)?,
                    energy_type: row.get(2)?,
                    energy_type_hash: row.get(3)?,
                },
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, cost) = row.map_err(|_| storage_failure())?;
        result.entry(hash).or_default().energy_cost = Some(cost);
    }
    super::definition_sockets::read(connection, hash, &mut result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStore;
    use destiny_core::ports::WorldDefinitionRepository;
    use destiny_core::{DefinitionRepository, ItemHash, Resolution};

    fn input() -> destiny_core::DefinitionReplacementInput {
        let acquisition = crate::manifest::decode_inventory_items(
            destiny_core::DefinitionVersion::new("v1"),
            br#"{"1":{"hash":1,"displayProperties":{"name":"Armor"},"investmentStats":[{"statTypeHash":2,"value":-5,"isConditionallyActive":false}],"stats":{"statGroupHash":3},"plug":{"energyCost":{"energyCost":0,"energyType":0,"energyTypeHash":0}}}}"#,
        )
        .unwrap();
        destiny_core::DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(acquisition).unwrap(),
            checked_at: destiny_core::Timestamp::new(1),
            source: destiny_core::SourceMarker::Manifest,
        }
    }

    #[test]
    fn single_hash_armor_queries_search_indexes() {
        let store = SqliteStore::open_in_memory().unwrap();
        for table in [
            "definition_armor",
            "definition_investment_stats",
            "definition_energy_costs",
            "definition_socket_entries",
            "definition_socket_plugs",
            "definition_socket_categories",
            "definition_socket_indexes",
        ] {
            let sql = format!(
                "EXPLAIN QUERY PLAN {}",
                read_sql(table, "definition_hash", false, Some(1))
            );
            let plan: String = store
                .connection()
                .query_row(&sql, [1], |row| row.get(3))
                .unwrap();
            assert!(plan.contains("SEARCH"), "{table}: {plan}");
            assert!(!plan.contains("SCAN"), "{table}: {plan}");
        }
    }

    fn helmet() -> destiny_core::DefinitionReplacementInput {
        let acquisition = crate::manifest::decode_inventory_items(
            destiny_core::DefinitionVersion::new("v1"),
            br#"{"1":{"hash":1,"displayProperties":{"name":"Helmet"},"sockets":{"socketEntries":[{"socketTypeHash":0,"singleInitialItemHash":0,"reusablePlugItems":[]},{"socketTypeHash":202,"singleInitialItemHash":302,"reusablePlugSetHash":1155052024,"reusablePlugItems":[{"plugItemHash":402}]},{"socketTypeHash":203,"singleInitialItemHash":303,"randomizedPlugSetHash":503},{"socketTypeHash":204,"singleInitialItemHash":304,"plugSources":1,"defaultVisible":true}],"socketCategories":[{"socketCategoryHash":601,"socketIndexes":[1]},{"socketCategoryHash":602,"socketIndexes":[2,3]}]}}}"#,
        ).unwrap();
        destiny_core::DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(acquisition).unwrap(),
            checked_at: destiny_core::Timestamp::new(1),
            source: destiny_core::SourceMarker::Manifest,
        }
    }

    #[test]
    fn manifest_helmet_keeps_definition_socket_positions() {
        let input = helmet();
        let armor = &input.definitions.definitions[0].armor;
        assert_eq!(armor.socket_entries.len(), 4);
        assert_eq!(armor.socket_entries[0].socket_type_hash, 0);
        assert_eq!(armor.socket_entries[1].socket_type_hash, 202);
        assert_eq!(armor.socket_entries[1].single_initial_item_hash, 302);
        assert_eq!(
            armor.socket_entries[1].reusable_plug_set_hash,
            Some(1155052024)
        );
        assert_eq!(armor.socket_entries[1].reusable_plug_items, vec![402]);
        assert_eq!(armor.socket_entries[2].randomized_plug_set_hash, Some(503));
        assert_eq!(armor.socket_entries[3].plug_sources, Some(1));
        assert_eq!(armor.socket_entries[3].default_visible, Some(true));
        assert_eq!(armor.socket_entries[0].default_visible, None);
        assert_eq!(armor.socket_categories[1].socket_indexes, vec![2, 3]);
        let mut sqlite = SqliteStore::open_in_memory().unwrap();
        sqlite.migrate_to(27).unwrap();
        let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::default();
        let first = sqlite.replace_definitions(input.clone()).unwrap();
        assert_eq!(fake.replace_definitions(input.clone()).unwrap(), first);
        assert_eq!(
            sqlite.lookup_definition(ItemHash::new(1)).unwrap(),
            fake.lookup_definition(ItemHash::new(1)).unwrap()
        );
        assert_eq!(sqlite.replace_definitions(input.clone()).unwrap(), first);
        let mut changed = input.clone();
        changed.definitions.definitions[0].armor.socket_entries[0].default_visible = Some(false);
        let next = sqlite.replace_definitions(changed.clone()).unwrap();
        assert_ne!(next, first);
        assert_eq!(fake.replace_definitions(changed).unwrap(), next);
        assert!(sqlite.migrate_to(26).is_err());
        assert_eq!(sqlite.current_metadata().unwrap(), Some(next));
    }

    #[test]
    fn every_socket_field_participates_in_revision_identity() {
        for field in 0..9 {
            let input = helmet();
            let mut changed = input.clone();
            let armor = &mut changed.definitions.definitions[0].armor;
            match field {
                0 => armor.socket_entries[0].socket_type_hash = 1,
                1 => armor.socket_entries[0].single_initial_item_hash = 1,
                2 => armor.socket_entries[0].reusable_plug_set_hash = Some(0),
                3 => armor.socket_entries[0].randomized_plug_set_hash = Some(0),
                4 => armor.socket_entries[0].reusable_plug_items = vec![0, u32::MAX, 0],
                5 => armor.socket_entries[0].plug_sources = Some(0),
                6 => armor.socket_entries[0].default_visible = Some(false),
                7 => armor.socket_categories[0].socket_category_hash = u32::MAX,
                _ => armor.socket_categories[0].socket_indexes = vec![3, 0, 3],
            }
            let mut sqlite = SqliteStore::open_in_memory().unwrap();
            let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::default();
            let first = sqlite.replace_definitions(input.clone()).unwrap();
            fake.replace_definitions(input).unwrap();
            let next = sqlite.replace_definitions(changed.clone()).unwrap();
            assert_ne!(next, first, "field {field}");
            assert_eq!(fake.replace_definitions(changed.clone()).unwrap(), next);
            assert_eq!(sqlite.replace_definitions(changed.clone()).unwrap(), next);
            assert_eq!(
                sqlite.lookup_definition(ItemHash::new(1)).unwrap(),
                Resolution::Unique(changed.definitions.definitions[0].clone())
            );
        }
    }

    #[test]
    fn malformed_socket_projection_and_inclusive_bounds() {
        for sockets in [
            serde_json::json!({"socketEntries":[{"socketTypeHash":-1,"singleInitialItemHash":0}],"socketCategories":[]}),
            serde_json::json!({"socketEntries":[{"socketTypeHash":0,"singleInitialItemHash":0,"reusablePlugItems":[{"plugItemHash":"bad"}]}],"socketCategories":[]}),
            serde_json::json!({"socketEntries":[],"socketCategories":[{"socketCategoryHash":0,"socketIndexes":[0]}]}),
        ] {
            let raw = serde_json::json!({"1":{"hash":1,"displayProperties":{"name":"Helmet"},"sockets":sockets}});
            assert!(
                crate::manifest::decode_inventory_items(
                    destiny_core::DefinitionVersion::new("v1"),
                    &serde_json::to_vec(&raw).unwrap()
                )
                .is_err()
            );
        }
        let mut input = helmet();
        let armor = &mut input.definitions.definitions[0].armor;
        armor.socket_entries.resize(32, Default::default());
        armor.socket_entries[0].reusable_plug_items = vec![u32::MAX; 1024];
        armor
            .socket_categories
            .resize(32, armor.socket_categories[0].clone());
        armor.socket_categories[0].socket_indexes = vec![31; 32];
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.replace_definitions(input.clone()).unwrap();
        assert_eq!(
            store.lookup_definition(ItemHash::new(1)).unwrap(),
            Resolution::Unique(input.definitions.definitions[0].clone())
        );
        let mut shrunk = input;
        shrunk.definitions.definitions[0].armor = Default::default();
        store.replace_definitions(shrunk.clone()).unwrap();
        assert_eq!(
            store.lookup_definition(ItemHash::new(1)).unwrap(),
            Resolution::Unique(shrunk.definitions.definitions[0].clone())
        );
        store.migrate_to(26).unwrap();
    }

    #[test]
    fn socket_corruption_and_failed_writes_are_atomic() {
        for mutation in [
            "UPDATE definition_socket_entries SET default_visible=2",
            "UPDATE definition_socket_entries SET socket_type_hash=-1",
            "UPDATE definition_socket_plugs SET ordinal=1",
            "UPDATE definition_socket_categories SET ordinal=2 WHERE ordinal=0",
            "UPDATE definition_socket_indexes SET ordinal=2 WHERE ordinal=0",
        ] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let input = helmet();
            store.replace_definitions(input.clone()).unwrap();
            store
                .connection()
                .pragma_update(None, "foreign_keys", 0)
                .unwrap();
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", 1)
                .unwrap();
            store.connection().execute(mutation, []).unwrap();
            assert_eq!(
                store.lookup_definition(ItemHash::new(1)),
                Err(storage_failure()),
                "{mutation}"
            );
            assert_eq!(
                store.replace_definitions(input),
                Err(storage_failure()),
                "{mutation}"
            );
        }
        for table in [
            "definition_socket_entries",
            "definition_socket_plugs",
            "definition_socket_categories",
            "definition_socket_indexes",
        ] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let input = helmet();
            let metadata = store.replace_definitions(input.clone()).unwrap();
            store.connection().execute_batch(&format!("CREATE TRIGGER refuse BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'test'); END;")).unwrap();
            let mut changed = input.clone();
            changed.definitions.definitions[0].name.push_str(" changed");
            assert_eq!(store.replace_definitions(changed), Err(storage_failure()));
            assert_eq!(store.current_metadata().unwrap(), Some(metadata));
            assert_eq!(
                store.lookup_definition(ItemHash::new(1)).unwrap(),
                Resolution::Unique(input.definitions.definitions[0].clone())
            );
        }
    }

    #[test]
    fn socket_bounds_and_references_have_fake_sqlite_parity() {
        for field in 0..5 {
            let mut input = helmet();
            let armor = &mut input.definitions.definitions[0].armor;
            match field {
                0 => armor.socket_entries.resize(33, Default::default()),
                1 => armor.socket_entries[1].reusable_plug_items.resize(1025, 0),
                2 => armor
                    .socket_categories
                    .resize(33, armor.socket_categories[0].clone()),
                3 => armor.socket_categories[0].socket_indexes.resize(33, 0),
                _ => armor.socket_categories[0].socket_indexes.push(4),
            }
            let mut sqlite = SqliteStore::open_in_memory().unwrap();
            let mut fake = destiny_app::test_support::InMemoryDefinitionRepository::default();
            assert!(sqlite.replace_definitions(input.clone()).is_err());
            assert!(fake.replace_definitions(input).is_err());
        }
        let mut legacy = SqliteStore::open_in_memory().unwrap();
        legacy.migrate_to(26).unwrap();
        assert_eq!(legacy.replace_definitions(helmet()), Err(storage_failure()));
    }

    #[test]
    fn socket_upgrade_forces_same_version_refresh_once() {
        use destiny_app::{
            ManifestService,
            test_support::{QueuedManifestGateway, synthetic_world},
        };
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.migrate_to(26).unwrap();
        store
            .publish_manifest(
                input(),
                synthetic_world(destiny_core::DefinitionVersion::new("v1")),
            )
            .unwrap();
        assert!(store.current_metadata().unwrap().is_some());
        store.migrate_to(27).unwrap();
        assert!(store.current_metadata().unwrap().is_none());
        let mut gateway = QueuedManifestGateway::new();
        for _ in 0..2 {
            gateway.queue(Ok(crate::manifest::decode_inventory_items(
                destiny_core::DefinitionVersion::new("v1"),
                br#"{"1":{"hash":1,"displayProperties":{"name":"Helmet"}}}"#,
            )
            .unwrap()));
        }
        let mut service = ManifestService::new(gateway, store);
        assert!(matches!(
            service.sync(destiny_core::Timestamp::new(2)).unwrap(),
            destiny_app::ManifestSyncReport::Updated { .. }
        ));
        assert!(matches!(
            service.sync(destiny_core::Timestamp::new(3)).unwrap(),
            destiny_app::ManifestSyncReport::Unchanged { .. }
        ));
        assert_eq!(service.gateway().component_calls(), 1);
        assert_eq!(service.gateway().world_calls(), 1);
    }

    #[test]
    fn corrupt_armor_fails_closed_in_lookup_and_revision_comparison() {
        for mutation in [
            "UPDATE definition_investment_stats SET ordinal=1",
            "UPDATE definition_investment_stats SET conditionally_active=2",
            "UPDATE definition_armor SET stat_group_hash=-1",
            "UPDATE definition_energy_costs SET energy_type_hash=4294967296",
        ] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let input = input();
            store.replace_definitions(input.clone()).unwrap();
            store
                .connection()
                .pragma_update(None, "ignore_check_constraints", 1)
                .unwrap();
            store.connection().execute(mutation, []).unwrap();
            assert_eq!(
                store.lookup_definition(ItemHash::new(1)),
                Err(storage_failure())
            );
            assert_eq!(store.replace_definitions(input), Err(storage_failure()));
        }
    }

    #[test]
    fn mid_write_failure_keeps_every_prior_catalog_and_metadata() {
        for table in [
            "definition_armor",
            "definition_investment_stats",
            "definition_energy_costs",
        ] {
            let mut store = SqliteStore::open_in_memory().unwrap();
            let input = input();
            let metadata = store.replace_definitions(input.clone()).unwrap();
            store.connection().execute_batch(&format!("CREATE TRIGGER refuse BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'test'); END;")).unwrap();
            let mut changed = input.clone();
            changed.definitions.definitions[0].name = "Changed".into();
            assert_eq!(store.replace_definitions(changed), Err(storage_failure()));
            assert_eq!(store.current_metadata().unwrap(), Some(metadata));
            assert_eq!(
                store.lookup_definition(ItemHash::new(1)).unwrap(),
                Resolution::Unique(input.definitions.definitions[0].clone())
            );
        }
    }
}
