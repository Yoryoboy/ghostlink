//! Transactional SQLite persistence for the normalized Manifest definition set.
//!
//! This module implements [`destiny_core::DefinitionRepository`] on the
//! application-owned [`SqliteStore`]. It performs no provider I/O, no async
//! work, and no serialization: every value it stores arrives already normalized,
//! and every read reconstructs the provider-neutral core records from normalized
//! columns, so no raw provider payload, SQL statement, or stored value can
//! escape through a returned value or a failure.
//!
//! Publication is exactly one `IMMEDIATE` transaction. Inside it the repository
//! reads the committed Manifest metadata and the committed catalog, decides
//! whether the incoming complete set is a content change, allocates or retains
//! the revision, replaces every definition and tag row, and upserts the
//! committed metadata. Any failure, including a constraint failure partway
//! through the replacement, drops the transaction, and rusqlite rolls the whole
//! thing back, so the prior catalog, its tags, the metadata, the revision, and
//! the version remain exactly readable.
//!
//! Revision identity is the complete normalized definition content and its
//! version, compared structurally; there is no fingerprint or hash, because
//! distinct normalized inputs can collide on a single `u64`. The committed
//! catalog is reconstructed from the normalized columns and compared field by
//! field. Definition order is deliberately not part of identity: the schema
//! stores no definition ordinal and reads are ordered by primary key, so an
//! identical set retains its revision regardless of input order. Tag order is
//! part of identity because the normalized [`ItemDefinition`] carries an ordered
//! `Vec`, and the schema preserves that order through `definition_tags.ordinal`.
//! `checked_at` is committed freshness, never content identity, so an otherwise
//! identical refresh retains the revision and refreshes only the timestamp.
//!
//! Validation that needs no storage runs before the transaction is opened and
//! reports a sanitized provider-validation failure: the source marker must be
//! the fixed Manifest marker, the set must be non-empty, every definition must
//! carry the one exact set version, and hashes must be duplicate-free. Every
//! failure after that is treated as local state and maps only to the sanitized
//! [`ErrorCategory::LocalStorage`] category.

use super::{SqliteStore, encode_decimal, parse_decimal, storage_failure};
use destiny_core::domain::{ClassType, DamageType, ItemSlot};
use destiny_core::{
    AppError, BucketHash, Dataset, DefinitionReplacementInput, DefinitionRepository,
    DefinitionVersion, ErrorCategory, ItemDefinition, ItemHash, ManifestDefinitions, Resolution,
    SourceMarker, SyncMetadata, SyncRevision, Timestamp,
};
use rusqlite::{Connection, Error as SqlError, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};

/// The fixed `sync_metadata` dataset row this repository owns.
const MANIFEST_DATASET: &str = "manifest_definitions";

/// The source marker the schema pairs with the Manifest dataset.
const MANIFEST_SOURCE: &str = "manifest";

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

impl DefinitionRepository for SqliteStore {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        match read_manifest_metadata(self.connection())? {
            Some(metadata) => Ok(metadata.definition_version),
            None => Err(storage_failure()),
        }
    }

    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        Ok(match read_definition(self.connection(), hash)? {
            Some(definition) => Resolution::Unique(definition),
            None => Resolution::Missing,
        })
    }

    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        read_manifest_metadata(self.connection())
    }

    fn replace_definitions(
        &mut self,
        replacement: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.publish_catalog(replacement, None)
    }

    fn replace_definitions_with_descriptions(
        &mut self,
        replacement: DefinitionReplacementInput,
        descriptions: Vec<destiny_core::ItemDescription>,
    ) -> Result<SyncMetadata, AppError> {
        self.publish_catalog_with_descriptions(replacement, None, descriptions)
    }
}

