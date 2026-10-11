//! Socket catalogs extend the same atomic Manifest projection and catalog port.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, armor_catalog::*};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeMap;

pub(super) fn write(
    connection: &Connection,
    record: &ArmorCatalogRecord,
    version: &str,
) -> Result<(), AppError> {
    let (category, random, style, name) = match record {
        ArmorCatalogRecord::SocketType(v) => (
            Some(v.socket_category_hash),
            Some(v.always_randomize_sockets),
            None,
            None,
        ),
        ArmorCatalogRecord::SocketCategory(v) => {
            (None, None, Some(v.category_style), Some(v.name.as_str()))
        }
        ArmorCatalogRecord::PlugSet(_) => (None, None, None, None),
        _ => return Err(storage_failure()),
    };
    connection
        .prepare_cached("INSERT INTO socket_catalogs VALUES (?1,?2,?3,?4,?5,?6,?7)")
        .map_err(|_| storage_failure())?
        .execute(params![
            record.kind().id(),
            record.hash(),
            version,
            category,
            random,
            style,
            name
        ])
        .map_err(|_| storage_failure())?;
    match record {
        ArmorCatalogRecord::SocketType(v) => {
            for (ordinal, entry) in v.plug_whitelist.iter().enumerate() {
                connection
                    .prepare_cached("INSERT INTO catalog_plug_whitelist VALUES (2,?1,?2,?3,?4)")
                    .map_err(|_| storage_failure())?
                    .execute(params![
                        v.hash,
                        ordinal as i64,
                        entry.category_hash,
                        entry.category_identifier
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        ArmorCatalogRecord::PlugSet(v) => {
            for (ordinal, entry) in v.reusable_plug_items.iter().enumerate() {
                connection
                    .prepare_cached("INSERT INTO catalog_plug_items VALUES (4,?1,?2,?3,?4)")
                    .map_err(|_| storage_failure())?
                    .execute(params![
                        v.hash,
                        ordinal as i64,
                        entry.plug_item_hash,
                        entry.currently_can_roll
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn child_sql(table: &str, columns: &str, single: bool) -> String {
    let filter = if single { " WHERE hash = ?1" } else { "" };
    format!("SELECT {columns} FROM {table}{filter} ORDER BY hash, ordinal")
}
pub(super) fn read(
    connection: &Connection,
    kind: ArmorCatalogKind,
    hash: Option<u32>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    if read_user_version(connection)? < 29 {
        return Ok(Vec::new());
    }
    let query = if hash.is_some() {
        "SELECT hash, definition_version, category_hash, randomize, style, name FROM socket_catalogs WHERE kind = ?1 AND hash = ?2 ORDER BY hash"
    } else {
        "SELECT hash, definition_version, category_hash, randomize, style, name FROM socket_catalogs WHERE kind = ?1 ORDER BY hash"
    };
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let mut statement = connection
        .prepare_cached(query)
        .map_err(|_| storage_failure())?;
    let arguments: Vec<_> = std::iter::once(kind.id()).chain(hash).collect();
    let rows = statement
        .query_map(params_from_iter(arguments), |r| {
            Ok((
                r.get::<_, u32>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<u32>>(2)?,
                r.get::<_, Option<i32>>(3)?,
                r.get::<_, Option<u32>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    let mut records = BTreeMap::new();
    for row in rows {
        let (hash, version, category, random, style, name) = row.map_err(|_| storage_failure())?;
        if !metadata
            .as_ref()
            .is_some_and(|m| m.definition_version.as_str() == version)
        {
            return Err(storage_failure());
        }
        let record = match kind {
            ArmorCatalogKind::SocketType if style.is_none() && name.is_none() => {
                let random = random.ok_or_else(storage_failure)?;
                if !(0..=1).contains(&random) {
                    return Err(storage_failure());
                }
                ArmorCatalogRecord::SocketType(SocketTypeDefinition {
                    hash,
                    socket_category_hash: category.ok_or_else(storage_failure)?,
                    always_randomize_sockets: random == 1,
                    plug_whitelist: Vec::new(),
                })
            }
            ArmorCatalogKind::SocketCategory if category.is_none() && random.is_none() => {
                ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
                    hash,
                    category_style: style.ok_or_else(storage_failure)?,
                    name: name.ok_or_else(storage_failure)?,
                })
            }
            ArmorCatalogKind::PlugSet
                if category.is_none() && random.is_none() && style.is_none() && name.is_none() =>
            {
                ArmorCatalogRecord::PlugSet(PlugSetDefinition {
                    hash,
                    reusable_plug_items: Vec::new(),
                })
            }
            _ => return Err(storage_failure()),
        };
        if records.insert(hash, record).is_some() || records.len() > kind.max_records() {
            return Err(storage_failure());
        }
    }
    if kind == ArmorCatalogKind::SocketType {
        let mut statement = connection
            .prepare_cached(&child_sql(
                "catalog_plug_whitelist",
                "hash, ordinal, category_hash, identifier",
                hash.is_some(),
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, u32>(1)? as usize,
                    r.get::<_, u32>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, ordinal, category_hash, category_identifier) =
                row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::SocketType(v)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != v.plug_whitelist.len() || ordinal >= MAX_PLUG_WHITELIST {
                return Err(storage_failure());
            }
            v.plug_whitelist.push(PlugWhitelistEntry {
                category_hash,
                category_identifier,
            });
        }
    } else if kind == ArmorCatalogKind::PlugSet {
        let mut statement = connection
            .prepare_cached(&child_sql(
                "catalog_plug_items",
                "hash, ordinal, item_hash, can_roll",
                hash.is_some(),
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |r| {
                Ok((
                    r.get::<_, u32>(0)?,
                    r.get::<_, u32>(1)? as usize,
                    r.get::<_, u32>(2)?,
                    r.get::<_, i32>(3)?,
                ))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, ordinal, plug_item_hash, can_roll) = row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::PlugSet(v)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != v.reusable_plug_items.len()
                || ordinal >= MAX_PLUG_SET_ITEMS
                || !(0..=1).contains(&can_roll)
            {
                return Err(storage_failure());
            }
            v.reusable_plug_items.push(PlugSetItem {
                plug_item_hash,
                currently_can_roll: can_roll == 1,
            });
        }
    }
    let records: Vec<_> = records.into_values().collect();
    validate_catalogs(&records).map_err(|_| storage_failure())?;
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_child_write_rolls_back_and_corruption_fails_closed() {
        use destiny_core::ports::{DefinitionRepository, WorldDefinitionRepository};
        use destiny_core::{
            DefinitionReplacementInput, DefinitionVersion, SourceMarker, Timestamp,
        };
        let mut store = super::super::SqliteStore::open_in_memory().unwrap();
        let items = DefinitionReplacementInput {
            definitions: destiny_app::manifest::normalize_manifest(
                crate::manifest::decode_inventory_items(
                    DefinitionVersion::new("v1"),
                    br#"{"111":{"hash":111,"displayProperties":{"name":"Helmet"}}}"#,
                )
                .unwrap(),
            )
            .unwrap(),
            checked_at: Timestamp::new(1),
            source: SourceMarker::Manifest,
        };
        let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
        world.armor_catalogs = crate::manifest::decode_armor_catalog(
            ArmorCatalogKind::PlugSet,
            br#"{"789":{"hash":789,"reusablePlugItems":[{"plugItemHash":111,"currentlyCanRoll":true}]}}"#,
        ).unwrap();
        let prior = store
            .publish_manifest(items.clone(), world.clone())
            .unwrap();
        let catalogs = store
            .armor_catalog(ArmorCatalogKind::PlugSet, None)
            .unwrap();
        store.connection().execute_batch("CREATE TRIGGER reject_plug BEFORE INSERT ON catalog_plug_items BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
        let ArmorCatalogRecord::PlugSet(v) = &mut world.armor_catalogs[0] else {
            panic!()
        };
        v.reusable_plug_items[0].currently_can_roll = false;
        assert!(store.publish_manifest(items, world).is_err());
        assert_eq!(store.current_metadata().unwrap(), Some(prior));
        assert_eq!(
            store
                .armor_catalog(ArmorCatalogKind::PlugSet, None)
                .unwrap(),
            catalogs
        );
        store
            .connection()
            .execute_batch("DROP TRIGGER reject_plug; UPDATE catalog_plug_items SET ordinal = 3;")
            .unwrap();
        assert!(
            store
                .lookup_armor_catalog(ArmorCatalogKind::PlugSet, 789)
                .is_err()
        );
    }
    #[test]
    fn single_hash_queries_search_indexes() {
        let store = super::super::SqliteStore::open_in_memory().unwrap();
        for table in ["catalog_plug_whitelist", "catalog_plug_items"] {
            let query = format!(
                "EXPLAIN QUERY PLAN {}",
                child_sql(table, "hash, ordinal", true)
            );
            let details: Vec<String> = store
                .connection()
                .prepare(&query)
                .unwrap()
                .query_map([7], |r| r.get(3))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert!(
                details
                    .iter()
                    .any(|d| d.contains("SEARCH") && d.contains("hash=?")),
                "{details:?}"
            );
            assert!(!details.iter().any(|d| d.contains("SCAN")), "{details:?}");
        }
        let detail: String = store
            .connection()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT hash FROM socket_catalogs WHERE kind = ?1 AND hash = ?2",
                [2, 7],
                |r| r.get(3),
            )
            .unwrap();
        assert!(detail.contains("SEARCH") && detail.contains("kind=? AND hash=?"));
    }
}
