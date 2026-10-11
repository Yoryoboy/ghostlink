//! Normalized Manifest-only investment and plug metadata, joined by item hash.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, ArmorDefinition, InvestmentStat, ItemDefinition, PlugEnergyCost};
use rusqlite::{Connection, params};
use std::collections::HashMap;

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
    Ok(())
}

/// Three bounded queries, not three queries per item during revision comparison.
/// Legacy schemas preserve unknown metadata; no backfill or inferred values.
pub(super) fn read(
    connection: &Connection,
    hash: Option<i64>,
) -> Result<HashMap<i64, ArmorDefinition>, AppError> {
    let mut result: HashMap<i64, ArmorDefinition> = HashMap::new();
    if read_user_version(connection)? < 26 {
        return Ok(result);
    }
    let mut statement = connection.prepare("SELECT definition_hash, stat_group_hash, plug_category_hash, aspect_capacity, equipable_item_set_hash FROM definition_armor WHERE (?1 IS NULL OR definition_hash = ?1)").map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([hash], |row| {
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
        result.insert(hash, armor);
    }
    let mut statement = connection.prepare("SELECT definition_hash, ordinal, stat_type_hash, value, conditionally_active FROM definition_investment_stats WHERE (?1 IS NULL OR definition_hash = ?1) ORDER BY definition_hash, ordinal").map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([hash], |row| {
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
    let mut statement = connection.prepare("SELECT definition_hash, energy_cost, energy_type, energy_type_hash FROM definition_energy_costs WHERE (?1 IS NULL OR definition_hash = ?1)").map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([hash], |row| {
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
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStore;
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
