//! Nameless Armor 3.0 `armor_stats` plugs that cannot live in `definitions`.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, InvestmentStat, armor_catalog::*};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeMap;

pub(super) fn write(
    connection: &Connection,
    plug: &StatPlugDefinition,
    version: &str,
) -> Result<(), AppError> {
    connection
        .prepare_cached("INSERT INTO definition_stat_plugs VALUES (?1,?2,?3)")
        .map_err(|_| storage_failure())?
        .execute(params![plug.hash, version, plug.plug_category_hash])
        .map_err(|_| storage_failure())?;
    let mut statement = connection
        .prepare_cached("INSERT INTO definition_stat_plug_stats VALUES (?1,?2,?3,?4,?5)")
        .map_err(|_| storage_failure())?;
    for (ordinal, stat) in plug.investment_stats.iter().enumerate() {
        statement
            .execute(params![
                plug.hash,
                ordinal as i64,
                stat.stat_type_hash,
                stat.value,
                stat.is_conditionally_active
            ])
            .map_err(|_| storage_failure())?;
    }
    Ok(())
}

pub(super) fn read(
    connection: &Connection,
    hash: Option<u32>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    if read_user_version(connection)? < 36 {
        return Ok(Vec::new());
    }
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let (plug_filter, stat_filter) = if hash.is_some() {
        (" WHERE hash = ?1", " WHERE plug_hash = ?1")
    } else {
        ("", "")
    };
    let mut plugs = BTreeMap::new();
    let mut statement = connection
        .prepare_cached(&format!(
            "SELECT hash, definition_version, plug_category_hash FROM definition_stat_plugs{plug_filter} ORDER BY hash"
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, version, plug_category_hash) = row.map_err(|_| storage_failure())?;
        if !metadata
            .as_ref()
            .is_some_and(|m| m.definition_version.as_str() == version)
            || plugs
                .insert(
                    hash,
                    StatPlugDefinition {
                        hash,
                        plug_category_hash,
                        investment_stats: Vec::new(),
                    },
                )
                .is_some()
            || plugs.len() > ArmorCatalogKind::StatPlug.max_records()
        {
            return Err(storage_failure());
        }
    }
    let mut statement = connection
        .prepare_cached(&format!(
            "SELECT plug_hash, ordinal, stat_type_hash, value, is_conditionally_active FROM definition_stat_plug_stats{stat_filter} ORDER BY plug_hash, ordinal"
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)? as usize,
                InvestmentStat {
                    stat_type_hash: row.get(2)?,
                    value: row.get(3)?,
                    is_conditionally_active: row.get(4)?,
                },
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, ordinal, stat) = row.map_err(|_| storage_failure())?;
        let Some(plug) = plugs.get_mut(&hash) else {
            return Err(storage_failure());
        };
        if ordinal != plug.investment_stats.len() {
            return Err(storage_failure());
        }
        plug.investment_stats.push(stat);
    }
    let records: Vec<_> = plugs
        .into_values()
        .map(ArmorCatalogRecord::StatPlug)
        .collect();
    validate_catalogs(&records).map_err(|_| storage_failure())?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::super::SqliteStore;
    use destiny_core::armor_catalog::*;
    use destiny_core::ports::{DefinitionRepository, WorldDefinitionRepository};
    use destiny_core::{
        DefinitionReplacementInput, DefinitionVersion, InvestmentStat, SourceMarker, Timestamp,
    };

    fn items(version: &str) -> DefinitionReplacementInput {
        DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(
                crate::manifest::decode_inventory_items(
                    DefinitionVersion::new(version),
                    br#"{"1":{"hash":1,"displayProperties":{"name":"Synthetic helmet"}}}"#,
                )
                .unwrap(),
            )
            .unwrap(),
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        }
    }
    fn plug(hash: u32, stat: u32, value: i32, conditional: bool) -> ArmorCatalogRecord {
        ArmorCatalogRecord::StatPlug(StatPlugDefinition {
            hash,
            plug_category_hash: 748854354,
            investment_stats: vec![InvestmentStat {
                stat_type_hash: stat,
                value,
                is_conditionally_active: conditional,
            }],
        })
    }
    fn world(
        version: &str,
        plugs: Vec<ArmorCatalogRecord>,
    ) -> destiny_core::WorldDefinitionsAcquisition {
        let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new(version));
        world.armor_catalogs = plugs;
        world
    }

    #[test]
    fn stat_plugs_round_trip_are_replaced_and_stay_out_of_other_catalogs() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let first = vec![
            plug(481842213, 392767087, 30, false),
            plug(627959599, 1943323491, -25, true),
        ];
        let metadata = store
            .publish_manifest(items("v1"), world("v1", first.clone()))
            .unwrap();
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::StatPlug, None)
                .unwrap(),
            first
        );
        assert_eq!(
            store
                .armor_catalog_batch(ArmorCatalogKind::StatPlug, &[627959599, 7, 627959599])
                .unwrap(),
            vec![first[1].clone()]
        );
        assert_eq!(
            store
                .lookup_armor_catalog(ArmorCatalogKind::StatPlug, 1)
                .unwrap(),
            None
        );
        // Never a named definition and never a row of the shared catalog table.
        fn count(store: &SqliteStore, sql: &str) -> i64 {
            store
                .connection()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        }
        assert_eq!(
            count(
                &store,
                "SELECT COUNT(*) FROM definitions WHERE hash = 481842213"
            ),
            0
        );
        assert_eq!(count(&store, "SELECT COUNT(*) FROM armor_catalogs"), 0);
        // An identical re-import keeps the revision.
        let again = store
            .publish_manifest(items("v1"), world("v1", first.clone()))
            .unwrap();
        assert_eq!(again.revision, metadata.revision);
        // A different set replaces the old one completely.
        let second = vec![plug(462856101, 4244567218, 20, false)];
        let replaced = store
            .publish_manifest(items("v1"), world("v1", second.clone()))
            .unwrap();
        assert!(replaced.revision.value() > metadata.revision.value());
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::StatPlug, None)
                .unwrap(),
            second
        );
        assert_eq!(
            count(&store, "SELECT COUNT(*) FROM definition_stat_plug_stats"),
            1
        );
        // A duplicate hash is rejected without touching the committed set.
        let duplicated = vec![plug(5, 1, 1, false), plug(5, 2, 2, false)];
        assert!(
            store
                .publish_manifest(items("v1"), world("v1", duplicated))
                .is_err()
        );
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::StatPlug, None)
                .unwrap(),
            second
        );
    }

    #[test]
    fn migration_36_clears_manifest_metadata_and_guards_the_downgrade() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        store.migrate_to(35).unwrap();
        store
            .publish_manifest(items("v1"), world("v1", Vec::new()))
            .unwrap();
        assert!(store.current_metadata().unwrap().is_some());
        // The legacy schema cannot store stat plugs.
        assert!(
            store
                .publish_manifest(items("v1"), world("v1", vec![plug(1, 2, 3, false)]))
                .is_err()
        );
        store.migrate_to(36).unwrap();
        assert_eq!(store.schema_version().unwrap(), 36);
        // The next sync-manifest must re-import.
        assert_eq!(store.current_metadata().unwrap(), None);
        for table in ["definition_stat_plugs", "definition_stat_plug_stats"] {
            let sql: String = store
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_schema WHERE name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(sql.contains("STRICT"), "{table}");
        }
        store
            .publish_manifest(items("v2"), world("v2", vec![plug(1, 2, 3, false)]))
            .unwrap();
        // Stored stat plugs refuse the downgrade and leave everything intact.
        assert!(store.migrate_to(35).is_err());
        assert_eq!(store.schema_version().unwrap(), 36);
        assert!(store.current_metadata().unwrap().is_some());
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::StatPlug, None)
                .unwrap()
                .len(),
            1
        );
        store
            .connection()
            .execute_batch("DELETE FROM definition_stat_plugs")
            .unwrap();
        store.migrate_to(35).unwrap();
        assert_eq!(store.schema_version().unwrap(), 35);
        assert_eq!(store.current_metadata().unwrap(), None);
    }
}