impl SqliteStore {
    pub(super) fn publish_catalog(
        &mut self,
        replacement: DefinitionReplacementInput,
        world: Option<destiny_core::WorldDefinitionsAcquisition>,
    ) -> Result<SyncMetadata, AppError> {
        let descriptions = world
            .as_ref()
            .map(|world| world.item_descriptions.clone())
            .unwrap_or_default();
        self.publish_catalog_with_descriptions(replacement, world, descriptions)
    }

    fn publish_catalog_with_descriptions(
        &mut self,
        replacement: DefinitionReplacementInput,
        world: Option<destiny_core::WorldDefinitionsAcquisition>,
        mut descriptions: Vec<destiny_core::ItemDescription>,
    ) -> Result<SyncMetadata, AppError> {
        // Test seams permit legacy schemas. Refuse known metadata before any
        // transaction or freshness write when it cannot be represented.
        if super::migrations::read_user_version(self.connection())? < 18
            && replacement
                .definitions
                .definitions
                .iter()
                .any(|definition| {
                    definition.does_postmaster_pull_have_side_effects.is_some()
                        || definition.is_instance_item.is_some()
                        || definition.max_stack_size.is_some()
                        || definition.stack_unique_label.is_some()
                })
        {
            return Err(storage_failure());
        }
        if super::migrations::read_user_version(self.connection())? < 26
            && replacement
                .definitions
                .definitions
                .iter()
                .any(|item| item.armor != destiny_core::ArmorDefinition::default())
        {
            return Err(storage_failure());
        }
        if super::migrations::read_user_version(self.connection())? < 27
            && replacement.definitions.definitions.iter().any(|item| {
                !item.armor.socket_entries.is_empty() || !item.armor.socket_categories.is_empty()
            })
        {
            return Err(storage_failure());
        }
        if super::migrations::read_user_version(self.connection())? < 32
            && replacement.definitions.definitions.iter().any(|item| {
                item.armor.insertion_material_requirement_hash.is_some()
                    || item.armor.energy_capacity_type.is_some()
                    || item.armor.plug_availability.is_some()
            })
        {
            return Err(storage_failure());
        }
        if super::migrations::read_user_version(self.connection())? < 33
            && replacement
                .definitions
                .definitions
                .iter()
                .any(|item| item.armor.unique_label.is_some())
        {
            return Err(storage_failure());
        }
        if super::migrations::read_user_version(self.connection())? < 35
            && replacement.definitions.definitions.iter().any(|item| {
                !item.armor.item_category_hashes.is_empty()
                    || item.armor.item_type_code.is_some()
                    || item.armor.item_sub_type_code.is_some()
            })
        {
            return Err(storage_failure());
        }
        if let Some(world) = &world {
            if super::migrations::read_user_version(self.connection())? < 32
                && world.armor_catalogs.iter().any(|r| {
                    r.kind()
                        == destiny_core::armor_catalog::ArmorCatalogKind::MaterialRequirementSet
                })
            {
                return Err(storage_failure());
            }
            world.validate_for(&replacement.definitions.version)?;
            if super::migrations::read_user_version(self.connection())? < 36
                && world
                    .armor_catalogs
                    .iter()
                    .any(|r| r.kind() == destiny_core::armor_catalog::ArmorCatalogKind::StatPlug)
            {
                return Err(storage_failure());
            }
            if super::migrations::read_user_version(self.connection())? < 34
                && world.armor_catalogs.iter().any(|r| r.kind().id() >= 6)
            {
                return Err(storage_failure());
            }
            if super::migrations::read_user_version(self.connection())? < 29
                && world.armor_catalogs.iter().any(|r| r.kind().id() >= 2)
            {
                return Err(storage_failure());
            }
            if super::migrations::read_user_version(self.connection())? < 28
                && !world.armor_catalogs.is_empty()
            {
                return Err(storage_failure());
            }
        }
        destiny_core::world_definitions::validate_item_descriptions(
            &replacement.definitions,
            &descriptions,
        )?;
        descriptions.sort_by_key(destiny_core::ItemDescription::hash);
        // Everything that can be rejected without touching storage is rejected
        // here, before the transaction is opened, so the prior committed state
        // stays readable and no write is ever attempted.
        if replacement.source != SourceMarker::Manifest {
            return Err(provider_validation());
        }
        validate_publication(&replacement.definitions)?;

        // One IMMEDIATE transaction determines the prior state, allocates the
        // revision, replaces the catalog, and upserts metadata. Rusqlite rolls
        // the transaction back on drop, so any early return below leaves the
        // prior committed state untouched.
        let transaction = self
            .connection_mut()
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| storage_failure())?;

