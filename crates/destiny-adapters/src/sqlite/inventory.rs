//! Transactional SQLite persistence for the single active profile projection.
//!
//! This module implements [`destiny_core::InventoryRepository`] on the
//! application-owned [`SqliteStore`]. It performs no provider I/O, no async
//! work, and no serialization: every value it stores arrives already normalized,
//! and every read reconstructs the provider-neutral core records from normalized
//! columns, so no raw provider payload, SQL statement, or stored value can
//! escape through a returned value or a failure.
//!
//! The projection is exactly four tables read as one unit: the singleton
//! `active_profile` anchor, the `characters` rows, the `items` rows, and the
//! single fixed `profile_inventory` row in `sync_metadata`. A projection exists
//! only when both the anchor and its metadata row are present, and the anchor
//! must be exactly one row whose discriminator is `1`; a fully empty store
//! reports `None`, and every partial or corrupt combination — metadata without
//! the anchor or the anchor without metadata, child rows without both, an extra
//! anchor row, or any malformed source, decimal, boolean, range, location, or
//! reference value — fails closed as a sanitized local-storage error rather
//! than returning a partially constructed snapshot.
//!
//! Publication is exactly one `IMMEDIATE` transaction. Inside it the repository
//! reads the committed projection, decides whether the incoming complete
//! snapshot is a content change, allocates or retains the revision, replaces the
//! profile rows, and upserts the committed metadata. Any failure, including a
//! constraint failure partway through the replacement, drops the transaction,
//! and rusqlite rolls the whole thing back, so the prior profile rows, their
//! metadata, and the revision remain exactly readable.
//!
//! Revision identity is the complete normalized projection content compared
//! through the shared order-insensitive [`InventorySnapshotInput::content_eq`]:
//! characters and items are unordered sets keyed by their record identifiers, so
//! a pure permutation is not a content change. `checked_at` and `source` are
//! freshness and provenance, never content identity, so an otherwise identical
//! refresh retains the revision and refreshes only the committed timestamp.
//! Validation that needs no storage runs before the transaction is opened and
//! reports a sanitized provider-validation failure. Every failure after that is
//! treated as local state and maps only to the sanitized
//! [`ErrorCategory::LocalStorage`] category.

use super::{SqliteStore, encode_decimal, parse_decimal, storage_failure};
use destiny_core::{
    AppError, Character, CharacterId, Dataset, DefinitionVersion, ErrorCategory,
    InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemHash, ItemInstance,
    ItemLocation, MembershipId, MembershipType, SnapshotMetadata, SourceMarker, SyncMetadata,
    SyncRevision, Timestamp,
};
use rusqlite::{Connection, Error as SqlError, TransactionBehavior, params};
use std::collections::HashSet;

/// The fixed `sync_metadata` dataset row this repository owns.
const PROFILE_DATASET: &str = "profile_inventory";

/// The source marker the schema pairs with the profile dataset.
const PROFILE_SOURCE: &str = "profile";

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

impl InventoryRepository for SqliteStore {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        read_projection(self.connection())
    }

    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        // Provenance and structural validity need no storage, so they are
        // rejected here before the transaction is opened and no write is ever
        // attempted.
        if input.source != SourceMarker::Profile {
            return Err(provider_validation());
        }
        if !input.is_structurally_valid() {
            return Err(provider_validation());
        }

        // Characters and items are unordered sets, so the committed and
        // returned vectors are canonicalized by identifier. This keeps reads
        // deterministic and makes the returned snapshot byte-identical to a
        // later `load_current`.
        let ordered = canonicalize(input);

        // One IMMEDIATE transaction determines the prior state, allocates the
        // revision, replaces the profile rows, and upserts metadata. Rusqlite
        // rolls the transaction back on drop, so any early return below leaves
        // the prior committed state untouched.
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| storage_failure())?;

        let prior = read_projection(&transaction)?;
        let unchanged = match prior.as_ref() {
            Some(snapshot) => InventorySnapshotInput::with_metadata(snapshot).content_eq(&ordered),
            None => false,
        };

        // Revision allocation happens before any write. An overflow at the
        // boundary therefore fails with the profile rows and metadata intact.
        let revision = if unchanged {
            prior
                .as_ref()
                .map(|snapshot| snapshot.metadata.revision)
                .ok_or_else(storage_failure)?
        } else {
            match prior.as_ref() {
                Some(snapshot) => SyncRevision::new(
                    snapshot
                        .metadata
                        .revision
                        .value()
                        .checked_add(1)
                        .ok_or_else(storage_failure)?,
                ),
                None => SyncRevision::new(1),
            }
        };

        // Unchanged content keeps the committed profile rows and rewrites only
        // the freshness metadata; changed content replaces every row.
        if !unchanged {
            replace_projection(&transaction, &ordered)?;
        }
        upsert_metadata(&transaction, &ordered, revision)?;
        transaction.commit().map_err(|_| storage_failure())?;

        Ok(InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: ordered.membership_id,
                membership_type: ordered.membership_type,
                revision,
                definition_version: ordered.definition_version,
                checked_at: ordered.checked_at,
                source: SourceMarker::Profile,
            },
            characters: ordered.characters,
            items: ordered.items,
        })
    }
}

