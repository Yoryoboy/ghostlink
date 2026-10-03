//! Durable, bounded search over the SQLite projection.
//!
//! This module implements [`destiny_core::SearchRepository`] on the
//! application-owned [`SqliteStore`] without adding any schema: it reads only the
//! existing version-2 source-of-truth tables (`definitions`, `definition_tags`,
//! `items`, and `item_perks`). It performs no provider I/O, no async work, and no
//! serialization, and it returns the same bounded, deterministic, provider-neutral
//! core records the in-memory fake returns.
//!
//! # Matching parity
//!
//! Definition matching applies the shared [`destiny_core::definition_matches`]
//! Rust predicate directly, so the durable result can never drift from the
//! in-memory contract on case, whitespace, Unicode, type equality, or
//! all-tag containment. Definitions are enumerated in deterministic [`ItemHash`]
//! ascending order, reconstructed through the same reader the
//! [`destiny_core::DefinitionRepository`] exact lookup uses, and filtered in Rust;
//! no SQLite `lower()` or precomputed normalized value is involved, so there is no
//! derived structure that could become corrupt or diverge.
//!
//! Instance search preselects exact hash, perk and character constraints in
//! parameterized SQL, then applies the shared Rust instance/definition predicate
//! to each reconstructed row. Active definition criteria resolve the exact
//! definition; missing definitions do not match. Power and definition matching
//! happen before truncation in deterministic instance-identifier order.
//!
//! # Bounds, determinism, and sanitization
//!
//! Every search validates the caller's query first and rejects an out-of-range
//! limit, an over-large tag list, or an over-large hash candidate set with the
//! sanitized [`destiny_core::ErrorCategory::ProviderValidation`] category, without
//! echoing the offending value. Only the bounded result set is ever materialized:
//! the definition scan stops as soon as the validated limit is reached, and the
//! instance scan likewise stops after `limit` matching rows. Caller text and identifiers are
//! only ever bound as query parameters, and the number of placeholders is bounded
//! by the contract itself (at most [`destiny_core::MAX_HASH_FILTERS`] hashes),
//! never by interpolating a value into SQL text. Every storage failure maps to the
//! single sanitized [`destiny_core::ErrorCategory::LocalStorage`] category.
//!
//! # Performance tradeoff and future index seam
//!
//! Definition search is a bounded sequential scan of the definition primary key
//! plus one ordered tag lookup per visited definition, and it stops at the limit.
//! That is deliberately chosen over a precomputed normalized projection or a
//! full-text index: the architecture keeps semantic behavior stable while indexes
//! evolve, and an MVP correctness slice should not carry a derived structure whose
//! corruption handling and migration cost outweigh the benefit. When the catalog
//! grows, the seam is the enumeration query itself and the per-definition tag
//! lookup: a future migration can add an index (or a normalized companion table)
//! behind this same `SearchRepository` contract without changing any answer.

use super::{SqliteStore, storage_failure};
use destiny_core::query::instance_matches_with_definition;
use destiny_core::{
    AppError, DefinitionRepository, DefinitionSearchQuery, InstanceSearchQuery, ItemDefinition,
    ItemHash, ItemHashFilter, ItemInstance, ItemInstanceId, ItemLocation, PerkHash, PerkSummary,
    Resolution, SearchRepository, definition_matches,
};
use rusqlite::types::Value;
use rusqlite::{Connection, Error as SqlError, params_from_iter};

/// The raw column tuple one `items` row decodes into before it is validated.
type RawInstance = (
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    i64,
    Option<i64>,
);

impl SearchRepository for SqliteStore {
    fn search_definitions(
        &self,
        query: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        query.validate()?;

        // Enumerate the source-of-truth definitions in deterministic hash order
        // and stop as soon as the validated limit is reached. Only the bounded
        // match set is materialized, and each definition is reconstructed by the
        // definition repository's own reader, so the corruption defenses of the
        // exact lookup path apply unchanged.
        let mut matches: Vec<ItemDefinition> = Vec::new();
        {
            let mut statement = self
                .connection()
                .prepare("SELECT hash FROM definitions ORDER BY hash")
                .map_err(|_| storage_failure())?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(|_| storage_failure())?;

            for row in rows {
                let hash = ItemHash::new(row.map_err(|_| storage_failure())?);
                // A hash enumerated from the definition table cannot be missing
                // or duplicated; anything else is corrupt local state.
                let definition = match self.lookup_definition(hash)? {
                    Resolution::Unique(definition) => definition,
                    _ => return Err(storage_failure()),
                };
                if definition_matches(query, &definition) {
                    matches.push(definition);
                    if matches.len() == query.limit {
                        break;
                    }
                }
            }
        }
        Ok(matches)
    }