        if !descriptions.is_empty() && super::migrations::read_user_version(&transaction)? < 14 {
            return Err(provider_validation());
        }
        let prior = read_manifest_metadata(&transaction)?;
        let unchanged = match prior.as_ref() {
            Some(_) => {
                let committed = read_committed_content(&transaction)?;
                contents_match(&committed, &replacement.definitions.definitions)
                    && (super::migrations::read_user_version(&transaction)? < 14
                        || super::world::read_item_descriptions(&transaction)? == descriptions)
                    && match &world {
                        Some(world) => super::world::contents_match(&transaction, world)?,
                        None => true,
                    }
            }
            None => false,
        };

        // Revision allocation happens before any write. An overflow at the
        // boundary therefore fails with the catalog, tags, and metadata intact.
        let revision = if unchanged {
            prior
                .as_ref()
                .map(|metadata| metadata.revision)
                .ok_or_else(storage_failure)?
        } else {
            match prior.as_ref() {
                Some(metadata) => SyncRevision::new(
                    metadata
                        .revision
                        .value()
                        .checked_add(1)
                        .ok_or_else(storage_failure)?,
                ),
                None => SyncRevision::new(1),
            }
        };

        // Unchanged content keeps the committed catalog and rewrites only the
        // freshness metadata; changed content replaces every row.
        if !unchanged {
            replace_catalog(&transaction, &replacement.definitions.definitions)?;
            if let Some(world) = &world {
                super::world::replace_catalog(&transaction, world)?;
            } else if super::migrations::read_user_version(&transaction)? >= 3 {
                // An item-only publication must not leave stale world rows
                // carrying a different version under the shared metadata.
                super::world::clear_catalog(&transaction)?;
            }
            if world.is_none() && super::migrations::read_user_version(&transaction)? >= 14 {
                super::world::replace_item_descriptions(&transaction, &descriptions)?;
            }
        }
        if super::migrations::read_user_version(&transaction)? >= 22 {
            super::search::validate_search_coverage(&transaction)?;
        }
        upsert_metadata(&transaction, &replacement, revision)?;
        transaction.commit().map_err(|_| storage_failure())?;

        Ok(SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: replacement.checked_at,
            source: SourceMarker::Manifest,
            revision,
            definition_version: replacement.definitions.version,
        })
    }
}

impl destiny_core::ports::DefinitionCatalogRepository for SqliteStore {
    fn definition_count(&self) -> Result<usize, AppError> {
        let count: i64 = self
            .connection()
            .query_row("SELECT COUNT(*) FROM definitions", [], |row| row.get(0))
            .map_err(|_| storage_failure())?;
        usize::try_from(count).map_err(|_| storage_failure())
    }
}

/// Rejects a set that the application Manifest service should never publish.
///
/// A publishable set is non-empty, internally consistent with one exact version,
/// and duplicate-free. This mirrors the deterministic fake so the two
/// repositories accept and reject the same inputs before any durable mutation.
fn validate_publication(definitions: &ManifestDefinitions) -> Result<(), AppError> {
    if definitions.definitions.is_empty() {
        return Err(provider_validation());
    }
    let mut seen = HashSet::with_capacity(definitions.definitions.len());
    for definition in &definitions.definitions {
        definition.armor.validate()?;
        if definition.version != definitions.version || !seen.insert(definition.hash) {
            return Err(provider_validation());
        }
    }
    Ok(())
}

