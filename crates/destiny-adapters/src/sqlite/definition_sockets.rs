//! Raw Manifest socket arrays, preserving zero-type placeholders and order.
use super::{armor_definitions::read_sql, migrations::read_user_version, storage_failure};
use destiny_core::{
    AppError, ArmorDefinition, DefinitionSocketCategory, DefinitionSocketEntry, ItemDefinition,
};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::HashMap;

pub(super) fn write(
    connection: &Connection,
    definitions: &[ItemDefinition],
) -> Result<(), AppError> {
    if read_user_version(connection)? < 27 {
        return Ok(());
    }
    let mut entries = connection
        .prepare_cached("INSERT INTO definition_socket_entries VALUES (?1,?2,?3,?4,?5,?6,?7,?8)")
        .map_err(|_| storage_failure())?;
    let mut plugs = connection
        .prepare_cached("INSERT INTO definition_socket_plugs VALUES (?1,?2,?3,?4)")
        .map_err(|_| storage_failure())?;
    let mut categories = connection
        .prepare_cached("INSERT INTO definition_socket_categories VALUES (?1,?2,?3)")
        .map_err(|_| storage_failure())?;
    let mut indexes = connection
        .prepare_cached("INSERT INTO definition_socket_indexes VALUES (?1,?2,?3,?4)")
        .map_err(|_| storage_failure())?;
    for item in definitions {
        for (index, entry) in item.armor.socket_entries.iter().enumerate() {
            entries
                .execute(params![
                    item.hash.value(),
                    index as i64,
                    entry.socket_type_hash,
                    entry.single_initial_item_hash,
                    entry.reusable_plug_set_hash,
                    entry.randomized_plug_set_hash,
                    entry.plug_sources,
                    entry.default_visible
                ])
                .map_err(|_| storage_failure())?;
            for (ordinal, hash) in entry.reusable_plug_items.iter().enumerate() {
                plugs
                    .execute(params![
                        item.hash.value(),
                        index as i64,
                        ordinal as i64,
                        hash
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        for (ordinal, category) in item.armor.socket_categories.iter().enumerate() {
            categories
                .execute(params![
                    item.hash.value(),
                    ordinal as i64,
                    category.socket_category_hash
                ])
                .map_err(|_| storage_failure())?;
            for (position, index) in category.socket_indexes.iter().enumerate() {
                indexes
                    .execute(params![
                        item.hash.value(),
                        ordinal as i64,
                        position as i64,
                        index
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
    }
    Ok(())
}

pub(super) fn read(
    connection: &Connection,
    hash: Option<i64>,
    result: &mut HashMap<i64, ArmorDefinition>,
) -> Result<(), AppError> {
    if read_user_version(connection)? < 27 {
        return Ok(());
    }
    let mut statement = connection.prepare_cached(&read_sql("definition_socket_entries", "definition_hash, ordinal, socket_type_hash, initial_hash, reusable_set_hash, randomized_set_hash, plug_sources, default_visible", true, hash)).map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            let visible = match row.get::<_, Option<i64>>(7)? {
                None => None,
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                DefinitionSocketEntry {
                    socket_type_hash: row.get(2)?,
                    single_initial_item_hash: row.get(3)?,
                    reusable_plug_set_hash: row.get(4)?,
                    randomized_plug_set_hash: row.get(5)?,
                    plug_sources: row.get(6)?,
                    default_visible: visible,
                    ..Default::default()
                },
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, ordinal, entry) = row.map_err(|_| storage_failure())?;
        let data = result.entry(hash).or_default();
        if ordinal != data.socket_entries.len() as i64 || !(0..32).contains(&ordinal) {
            return Err(storage_failure());
        }
        data.socket_entries.push(entry);
    }
    let mut statement = connection
        .prepare_cached(&format!(
            "{} ORDER BY definition_hash, socket_index, ordinal",
            read_sql(
                "definition_socket_plugs",
                "definition_hash, socket_index, ordinal, plug_hash",
                false,
                hash
            )
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, u32>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, index, ordinal, plug) = row.map_err(|_| storage_failure())?;
        let entry = result
            .get_mut(&hash)
            .and_then(|data| data.socket_entries.get_mut(index as usize))
            .ok_or_else(storage_failure)?;
        if ordinal != entry.reusable_plug_items.len() as i64 || !(0..1024).contains(&ordinal) {
            return Err(storage_failure());
        }
        entry.reusable_plug_items.push(plug);
    }
    let mut statement = connection
        .prepare_cached(&read_sql(
            "definition_socket_categories",
            "definition_hash, ordinal, category_hash",
            true,
            hash,
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, ordinal, category) = row.map_err(|_| storage_failure())?;
        let data = result.entry(hash).or_default();
        if ordinal != data.socket_categories.len() as i64 || !(0..32).contains(&ordinal) {
            return Err(storage_failure());
        }
        data.socket_categories.push(DefinitionSocketCategory {
            socket_category_hash: category,
            socket_indexes: Vec::new(),
        });
    }
    let mut statement = connection
        .prepare_cached(&format!(
            "{} ORDER BY definition_hash, category_ordinal, ordinal",
            read_sql(
                "definition_socket_indexes",
                "definition_hash, category_ordinal, ordinal, socket_index",
                false,
                hash
            )
        ))
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map(params_from_iter(hash), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, u32>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    for row in rows {
        let (hash, category, ordinal, index) = row.map_err(|_| storage_failure())?;
        let data = result.get_mut(&hash).ok_or_else(storage_failure)?;
        if index as usize >= data.socket_entries.len() {
            return Err(storage_failure());
        }
        let category = data
            .socket_categories
            .get_mut(category as usize)
            .ok_or_else(storage_failure)?;
        if ordinal != category.socket_indexes.len() as i64 || !(0..32).contains(&ordinal) {
            return Err(storage_failure());
        }
        category.socket_indexes.push(index);
    }
    Ok(())
}
