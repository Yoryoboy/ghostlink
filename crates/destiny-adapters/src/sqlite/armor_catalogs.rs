//! Normalized field-limited catalogs sharing the atomic Manifest transaction.
use super::{SqliteStore, migrations::read_user_version, storage_failure};
use destiny_core::{AppError, armor_catalog::*};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeMap;

pub(super) fn write(
    connection: &Connection,
    records: &[ArmorCatalogRecord],
    version: &str,
) -> Result<(), AppError> {
    if read_user_version(connection)? < 28 {
        return Ok(());
    }
    for record in records {
        if record.kind().id() >= 6 {
            super::vault_catalogs::write(connection, record, version)?;
            continue;
        }
        if let ArmorCatalogRecord::MaterialRequirementSet(value) = record {
            super::material_catalogs::write(connection, value, version)?;
            continue;
        }
        if record.kind().id() >= 2 {
            super::socket_catalogs::write(connection, record, version)?;
            continue;
        }
        let name = match record {
            ArmorCatalogRecord::EquipableSet(set) => Some(set.name.as_str()),
            _ => None,
        };
        let maximum = match record {
            ArmorCatalogRecord::StatGroup(group) => group.maximum_value,
            _ => None,
        };
        connection
            .prepare_cached("INSERT INTO armor_catalogs VALUES (?1,?2,?3,?4,?5)")
            .map_err(|_| storage_failure())?
            .execute(params![
                record.kind().id(),
                record.hash(),
                version,
                name,
                maximum
            ])
            .map_err(|_| storage_failure())?;
        match record {
            ArmorCatalogRecord::StatGroup(group) => {
                for (ordinal, stat) in group.scaled_stats.iter().enumerate() {
                    connection
                        .prepare_cached(
                            "INSERT INTO catalog_scaled_stats VALUES (0,?1,?2,?3,?4,?5)",
                        )
                        .map_err(|_| storage_failure())?
                        .execute(params![
                            group.hash,
                            ordinal as i64,
                            stat.stat_hash,
                            stat.maximum_value,
                            stat.display_as_numeric
                        ])
                        .map_err(|_| storage_failure())?;
                    for (point_ordinal, point) in stat.display_interpolation.iter().enumerate() {
                        connection
                            .prepare_cached(
                                "INSERT INTO catalog_interpolation VALUES (0,?1,?2,?3,?4,?5)",
                            )
                            .map_err(|_| storage_failure())?
                            .execute(params![
                                group.hash,
                                ordinal as i64,
                                point_ordinal as i64,
                                point.value,
                                point.weight
                            ])
                            .map_err(|_| storage_failure())?;
                    }
                }
            }
            ArmorCatalogRecord::MaterialRequirementSet(_)
            | ArmorCatalogRecord::InventoryBucket(_)
            | ArmorCatalogRecord::VaultVendor(_)
            | ArmorCatalogRecord::SocketType(_)
            | ArmorCatalogRecord::SocketCategory(_)
            | ArmorCatalogRecord::PlugSet(_) => unreachable!(),
            ArmorCatalogRecord::EquipableSet(set) => {
                for (ordinal, hash) in set.set_items.iter().enumerate() {
                    connection
                        .prepare_cached("INSERT INTO catalog_set_items VALUES (1,?1,?2,?3)")
                        .map_err(|_| storage_failure())?
                        .execute(params![set.hash, ordinal as i64, hash])
                        .map_err(|_| storage_failure())?;
                }
                for (ordinal, perk) in set.set_perks.iter().enumerate() {
                    connection
                        .prepare_cached("INSERT INTO catalog_set_perks(kind,hash,ordinal,required_count,perk_hash) VALUES (1,?1,?2,?3,?4)")
                        .map_err(|_| storage_failure())?
                        .execute(params![
                            set.hash,
                            ordinal as i64,
                            perk.required_set_count,
                            perk.sandbox_perk_hash
                        ])
                        .map_err(|_| storage_failure())?;
                    if read_user_version(connection)? >= 30 {
                        connection.prepare_cached("UPDATE catalog_set_perks SET name=?1,icon=?2 WHERE hash=?3 AND ordinal=?4").map_err(|_| storage_failure())?
                            .execute(params![perk.name, perk.icon, set.hash, ordinal as i64]).map_err(|_| storage_failure())?;
                    } else if perk.name.is_some() || perk.icon.is_some() {
                        return Err(storage_failure());
                    }
                }
            }
        }
    }
    Ok(())
}
/// Distinct cached all-rows and indexed single-hash statements; no optional OR predicate.
fn sql(table: &str, columns: &str, hash: Option<u32>) -> String {
    let filter = if hash.is_some() {
        " WHERE hash = ?1"
    } else {
        ""
    };
    let order = if table == "catalog_interpolation" {
        ", stat_ordinal, ordinal"
    } else {
        ", ordinal"
    };
    format!("SELECT {columns} FROM {table}{filter} ORDER BY hash{order}")
}
fn read(
    connection: &Connection,
    kind: ArmorCatalogKind,
    hash: Option<u32>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    if kind.id() >= 6 {
        return super::vault_catalogs::read(connection, kind, hash);
    }
    if kind == ArmorCatalogKind::MaterialRequirementSet {
        return super::material_catalogs::read(connection, hash);
    }
    if kind.id() >= 2 {
        return super::socket_catalogs::read(connection, kind, hash);
    }
    if read_user_version(connection)? < 28 {
        return Ok(Vec::new());
    }
    let mut records = BTreeMap::new();
    let query = if hash.is_some() {
        "SELECT hash, definition_version, name, maximum_value FROM armor_catalogs WHERE kind = ?1 AND hash = ?2 ORDER BY hash"
    } else {
        "SELECT hash, definition_version, name, maximum_value FROM armor_catalogs WHERE kind = ?1 ORDER BY hash"
    };
    let mut statement = connection
        .prepare_cached(query)
        .map_err(|_| storage_failure())?;
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let arguments: Vec<u32> = std::iter::once(kind.id()).chain(hash).collect();
    let rows = statement
        .query_map(params_from_iter(arguments), |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i32>>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, version, name, maximum_value) = row.map_err(|_| storage_failure())?;
        if !metadata
            .as_ref()
            .is_some_and(|metadata| metadata.definition_version.as_str() == version)
        {
            return Err(storage_failure());
        }
        let record = match kind {
            ArmorCatalogKind::StatGroup if name.is_none() => {
                ArmorCatalogRecord::StatGroup(StatGroupDefinition {
                    hash,
                    maximum_value,
                    scaled_stats: Vec::new(),
                })
            }
            ArmorCatalogKind::EquipableSet if maximum_value.is_none() => {
                ArmorCatalogRecord::EquipableSet(EquipableSetDefinition {
                    hash,
                    name: name.ok_or_else(storage_failure)?,
                    set_items: Vec::new(),
                    set_perks: Vec::new(),
                })
            }
            _ => return Err(storage_failure()),
        };
        if records.insert(hash, record).is_some() || records.len() > kind.max_records() {
            return Err(storage_failure());
        }
    }
    // Each child table belongs to one catalog kind. Decode all ordinals strictly.
    if kind == ArmorCatalogKind::StatGroup {
        let mut statement = connection
            .prepare_cached(&sql(
                "catalog_scaled_stats",
                "hash, ordinal, stat_hash, maximum_value, numeric",
                hash,
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)? as usize,
                    row.get::<_, u32>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, ordinal, stat_hash, maximum_value, numeric) =
                row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::StatGroup(group)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != group.scaled_stats.len() || ordinal >= 64 || !(0..=1).contains(&numeric) {
                return Err(storage_failure());
            }
            group.scaled_stats.push(ScaledStat {
                stat_hash,
                maximum_value,
                display_as_numeric: numeric == 1,
                display_interpolation: Vec::new(),
            });
        }
        let mut statement = connection
            .prepare_cached(&sql(
                "catalog_interpolation",
                "hash, stat_ordinal, ordinal, value, weight",
                hash,
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)? as usize,
                    row.get::<_, u32>(2)? as usize,
                    row.get::<_, i32>(3)?,
                    row.get::<_, i32>(4)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, stat, ordinal, value, weight) = row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::StatGroup(group)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            let points = &mut group
                .scaled_stats
                .get_mut(stat)
                .ok_or_else(storage_failure)?
                .display_interpolation;
            if ordinal != points.len() || ordinal >= 256 {
                return Err(storage_failure());
            }
            points.push(InterpolationPoint { value, weight });
        }
    } else {
        let mut statement = connection
            .prepare_cached(&sql("catalog_set_items", "hash, ordinal, item_hash", hash))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)? as usize,
                    row.get::<_, u32>(2)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, ordinal, item) = row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::EquipableSet(set)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != set.set_items.len() || ordinal >= 1024 {
                return Err(storage_failure());
            }
            set.set_items.push(item);
        }
        let mut statement = connection
            .prepare_cached(&sql(
                "catalog_set_perks",
                if read_user_version(connection)? >= 30 {
                    "hash, ordinal, required_count, perk_hash, name, icon"
                } else {
                    "hash, ordinal, required_count, perk_hash, NULL, NULL"
                },
                hash,
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)? as usize,
                    row.get::<_, i32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, ordinal, required_set_count, sandbox_perk_hash, name, icon) =
                row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::EquipableSet(set)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != set.set_perks.len() || ordinal >= 64 {
                return Err(storage_failure());
            }
            set.set_perks.push(SetPerk {
                name,
                icon,
                required_set_count,
                sandbox_perk_hash,
            });
        }
    }
    let records: Vec<_> = records.into_values().collect();
    validate_catalogs(&records).map_err(|_| storage_failure())?;
    Ok(records)
}
impl ArmorCatalogRepository for SqliteStore {
    fn armor_catalog(
        &self,
        kind: ArmorCatalogKind,
        hash: Option<u32>,
    ) -> Result<Vec<ArmorCatalogRecord>, AppError> {
        read(self.connection(), kind, hash)
    }
}
pub(super) fn contents_match(
    connection: &Connection,
    records: &[ArmorCatalogRecord],
) -> Result<bool, AppError> {
    if read_user_version(connection)? < 28 {
        return Ok(records.is_empty());
    }
    for kind in ArmorCatalogKind::ALL {
        let mut incoming: Vec<_> = records
            .iter()
            .filter(|record| record.kind() == kind)
            .cloned()
            .collect();
        incoming.sort_by_key(ArmorCatalogRecord::hash);
        if read(connection, kind, None)? != incoming {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_hash_queries_use_indexed_search() {
        let store = SqliteStore::open_in_memory().unwrap();
        for (table, columns) in [
            ("catalog_scaled_stats", "hash, ordinal"),
            ("catalog_interpolation", "hash, stat_ordinal, ordinal"),
            ("catalog_set_items", "hash, ordinal"),
            ("catalog_set_perks", "hash, ordinal"),
        ] {
            let query = format!("EXPLAIN QUERY PLAN {}", sql(table, columns, Some(7)));
            let details: Vec<String> = store
                .connection()
                .prepare(&query)
                .unwrap()
                .query_map([7], |row| row.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                details
                    .iter()
                    .any(|detail| detail.contains("SEARCH") && detail.contains("hash=?")),
                "{details:?}"
            );
            assert!(
                !details.iter().any(|detail| detail.contains("SCAN")),
                "{details:?}"
            );
        }
        let details: String = store.connection().query_row("EXPLAIN QUERY PLAN SELECT hash, definition_version, name FROM armor_catalogs WHERE kind = ?1 AND hash = ?2", [0,7], |row| row.get(3)).unwrap();
        assert!(details.contains("SEARCH") && details.contains("kind=? AND hash=?"));
    }

    #[test]
    fn failed_child_write_rolls_back_catalogs_items_and_metadata() {
        use destiny_core::ports::{DefinitionRepository, WorldDefinitionRepository};
        use destiny_core::{
            DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
        };
        let version = DefinitionVersion::new("v1");
        let items = DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(
                crate::manifest::decode_inventory_items(
                    version.clone(),
                    br#"{"1":{"hash":1,"displayProperties":{"name":"Synthetic helmet"}}}"#,
                )
                .unwrap(),
            )
            .unwrap(),
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        };
        let mut world = destiny_app::test_support::synthetic_world(version);
        world.armor_catalogs = crate::manifest::decode_armor_catalog(ArmorCatalogKind::StatGroup, br#"{"123":{"hash":123,"scaledStats":[{"statHash":7,"maximumValue":200,"displayAsNumeric":false,"displayInterpolation":[{"value":0,"weight":0}]}]}}"#).unwrap();
        let mut store = SqliteStore::open_in_memory().unwrap();
        let metadata = store
            .publish_manifest(items.clone(), world.clone())
            .unwrap();
        let prior = store
            .armor_catalog(ArmorCatalogKind::StatGroup, None)
            .unwrap();
        store.connection().execute_batch("CREATE TRIGGER reject_point BEFORE INSERT ON catalog_interpolation BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        let ArmorCatalogRecord::StatGroup(group) = &mut world.armor_catalogs[0] else {
            panic!()
        };
        group.scaled_stats[0].maximum_value += 1;
        assert!(
            store
                .publish_manifest(items.clone(), world.clone())
                .is_err()
        );
        assert_eq!(store.current_metadata().unwrap(), Some(metadata));
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::StatGroup, None)
                .unwrap(),
            prior
        );
        store
            .connection()
            .execute_batch(
                "DROP TRIGGER reject_point; UPDATE catalog_interpolation SET ordinal = 1;",
            )
            .unwrap();
        assert!(
            store
                .armor_catalog(ArmorCatalogKind::StatGroup, None)
                .is_err()
        );
        assert!(
            store
                .lookup_armor_catalog(ArmorCatalogKind::StatGroup, 123)
                .is_err()
        );
        assert!(store.publish_manifest(items, world).is_err());
    }
}