/// Compares the committed catalog with an incoming set as one normalized set.
///
/// Both sides are ordered by the definition primary key before comparison, so
/// the result depends only on normalized content and never on input or storage
/// order. Tag vectors are compared in their stored ordinal order, so tag
/// reordering is a content change while definition reordering is not.
fn contents_match(committed: &[ItemDefinition], incoming: &[ItemDefinition]) -> bool {
    if committed.len() != incoming.len() {
        return false;
    }
    let mut committed: Vec<&ItemDefinition> = committed.iter().collect();
    let mut incoming: Vec<&ItemDefinition> = incoming.iter().collect();
    committed.sort_by_key(|definition| definition.hash);
    incoming.sort_by_key(|definition| definition.hash);
    committed == incoming
}

/// Removes the complete prior catalog and writes the complete replacement.
///
/// Tags are cleared before definitions so the removal order is explicit and
/// deterministic instead of relying on cascade, and the child rows are gone
/// before the parent rows the foreign key points at.
fn replace_catalog(
    connection: &Connection,
    definitions: &[ItemDefinition],
) -> Result<(), AppError> {
    connection
        .execute("DELETE FROM definition_tags", [])
        .map_err(|_| storage_failure())?;
    connection
        .execute("DELETE FROM definitions", [])
        .map_err(|_| storage_failure())?;

    let schema = super::migrations::read_user_version(connection)?;
    let has_attributes = schema >= 5;
    let has_plug_category = schema >= 6;
    let has_qualified_metadata = schema >= 18;
    let mut insert_definition = connection
        .prepare_cached(if schema >= 24 {
            "INSERT INTO definitions (hash, bucket_hash, name, item_type, tier, element, definition_version, damage_type, slot, class_type, plug_category, does_postmaster_pull_have_side_effects, is_instance_item, max_stack_size, stack_unique_label, non_transferrable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        } else if has_qualified_metadata {
            "INSERT INTO definitions \
             (hash, bucket_hash, name, item_type, tier, element, definition_version, damage_type, slot, class_type, plug_category, does_postmaster_pull_have_side_effects, is_instance_item, max_stack_size, stack_unique_label) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        } else if has_plug_category {
            "INSERT INTO definitions \
             (hash, bucket_hash, name, item_type, tier, element, definition_version, damage_type, slot, class_type, plug_category) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
        } else if has_attributes {
            "INSERT INTO definitions \
             (hash, bucket_hash, name, item_type, tier, element, definition_version, damage_type, slot, class_type) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
        } else {
            "INSERT INTO definitions \
             (hash, bucket_hash, name, item_type, tier, element, definition_version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        })
        .map_err(|_| storage_failure())?;
    let mut insert_tag = connection
        .prepare_cached(
            "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (?1, ?2, ?3)",
        )
        .map_err(|_| storage_failure())?;
    for definition in definitions {
        let mut values = vec![
            rusqlite::types::Value::from(definition.hash.value()),
            definition.bucket_hash.map(BucketHash::value).into(),
            definition.name.clone().into(),
            definition.item_type.clone().into(),
            definition.tier.clone().into(),
            definition.element.clone().into(),
            definition.version.as_str().to_owned().into(),
        ];
        if has_attributes {
            values.extend([
                definition
                    .element
                    .as_deref()
                    .and_then(|value| value.parse::<DamageType>().ok())
                    .map(|value| value.as_str().to_owned())
                    .into(),
                definition
                    .slot
                    .map(|value| value.as_str().to_owned())
                    .into(),
                definition
                    .class_type
                    .map(|value| value.as_str().to_owned())
                    .into(),
            ]);
        } else if definition.slot.is_some() || definition.class_type.is_some() {
            return Err(storage_failure());
        }
        let plug_category = validate_plug_category(definition.plug_category.clone())?;
        if has_plug_category {
            values.push(plug_category.into());
        } else if plug_category.is_some() {
            return Err(storage_failure());
        }
        if has_qualified_metadata {
            values.extend([
                definition
                    .does_postmaster_pull_have_side_effects
                    .map(i64::from)
                    .into(),
                definition.is_instance_item.map(i64::from).into(),
                definition.max_stack_size.map(i64::from).into(),
                definition.stack_unique_label.clone().into(),
            ]);
        }
        if schema >= 24 {
            values.push(definition.non_transferrable.map(i64::from).into());
        } else if definition.non_transferrable.is_some() {
            return Err(storage_failure());
        }
        insert_definition
            .execute(rusqlite::params_from_iter(values))
            .map_err(|_| storage_failure())?;
        for (ordinal, tag) in definition.tags.iter().enumerate() {
            let ordinal = i64::try_from(ordinal).map_err(|_| storage_failure())?;
            insert_tag
                .execute(params![definition.hash.value(), ordinal, tag])
                .map_err(|_| storage_failure())?;
        }
    }
    super::armor_definitions::write(connection, definitions)?;
    if schema >= 22 {
        write_search_rows(connection, definitions)?;
    }
    Ok(())
}

/// Rust normalization and name-kind classification are shared with the arbiter.
fn write_search_rows(
    connection: &Connection,
    definitions: &[ItemDefinition],
) -> Result<(), AppError> {
    use destiny_core::{name_matching::is_name_kind, normalize_search_text};
    let mut insert = connection.prepare_cached(
        "INSERT INTO definition_search (hash, name, name_norm, item_type_norm, tier_norm, tags_norm, element, slot, class_type, perk, mod, archetype) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
    ).map_err(|_| storage_failure())?;
    for definition in definitions {
        // Normalization collapses all whitespace, including newline, so newline
        // delimiters cannot occur inside a token. No LIKE wildcards are involved.
        let mut tags = String::from("\n");
        for tag in &definition.tags {
            tags.push_str(&normalize_search_text(tag));
            tags.push('\n');
        }
        insert
            .execute(params![
                definition.hash.value(),
                definition.name,
                normalize_search_text(&definition.name),
                definition.item_type.as_deref().map(normalize_search_text),
                definition.tier.as_deref().map(normalize_search_text),
                tags,
                definition
                    .element
                    .as_deref()
                    .and_then(|v| v.parse::<DamageType>().ok())
                    .map(|v| v.as_str()),
                definition.slot.map(|v| v.as_str()),
                definition.class_type.map(|v| v.as_str()),
                is_name_kind(definition, "perk"),
                is_name_kind(definition, "mod"),
                is_name_kind(definition, "archetype"),
            ])
            .map_err(|_| storage_failure())?;
    }
    Ok(())
}

/// Called exactly once in the migration transaction; corruption aborts the step.
pub(super) fn backfill_search(connection: &Connection) -> Result<(), AppError> {
    write_search_rows(connection, &read_committed_content(connection)?)
}

/// Inserts or refreshes the committed Manifest metadata row.
///
/// The row is the sole durable source of the committed revision, freshness
/// timestamp, and definition version, so it is upserted on every successful
/// publication, including one that retained the revision.
fn upsert_metadata(
    connection: &Connection,
    replacement: &DefinitionReplacementInput,
    revision: SyncRevision,
) -> Result<(), AppError> {
    let checked_at = encode_decimal(replacement.checked_at.seconds());
    let revision_text = encode_decimal(revision.value());
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
                MANIFEST_DATASET,
                checked_at,
                MANIFEST_SOURCE,
                revision_text,
                replacement.definitions.version.as_str(),
            ],
        )
        .map_err(|_| storage_failure())?;
    Ok(())
}

