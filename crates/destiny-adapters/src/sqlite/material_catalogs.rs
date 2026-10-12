//! Insertion costs: ordered material rows, including duplicates and zero counts.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, armor_catalog::*};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeMap;

pub(super) fn write(
    connection: &Connection,
    value: &MaterialRequirementSetDefinition,
    version: &str,
) -> Result<(), AppError> {
    connection
        .prepare_cached("INSERT INTO material_requirement_sets VALUES (?1,?2)")
        .map_err(|_| storage_failure())?
        .execute(params![value.hash, version])
        .map_err(|_| storage_failure())?;
    let mut statement = connection
        .prepare_cached("INSERT INTO material_requirements VALUES (?1,?2,?3,?4)")
        .map_err(|_| storage_failure())?;
    for (ordinal, material) in value.materials.iter().enumerate() {
        statement
            .execute(params![
                value.hash,
                ordinal as i64,
                material.item_hash,
                material.count
            ])
            .map_err(|_| storage_failure())?;
    }
    Ok(())
}
pub(super) fn read(
    connection: &Connection,
    hash: Option<u32>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    if read_user_version(connection)? < 32 {
        return Ok(Vec::new());
    }
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let sql = if hash.is_some() {
        "SELECT hash, definition_version FROM material_requirement_sets WHERE hash = ?1 ORDER BY hash"
    } else {
        "SELECT hash, definition_version FROM material_requirement_sets ORDER BY hash"
    };
    let mut statement = connection
        .prepare_cached(sql)
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| storage_failure())?;
    let mut records = BTreeMap::new();
    for row in rows {
        let (hash, version) = row.map_err(|_| storage_failure())?;
        if !metadata
            .as_ref()
            .is_some_and(|m| m.definition_version.as_str() == version)
        {
            return Err(storage_failure());
        }
        if records
            .insert(
                hash,
                MaterialRequirementSetDefinition {
                    hash,
                    materials: Vec::new(),
                },
            )
            .is_some()
            || records.len() > ArmorCatalogKind::MaterialRequirementSet.max_records()
        {
            return Err(storage_failure());
        }
    }
    let sql = if hash.is_some() {
        "SELECT hash, ordinal, item_hash, count FROM material_requirements WHERE hash = ?1 ORDER BY hash, ordinal"
    } else {
        "SELECT hash, ordinal, item_hash, count FROM material_requirements ORDER BY hash, ordinal"
    };
    let mut statement = connection
        .prepare_cached(sql)
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)? as usize,
                row.get::<_, u32>(2)?,
                row.get::<_, u32>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, ordinal, item_hash, count) = row.map_err(|_| storage_failure())?;
        let value = records.get_mut(&hash).ok_or_else(storage_failure)?;
        if ordinal != value.materials.len() || ordinal >= MAX_MATERIAL_REQUIREMENTS {
            return Err(storage_failure());
        }
        value
            .materials
            .push(MaterialRequirement { item_hash, count });
    }
    let result: Vec<_> = records
        .into_values()
        .map(ArmorCatalogRecord::MaterialRequirementSet)
        .collect();
    validate_catalogs(&result).map_err(|_| storage_failure())?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
        ports::{DefinitionRepository, WorldDefinitionRepository},
    };
    fn input() -> DefinitionReplacementInput {
        DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(crate::manifest::decode_inventory_items(DefinitionVersion::new("v1"), br#"{"111":{"hash":111,"displayProperties":{"name":"Plug"},"plug":{"insertionMaterialRequirementHash":123}}}"#).unwrap()).unwrap(),
            checked_at: Timestamp::new(1), source: SourceMarker::Manifest,
        }
    }
    fn world() -> destiny_core::WorldDefinitionsAcquisition {
        let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
        world.armor_catalogs = vec![ArmorCatalogRecord::MaterialRequirementSet(
            MaterialRequirementSetDefinition {
                hash: 123,
                materials: vec![MaterialRequirement {
                    item_hash: 1,
                    count: 0,
                }],
            },
        )];
        world
    }
    #[test]
    fn material_writes_are_atomic_and_corruption_fails_closed() {
        let mut store = super::super::SqliteStore::open_in_memory().unwrap();
        let prior = store.publish_manifest(input(), world()).unwrap();
        store.connection().execute_batch("CREATE TRIGGER reject_cost BEFORE INSERT ON material_requirements BEGIN SELECT RAISE(ABORT, 'synthetic'); END;").unwrap();
        let mut changed = world();
        let ArmorCatalogRecord::MaterialRequirementSet(set) = &mut changed.armor_catalogs[0] else {
            panic!()
        };
        set.materials[0].count = 1;
        assert!(store.publish_manifest(input(), changed).is_err());
        assert_eq!(store.current_metadata().unwrap(), Some(prior.clone()));
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::MaterialRequirementSet, None)
                .unwrap(),
            world().armor_catalogs
        );
        store
            .connection()
            .execute_batch(
                "DROP TRIGGER reject_cost; UPDATE material_requirements SET ordinal = 2;",
            )
            .unwrap();
        assert!(
            store
                .armor_catalog(ArmorCatalogKind::MaterialRequirementSet, Some(123))
                .is_err()
        );
        assert!(store.publish_manifest(input(), world()).is_err());
        assert_eq!(store.current_metadata().unwrap(), Some(prior));
    }
    #[test]
    fn material_schema_is_strict_cascading_guarded_and_indexed() {
        let mut store = super::super::SqliteStore::open_in_memory().unwrap();
        for sql in [
            "INSERT INTO material_requirement_sets VALUES (-1,'v1')",
            "INSERT INTO material_requirements VALUES (1,0,1,0)",
            "INSERT INTO definition_material_inputs VALUES (1,1,NULL,NULL)",
        ] {
            assert!(store.connection().execute_batch(sql).is_err());
        }
        store
            .connection()
            .execute_batch("INSERT INTO material_requirement_sets VALUES (123,'v1');")
            .unwrap();
        for sql in [
            "INSERT INTO material_requirements VALUES (123,256,1,0)",
            "INSERT INTO material_requirements VALUES (123,0,1,-1)",
            "INSERT INTO material_requirements VALUES (123,0,1,1.5)",
            "INSERT INTO material_requirements VALUES (123,0,4294967296,1)",
        ] {
            assert!(store.connection().execute_batch(sql).is_err());
        }
        assert!(store.migrate_to(31).is_err()); // observed-empty parent also guards
        store.connection().execute_batch("INSERT INTO material_requirements VALUES (123,0,1,0); DELETE FROM material_requirement_sets;").unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM material_requirements", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        store
            .publish_manifest(
                input(),
                destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1")),
            )
            .unwrap();
        assert!(store.migrate_to(31).is_err()); // plug inputs independently guard
        for query in [
            "EXPLAIN QUERY PLAN SELECT hash FROM material_requirement_sets WHERE hash = ?1",
            "EXPLAIN QUERY PLAN SELECT hash,ordinal FROM material_requirements WHERE hash = ?1 ORDER BY hash,ordinal",
            "EXPLAIN QUERY PLAN SELECT definition_hash FROM definition_material_inputs WHERE definition_hash = ?1",
        ] {
            let detail: String = store
                .connection()
                .query_row(query, [123], |r| r.get(3))
                .unwrap();
            assert!(
                detail.contains("SEARCH") && !detail.contains("SCAN"),
                "{detail}"
            );
        }
        store
            .connection()
            .execute_batch("DELETE FROM definitions;")
            .unwrap();
        let count: i64 = store
            .connection()
            .query_row("SELECT count(*) FROM definition_material_inputs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
        store.migrate_to(31).unwrap();
        store.migrate_to(32).unwrap();
    }
    #[test]
    fn material_reads_reject_version_drift_and_bad_plug_inputs() {
        let mut store = super::super::SqliteStore::open_in_memory().unwrap();
        store.publish_manifest(input(), world()).unwrap();
        store
            .connection()
            .execute_batch("UPDATE material_requirement_sets SET definition_version='other';")
            .unwrap();
        assert!(
            store
                .armor_catalog(ArmorCatalogKind::MaterialRequirementSet, None)
                .is_err()
        );
        store.connection().execute_batch("PRAGMA ignore_check_constraints = ON; UPDATE definition_material_inputs SET requirement_hash=-1;").unwrap();
        assert!(
            store
                .lookup_definition(destiny_core::ItemHash::new(111))
                .is_err()
        );
    }
}