    fn search_instances(&self, query: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
        query.validate()?;

        // `AnyOf(empty)` is a validated, explicit no-match: it keeps its
        // fail-closed contract meaning and returns before any row is read.
        let hashes = match &query.item_hashes {
            ItemHashFilter::Any => None,
            ItemHashFilter::AnyOf(hashes) if hashes.is_empty() => return Ok(Vec::new()),
            ItemHashFilter::AnyOf(hashes) => Some(hashes),
        };

        let mut sql = String::from(
            "SELECT instance_id, item_hash, location_kind, location_character_id, \
             location_other, quantity, equipped, locked, power \
             FROM items WHERE 1 = 1",
        );
        let mut parameters: Vec<Value> = Vec::new();

        if let Some(hashes) = hashes {
            // The candidate list is bounded by `MAX_HASH_FILTERS`, so the
            // generated `IN` list stays finite; only placeholders are generated.
            sql.push_str(" AND item_hash IN (");
            for index in 0..hashes.len() {
                if index > 0 {
                    sql.push_str(", ");
                }
                sql.push('?');
            }
            sql.push(')');
            for hash in hashes {
                parameters.push(Value::Integer(hash.value()));
            }
        }
        if let Some(perk_hash) = query.perk_hash {
            sql.push_str(
                " AND EXISTS (SELECT 1 FROM item_perks p \
                 WHERE p.instance_id = items.instance_id AND p.perk_hash = ?)",
            );
            parameters.push(Value::Integer(i64::from(perk_hash.value())));
        }
        if let Some(character_id) = &query.character_id {
            sql.push_str(" AND location_kind = 'character' AND location_character_id = ?");
            parameters.push(Value::Text(character_id.as_str().to_owned()));
        }
        // Never truncate SQL candidates before shared definition/power matching.
        sql.push_str(" ORDER BY instance_id");

        let mut instances = Vec::new();
        // Resolve each distinct plug hash at most once across candidate rows.
        // Rust applies the Unicode-aware exact match before truncation.
        let mut plug_definitions = std::collections::HashMap::new();
        {
            let mut statement = self
                .connection()
                .prepare(&sql)
                .map_err(|_| storage_failure())?;
            let rows = statement
                .query_map(params_from_iter(parameters), decode_instance_row)
                .map_err(|_| storage_failure())?;

            for row in rows {
                let raw: RawInstance = row.map_err(|_| storage_failure())?;
                let instance = read_instance(self.connection(), raw)?;
                let definition = if query
                    .definitions
                    .as_ref()
                    .is_some_and(|criteria| criteria.has_filters())
                    || query.perk_name.is_some()
                    || query.mod_name.is_some()
                {
                    match self.lookup_definition(instance.item_hash)? {
                        Resolution::Unique(definition) => Some(definition),
                        Resolution::Missing => None,
                        Resolution::Ambiguous(_) => return Err(storage_failure()),
                    }
                } else {
                    None
                };
                let mut plugs = Vec::new();
                if query.perk_name.is_some() || query.mod_name.is_some() {
                    for plug in &instance.plugs {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            plug_definitions.entry(plug.plug_hash)
                        {
                            let resolved = match self.lookup_definition(plug.plug_hash)? {
                                Resolution::Unique(definition) => Some(definition),
                                Resolution::Missing => None,
                                Resolution::Ambiguous(_) => return Err(storage_failure()),
                            };
                            entry.insert(resolved);
                        }
                        if let Some(definition) = &plug_definitions[&plug.plug_hash] {
                            plugs.push(definition.clone());
                        }
                    }
                }
                if instance_matches_with_definition(query, &instance, definition.as_ref())
                    && destiny_core::query::instance_matches_plug_names(query, &plugs)
                {
                    instances.push(instance);
                    if instances.len() == query.limit {
                        break;
                    }
                }
            }
        }
        Ok(instances)
    }