/// Reads the committed Manifest metadata, if any.
///
/// Canonical decimal values are parsed and range-checked rather than trusted,
/// and any malformed or unexpected stored value is a sanitized local-storage
/// failure, never a partially constructed core record.
pub(super) fn read_manifest_metadata(
    connection: &Connection,
) -> Result<Option<SyncMetadata>, AppError> {
    let row = connection.query_row(
        "SELECT checked_at, source, revision, definition_version \
         FROM sync_metadata WHERE dataset = ?1",
        [MANIFEST_DATASET],
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
            if source != MANIFEST_SOURCE {
                return Err(storage_failure());
            }
            Ok(Some(SyncMetadata {
                dataset: Dataset::ManifestDefinitions,
                checked_at: Timestamp::new(parse_decimal(&checked_at)?),
                source: SourceMarker::Manifest,
                revision: SyncRevision::new(parse_decimal(&revision)?),
                definition_version: DefinitionVersion::new(definition_version),
            }))
        }
        Err(SqlError::QueryReturnedNoRows) => Ok(None),
        Err(_) => Err(storage_failure()),
    }
}

/// Reconstructs one normalized definition with its ordered tags, if present.
///
/// The definition primary key makes the outcome `Missing` or `Unique`; the
/// `Ambiguous` resolution the port allows cannot occur here.
fn read_definition(
    connection: &Connection,
    hash: ItemHash,
) -> Result<Option<ItemDefinition>, AppError> {
    let row = connection.query_row(
        &definition_select(connection, false)?,
        [hash.value()],
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                read_bool(row, 10)?,
                read_bool(row, 11)?,
                row.get::<_, Option<i32>>(12)?,
                row.get::<_, Option<String>>(13)?,
                read_bool(row, 14)?,
            ))
        },
    );

    match row {
        Ok((
            bucket_hash,
            name,
            item_type,
            tier,
            element,
            version,
            damage,
            slot,
            class_type,
            plug_category,
            does_postmaster_pull_have_side_effects,
            is_instance_item,
            max_stack_size,
            stack_unique_label,
            non_transferrable,
        )) => Ok(Some(ItemDefinition {
            armor: super::armor_definitions::read(connection, Some(hash.value()))?
                .remove(&hash.value())
                .unwrap_or_default(),
            non_transferrable,
            does_postmaster_pull_have_side_effects,
            is_instance_item,
            max_stack_size,
            stack_unique_label,
            hash,
            bucket_hash: bucket_hash.map(BucketHash::new),
            name,
            item_type,
            tier,
            element: validate_element(element, damage)?,
            slot: decode_attribute(slot, ItemSlot::as_str)?,
            class_type: decode_attribute(class_type, ClassType::as_str)?,
            plug_category: validate_plug_category(plug_category)?,
            tags: read_tags(connection, hash.value())?,
            version: DefinitionVersion::new(version),
        })),
        Err(SqlError::QueryReturnedNoRows) => Ok(None),
        Err(_) => Err(storage_failure()),
    }
}

