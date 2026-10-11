//! Transactional SQLite persistence for the single active profile projection.
//!
//! This module implements [`destiny_core::InventoryRepository`] on the
//! application-owned [`SqliteStore`]. It performs no provider I/O, no async
//! work, and no serialization: every value it stores arrives already normalized,
//! and every read reconstructs the provider-neutral core records from normalized
//! columns, so no raw provider payload, SQL statement, or stored value can
//! escape through a returned value or a failure.
//!
//! The projection is six tables read as one unit: the singleton
//! `active_profile` anchor, the `characters` rows, the `items` rows, each item's
//! ordered `item_perks` and `item_plugs` child rows, and the fixed `profile_inventory` row
//! in `sync_metadata`. A projection exists
//! only when both the anchor and its metadata row are present, and the anchor
//! must be exactly one row whose discriminator is `1`; a fully empty store
//! reports `None`, and every partial or corrupt combination — metadata without
//! the anchor or the anchor without metadata, child rows without both, an extra
//! anchor row, or any malformed source, decimal, boolean, range, location,
//! reference, perk ordinal, perk boolean, or perk hash value — fails closed as a
//! sanitized local-storage error rather than returning a partially constructed
//! snapshot.
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
//! a pure permutation is not a content change. An item's perk list is compared in
//! its stored order, so perk order, perk hash, and both perk flags are part of
//! revision identity. Enabled socket plug hashes and order also participate.
//! `checked_at` and `source` are
//! freshness and provenance, never content identity, so an otherwise identical
//! refresh retains the revision and refreshes only the committed timestamp.
//! Validation that needs no storage runs before the transaction is opened and
//! reports a sanitized provider-validation failure. Every failure after that is
//! treated as local state and maps only to the sanitized
//! [`ErrorCategory::LocalStorage`] category.

use super::{SqliteStore, encode_decimal, parse_decimal, storage_failure};
use destiny_core::{
    AppError, BucketHash, Character, CharacterId, Dataset, DefinitionVersion, ErrorCategory,
    InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemHash, ItemInstance,
    ItemLocation, MembershipId, MembershipType, PerkHash, PerkSummary, PlugSetEntry, PlugSetScope,
    PlugSummary, ProfileCurrenciesCoverage, SnapshotMetadata, SourceMarker, StackContainer,
    StackObservation, SyncMetadata, SyncRevision, Timestamp,
};
use rusqlite::{Connection, Error as SqlError, OptionalExtension, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};

/// The fixed `sync_metadata` dataset row this repository owns.
const PROFILE_DATASET: &str = "profile_inventory";

/// The source marker the schema pairs with the profile dataset.
const PROFILE_SOURCE: &str = "profile";

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