    fn lookup_instance(
        &self,
        instance_id: &ItemInstanceId,
    ) -> Result<Resolution<ItemInstance>, AppError> {
        // The instance identifier is the table primary key, so a structurally
        // valid projection carries at most one row and `Ambiguous` is
        // unreachable here. It stays a documented outcome of the shared
        // `Resolution` type rather than a silently collapsing branch.
        let raw = self.connection().query_row(
            "SELECT instance_id, item_hash, location_kind, location_character_id, \
             location_other, quantity, equipped, locked, power \
             FROM items WHERE instance_id = ?1",
            [instance_id.as_str()],
            decode_instance_row,
        );

        match raw {
            Ok(raw) => Ok(Resolution::Unique(read_instance(self.connection(), raw)?)),
            Err(SqlError::QueryReturnedNoRows) => Ok(Resolution::Missing),
            Err(_) => Err(storage_failure()),
        }
    }
}

/// Decodes one `items` row into its raw column tuple.
fn decode_instance_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawInstance> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

/// Reconstructs one complete, validated instance with its ordered perk summaries.
///
/// The typed location, quantity, boolean flags, and optional power are validated
/// exactly as the profile repository validates them: an inconsistent location
/// shape, a non-`0`/`1` flag, an out-of-range numeric value, or a perk ordinal
/// sequence that does not run contiguously from zero is corrupt local state and
/// fails closed as a sanitized local-storage error rather than returning a
/// partially constructed record.
fn read_instance(connection: &Connection, raw: RawInstance) -> Result<ItemInstance, AppError> {
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
    ) = raw;

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
    let perks = read_perks(connection, instance_id.as_str())?;
    let plugs = super::inventory::read_plugs(connection, instance_id.as_str())?;

    Ok(ItemInstance {
        instance_id: instance_id.into(),
        item_hash: ItemHash::new(item_hash),
        location,
        quantity: u32::try_from(quantity).map_err(|_| storage_failure())?,
        equipped: decode_boolean(equipped)?,
        locked: decode_boolean(locked)?,
        power: decode_optional_u16(power)?,
        plugs,
        perks,
    })
}