// Use one schema read per query, never per catalog row. Legacy schemas have
// genuinely unknown fields; v18 must read the actual columns or fail closed.
fn definition_select(connection: &Connection, catalog: bool) -> Result<String, AppError> {
    let schema = super::migrations::read_user_version(connection)?;
    let attributes = if schema >= 5 {
        "damage_type, slot, class_type"
    } else {
        "NULL, NULL, NULL"
    };
    let plug = if schema >= 6 { "plug_category" } else { "NULL" };
    let qualified = if schema >= 18 {
        "does_postmaster_pull_have_side_effects, is_instance_item, max_stack_size, stack_unique_label"
    } else {
        "NULL, NULL, NULL, NULL"
    };
    let transfer = if schema >= 24 {
        "non_transferrable"
    } else {
        "NULL"
    };
    let hash = if catalog { "hash, " } else { "" };
    let suffix = if catalog {
        "ORDER BY hash"
    } else {
        "WHERE hash = ?1"
    };
    Ok(format!(
        "SELECT {hash}bucket_hash, name, item_type, tier, element, definition_version, {attributes}, {plug}, {qualified}, {transfer} FROM definitions {suffix}"
    ))
}

// rusqlite's bool conversion treats every nonzero integer as true; that would
// conceal corruption. Only the two canonical stored integers are accepted.
fn read_bool(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<bool>> {
    match row.get::<_, Option<i64>>(index)? {
        None => Ok(None),
        Some(0) => Ok(Some(false)),
        Some(1) => Ok(Some(true)),
        Some(_) => Err(SqlError::InvalidQuery),
    }
}

fn validate_plug_category(raw: Option<String>) -> Result<Option<String>, AppError> {
    if raw.as_ref().is_some_and(|value| {
        value.is_empty() || value.chars().count() > 256 || value.contains('\0')
    }) {
        return Err(storage_failure());
    }
    Ok(raw)
}

/// Stored names must be canonical, not merely parseable user input.
fn decode_attribute<T: std::str::FromStr + Copy>(
    raw: Option<String>,
    canonical: fn(T) -> &'static str,
) -> Result<Option<T>, AppError> {
    raw.map(|raw| {
        let value = raw.parse::<T>().map_err(|_| storage_failure())?;
        if canonical(value) != raw {
            return Err(storage_failure());
        }
        Ok(value)
    })
    .transpose()
}

fn validate_element(
    element: Option<String>,
    raw: Option<String>,
) -> Result<Option<String>, AppError> {
    if let Some(damage) = decode_attribute(raw, DamageType::as_str)?
        && element
            .as_deref()
            .and_then(|value| value.parse::<DamageType>().ok())
            != Some(damage)
    {
        return Err(storage_failure());
    }
    Ok(element)
}

/// Reads the tags of one definition in their stored ordinal order.
fn read_tags(connection: &Connection, hash: i64) -> Result<Vec<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT tag FROM definition_tags WHERE definition_hash = ?1 ORDER BY ordinal")
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([hash], |row| row.get::<_, String>(0))
        .map_err(|_| storage_failure())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| storage_failure())
}