/// Returns the input with its collections in the deterministic canonical order.
fn canonicalize(mut input: InventorySnapshotInput) -> InventorySnapshotInput {
    input
        .characters
        .sort_by(|left, right| left.id.cmp(&right.id));
    input
        .items
        .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    input
}

/// Reads the complete profile projection, or `None` when nothing is committed.
///
/// The projection is a single unit: the anchor and its metadata row must both be
/// present or both be absent. A complete anchor reconstructs every character and
/// item field, validates each location and reference, and returns the
/// canonicalized snapshot. Anything else is corrupt local state and fails closed
/// as a sanitized local-storage error.
fn read_projection(connection: &Connection) -> Result<Option<InventorySnapshot>, AppError> {
    let metadata = read_metadata(connection)?;
    let anchor = read_anchor(connection)?;

    match (metadata, anchor) {
        (None, None) => {
            if projection_rows_exist(connection)? {
                return Err(storage_failure());
            }
            Ok(None)
        }
        (Some(metadata), Some((membership_id, membership_type))) => {
            let mut characters = read_characters(connection)?;
            let mut items = read_items(connection)?;

            {
                let character_ids: HashSet<&str> = characters
                    .iter()
                    .map(|character| character.id.as_str())
                    .collect();
                for item in &items {
                    if let ItemLocation::Character(character_id) = &item.location
                        && !character_ids.contains(character_id.as_str())
                    {
                        return Err(storage_failure());
                    }
                }
            }

            characters.sort_by(|left, right| left.id.cmp(&right.id));
            items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));

            Ok(Some(InventorySnapshot {
                metadata: SnapshotMetadata {
                    membership_id,
                    membership_type,
                    revision: metadata.revision,
                    definition_version: metadata.definition_version,
                    checked_at: metadata.checked_at,
                    source: metadata.source,
                },
                characters,
                items,
            }))
        }
        _ => Err(storage_failure()),
    }
}

/// Reads the single committed `profile_inventory` metadata row, if present.
///
/// Canonical decimal values are parsed and range-checked rather than trusted,
/// and the fixed dataset/source pairing is revalidated, so a malformed or
/// mismatched stored value is a sanitized local-storage failure, never a
/// partially constructed core record.
fn read_metadata(connection: &Connection) -> Result<Option<SyncMetadata>, AppError> {
    let row = connection.query_row(
        "SELECT checked_at, source, revision, definition_version \
         FROM sync_metadata WHERE dataset = ?1",
        [PROFILE_DATASET],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );

    match row {
        Ok((checked_at, source, revision, definition_version)) => {
            if source != PROFILE_SOURCE {
                return Err(storage_failure());
            }
            Ok(Some(SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(parse_decimal(&checked_at)?),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(parse_decimal(&revision)?),
                definition_version: DefinitionVersion::new(definition_version),
            }))
        }
        Err(SqlError::QueryReturnedNoRows) => Ok(None),
        Err(_) => Err(storage_failure()),
    }
}

/// Reads the singleton active-profile anchor, if present.
///
/// The schema pins `singleton = 1` and the primary key makes that row unique in
/// a well-formed store, but a corrupt database could carry an additional row.
/// The read therefore enumerates every row and fails closed unless exactly one
/// row exists and its discriminator is `1`, so a fabricated extra anchor can
/// never be silently ignored.
fn read_anchor(
    connection: &Connection,
) -> Result<Option<(MembershipId, MembershipType)>, AppError> {
    let mut statement = connection
        .prepare("SELECT singleton, membership_id, membership_type FROM active_profile")
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| storage_failure())?;

    let mut anchor = None;
    for row in rows {
        let (singleton, membership_id, membership_type) = row.map_err(|_| storage_failure())?;
        if singleton != 1 || anchor.is_some() {
            return Err(storage_failure());
        }
        let membership_type =
            u32::try_from(parse_decimal(&membership_type)?).map_err(|_| storage_failure())?;
        anchor = Some((
            MembershipId::new(membership_id),
            MembershipType::new(membership_type),
        ));
    }
    Ok(anchor)
}

/// Reports whether any child projection rows exist without their anchor.
fn projection_rows_exist(connection: &Connection) -> Result<bool, AppError> {
    let characters: i64 = connection
        .query_row("SELECT count(*) FROM characters", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    if characters > 0 {
        return Ok(true);
    }
    let items: i64 = connection
        .query_row("SELECT count(*) FROM items", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    Ok(items > 0)
}

/// Reconstructs every stored character with its optional fields, checked.
fn read_characters(connection: &Connection) -> Result<Vec<Character>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT character_id, name, class_name, race, gender, light \
             FROM characters ORDER BY character_id",
        )
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(|_| storage_failure())?;

    let mut characters = Vec::new();
    for row in rows {
        let (id, name, class, race, gender, light) = row.map_err(|_| storage_failure())?;
        characters.push(Character {
            id: CharacterId::new(id),
            name,
            class,
            race,
            gender,
            light: decode_optional_u16(light)?,
        });
    }
    Ok(characters)
}