/// Reads one instance's perk summaries in ordinal order, requiring `0..n`.
fn read_perks(connection: &Connection, instance_id: &str) -> Result<Vec<PerkSummary>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, perk_hash, active, visible FROM item_perks \
             WHERE instance_id = ?1 ORDER BY ordinal",
        )
        .map_err(|_| storage_failure())?;
    let rows = statement
        .query_map([instance_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|_| storage_failure())?;

    let mut perks = Vec::new();
    for (position, row) in rows.enumerate() {
        let (ordinal, perk_hash, active, visible) = row.map_err(|_| storage_failure())?;
        let expected = i64::try_from(position).map_err(|_| storage_failure())?;
        if ordinal != expected {
            return Err(storage_failure());
        }
        perks.push(PerkSummary {
            perk_hash: PerkHash::new(u32::try_from(perk_hash).map_err(|_| storage_failure())?),
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

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_app::test_support::InMemorySearchRepository;
    use destiny_core::{
        BucketHash, Character, CharacterId, DefinitionReplacementInput, DefinitionVersion,
        ErrorCategory, InventoryRepository, InventorySnapshot, InventorySnapshotInput,
        MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_TAG_FILTERS, ManifestDefinitions, MembershipId,
        MembershipType, SearchTerm, SourceMarker, Timestamp,
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const MANIFEST_VERSION: &str = "manifest-v1";
    const CHARACTER_ID: &str = "warlock";

    static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

    /// A private temporary database path plus cleanup of its SQLite sidecars.
    struct TempDatabase {
        path: PathBuf,
    }

    impl TempDatabase {
        fn new() -> Self {
            let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "destiny-search-{}-{sequence}.db",
                std::process::id()
            ));
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDatabase {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut sidecar = self.path.clone().into_os_string();
                sidecar.push(suffix);
                let _ = std::fs::remove_file(PathBuf::from(sidecar));
            }
        }
    }

    fn definition(hash: i64, name: &str, item_type: Option<&str>, tags: &[&str]) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: Some(BucketHash::new(10_000 + hash)),
            name: name.to_owned(),
            item_type: item_type.map(str::to_owned),
            tier: Some("Legendary".to_owned()),
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            version: DefinitionVersion::new(MANIFEST_VERSION),
        }
    }

    fn replacement(definitions: Vec<ItemDefinition>) -> DefinitionReplacementInput {
        DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new(MANIFEST_VERSION),
                definitions,
            },
            checked_at: Timestamp::new(100),
            source: SourceMarker::Manifest,
        }
    }

    fn instance(
        instance_id: &str,
        item_hash: i64,
        location: ItemLocation,
        perk_hashes: &[u32],
    ) -> ItemInstance {
        ItemInstance {
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            plugs: Vec::new(),
            perks: perk_hashes
                .iter()
                .map(|hash| PerkSummary {
                    perk_hash: PerkHash::new(*hash),
                    active: true,
                    visible: true,
                })
                .collect(),
        }
    }

    fn snapshot(items: Vec<ItemInstance>) -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(3),
            definition_version: DefinitionVersion::new(MANIFEST_VERSION),
            checked_at: Timestamp::new(100),
            source: SourceMarker::Profile,
            characters: vec![Character {
                id: CharacterId::new(CHARACTER_ID),
                name: Some("Nova".to_owned()),
                class: Some("Warlock".to_owned()),
                race: None,
                gender: None,
                light: Some(1810),
            }],
            items,
        }
    }

    fn seed_definitions(store: &mut SqliteStore, definitions: &[ItemDefinition]) {
        store
            .replace_definitions(replacement(definitions.to_vec()))
            .expect("publish the definition set");
    }

    fn seed_instances(store: &mut SqliteStore, items: Vec<ItemInstance>) -> InventorySnapshot {
        store
            .replace_current(snapshot(items))
            .expect("publish the profile snapshot")
    }

    fn term(value: &str) -> SearchTerm {
        SearchTerm::new(value).expect("valid search term")
    }

    fn definition_query(
        text: Option<&str>,
        item_type: Option<&str>,
        tags: &[&str],
        limit: usize,
    ) -> DefinitionSearchQuery {
        DefinitionSearchQuery {
            text: text.map(term),
            item_type: item_type.map(term),
            tags: tags.iter().map(|tag| term(tag)).collect(),
            element: None,
            slot: None,
            class_type: None,
            limit,
        }
    }

    fn instance_query(
        item_hashes: ItemHashFilter,
        perk_hash: Option<u32>,
        character: Option<&str>,
        limit: usize,
    ) -> InstanceSearchQuery {
        InstanceSearchQuery {
            item_hashes,
            perk_hash: perk_hash.map(PerkHash::new),
            perk_name: None,
            mod_name: None,
            character_id: character.map(CharacterId::new),
            definitions: None,
            min_power: None,
            limit,
        }
    }

    fn hashes(definitions: &[ItemDefinition]) -> Vec<i64> {
        definitions.iter().map(|item| item.hash.value()).collect()
    }

    fn names(instances: &[ItemInstance]) -> Vec<&str> {
        instances
            .iter()
            .map(|item| item.instance_id.as_str())
            .collect()
    }

    /// Representative definition fixture, including Unicode and empty norms.
    fn definition_fixture() -> Vec<ItemDefinition> {
        vec![
            definition(
                5,
                "Gjallarhorn",
                Some("Rocket Launcher"),
                &["exotic", "solar"],
            ),
            definition(2, "Gjallarswift", Some("Rocket Launcher"), &["legendary"]),
            definition(9, "Ghost Shell", Some("Ghost Shell"), &["exotic"]),
            definition(
                11,
                "ГЬЯЛЛАРХОРН",
                Some("Rocket Launcher"),
                &["ЭКЗОТИЧЕСКИЙ"],
            ),
            // `İ` lowercases to `i` plus a combining dot, which only a
            // Unicode-aware `str::to_lowercase` produces.
            definition(13, "İrem Kılıcı", None, &[]),
            // A whitespace-only name normalizes to the empty string.
            definition(17, "   ", None, &[]),
        ]
    }

    #[test]
    fn perk_name_search_matches_fake_before_limit_and_ignores_unknown_cosmetics() {
        use destiny_core::PlugSummary;
        let temp = TempDatabase::new();
        let mut store = SqliteStore::open(temp.path()).unwrap();
        let mut rampage = definition(60, "  RAMPAGE ", None, &[]);
        rampage.plug_category = Some("frames".into());
        let mut cosmetic = definition(61, "Rampage", None, &[]);
        cosmetic.plug_category = Some("shader".into());
        let mut spec = definition(62, "Rampage Spec", None, &[]);
        spec.plug_category = Some("frames".into());
        let mut unicode = definition(63, "İREM   Kılıcı", None, &[]);
        unicode.plug_category = Some("frames".into());
        let mut weapon = definition(1, "Weapon", None, &[]);
        weapon.slot = Some(destiny_core::domain::ItemSlot::Energy);
        let definitions = vec![weapon, rampage, cosmetic, spec, unicode];
        let items: Vec<_> = [
            ("a", 61),
            ("b", 62),
            ("c", 99),
            ("d", 60),
            ("e", 60),
            ("f", 63),
        ]
        .into_iter()
        .map(|(id, hash)| {
            let mut item = instance(id, 1, ItemLocation::Vault, &[]);
            item.plugs = vec![PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            }];
            item
        })
        .collect();
        seed_definitions(&mut store, &definitions);
        seed_instances(&mut store, items.clone());
        let mut fake = InMemorySearchRepository::new();
        fake.set_definitions(definitions);
        fake.set_instances(items);
        for (name, expected) in [
            ("rampage", vec!["d"]),
            ("Rampage Spec", vec!["b"]),
            ("i\u{307}rem kılıcı", vec!["f"]),
            ("ram", vec![]),
        ] {
            let mut query = instance_query(ItemHashFilter::Any, None, None, 1);
            query.perk_name = Some(term(name));
            let actual = store.search_instances(&query).unwrap();
            assert_eq!(actual, fake.search_instances(&query).unwrap());
            assert_eq!(names(&actual), expected);
        }
    }

    #[test]
    fn armor_mod_and_intrinsic_search_match_fake_with_slot_guards_before_limit() {
        use destiny_core::PlugSummary;
        use destiny_core::domain::ItemSlot;
        let temp = TempDatabase::new();
        let mut store = SqliteStore::open(temp.path()).unwrap();
        let mut armor = definition(1, "Exotic helmet", None, &[]);
        armor.slot = Some(ItemSlot::Helmet);
        let mut weapon = definition(2, "Weapon", None, &[]);
        weapon.slot = Some(ItemSlot::Kinetic);
        let mut mod_plug = definition(60, " Harmonic   SIPHON ", None, &[]);
        mod_plug.plug_category = Some("enhancements.v2_head".into());
        let mut intrinsic = definition(61, "Intrinsic", None, &[]);
        intrinsic.plug_category = Some("intrinsics".into());
        let mut ghost = definition(62, "Harmonic Siphon", None, &[]);
        ghost.plug_category = Some("enhancements.ghosts_activity".into());
        let definitions = vec![armor, weapon, mod_plug, intrinsic, ghost];
        let items = [
            ("a-weapon-mod", 2, 60),
            ("b-unresolved-mod", 99, 60),
            ("c-ghost", 1, 62),
            ("d-unknown-plug", 1, 99),
            ("e-armor-mod", 1, 60),
            ("f-armor-mod", 1, 60),
            ("g-armor-intrinsic", 1, 61),
            ("h-unknown-intrinsic", 99, 61),
            ("i-weapon-intrinsic", 2, 61),
        ]
        .into_iter()
        .map(|(id, hash, plug)| {
            let mut item = instance(id, hash, ItemLocation::Vault, &[]);
            item.plugs = vec![PlugSummary {
                plug_hash: ItemHash::new(plug),
                enabled: true,
            }];
            item
        })
        .collect::<Vec<_>>();
        seed_definitions(&mut store, &definitions);
        seed_instances(&mut store, items.clone());
        let mut fake = InMemorySearchRepository::with_definitions(definitions);
        fake.set_instances(items);
        for (perk, mod_name, expected) in [
            (None, Some("harmonic siphon"), vec!["e-armor-mod"]),
            (None, Some("harmonic"), vec![]),
            (Some("intrinsic"), None, vec!["i-weapon-intrinsic"]),
            (Some("intrinsic"), Some("harmonic siphon"), vec![]),
        ] {
            let mut query = instance_query(ItemHashFilter::Any, None, None, 1);
            query.perk_name = perk.map(term);
            query.mod_name = mod_name.map(term);
            let actual = store.search_instances(&query).unwrap();
            assert_eq!(actual, fake.search_instances(&query).unwrap());
            assert_eq!(names(&actual), expected);
        }
    }

    #[test]
    fn sqlite_definition_search_matches_fake_for_name_type_tag_and_unicode() {
        let temp = TempDatabase::new();
        let mut store = SqliteStore::open(temp.path()).expect("open the file store");
        let definitions = definition_fixture();
        seed_definitions(&mut store, &definitions);
        let fake = InMemorySearchRepository::with_definitions(definitions);

        let queries = vec![
            definition_query(Some("  GJALLAR "), None, &[], MAX_SEARCH_LIMIT),
            // The Cyrillic name is matched through Unicode lowercasing.
            definition_query(Some("гьяллархорн"), None, &[], MAX_SEARCH_LIMIT),
            // The Turkish dotted capital I needs the exact Rust expansion.
            definition_query(Some("i\u{307}rem"), None, &[], MAX_SEARCH_LIMIT),
            // Different case rules must not collapse the combining-dot form.
            definition_query(Some("irem"), None, &[], MAX_SEARCH_LIMIT),
            definition_query(None, Some("rocket launcher"), &[], MAX_SEARCH_LIMIT),
            definition_query(None, Some("ROCKET"), &[], MAX_SEARCH_LIMIT),
            definition_query(None, None, &["EXOTIC", " solar "], MAX_SEARCH_LIMIT),
            definition_query(None, None, &["экзотический"], MAX_SEARCH_LIMIT),
            definition_query(None, None, &[], 2),
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
        ];

        for query in queries {
            let durable = store
                .search_definitions(&query)
                .expect("durable search succeeds");
            let in_memory = fake
                .search_definitions(&query)
                .expect("fake search succeeds");
            assert_eq!(durable, in_memory, "definition query {query:?}");
        }

        // Ordering is hash-ascending and stable across repeated reads.
        for _ in 0..3 {
            assert_eq!(
                hashes(
                    &store
                        .search_definitions(&definition_query(None, None, &[], 2))
                        .expect("search")
                ),
                [2, 5]
            );
        }
    }

    #[test]
    fn sqlite_instance_search_matches_fake_for_hash_perk_character_and_boundaries() {
        let temp = TempDatabase::new();
        let mut store = SqliteStore::open(temp.path()).expect("open the file store");
        let definitions = vec![definition(7, "Gjallarhorn", None, &[])];
        seed_definitions(&mut store, &definitions);
        let items = vec![
            instance(
                "instance-1",
                7,
                ItemLocation::Character(CharacterId::new(CHARACTER_ID)),
                &[10, 20],
            ),
            instance("instance-2", 7, ItemLocation::Vault, &[30]),
            instance("instance-3", 8, ItemLocation::Vault, &[10]),
        ];
        let committed = seed_instances(&mut store, items.clone());
        let fake = InMemorySearchRepository::with_projection(definitions, &committed);

        // The exact 64-hash boundary is accepted and still matches hash 7.
        let mut boundary: Vec<ItemHash> = (100..=162).map(ItemHash::new).collect();
        boundary.push(ItemHash::new(7));

        let queries = vec![
            instance_query(ItemHashFilter::Any, None, None, MAX_SEARCH_LIMIT),
            instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(7)]),
                None,
                None,
                MAX_SEARCH_LIMIT,
            ),
            instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(7)]),
                Some(10),
                None,
                MAX_SEARCH_LIMIT,
            ),
            instance_query(ItemHashFilter::Any, Some(10), None, MAX_SEARCH_LIMIT),
            instance_query(
                ItemHashFilter::Any,
                None,
                Some(CHARACTER_ID),
                MAX_SEARCH_LIMIT,
            ),
            instance_query(
                ItemHashFilter::AnyOf(Vec::new()),
                None,
                None,
                MAX_SEARCH_LIMIT,
            ),
            instance_query(ItemHashFilter::Any, None, None, 2),
            instance_query(
                ItemHashFilter::AnyOf(boundary),
                None,
                None,
                MAX_SEARCH_LIMIT,
            ),
        ];

        for query in queries {
            let durable = store
                .search_instances(&query)
                .expect("durable instance search");
            let in_memory = fake.search_instances(&query).expect("fake instance search");
            assert_eq!(durable, in_memory, "instance query {query:?}");
        }

        // Ordering is instance-identifier ascending and truncates after sort.
        assert_eq!(
            names(
                &store
                    .search_instances(&instance_query(ItemHashFilter::Any, None, None, 2,))
                    .expect("search")
            ),
            ["instance-1", "instance-2"]
        );
    }

    #[test]
    fn sqlite_instance_definition_and_power_filters_match_fake_before_limit() {
        use destiny_core::domain::{ClassType, DamageType, ItemSlot};
        let mut store = SqliteStore::open_in_memory().unwrap();
        let mut matching = definition(
            7,
            "İrem   Cannon",
            Some("Hand   Cannon"),
            &["Exotic", "Solar"],
        );
        matching.element = Some("solar".into());
        matching.slot = Some(ItemSlot::Energy);
        matching.class_type = Some(ClassType::Hunter);
        let definitions = vec![definition(8, "Ghost", Some("Ghost Shell"), &[]), matching];
        seed_definitions(&mut store, &definitions);
        let mut items: Vec<_> = (0..120)
            .map(|index| instance(&format!("a-{index:03}"), 8, ItemLocation::Vault, &[]))
            .collect();
        // Orphans fail definition criteria even when their power matches.
        let mut orphan = instance("b-orphan", 99, ItemLocation::Vault, &[]);
        orphan.power = Some(1800);
        items.push(orphan);
        for (id, power) in [
            ("c-low", Some(1799)),
            ("d-missing", None),
            ("y-match", Some(1800)),
            ("z-match", Some(1801)),
        ] {
            let mut item = instance(id, 7, ItemLocation::Vault, &[10]);
            item.power = power;
            items.push(item);
        }
        let committed = seed_instances(&mut store, items);
        let fake = InMemorySearchRepository::with_projection(definitions, &committed);
        let mut criteria = vec![
            definition_query(Some("i\u{307}rem cannon"), None, &[], 1),
            definition_query(None, Some(" HAND CANNON "), &[], 1),
            definition_query(None, Some("hand"), &[], 1),
            definition_query(None, None, &["EXOTIC", "solar"], 1),
            definition_query(None, None, &["exotic", "absent"], 1),
        ];
        for (element, slot, class_type) in [
            (Some(DamageType::Solar), None, None),
            (Some(DamageType::Void), None, None),
            (None, Some(ItemSlot::Energy), None),
            (None, Some(ItemSlot::Power), None),
            (None, None, Some(ClassType::Hunter)),
            (None, None, Some(ClassType::Titan)),
        ] {
            let mut filter = definition_query(None, None, &[], 1);
            filter.element = element;
            filter.slot = slot;
            filter.class_type = class_type;
            criteria.push(filter);
        }
        let mut queries = Vec::new();
        for filter in criteria {
            let mut query = instance_query(ItemHashFilter::Any, None, None, 2);
            query.definitions = Some(Box::new(filter));
            queries.push(query.clone());
            query.min_power = Some(1800);
            queries.push(query);
        }
        let mut power_only = instance_query(ItemHashFilter::Any, None, None, 2);
        power_only.min_power = Some(1800);
        queries.push(power_only);
        let mut combined = instance_query(
            ItemHashFilter::AnyOf(vec![ItemHash::new(7)]),
            Some(10),
            None,
            1,
        );
        combined.definitions = Some(Box::new(definition_query(
            None,
            Some("hand cannon"),
            &[],
            1,
        )));
        combined.min_power = Some(1800);
        queries.push(combined);
        for query in queries {
            let expected = fake.search_instances(&query).unwrap();
            assert_eq!(
                store.search_instances(&query).unwrap(),
                expected,
                "{query:?}"
            );
        }
        let mut query = instance_query(ItemHashFilter::Any, None, None, 2);
        query.definitions = Some(Box::new(definition_query(
            None,
            Some("hand cannon"),
            &[],
            1,
        )));
        query.min_power = Some(1800);
        assert_eq!(
            names(&store.search_instances(&query).unwrap()),
            ["y-match", "z-match"]
        );
    }

    #[test]
    fn sqlite_search_rejects_invalid_queries_with_sanitized_errors_like_fake() {
        let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
        let definitions = definition_fixture();
        seed_definitions(&mut store, &definitions);
        seed_instances(
            &mut store,
            vec![instance("instance-1", 5, ItemLocation::Vault, &[10])],
        );
        let fake = InMemorySearchRepository::with_definitions(definitions);

        let sanitized = AppError::new(ErrorCategory::ProviderValidation);
        let mut zero_limit = definition_query(Some("gjallar"), None, &[], 10);
        zero_limit.limit = 0;
        assert_eq!(store.search_definitions(&zero_limit), Err(sanitized));

        let mut over_limit = instance_query(ItemHashFilter::Any, None, None, 10);
        over_limit.limit = MAX_SEARCH_LIMIT + 1;
        assert_eq!(store.search_instances(&over_limit), Err(sanitized));

        let mut too_many_tags = definition_query(Some("gjallar"), None, &[], 10);
        too_many_tags.tags = (0..=MAX_TAG_FILTERS)
            .map(|index| term(&format!("tag-{index}")))
            .collect();
        assert_eq!(store.search_definitions(&too_many_tags), Err(sanitized));

        // The over-bound hash list is rejected by both implementations with the
        // same sanitized category.
        let over: Vec<ItemHash> = (0..=MAX_HASH_FILTERS as i64).map(ItemHash::new).collect();
        let over_bound = instance_query(ItemHashFilter::AnyOf(over), None, None, 10);
        assert_eq!(store.search_instances(&over_bound), Err(sanitized));
        assert_eq!(
            fake.search_instances(&over_bound),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
    }

    #[test]
    fn sqlite_search_file_journey_covers_replacement_reopen_and_missing_lookup() {
        let temp = TempDatabase::new();
        let first = vec![definition(
            5,
            "First Name",
            Some("Rocket Launcher"),
            &["exotic"],
        )];
        let second = vec![definition(9, "Second  İrem", Some("Sword"), &["legendary"])];

        {
            let mut store = SqliteStore::open(temp.path()).expect("open the file store");
            seed_definitions(&mut store, &first);
            seed_instances(
                &mut store,
                vec![instance("instance-1", 5, ItemLocation::Vault, &[10])],
            );

            // Unicode name, filter, and ordering all answer over the file store.
            assert_eq!(
                hashes(
                    &store
                        .search_definitions(&definition_query(
                            Some("first"),
                            None,
                            &[],
                            MAX_SEARCH_LIMIT,
                        ))
                        .expect("search the first revision")
                ),
                [5]
            );

            // A complete replacement retires the first definition and instance.
            seed_definitions(&mut store, &second);
            seed_instances(
                &mut store,
                vec![instance("instance-2", 9, ItemLocation::Vault, &[42])],
            );
            assert!(
                store
                    .search_definitions(&definition_query(
                        Some("first"),
                        None,
                        &[],
                        MAX_SEARCH_LIMIT,
                    ))
                    .expect("search the retired name")
                    .is_empty()
            );
            assert_eq!(
                hashes(
                    &store
                        .search_definitions(&definition_query(
                            Some("i\u{307}rem"),
                            None,
                            &[],
                            MAX_SEARCH_LIMIT,
                        ))
                        .expect("search the Unicode name")
                ),
                [9]
            );
            assert_eq!(
                store
                    .lookup_instance(&ItemInstanceId::new("instance-1"))
                    .expect("lookup the retired instance"),
                Resolution::Missing
            );
        }

        // Reopening the same file preserves both projections and the answers.
        let store = SqliteStore::open(temp.path()).expect("reopen the file store");
        assert_eq!(
            hashes(
                &store
                    .search_definitions(&definition_query(
                        Some("i\u{307}rem"),
                        None,
                        &[],
                        MAX_SEARCH_LIMIT,
                    ))
                    .expect("reopened definition search")
            ),
            [9]
        );
        assert_eq!(
            names(
                &store
                    .search_instances(&instance_query(
                        ItemHashFilter::Any,
                        Some(42),
                        None,
                        MAX_SEARCH_LIMIT,
                    ))
                    .expect("reopened instance search")
            ),
            ["instance-2"]
        );
        assert_eq!(
            store
                .lookup_instance(&ItemInstanceId::new("instance-2"))
                .expect("reopened lookup"),
            Resolution::Unique(instance("instance-2", 9, ItemLocation::Vault, &[42]))
        );
        assert_eq!(
            store
                .lookup_instance(&ItemInstanceId::new("missing"))
                .expect("missing lookup"),
            Resolution::Missing
        );
    }

    #[test]
    fn sqlite_search_fails_closed_on_corrupt_source_of_truth_records() {
        let temp = TempDatabase::new();
        let mut store = SqliteStore::open(temp.path()).expect("open the file store");
        seed_instances(
            &mut store,
            vec![
                instance("instance-1", 5, ItemLocation::Vault, &[10, 20]),
                instance("instance-2", 5, ItemLocation::Vault, &[10]),
            ],
        );

        // Deleting the first perk ordinal leaves a non-contiguous sequence, which
        // is corrupt local state and must fail closed instead of returning a
        // truncated perk list.
        store
            .connection()
            .execute(
                "DELETE FROM item_perks WHERE instance_id = 'instance-1' AND ordinal = 0",
                [],
            )
            .expect("remove a perk row");
        assert_eq!(
            store.search_instances(&instance_query(
                ItemHashFilter::Any,
                None,
                None,
                MAX_SEARCH_LIMIT,
            )),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            store.lookup_instance(&ItemInstanceId::new("instance-1")),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );

        // A tampered instance row that slipped past the schema checks must also
        // fail closed when it is reconstructed.
        store
            .connection()
            .pragma_update(None, "ignore_check_constraints", 1_i32)
            .expect("disable schema checks on this connection");
        store
            .connection()
            .execute(
                "UPDATE items SET equipped = 2 WHERE instance_id = 'instance-2'",
                [],
            )
            .expect("write a non-boolean flag");
        assert_eq!(
            store.lookup_instance(&ItemInstanceId::new("instance-2")),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
    }
}