/// Reconstructs the complete committed catalog, ordered by definition hash.
///
/// This is the structural-equality reference the unchanged check compares
/// against. It reads only normalized columns, and tag rows are grouped by their
/// definition hash and appended in ordinal order so tag order round-trips
/// exactly. An orphan tag row is treated as corrupt local state.
fn read_committed_content(connection: &Connection) -> Result<Vec<ItemDefinition>, AppError> {
    let mut definitions = Vec::new();
    let mut positions = HashMap::new();
    let mut armor = super::armor_definitions::read(connection, None)?;

    {
        let mut statement = connection
            .prepare(&definition_select(connection, true)?)
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    read_bool(row, 11)?,
                    read_bool(row, 12)?,
                    row.get::<_, Option<i32>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    read_bool(row, 15)?,
                ))
            })
            .map_err(|_| storage_failure())?;

        for row in rows {
            let (
                hash,
                bucket_hash,
                name,
                item_type,
                tier,
                element,
                version,
                damage,
                slot,
                class_type,
                plug_category,
                does_postmaster_pull_have_side_effects,
                is_instance_item,
                max_stack_size,
                stack_unique_label,
                non_transferrable,
            ) = row.map_err(|_| storage_failure())?;
            positions.insert(hash, definitions.len());
            definitions.push(ItemDefinition {
                armor: armor.remove(&hash).unwrap_or_default(),
                non_transferrable,
                does_postmaster_pull_have_side_effects,
                is_instance_item,
                max_stack_size,
                stack_unique_label,
                hash: ItemHash::new(hash),
                bucket_hash: bucket_hash.map(BucketHash::new),
                name,
                item_type,
                tier,
                element: validate_element(element, damage)?,
                slot: decode_attribute(slot, ItemSlot::as_str)?,
                class_type: decode_attribute(class_type, ClassType::as_str)?,
                plug_category: validate_plug_category(plug_category)?,
                tags: Vec::new(),
                version: DefinitionVersion::new(version),
            });
        }
    }

    {
        let mut statement = connection
            .prepare(
                "SELECT definition_hash, tag FROM definition_tags \
                 ORDER BY definition_hash, ordinal",
            )
            .map_err(|_| storage_failure())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|_| storage_failure())?;

        for row in rows {
            let (hash, tag) = row.map_err(|_| storage_failure())?;
            let position = *positions.get(&hash).ok_or_else(storage_failure)?;
            definitions[position].tags.push(tag);
        }
    }

    if !armor.is_empty() {
        return Err(storage_failure());
    }
    Ok(definitions)
}