impl InventoryRepository for SqliteStore {
    fn profile_minted_at(&self) -> Result<Option<destiny_core::ProfileMintedAt>, AppError> {
        let stored: Option<Option<String>> = self
            .connection()
            .query_row(
                "SELECT profile_minted_at FROM sync_metadata WHERE dataset = 'profile_inventory'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_failure())?;
        stored
            .flatten()
            .map(|raw| {
                let (seconds, nanos) = raw.split_once(':').ok_or_else(storage_failure)?;
                let minted = destiny_core::ProfileMintedAt::new(
                    seconds.parse().map_err(|_| storage_failure())?,
                    nanos.parse().map_err(|_| storage_failure())?,
                )
                .map_err(|_| storage_failure())?;
                if encode_minted(minted) != raw {
                    return Err(storage_failure());
                }
                Ok(minted)
            })
            .transpose()
    }

    fn freshness_invalidated(&self) -> Result<bool, AppError> {
        let value: Option<i64> = self
            .connection()
            .query_row(
                "SELECT invalidated FROM sync_metadata WHERE dataset = 'profile_inventory'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| storage_failure())?;
        match value {
            None | Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(storage_failure()),
        }
    }

    fn invalidate_freshness(&mut self) -> Result<(), AppError> {
        self.connection_mut()
            .execute(
                "UPDATE sync_metadata SET invalidated = 1 WHERE dataset = 'profile_inventory'",
                [],
            )
            .map_err(|_| storage_failure())?;
        Ok(())
    }

    fn apply_acknowledged_effects(
        &mut self,
        membership: &destiny_core::DestinyMembership,
        effects: &[destiny_core::InventoryEffect],
    ) -> Result<InventorySnapshot, AppError> {
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| storage_failure())?;
        let prior = read_projection(&transaction)?.ok_or_else(storage_failure)?;
        if &prior.metadata.membership_id != membership.membership_id()
            || prior.metadata.membership_type != membership.membership_type()
        {
            return Err(storage_failure());
        }
        let mut result = destiny_core::apply_inventory_effects(&prior, effects)
            .map_err(|_| provider_validation())?;
        // The pure reducer does not resolve character ownership. Reject dangling
        // destinations before writes, just as complete publication does.
        if !InventorySnapshotInput::with_metadata(&result.snapshot).is_structurally_valid() {
            return Err(provider_validation());
        }
        if !result.changed {
            return Ok(prior);
        }
        let revision = next_revision(prior.metadata.revision)?;
        for (before, after) in prior.items.iter().zip(&result.snapshot.items) {
            if before == after {
                continue;
            }
            let (kind, character, other) = encode_location(&after.location);
            // Search and inspection read these same rows and their unchanged
            // child tables; no independent derived state needs publication.
            let rows = transaction
                .execute(
                    "UPDATE items SET location_kind = ?1, location_character_id = ?2, \
                 location_other = ?3, equipped = ?4, locked = ?5 WHERE instance_id = ?6",
                    params![
                        kind,
                        character,
                        other,
                        encode_boolean(after.equipped),
                        encode_boolean(after.locked),
                        after.instance_id.as_str()
                    ],
                )
                .map_err(|_| storage_failure())?;
            if rows != 1 {
                return Err(storage_failure());
            }
        }
        if prior.stacks != result.snapshot.stacks {
            // Anonymous duplicate rows have no persistent identity. Rewrite this
            // profile's multiset without touching coverage or acquisition metadata.
            transaction
                .execute("DELETE FROM stacks", [])
                .map_err(|_| storage_failure())?;
            insert_stacks(&transaction, &result.snapshot.stacks)?;
        }
        let rows = transaction
            .execute(
                "UPDATE sync_metadata SET revision = ?1 WHERE dataset = 'profile_inventory'",
                [encode_decimal(revision.value())],
            )
            .map_err(|_| storage_failure())?;
        if rows != 1 {
            return Err(storage_failure());
        }
        transaction.commit().map_err(|_| storage_failure())?;
        result.snapshot.metadata.revision = revision;
        Ok(result.snapshot)
    }

    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        read_projection(self.connection())
    }

    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        self.replace_current_minted(input, None)
    }

    fn saved_loadouts(&self) -> Result<Option<Vec<destiny_core::CharacterLoadouts>>, AppError> {
        super::loadouts::read(self.connection())
    }

    fn replace_current_minted(
        &mut self,
        input: InventorySnapshotInput,
        minted: Option<destiny_core::ProfileMintedAt>,
    ) -> Result<InventorySnapshot, AppError> {
        self.replace_current_profile(input, minted, None)
    }

    fn replace_current_profile(
        &mut self,
        input: InventorySnapshotInput,
        minted: Option<destiny_core::ProfileMintedAt>,
        loadouts: Option<Vec<destiny_core::CharacterLoadouts>>,
    ) -> Result<InventorySnapshot, AppError> {
        let payload = loadouts.clone().map(super::loadouts::encode).transpose()?;
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
                Some(snapshot) => next_revision(snapshot.metadata.revision)?,
                None => SyncRevision::new(1),
            }
        };

        // Retain untouched rows; the prior reconstruction is also the diff base.
        if !unchanged {
            replace_projection(&transaction, prior.as_ref(), &ordered)?;
        }
        upsert_metadata(&transaction, &ordered, revision)?;
        transaction
            .execute(
                "UPDATE sync_metadata SET invalidated = 0, profile_minted_at = ?1 WHERE dataset = 'profile_inventory'",
                [minted.map(encode_minted)],
            )
            .map_err(|_| storage_failure())?;
        if super::loadouts::read(&transaction)? != loadouts {
            transaction
                .execute("DELETE FROM saved_loadouts", [])
                .map_err(|_| storage_failure())?;
            if let Some(payload) = payload {
                execute_cached(
                    &transaction,
                    "INSERT INTO saved_loadouts (singleton, normalized_loadouts) VALUES (1, ?1)",
                    [payload],
                )?;
            }
        }
        transaction.commit().map_err(|_| storage_failure())?;

        Ok(InventorySnapshot {
            profile_currencies_coverage: ordered.profile_currencies_coverage,
            stacks: ordered.stacks,
            plug_set_entries: ordered.plug_set_entries,
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

fn encode_minted(minted: destiny_core::ProfileMintedAt) -> String {
    format!("{}:{}", minted.seconds(), minted.nanoseconds())
}

fn next_revision(revision: SyncRevision) -> Result<SyncRevision, AppError> {
    Ok(SyncRevision::new(
        revision
            .value()
            .checked_add(1)
            .ok_or_else(storage_failure)?,
    ))
}

fn encode_location(location: &ItemLocation) -> (&str, Option<&str>, Option<&str>) {
    match location {
        ItemLocation::Vault => ("vault", None, None),
        ItemLocation::Character(id) => ("character", Some(id.as_str()), None),
        ItemLocation::Postmaster(id) => ("postmaster", Some(id.as_str()), None),
        ItemLocation::Other(other) => ("other", None, Some(other.as_str())),
    }
}

/// Returns the input with its collections in the deterministic canonical order.
fn canonicalize(mut input: InventorySnapshotInput) -> InventorySnapshotInput {
    input.stacks.sort();
    input.plug_set_entries.sort();
    input
        .characters
        .sort_by(|left, right| left.id.cmp(&right.id));
    input
        .items
        .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
    for item in &mut input.items {
        item.stats.sort_by_key(|stat| stat.stat_hash);
    }
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
            let profile_currencies_coverage =
                read_coverage(connection)?.unwrap_or(ProfileCurrenciesCoverage::NotObserved);
            let mut characters = read_characters(connection)?;
            let mut items = read_items(connection)?;

            {
                let character_ids: HashSet<&str> = characters
                    .iter()
                    .map(|character| character.id.as_str())
                    .collect();
                for item in &items {
                    if let ItemLocation::Character(character_id)
                    | ItemLocation::Postmaster(character_id) = &item.location
                        && !character_ids.contains(character_id.as_str())
                    {
                        return Err(storage_failure());
                    }
                }
            }

            characters.sort_by(|left, right| left.id.cmp(&right.id));
            items.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));

            let stacks = read_stacks(connection, &characters)?;
            if profile_currencies_coverage != ProfileCurrenciesCoverage::Available
                && stacks
                    .iter()
                    .any(|stack| stack.container() == &StackContainer::ProfileCurrencies)
            {
                return Err(storage_failure());
            }
            Ok(Some(InventorySnapshot {
                profile_currencies_coverage,
                stacks,
                plug_set_entries: read_subclass_options(connection, &characters)?,
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

/// Missing markers are legacy NotObserved, never evidence of empty holdings.
/// Validate every row rather than filtering by id and hiding corrupt markers.
fn read_coverage(connection: &Connection) -> Result<Option<ProfileCurrenciesCoverage>, AppError> {
    let mut statement = connection
        .prepare_cached("SELECT id, profile_currencies FROM inventory_coverage")
        .map_err(|_| storage_failure())?;
    let mut rows = statement.query([]).map_err(|_| storage_failure())?;
    let Some(row) = rows.next().map_err(|_| storage_failure())? else {
        return Ok(None);
    };
    let id: i64 = row.get(0).map_err(|_| storage_failure())?;
    let value: String = row.get(1).map_err(|_| storage_failure())?;
    if id != 1 {
        return Err(storage_failure());
    }
    let coverage = match value.as_str() {
        "not_observed" => ProfileCurrenciesCoverage::NotObserved,
        "unavailable" => ProfileCurrenciesCoverage::Unavailable,
        "available" => ProfileCurrenciesCoverage::Available,
        _ => return Err(storage_failure()),
    };
    if rows.next().map_err(|_| storage_failure())?.is_some() {
        return Err(storage_failure());
    }
    Ok(Some(coverage))
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
        .prepare_cached("SELECT singleton, membership_id, membership_type FROM active_profile")
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
    if items > 0 {
        return Ok(true);
    }
    // Perk rows are children of items, so with foreign-key enforcement on they
    // cannot outlive their item. They are still counted here so a database that
    // was corrupted with enforcement off cannot hide orphaned perk rows behind
    // an absent anchor.
    let perks: i64 = connection
        .query_row("SELECT count(*) FROM item_perks", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    let plugs: i64 = connection
        .query_row("SELECT count(*) FROM item_plugs", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    let stats: i64 = connection
        .query_row("SELECT count(*) FROM item_stats", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    let available: i64 = connection
        .query_row("SELECT count(*) FROM item_available_plugs", [], |row| {
            row.get(0)
        })
        .map_err(|_| storage_failure())?;
    let options: i64 = connection
        .query_row("SELECT count(*) FROM subclass_plug_options", [], |row| {
            row.get(0)
        })
        .map_err(|_| storage_failure())?;
    let stacks: i64 = connection
        .query_row("SELECT count(*) FROM stacks", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    Ok(perks > 0
        || plugs > 0
        || stats > 0
        || available > 0
        || options > 0
        || stacks > 0
        || read_coverage(connection)?.is_some())
}

/// Reconstruct observations through their validated constructor, preserving multiplicity.
fn read_stacks(
    connection: &Connection,
    characters: &[Character],
) -> Result<Vec<StackObservation>, AppError> {
    let mut statement = connection.prepare_cached(
        "SELECT ordinal, container, owner, bucket_hash, item_hash, quantity FROM stacks ORDER BY ordinal"
    ).map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    let mut stacks = Vec::new();
    for row in rows {
        let (ordinal, kind, owner, bucket, hash, quantity) = row.map_err(|_| storage_failure())?;
        if usize::try_from(ordinal).ok() != Some(stacks.len()) {
            return Err(storage_failure());
        }
        if let Some(owner) = &owner
            && !characters
                .iter()
                .any(|character| character.id.as_str() == owner)
        {
            return Err(storage_failure());
        }
        let container = match (kind.as_str(), owner) {
            ("profile_inventory", None) => StackContainer::ProfileInventory,
            ("profile_currencies", None) => StackContainer::ProfileCurrencies,
            ("character_inventory", Some(owner)) => {
                StackContainer::CharacterInventory(owner.into())
            }
            ("postmaster", Some(owner)) => StackContainer::Postmaster(owner.into()),
            _ => return Err(storage_failure()),
        };
        stacks.push(
            StackObservation::new(
                ItemHash::new(hash),
                BucketHash::new(bucket),
                quantity,
                container,
            )
            .map_err(|_| storage_failure())?,
        );
    }
    stacks.sort();
    Ok(stacks)
}

/// Reconstructs scoped options, rejecting corrupt flags, hashes and references.
fn read_subclass_options(
    connection: &Connection,
    characters: &[Character],
) -> Result<Vec<PlugSetEntry>, AppError> {
    let mut statement = connection
        .prepare_cached("SELECT scope, plug_hash, can_insert, enabled FROM subclass_plug_options")
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    let mut entries = Vec::new();
    let mut keys = HashSet::new();
    for row in rows {
        let (scope, hash, can_insert, enabled) = row.map_err(|_| storage_failure())?;
        let scope = if scope == "profile" {
            PlugSetScope::Profile
        } else {
            if scope.is_empty()
                || scope.len() > 128
                || !scope.bytes().all(|byte| byte.is_ascii_digit())
                || (scope.len() > 1 && scope.starts_with('0'))
                || !characters
                    .iter()
                    .any(|character| character.id.as_str() == scope)
            {
                return Err(storage_failure());
            }
            PlugSetScope::Character(CharacterId::new(scope))
        };
        if u32::try_from(hash).is_err()
            || !keys.insert((scope.clone(), hash))
            || entries.len() >= 4096 * 512
        {
            return Err(storage_failure());
        }
        entries.push(PlugSetEntry {
            scope,
            plug_hash: ItemHash::new(hash),
            can_insert: decode_boolean(can_insert)?,
            enabled: decode_boolean(enabled)?,
        });
    }
    entries.sort();
    Ok(entries)
}

/// Reconstructs every stored character with its optional fields, checked.
fn read_characters(connection: &Connection) -> Result<Vec<Character>, AppError> {
    let mut statement = connection
        .prepare_cached(
            "SELECT character_id, name, class_name, race, gender, light, date_last_played \
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
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|_| storage_failure())?;

    let mut characters = Vec::new();
    for row in rows {
        let (id, name, class, race, gender, light, date_last_played) =
            row.map_err(|_| storage_failure())?;
        characters.push(Character {
            id: CharacterId::new(id),
            name,
            class,
            race,
            gender,
            light: decode_optional_u16(light)?,
            date_last_played: date_last_played
                .as_deref()
                .map(parse_decimal)
                .transpose()?
                .map(Timestamp::new),
        });
    }
    Ok(characters)
}

/// Reconstructs every stored item with its typed location, checked fields, and
/// its ordered perk summaries.
///
/// Items are read in instance-identifier order and perk rows are read in
/// `(instance_id, ordinal)` order, so the two cursors advance together. A stored
/// item's perk ordinals must run contiguously from zero: a gap, a negative or
/// otherwise unexpected ordinal, an unknown item instance, or an unparseable perk
/// value is corrupt local state and fails closed.
fn read_items(connection: &Connection) -> Result<Vec<ItemInstance>, AppError> {
    let transfer = if super::migrations::read_user_version(connection)? >= 24 {
        "transfer_status"
    } else {
        "NULL"
    };
    let sql = format!(
        "SELECT instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power, {transfer} FROM items ORDER BY instance_id"
    );
    let mut statement = connection
        .prepare_cached(&sql)
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
                row.get::<_, Option<i64>>(9)?,
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
            transfer_status,
        ) = row.map_err(|_| storage_failure())?;

        let location = match (
            location_kind.as_str(),
            location_character_id,
            location_other,
        ) {
            ("vault", None, None) => ItemLocation::Vault,
            ("character", Some(character_id), None) => ItemLocation::Character(character_id.into()),
            ("postmaster", Some(character_id), None) if equipped == 0 => {
                ItemLocation::Postmaster(character_id.into())
            }
            ("other", None, Some(other)) => ItemLocation::Other(other),
            _ => return Err(storage_failure()),
        };

        items.push(ItemInstance {
            transfer_status: transfer_status
                .map(u32::try_from)
                .transpose()
                .map_err(|_| storage_failure())?,
            available_plugs: Vec::new(),
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: u32::try_from(quantity).map_err(|_| storage_failure())?,
            equipped: decode_boolean(equipped)?,
            locked: decode_boolean(locked)?,
            power: decode_optional_u16(power)?,
            plugs: Vec::new(),
            stats: Vec::new(),
            perks: Vec::new(),
        });
    }

    attach_components(connection, &mut items, true)?;
    let orphan: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM item_plugs p LEFT JOIN items i \
         ON p.instance_id = i.instance_id WHERE i.instance_id IS NULL) OR \
         EXISTS(SELECT 1 FROM item_stats s LEFT JOIN items i \
         ON s.instance_id = i.instance_id WHERE i.instance_id IS NULL) OR \
         EXISTS(SELECT 1 FROM item_available_plugs a LEFT JOIN items i \
         ON a.instance_id = i.instance_id WHERE i.instance_id IS NULL)",
            [],
            |row| row.get(0),
        )
        .map_err(|_| storage_failure())?;
    if orphan {
        return Err(storage_failure());
    }
    Ok(items)
}

/// Checked selectable plugs: socket order, contiguous per-socket ordinals and unique hashes.
pub(super) fn read_available_plugs(
    connection: &Connection,
    instance_id: &str,
) -> Result<Vec<destiny_core::AvailablePlug>, AppError> {
    let mut statement = connection.prepare_cached("SELECT socket_index, ordinal, plug_hash FROM item_available_plugs WHERE instance_id=?1 ORDER BY socket_index, ordinal").map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([instance_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|_| storage_failure())?;
    decode_available_plugs(
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())?,
    )
}

fn decode_available_plugs(
    rows: Vec<(i64, i64, i64)>,
) -> Result<Vec<destiny_core::AvailablePlug>, AppError> {
    let mut plugs = Vec::new();
    let mut previous = None;
    let mut expected = 0;
    let mut sockets = 0;
    let mut seen = HashSet::new();
    for row in rows {
        let (index, ordinal, hash) = row;
        let index = u8::try_from(index).map_err(|_| storage_failure())?;
        if previous != Some(index) {
            sockets += 1;
            expected = 0;
            previous = Some(index);
        }
        let hash = u32::try_from(hash).map_err(|_| storage_failure())?;
        if sockets > 32 || ordinal != expected || ordinal >= 64 || !seen.insert((index, hash)) {
            return Err(storage_failure());
        }
        expected += 1;
        plugs.push(destiny_core::AvailablePlug {
            socket_index: index,
            plug_hash: ItemHash::new(i64::from(hash)),
        });
    }
    Ok(plugs)
}

/// Shared checked reconstruction of a bounded, hash-sorted stat set.
pub(super) fn read_stats(
    connection: &Connection,
    instance_id: &str,
) -> Result<Vec<destiny_core::StatSummary>, AppError> {
    let mut statement = connection
        .prepare_cached(
            "SELECT stat_hash, value FROM item_stats WHERE instance_id = ?1 ORDER BY stat_hash",
        )
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([instance_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|_| storage_failure())?;
    decode_stats(
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())?,
    )
}

fn decode_stats(rows: Vec<(i64, i64)>) -> Result<Vec<destiny_core::StatSummary>, AppError> {
    let mut stats = Vec::new();
    let mut previous = None;
    for row in rows {
        let (hash, value) = row;
        let hash = u32::try_from(hash).map_err(|_| storage_failure())?;
        if stats.len() >= 64 || previous.is_some_and(|previous| previous >= hash) {
            return Err(storage_failure());
        }
        previous = Some(hash);
        stats.push(destiny_core::StatSummary {
            stat_hash: hash,
            value: i32::try_from(value).map_err(|_| storage_failure())?,
        });
    }
    Ok(stats)
}

/// Shared inventory/search reconstruction of enabled plugs in contiguous order.
pub(super) fn read_plugs(
    connection: &Connection,
    instance_id: &str,
) -> Result<Vec<PlugSummary>, AppError> {
    let mut statement = connection
        .prepare_cached(
            "SELECT ordinal, plug_hash FROM item_plugs WHERE instance_id = ?1 ORDER BY ordinal",
        )
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([instance_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|_| storage_failure())?;
    decode_plugs(
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| storage_failure())?,
    )
}

fn decode_plugs(rows: Vec<(i64, i64)>) -> Result<Vec<PlugSummary>, AppError> {
    let mut plugs = Vec::new();
    for (position, (ordinal, hash)) in rows.into_iter().enumerate() {
        if position >= 32 || ordinal != i64::try_from(position).map_err(|_| storage_failure())? {
            return Err(storage_failure());
        }
        let hash = u32::try_from(hash).map_err(|_| storage_failure())?;
        plugs.push(PlugSummary {
            plug_hash: ItemHash::new(i64::from(hash)),
            enabled: true,
        });
    }
    Ok(plugs)
}

/// Set-based component reconstruction. Full reads include orphan rows so they
/// fail closed; candidate reads use bounded IN chunks below SQLite's bind limit.
fn attach_component<T>(
    connection: &Connection,
    items: &mut [ItemInstance],
    full: bool,
    select: &str,
    order: &str,
    decode: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    attach: impl Fn(&mut ItemInstance, Vec<T>) -> Result<(), AppError>,
) -> Result<(), AppError> {
    let chunk_size = if full { items.len().max(1) } else { 500 };
    // An empty full projection must still detect orphan component rows.
    let chunks = items.len().max(1).div_ceil(chunk_size);
    for chunk in 0..chunks {
        let start = (chunk * chunk_size).min(items.len());
        let end = (start + chunk_size).min(items.len());
        let items = &mut items[start..end];
        let mut sql = select.to_owned();
        if !full {
            if items.is_empty() {
                continue;
            }
            sql.push_str(" WHERE instance_id IN (");
            sql.push_str(&vec!["?"; items.len()].join(","));
            sql.push(')');
        }
        sql.push_str(" ORDER BY instance_id, ");
        sql.push_str(order);
        let mut statement = connection
            .prepare_cached(&sql)
            .map_err(|_| storage_failure())?;
        let ids: Vec<&str> = if full {
            Vec::new()
        } else {
            items.iter().map(|item| item.instance_id.as_str()).collect()
        };
        let rows = statement
            .query_map(rusqlite::params_from_iter(ids), |row| {
                Ok((row.get::<_, String>(0)?, decode(row)?))
            })
            .map_err(|_| storage_failure())?;
        let mut grouped = std::collections::HashMap::<String, Vec<T>>::new();
        for row in rows {
            let (id, value) = row.map_err(|_| storage_failure())?;
            grouped.entry(id).or_default().push(value);
        }
        for item in items {
            let rows = grouped
                .remove(item.instance_id.as_str())
                .unwrap_or_default();
            attach(item, rows)?;
        }
        if !grouped.is_empty() {
            return Err(storage_failure());
        }
    }
    Ok(())
}

pub(super) fn attach_components(
    connection: &Connection,
    items: &mut [ItemInstance],
    full: bool,
) -> Result<(), AppError> {
    attach_component(
        connection,
        items,
        full,
        "SELECT instance_id, ordinal, plug_hash FROM item_plugs",
        "ordinal",
        |row| Ok((row.get(1)?, row.get(2)?)),
        |item, rows| {
            item.plugs = decode_plugs(rows)?;
            Ok(())
        },
    )?;
    attach_component(
        connection,
        items,
        full,
        "SELECT instance_id, stat_hash, value FROM item_stats",
        "stat_hash",
        |row| Ok((row.get(1)?, row.get(2)?)),
        |item, rows| {
            item.stats = decode_stats(rows)?;
            Ok(())
        },
    )?;
    attach_component(
        connection,
        items,
        full,
        "SELECT instance_id, socket_index, ordinal, plug_hash FROM item_available_plugs",
        "socket_index, ordinal",
        |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)),
        |item, rows| {
            item.available_plugs = decode_available_plugs(rows)?;
            Ok(())
        },
    )?;
    attach_component(
        connection,
        items,
        full,
        "SELECT instance_id, ordinal, perk_hash, active, visible FROM item_perks",
        "ordinal",
        |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        |item, rows| {
            item.perks = decode_perks(rows)?;
            Ok(())
        },
    )
}

pub(super) fn decode_perks(rows: Vec<(i64, i64, i64, i64)>) -> Result<Vec<PerkSummary>, AppError> {
    let mut perks = Vec::new();
    for (position, (ordinal, hash, active, visible)) in rows.into_iter().enumerate() {
        if ordinal != i64::try_from(position).map_err(|_| storage_failure())? {
            return Err(storage_failure());
        }
        perks.push(PerkSummary {
            perk_hash: PerkHash::new(u32::try_from(hash).map_err(|_| storage_failure())?),
            active: decode_boolean(active)?,
            visible: decode_boolean(visible)?,
        });
    }
    Ok(perks)
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

/// Executes a hot per-row write using the connection's statement cache.
fn execute_cached(
    connection: &Connection,
    sql: &str,
    parameters: impl rusqlite::Params,
) -> Result<(), AppError> {
    connection
        .prepare_cached(sql)
        .map_err(|_| storage_failure())?
        .execute(parameters)
        .map_err(|_| storage_failure())?;
    Ok(())
}

/// Writes canonical anonymous rows, retaining duplicates and contiguous ordinals.
fn insert_stacks(connection: &Connection, stacks: &[StackObservation]) -> Result<(), AppError> {
    for (ordinal, stack) in stacks.iter().enumerate() {
        let (container, owner) = match stack.container() {
            StackContainer::ProfileInventory => ("profile_inventory", None),
            StackContainer::ProfileCurrencies => ("profile_currencies", None),
            StackContainer::CharacterInventory(owner) => {
                ("character_inventory", Some(owner.as_str()))
            }
            StackContainer::Postmaster(owner) => ("postmaster", Some(owner.as_str())),
        };
        execute_cached(connection,
            "INSERT INTO stacks(ordinal,container,owner,bucket_hash,item_hash,quantity) VALUES(?1,?2,?3,?4,?5,?6)",
            params![i64::try_from(ordinal).map_err(|_| storage_failure())?, container, owner,
                stack.bucket_hash().value(), stack.item_hash().value(), i64::from(stack.quantity())],
        ).map_err(|_| storage_failure())?;
    }
    Ok(())
}

/// Applies the row diff within the caller's publication transaction.
/// Changed items are replaced as units; foreign-key cascades remove only their
/// components. New characters precede dependent rows, and removed characters
/// follow all dependent deletes/updates. Anonymous stacks use the schema's
/// canonical ordinal key, preserving duplicates and contiguous ordinals.
fn replace_projection(
    connection: &Connection,
    prior: Option<&InventorySnapshot>,
    input: &InventorySnapshotInput,
) -> Result<(), AppError> {
    let old_items: HashMap<_, _> = prior
        .into_iter()
        .flat_map(|p| &p.items)
        .map(|item| (&item.instance_id, item))
        .collect();
    let new_items: HashMap<_, _> = input
        .items
        .iter()
        .map(|item| (&item.instance_id, item))
        .collect();
    let old_characters: HashMap<_, _> = prior
        .into_iter()
        .flat_map(|p| &p.characters)
        .map(|character| (&character.id, character))
        .collect();
    for (&id, &item) in &old_items {
        if new_items.get(id).copied() != Some(item) {
            execute_cached(
                connection,
                "DELETE FROM items WHERE instance_id = ?1",
                [id.as_str()],
            )?;
        }
    }
    let old_stacks = prior.map_or(&[][..], |p| p.stacks.as_slice());
    for ordinal in input.stacks.len()..old_stacks.len() {
        execute_cached(
            connection,
            "DELETE FROM stacks WHERE ordinal = ?1",
            [i64::try_from(ordinal).map_err(|_| storage_failure())?],
        )?;
    }
    let old_options = prior.map_or(&[][..], |p| p.plug_set_entries.as_slice());
    for entry in old_options {
        if !input
            .plug_set_entries
            .iter()
            .any(|new| new.scope == entry.scope && new.plug_hash == entry.plug_hash)
        {
            let scope = match &entry.scope {
                PlugSetScope::Profile => "profile",
                PlugSetScope::Character(id) => id.as_str(),
            };
            execute_cached(
                connection,
                "DELETE FROM subclass_plug_options WHERE scope = ?1 AND plug_hash = ?2",
                params![scope, entry.plug_hash.value()],
            )?;
        }
    }

    if prior.is_none_or(|p| {
        p.metadata.membership_id != input.membership_id
            || p.metadata.membership_type != input.membership_type
    }) {
        execute_cached(
            connection,
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
    }

    for character in &input.characters {
        if old_characters.get(&character.id).copied() == Some(character) {
            continue;
        }
        execute_cached(connection,
                "INSERT INTO characters (character_id, name, class_name, race, gender, light, date_last_played) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
                 ON CONFLICT(character_id) DO UPDATE SET name = excluded.name, class_name = excluded.class_name, \
                 race = excluded.race, gender = excluded.gender, light = excluded.light, date_last_played = excluded.date_last_played",
                params![
                    character.id.as_str(),
                    character.name,
                    character.class,
                    character.race,
                    character.gender,
                    character.light.map(i64::from),
                    character.date_last_played.map(|date| encode_decimal(date.seconds())),
                ],
            )
            .map_err(|_| storage_failure())?;
    }

    let coverage = match input.profile_currencies_coverage {
        ProfileCurrenciesCoverage::NotObserved => "not_observed",
        ProfileCurrenciesCoverage::Unavailable => "unavailable",
        ProfileCurrenciesCoverage::Available => "available",
    };
    if prior.is_none_or(|p| p.profile_currencies_coverage != input.profile_currencies_coverage) {
        execute_cached(
            connection,
            "INSERT INTO inventory_coverage(id, profile_currencies) VALUES(1, ?1) \
         ON CONFLICT(id) DO UPDATE SET profile_currencies = excluded.profile_currencies",
            [coverage],
        )
        .map_err(|_| storage_failure())?;
    }
    for (ordinal, stack) in input.stacks.iter().enumerate() {
        if old_stacks.get(ordinal) == Some(stack) {
            continue;
        }
        let (container, owner) = match stack.container() {
            StackContainer::ProfileInventory => ("profile_inventory", None),
            StackContainer::ProfileCurrencies => ("profile_currencies", None),
            StackContainer::CharacterInventory(id) => ("character_inventory", Some(id.as_str())),
            StackContainer::Postmaster(id) => ("postmaster", Some(id.as_str())),
        };
        execute_cached(
            connection,
            "INSERT INTO stacks(ordinal,container,owner,bucket_hash,item_hash,quantity) VALUES(?1,?2,?3,?4,?5,?6) \
             ON CONFLICT(ordinal) DO UPDATE SET container = excluded.container, owner = excluded.owner, \
             bucket_hash = excluded.bucket_hash, item_hash = excluded.item_hash, quantity = excluded.quantity",
            params![
                i64::try_from(ordinal).map_err(|_| storage_failure())?,
                container,
                owner,
                stack.bucket_hash().value(),
                stack.item_hash().value(),
                i64::from(stack.quantity())
            ],
        )?;
    }

    let has_transfer = super::migrations::read_user_version(connection)? >= 24;
    for item in &input.items {
        if old_items.get(&item.instance_id).copied() == Some(item) {
            continue;
        }
        let (location_kind, location_character_id, location_other) =
            encode_location(&item.location);
        let mut values: Vec<rusqlite::types::Value> = vec![
            item.instance_id.as_str().to_owned().into(),
            item.item_hash.value().into(),
            location_kind.to_owned().into(),
            location_character_id.map(str::to_owned).into(),
            location_other.map(str::to_owned).into(),
            i64::from(item.quantity).into(),
            encode_boolean(item.equipped).into(),
            encode_boolean(item.locked).into(),
            item.power.map(i64::from).into(),
        ];
        let sql = if has_transfer {
            values.push(item.transfer_status.map(i64::from).into());
            "INSERT INTO items(instance_id,item_hash,location_kind,location_character_id,location_other,quantity,equipped,locked,power,transfer_status) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"
        } else {
            if item.transfer_status.is_some() {
                return Err(storage_failure());
            }
            "INSERT INTO items(instance_id,item_hash,location_kind,location_character_id,location_other,quantity,equipped,locked,power) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)"
        };
        execute_cached(connection, sql, rusqlite::params_from_iter(values))?;

        let mut ordinals = std::collections::HashMap::new();
        for plug in &item.available_plugs {
            let ordinal = ordinals.entry(plug.socket_index).or_insert(0_i64);
            execute_cached(connection, "INSERT INTO item_available_plugs(instance_id,socket_index,ordinal,plug_hash) VALUES(?1,?2,?3,?4)", params![item.instance_id.as_str(), i64::from(plug.socket_index), *ordinal, plug.plug_hash.value()]).map_err(|_| storage_failure())?;
            *ordinal += 1;
        }
        for stat in &item.stats {
            execute_cached(
                connection,
                "INSERT INTO item_stats (instance_id, stat_hash, value) VALUES (?1, ?2, ?3)",
                params![
                    item.instance_id.as_str(),
                    i64::from(stat.stat_hash),
                    i64::from(stat.value)
                ],
            )?;
        }
        for (ordinal, plug) in item.plugs.iter().enumerate() {
            execute_cached(
                connection,
                "INSERT INTO item_plugs (instance_id, ordinal, plug_hash) VALUES (?1, ?2, ?3)",
                params![
                    item.instance_id.as_str(),
                    i64::try_from(ordinal).map_err(|_| storage_failure())?,
                    plug.plug_hash.value()
                ],
            )?;
        }
        for (ordinal, perk) in item.perks.iter().enumerate() {
            execute_cached(
                connection,
                "INSERT INTO item_perks \
                     (instance_id, ordinal, perk_hash, active, visible) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    item.instance_id.as_str(),
                    i64::try_from(ordinal).map_err(|_| storage_failure())?,
                    i64::from(perk.perk_hash.value()),
                    encode_boolean(perk.active),
                    encode_boolean(perk.visible),
                ],
            )?;
        }
    }
    for character in prior.into_iter().flat_map(|p| &p.characters) {
        if !input.characters.iter().any(|new| new.id == character.id) {
            execute_cached(
                connection,
                "DELETE FROM characters WHERE character_id = ?1",
                [character.id.as_str()],
            )?;
        }
    }
    for entry in &input.plug_set_entries {
        if old_options.contains(entry) {
            continue;
        }
        let scope = match &entry.scope {
            PlugSetScope::Profile => "profile",
            PlugSetScope::Character(id) => id.as_str(),
        };
        execute_cached(connection,
                "INSERT INTO subclass_plug_options (scope, plug_hash, can_insert, enabled) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(scope, plug_hash) DO UPDATE SET can_insert = excluded.can_insert, enabled = excluded.enabled",
                params![
                    scope,
                    entry.plug_hash.value(),
                    encode_boolean(entry.can_insert),
                    encode_boolean(entry.enabled),
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