/// Reconstructs every stored item with its typed location and checked fields.
fn read_items(connection: &Connection) -> Result<Vec<ItemInstance>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT instance_id, item_hash, location_kind, location_character_id, \
             location_other, quantity, equipped, locked, power \
             FROM items ORDER BY instance_id",
        )
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .map_err(|_| storage_failure())?;

    let mut items = Vec::new();
    for row in rows {
        let (
            instance_id,
            item_hash,
            location_kind,
            location_character_id,
            location_other,
            quantity,
            equipped,
            locked,
            power,
        ) = row.map_err(|_| storage_failure())?;

        let location = match (
            location_kind.as_str(),
            location_character_id,
            location_other,
        ) {
            ("vault", None, None) => ItemLocation::Vault,
            ("character", Some(character_id), None) => ItemLocation::Character(character_id.into()),
            ("other", None, Some(other)) => ItemLocation::Other(other),
            _ => return Err(storage_failure()),
        };

        items.push(ItemInstance {
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: u32::try_from(quantity).map_err(|_| storage_failure())?,
            equipped: decode_boolean(equipped)?,
            locked: decode_boolean(locked)?,
            power: decode_optional_u16(power)?,
        });
    }
    Ok(items)
}

/// Decodes one stored integer boolean, accepting only the exact values `0`/`1`.
fn decode_boolean(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(storage_failure()),
    }
}

/// Decodes one optional stored `u16`, range-checked.
fn decode_optional_u16(value: Option<i64>) -> Result<Option<u16>, AppError> {
    match value {
        Some(raw) => u16::try_from(raw).map(Some).map_err(|_| storage_failure()),
        None => Ok(None),
    }
}

/// Encodes one boolean as the exact `0`/`1` integer the schema stores.
fn encode_boolean(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

/// Removes the complete prior projection and writes the replacement rows.
///
/// Items are deleted before characters so the removal order satisfies the
/// character foreign key explicitly instead of relying on cascade, and
/// characters are inserted before the items that reference them. Any failure
/// partway through drops the transaction and rolls all of it back.
fn replace_projection(
    connection: &Connection,
    input: &InventorySnapshotInput,
) -> Result<(), AppError> {
    connection
        .execute("DELETE FROM items", [])
        .map_err(|_| storage_failure())?;
    connection
        .execute("DELETE FROM characters", [])
        .map_err(|_| storage_failure())?;

    connection
        .execute(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (1, ?1, ?2) \
             ON CONFLICT(singleton) DO UPDATE SET \
                 membership_id = excluded.membership_id, \
                 membership_type = excluded.membership_type",
            params![
                input.membership_id.as_str(),
                encode_decimal(u64::from(input.membership_type.value())),
            ],
        )
        .map_err(|_| storage_failure())?;

    for character in &input.characters {
        connection
            .execute(
                "INSERT INTO characters (character_id, name, class_name, race, gender, light) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    character.id.as_str(),
                    character.name,
                    character.class,
                    character.race,
                    character.gender,
                    character.light.map(i64::from),
                ],
            )
            .map_err(|_| storage_failure())?;
    }

    for item in &input.items {
        let (location_kind, location_character_id, location_other) = match &item.location {
            ItemLocation::Vault => ("vault", None, None),
            ItemLocation::Character(character_id) => {
                ("character", Some(character_id.as_str()), None)
            }
            ItemLocation::Other(other) => ("other", None, Some(other.as_str())),
        };
        connection
            .execute(
                "INSERT INTO items \
                 (instance_id, item_hash, location_kind, location_character_id, location_other, \
                  quantity, equipped, locked, power) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    item.instance_id.as_str(),
                    item.item_hash.value(),
                    location_kind,
                    location_character_id,
                    location_other,
                    i64::from(item.quantity),
                    encode_boolean(item.equipped),
                    encode_boolean(item.locked),
                    item.power.map(i64::from),
                ],
            )
            .map_err(|_| storage_failure())?;
    }
    Ok(())
}

/// Inserts or refreshes the committed profile metadata row.
///
/// The row is the sole durable source of the committed revision, freshness
/// timestamp, and definition version, so it is upserted on every successful
/// publication, including one that retained the revision. The write targets
/// only the fixed profile dataset, so the Manifest metadata row is untouched.
fn upsert_metadata(
    connection: &Connection,
    input: &InventorySnapshotInput,
    revision: SyncRevision,
) -> Result<(), AppError> {
    connection
        .execute(
            "INSERT INTO sync_metadata \
             (dataset, checked_at, source, revision, definition_version) \
             VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(dataset) DO UPDATE SET \
                 checked_at = excluded.checked_at, \
                 source = excluded.source, \
                 revision = excluded.revision, \
                 definition_version = excluded.definition_version",
            params![
                PROFILE_DATASET,
                encode_decimal(input.checked_at.seconds()),
                PROFILE_SOURCE,
                encode_decimal(revision.value()),
                input.definition_version.as_str(),
            ],
        )
        .map_err(|_| storage_failure())?;
    Ok(())
}