#[cfg(test)]
mod live_publish_tests {
    use super::*;

    #[test]
    fn search_filter_stored_attributes_fail_closed_on_corruption() {
        use destiny_core::{DefinitionSearchQuery, SearchRepository};
        for column in ["damage_type", "slot", "class_type"] {
            for value in ["invalid", "SOLAR", " solar "] {
                let store = SqliteStore::open_in_memory().unwrap();
                store.connection().execute("INSERT INTO definitions(hash,name,definition_version) VALUES(1,'Test','v1')", []).unwrap();
                store
                    .connection()
                    .pragma_update(None, "ignore_check_constraints", 1)
                    .unwrap();
                store
                    .connection()
                    .execute(&format!("UPDATE definitions SET {column}=?1"), [value])
                    .unwrap();
                assert_eq!(
                    store.lookup_definition(ItemHash::new(1)),
                    Err(storage_failure())
                );
                assert_eq!(
                    store.search_definitions(&DefinitionSearchQuery {
                        tier: None,
                        text: None,
                        item_type: None,
                        tags: vec![],
                        element: None,
                        slot: None,
                        class_type: None,
                        limit: 10
                    }),
                    Err(storage_failure())
                );
                // A catalog comparison must also reject corrupted attributes.
                upsert_metadata(
                    store.connection(),
                    &DefinitionReplacementInput {
                        definitions: ManifestDefinitions {
                            version: DefinitionVersion::new("v1"),
                            definitions: vec![],
                        },
                        checked_at: Timestamp::new(1),
                        source: SourceMarker::Manifest,
                    },
                    SyncRevision::new(1),
                )
                .unwrap();
                assert!(read_committed_content(store.connection()).is_err());
            }
        }
    }

    #[test]
    fn publishes_forty_thousand_definitions_with_tags() {
        let mut store = SqliteStore::open_in_memory().unwrap();
        let version = DefinitionVersion::new("large-synthetic-v1");
        let definitions = (0..40_000)
            .map(|hash| ItemDefinition {
                armor: Default::default(),
                non_transferrable: None,
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: ItemHash::new(hash),
                bucket_hash: Some(BucketHash::new(1)),
                name: format!("Synthetic {hash}"),
                item_type: Some("Weapon".into()),
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: vec!["synthetic".into(), "weapon".into()],
                version: version.clone(),
            })
            .collect();
        let start = std::time::Instant::now();
        let metadata = store
            .replace_definitions(DefinitionReplacementInput {
                definitions: ManifestDefinitions {
                    version: version.clone(),
                    definitions,
                },
                checked_at: Timestamp::new(1_000),
                source: SourceMarker::Manifest,
            })
            .unwrap();
        // Write directly so the required workspace runner reports this evidence
        // even when the test harness captures ordinary diagnostic output.
        std::io::Write::write_fmt(
            &mut std::io::stdout().lock(),
            format_args!("40,000-definition publish: {:?}\n", start.elapsed()),
        )
        .unwrap();
        assert_eq!(metadata.definition_version, version);
        assert_eq!(metadata.revision.value(), 1);
        assert_eq!(
            read_committed_content(store.connection()).unwrap().len(),
            40_000
        );
        assert_eq!(store.current_metadata().unwrap(), Some(metadata));
        let Resolution::Unique(last) = store.lookup_definition(ItemHash::new(39_999)).unwrap()
        else {
            panic!("last definition missing");
        };
        assert_eq!(last.tags, ["synthetic", "weapon"]);
    }
}
