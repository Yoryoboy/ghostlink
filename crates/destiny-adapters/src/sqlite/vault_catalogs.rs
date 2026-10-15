//! Vault evidence: bucket capacities and the Vault vendor's accepted-item mapping.
use super::{migrations::read_user_version, storage_failure};
use destiny_core::{AppError, armor_catalog::*};
use rusqlite::{Connection, params, params_from_iter};
use std::collections::BTreeMap;

pub(super) fn write(
    connection: &Connection,
    record: &ArmorCatalogRecord,
    version: &str,
) -> Result<(), AppError> {
    match record {
        ArmorCatalogRecord::InventoryBucket(value) => {
            connection
                .prepare_cached("INSERT INTO inventory_buckets VALUES (?1,?2,?3)")
                .map_err(|_| storage_failure())?
                .execute(params![value.hash, version, value.item_count])
                .map_err(|_| storage_failure())?;
        }
        ArmorCatalogRecord::VaultVendor(value) => {
            connection
                .prepare_cached("INSERT INTO vault_vendors VALUES (?1,?2)")
                .map_err(|_| storage_failure())?
                .execute(params![value.hash, version])
                .map_err(|_| storage_failure())?;
            let mut statement = connection
                .prepare_cached("INSERT INTO vault_accepted_items VALUES (?1,?2,?3,?4)")
                .map_err(|_| storage_failure())?;
            for (ordinal, entry) in value.accepted_items.iter().enumerate() {
                statement
                    .execute(params![
                        value.hash,
                        ordinal as i64,
                        entry.accepted_bucket,
                        entry.destination_bucket
                    ])
                    .map_err(|_| storage_failure())?;
            }
        }
        _ => return Err(storage_failure()),
    }
    Ok(())
}
pub(super) fn read(
    connection: &Connection,
    kind: ArmorCatalogKind,
    hash: Option<u32>,
) -> Result<Vec<ArmorCatalogRecord>, AppError> {
    if read_user_version(connection)? < 34 {
        return Ok(Vec::new());
    }
    let metadata = super::definitions::read_manifest_metadata(connection)?;
    let fresh = |version: &str| {
        metadata
            .as_ref()
            .is_some_and(|m| m.definition_version.as_str() == version)
    };
    let filter = if hash.is_some() {
        " WHERE hash = ?1"
    } else {
        ""
    };
    let mut records = BTreeMap::new();
    if kind == ArmorCatalogKind::InventoryBucket {
        let mut statement = connection
            .prepare_cached(&format!(
                "SELECT hash, definition_version, item_count FROM inventory_buckets{filter} ORDER BY hash"
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
            let (hash, version, item_count) = row.map_err(|_| storage_failure())?;
            if !fresh(&version)
                || records
                    .insert(
                        hash,
                        ArmorCatalogRecord::InventoryBucket(InventoryBucketDefinition {
                            hash,
                            item_count,
                        }),
                    )
                    .is_some()
                || records.len() > kind.max_records()
            {
                return Err(storage_failure());
            }
        }
    } else {
        let mut statement = connection
            .prepare_cached(&format!(
                "SELECT hash, definition_version FROM vault_vendors{filter} ORDER BY hash"
            ))
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map(params_from_iter(hash), |row| {
                Ok((row.get::<_, u32>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| storage_failure())?;
        for row in rows {
            let (hash, version) = row.map_err(|_| storage_failure())?;
            if !fresh(&version)
                || records
                    .insert(
                        hash,
                        ArmorCatalogRecord::VaultVendor(VaultVendorDefinition {
                            hash,
                            accepted_items: Vec::new(),
                        }),
                    )
                    .is_some()
                || records.len() > kind.max_records()
            {
                return Err(storage_failure());
            }
        }
        let filter = if hash.is_some() {
            " WHERE vendor_hash = ?1"
        } else {
            ""
        };
        let mut statement = connection
            .prepare_cached(&format!(
                "SELECT vendor_hash, ordinal, accepted_bucket, destination_bucket FROM vault_accepted_items{filter} ORDER BY vendor_hash, ordinal"
            ))
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
            let (hash, ordinal, accepted_bucket, destination_bucket) =
                row.map_err(|_| storage_failure())?;
            let Some(ArmorCatalogRecord::VaultVendor(vendor)) = records.get_mut(&hash) else {
                return Err(storage_failure());
            };
            if ordinal != vendor.accepted_items.len() {
                return Err(storage_failure());
            }
            vendor.accepted_items.push(VaultAcceptedItem {
                accepted_bucket,
                destination_bucket,
            });
        }
    }
    let result: Vec<_> = records.into_values().collect();
    validate_catalogs(&result).map_err(|_| storage_failure())?;
    Ok(result)
}
