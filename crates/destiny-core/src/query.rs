//! Provider-neutral query contracts for character listing, inventory search,
//! and exact item inspection.
//!
//! # Scope
//!
//! This module owns the request and result *shapes* shared by the future
//! `list_characters`, `search_inventory`, and `inspect_item` application
//! services, together with the deterministic normalization and matching rules
//! those services must reproduce in every adapter. It performs no I/O, holds no
//! provider payload, and never returns a credential or an unbounded upstream
//! record.
//!
//! # Definition versus instance identity
//!
//! [`ItemHash`] identifies a Manifest *definition* and [`ItemInstanceId`]
//! identifies one concrete runtime *instance*. The two are structurally
//! distinct and never interchangeable: a definition-level filter uses
//! [`ItemHash`], while an exact inspection requires an [`ItemInstanceId`].
//! [`ItemInspection`] reports the matched definition and the concrete instance
//! in separate fields so a caller can never mistake one for the other.
//!
//! # Matching rules
//!
//! Searches are case-insensitive. Every searchable value is normalized with
//! [`normalize_search_text`]: surrounding whitespace is dropped, interior
//! whitespace runs collapse to a single space, and the result is lowercased.
//! A durable search projection must therefore store or compare values under the
//! same normalization, and no adapter may invent a different case rule.
//!
//! A definition matches:
//!
//! - its text filter when the normalized `name` *contains* the normalized term;
//! - its type filter on exact normalized equality against `item_type`, and an
//!   absent `item_type` never matches a present type filter;
//! - its tag filters only when every listed tag is present on the definition as
//!   an exact normalized value.
//!
//! An instance matches a [`PerkHash`] filter only when it carries a
//! [`PerkSummary`] with that exact hash. A perk-name filter instead matches any
//! stored socket plug resolving to a weapon-perk definition, on exact normalized
//! name equality, and requires a weapon slot. A mod-name filter similarly
//! requires an armor slot and an armor-mod plug. Unresolved item or plug
//! definitions never satisfy these name filters.
//!
//! [`PerkSummary`]: crate::domain::PerkSummary
//!
//! # Bounds and determinism
//!
//! Every search input is validated before use. A search limit must be between
//! one and [`MAX_SEARCH_LIMIT`] inclusive, a term must be non-empty and at most
//! [`MAX_SEARCH_TEXT_LENGTH`] characters, and a definition search may carry at
//! most [`MAX_TAG_FILTERS`] tags. Rejected input produces the sanitized
//! [`AppError`] category [`ErrorCategory::ProviderValidation`] and never echoes
//! the offending value.
//!
//! Definition matches are ordered by [`ItemHash`] ascending and instance
//! matches by [`ItemInstanceId`] ascending; the limit truncates the ordered
//! list. A valid projection carries unique definition hashes and unique
//! instance identifiers, so both orders are total, and an invalid projection
//! with duplicate keys is outside this contract.

use crate::domain::{
    Character, ClassType, DamageType, ItemDefinition, ItemInstance, ItemLocation, ItemSlot,
    SyncMetadata,
};
use crate::error::{AppError, ErrorCategory};
use crate::ids::{CharacterId, ItemHash, ItemInstanceId, PerkHash};
use crate::state::FreshnessStatus;

/// Maximum accepted length of a single normalized search term, in characters.
pub const MAX_SEARCH_TEXT_LENGTH: usize = 256;

/// Maximum number of matches any single search may return.
pub const MAX_SEARCH_LIMIT: usize = 100;

/// Maximum number of tag filters one definition search may carry.
pub const MAX_TAG_FILTERS: usize = 16;

/// Maximum number of definition hashes one instance search may constrain to.
///
/// The bound keeps a caller-supplied candidate set finite before it reaches a
/// durable query, so no adapter has to materialize an unbounded `IN (...)` set.
/// It does not apply to [`ItemHashFilter::Any`], which carries no hashes at all.
pub const MAX_HASH_FILTERS: usize = 64;

fn invalid_query() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// Rejects prohibited control characters and raw input longer than the bound.
fn reject_prohibited_text(value: &str, max_length: usize) -> Result<(), AppError> {
    let has_prohibited_control = value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace());
    if has_prohibited_control || value.chars().count() > max_length {
        return Err(invalid_query());
    }
    Ok(())
}

/// Normalizes one searchable value for deterministic, case-insensitive matching.
///
/// The transformation is total and provider-neutral: it trims surrounding
/// whitespace, collapses interior whitespace runs to one space, and lowercases
/// the result with Rust's Unicode-aware `str::to_lowercase`, not an ASCII-only
/// fold. Every search implementation, whether an in-memory fake or a durable
/// SQLite search projection, must apply this same rule to both the stored values
/// and the query term so the two can never disagree on case or spacing.
///
/// The Unicode case folding is part of the contract, not an incident of the
/// current implementation: a durable projection that relied on SQLite's default
/// byte-wise `lower()` would diverge from this function for non-ASCII names and
/// types. The safest way to reproduce it is to normalize values when they are
/// written (for example into precomputed normalized columns) with this same
/// function rather than re-deriving case in SQL.
pub fn normalize_search_text(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    for word in value.split_whitespace() {
        if !normalized.is_empty() {
            normalized.push(' ');
        }
        normalized.push_str(word);
    }
    normalized.to_lowercase()
}

/// A validated, normalized, case-insensitive search term.
///
/// Construction rejects empty input, prohibited control characters, and input
/// longer than [`MAX_SEARCH_TEXT_LENGTH`]; the stored value is already
/// normalized, so matching never repeats the transformation on the query side.
/// The term is not secret and may appear in diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchTerm(String);

impl SearchTerm {
    /// Validates and normalizes one search term.
    ///
    /// An empty, oversized, or control-bearing value is rejected with the
    /// sanitized [`ErrorCategory::ProviderValidation`] category; the rejected
    /// value never appears in the error.
    pub fn new(value: &str) -> Result<Self, AppError> {
        reject_prohibited_text(value, MAX_SEARCH_TEXT_LENGTH)?;
        let normalized = normalize_search_text(value);
        if normalized.is_empty() || normalized.chars().count() > MAX_SEARCH_TEXT_LENGTH {
            return Err(invalid_query());
        }
        Ok(Self(normalized))
    }

    /// The normalized term used for matching.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_search_limit(limit: usize) -> Result<(), AppError> {
    if limit == 0 || limit > MAX_SEARCH_LIMIT {
        return Err(invalid_query());
    }
    Ok(())
}

/// A validated definition-level inventory search.
///
/// Every filter is optional and independent; `None` (or an empty `tags` list)
/// imposes no restriction, so an unfiltered query matches every definition up to
/// `limit`. The text term matches on normalized `name` substring, `item_type`
/// matches on exact normalized equality, and each entry of `tags` must be
/// present. At most [`MAX_TAG_FILTERS`] tags are accepted, and `limit` must be
/// within `1..=MAX_SEARCH_LIMIT`.
///
/// A constructed value is validated by construction, but the fields are public
/// so a caller can build one directly; [`Self::validate`] is the single check
/// every search implementation runs before reading the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DefinitionSearchQuery {
    pub text: Option<SearchTerm>,
    pub item_type: Option<SearchTerm>,
    pub tags: Vec<SearchTerm>,
    pub element: Option<DamageType>,
    pub slot: Option<ItemSlot>,
    pub class_type: Option<ClassType>,
    pub limit: usize,
}

impl DefinitionSearchQuery {
    /// Whether matching requires a resolved definition. The limit is not a filter.
    pub fn has_filters(&self) -> bool {
        self.text.is_some()
            || self.item_type.is_some()
            || !self.tags.is_empty()
            || self.element.is_some()
            || self.slot.is_some()
            || self.class_type.is_some()
    }

    /// Rejects an out-of-range limit or too many tag filters.
    pub fn validate(&self) -> Result<(), AppError> {
        validate_search_limit(self.limit)?;
        if self.tags.len() > MAX_TAG_FILTERS {
            return Err(invalid_query());
        }
        Ok(())
    }
}

/// The definition-hash constraint carried by an instance search.
///
/// The distinction between "no constraint" and "an empty candidate set" is
/// explicit: [`ItemHashFilter::Any`] matches every instance, while
/// [`ItemHashFilter::AnyOf`] with an empty list matches nothing. An instance
/// search that follows a definition search passes the matched hashes; a
/// perk-only search passes [`ItemHashFilter::Any`]. Duplicate hashes inside
/// `AnyOf` are allowed and equivalent to a deduplicated list.
///
/// `AnyOf` is bounded: it may carry at most [`MAX_HASH_FILTERS`] hashes, and
/// [`InstanceSearchQuery::validate`] rejects a larger list with the sanitized
/// [`ErrorCategory::ProviderValidation`] category. An empty `AnyOf` remains
/// valid and still matches nothing; only an over-large list is rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemHashFilter {
    /// No definition constraint.
    Any,
    /// Only these definition hashes, and nothing when the list is empty.
    AnyOf(Vec<ItemHash>),
}

/// A validated instance-level inventory search.
///
/// A concrete instance matches when it satisfies the [`ItemHashFilter`], carries
/// the exact [`PerkHash`] when one is present, and is located on the named
/// character when a [`CharacterId`] filter is present. Vault and unknown
/// locations never satisfy a character filter. `limit` must be within
/// `1..=MAX_SEARCH_LIMIT`, an [`ItemHashFilter::AnyOf`] list may carry at most
/// [`MAX_HASH_FILTERS`] hashes, and [`Self::validate`] is the single check every
/// implementation runs before reading the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstanceSearchQuery {
    pub item_hashes: ItemHashFilter,
    pub perk_hash: Option<PerkHash>,
    /// Exact normalized weapon-perk name; validated by `SearchTerm` construction.
    pub perk_name: Option<SearchTerm>,
    /// Exact normalized armor-mod name; validated by `SearchTerm` construction.
    pub mod_name: Option<SearchTerm>,
    pub character_id: Option<CharacterId>,
    /// Definition criteria applied before the instance limit; its limit is ignored.
    pub definitions: Option<Box<DefinitionSearchQuery>>,
    pub min_power: Option<u16>,
    pub limit: usize,
}

impl InstanceSearchQuery {
    /// Rejects an out-of-range limit or an over-large definition-hash list.
    pub fn validate(&self) -> Result<(), AppError> {
        validate_search_limit(self.limit)?;
        if let Some(definitions) = &self.definitions {
            definitions.validate()?;
        }
        if let ItemHashFilter::AnyOf(hashes) = &self.item_hashes
            && hashes.len() > MAX_HASH_FILTERS
        {
            return Err(invalid_query());
        }
        Ok(())
    }
}

/// True when one normalized definition satisfies every filter of `query`.
///
/// This is the normative matching rule for definition search. It reads only the
/// definition record, performs no I/O, and is deterministic, so a durable
/// implementation can be checked against it and the in-memory fake can never
/// drift from the documented behavior.
pub fn definition_matches(query: &DefinitionSearchQuery, definition: &ItemDefinition) -> bool {
    if query.element.is_some_and(|element| {
        definition
            .element
            .as_deref()
            .and_then(|value| value.parse::<DamageType>().ok())
            != Some(element)
    }) || query.slot.is_some_and(|slot| definition.slot != Some(slot))
        || query
            .class_type
            .is_some_and(|class_type| definition.class_type != Some(class_type))
    {
        return false;
    }

    if let Some(term) = &query.text
        && !normalize_search_text(&definition.name).contains(term.as_str())
    {
        return false;
    }

    if let Some(item_type) = &query.item_type {
        match definition.item_type.as_deref() {
            Some(candidate) if normalize_search_text(candidate) == item_type.as_str() => {}
            _ => return false,
        }
    }

    if !query.tags.is_empty() {
        let stored: Vec<String> = definition
            .tags
            .iter()
            .map(|tag| normalize_search_text(tag))
            .collect();
        if !query
            .tags
            .iter()
            .all(|tag| stored.iter().any(|value| value == tag.as_str()))
        {
            return false;
        }
    }

    true
}

/// True when one normalized instance satisfies every filter of `query`.
///
/// This is the normative matching rule for instance search. Perk matching is an
/// exact [`PerkHash`] membership test on the instance's summarized perks and is
/// independent of the `active` and `visible` flags: a visible or inactive perk
/// still satisfies the filter, and a duplicated perk hash is harmless.
pub fn instance_matches(query: &InstanceSearchQuery, instance: &ItemInstance) -> bool {
    if query
        .min_power
        .is_some_and(|minimum| instance.power.is_none_or(|power| power < minimum))
    {
        return false;
    }
    if let ItemHashFilter::AnyOf(hashes) = &query.item_hashes
        && !hashes.contains(&instance.item_hash)
    {
        return false;
    }

    if let Some(perk_hash) = query.perk_hash
        && !instance
            .perks
            .iter()
            .any(|perk| perk.perk_hash == perk_hash)
    {
        return false;
    }

    if let Some(character_id) = &query.character_id {
        let located_on_character = matches!(
            &instance.location,
            ItemLocation::Character(location_id) if location_id == character_id
        );
        if !located_on_character {
            return false;
        }
    }

    true
}

/// Matches instance criteria and, when requested, its resolved definition.
/// Missing or incorrectly paired definitions fail every active definition filter
/// and plug-name slot guard. An empty definition query alone needs no resolution.
pub fn instance_matches_with_definition(
    query: &InstanceSearchQuery,
    instance: &ItemInstance,
    definition: Option<&ItemDefinition>,
) -> bool {
    instance_matches(query, instance)
        && (query.perk_name.is_none()
            || definition.is_some_and(|definition| {
                definition.hash == instance.item_hash && is_weapon_slot(definition.slot)
            }))
        && (query.mod_name.is_none()
            || definition.is_some_and(|definition| {
                definition.hash == instance.item_hash && is_armor_slot(definition.slot)
            }))
        && query.definitions.as_ref().is_none_or(|criteria| {
            !criteria.has_filters()
                || definition.is_some_and(|definition| {
                    definition.hash == instance.item_hash
                        && definition_matches(criteria, definition)
                })
        })
}

/// True when the socket-ordered resolved definitions satisfy the perk-name filter.
/// Callers supply only definitions resolved from this instance's stored plugs,
/// and must also apply `instance_matches_with_definition` for the weapon-slot guard.
pub fn instance_matches_perk_name(query: &InstanceSearchQuery, plugs: &[ItemDefinition]) -> bool {
    query.perk_name.as_ref().is_none_or(|term| {
        plugs.iter().any(|definition| {
            definition
                .plug_category
                .as_deref()
                .is_some_and(crate::domain::is_weapon_perk_plug)
                && normalize_search_text(&definition.name) == term.as_str()
        })
    })
}

fn is_weapon_slot(slot: Option<ItemSlot>) -> bool {
    matches!(
        slot,
        Some(ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power)
    )
}

fn is_armor_slot(slot: Option<ItemSlot>) -> bool {
    matches!(
        slot,
        Some(
            ItemSlot::Helmet
                | ItemSlot::Gauntlets
                | ItemSlot::Chest
                | ItemSlot::Legs
                | ItemSlot::ClassItem
        )
    )
}

/// Shared exact name matching for resolved plugs, paired with the slot guards
/// in `instance_matches_with_definition`. Missing plug definitions never match.
pub fn instance_matches_plug_names(query: &InstanceSearchQuery, plugs: &[ItemDefinition]) -> bool {
    instance_matches_perk_name(query, plugs)
        && query.mod_name.as_ref().is_none_or(|term| {
            plugs.iter().any(|definition| {
                is_named_armor_mod(definition)
                    && normalize_search_text(&definition.name) == term.as_str()
            })
        })
}

/// An armor-mod plug that is not an empty-socket placeholder. The provider
/// fills unused mod sockets with "Empty Mod Socket" plugs that share the
/// armor-mod categories, so they are recognized by their display name.
fn is_named_armor_mod(definition: &ItemDefinition) -> bool {
    definition.plug_category.as_deref().is_some_and(|category| {
        crate::domain::classify_plug(category) == crate::domain::PlugKind::ArmorMod
    }) && !is_empty_socket_placeholder(&definition.name)
}

fn is_empty_socket_placeholder(name: &str) -> bool {
    let normalized = normalize_search_text(name);
    normalized.starts_with("empty ") && normalized.ends_with(" socket")
}

/// Slot-guarded names for inventory hits; unknown slots expose neither kind.
pub fn inventory_plug_names(
    definition: &ItemDefinition,
    plugs: &[ItemDefinition],
) -> (Vec<String>, Vec<String>) {
    let perks = if is_weapon_slot(definition.slot) {
        weapon_perk_names(plugs)
    } else {
        Vec::new()
    };
    let mods = if is_armor_slot(definition.slot) {
        plugs
            .iter()
            .filter(|plug| is_named_armor_mod(plug))
            .map(|plug| plug.name.clone())
            .collect()
    } else {
        Vec::new()
    };
    (perks, mods)
}

/// Resolved weapon-perk names in socket order, omitting unknown/non-weapon plugs.
pub fn weapon_perk_names(plugs: &[ItemDefinition]) -> Vec<String> {
    plugs
        .iter()
        .filter(|definition| {
            definition
                .plug_category
                .as_deref()
                .is_some_and(crate::domain::is_weapon_perk_plug)
        })
        .map(|definition| definition.name.clone())
        .collect()
}

/// The projection metadata and freshness that accompany one query answer.
///
/// It reuses the repository-owned [`SyncMetadata`] and the shared
/// [`FreshnessStatus`], so no query result invents a parallel revision,
/// provenance, or freshness concept. `sync` describes the projection the answer
/// was read from: the profile inventory projection for character, inventory, and
/// inspection queries, and the Manifest definition projection for a
/// definition-only read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueryMetadata {
    pub sync: SyncMetadata,
    pub freshness: FreshnessStatus,
}

/// A bounded, sanitized listing of the characters in one profile projection.
///
/// The result reuses the normalized [`Character`] record and carries no raw
/// provider payload. The vector is bounded by the profile projection itself and
/// its order is the repository-owned canonical character order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterListResult {
    pub characters: Vec<Character>,
    pub metadata: QueryMetadata,
}

/// One concrete instance paired with the definition it references.
///
/// The pairing is explicit and sanitized: `definition` is the normalized
/// Manifest record for `instance.item_hash`, and `instance` is the current
/// runtime state. Both sides are bounded domain records with no provider payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySearchHit {
    pub definition: ItemDefinition,
    pub instance: ItemInstance,
    /// Resolved weapon-perk names in stored socket order; no plug hashes.
    pub perk_names: Vec<String>,
    /// Resolved armor-mod names for armor only, in stored socket order.
    pub mod_names: Vec<String>,
}

/// A bounded inventory search answer.
///
/// `candidates` are the definition-level matches in [`ItemHash`] order, and
/// `hits` are the concrete instance matches in [`ItemInstanceId`] order. Both
/// lists are independently truncated to the requested limit, so a caller always
/// sees a bounded, deterministic answer. An empty list is the explicit
/// "no match" outcome: a search never silently selects a single concrete
/// instance, and it never reports a definition as an instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySearchResult {
    pub candidates: Vec<ItemDefinition>,
    pub hits: Vec<InventorySearchHit>,
    pub metadata: QueryMetadata,
}

/// The exact inspection of one concrete instance.
///
/// The Manifest `definition` and the current `instance` state are separate
/// fields, so a caller reports them distinctly and never confuses a definition
/// hash with an instance identifier. Inspection requires a concrete
/// [`ItemInstanceId`]; a display-name query resolves through
/// [`InventorySearchResult`] instead and never reaches this type directly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInspection {
    pub definition: ItemDefinition,
    pub instance: ItemInstance,
    pub metadata: QueryMetadata,
}

/// A request to inspect exactly one concrete instance.
///
/// The request is keyed by a concrete [`ItemInstanceId`]. There is deliberately
/// no display-name field: a name query resolves through search candidates and
/// never reaches inspection directly, so a caller can never accidentally select
/// a concrete instance by display name alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectItemRequest {
    pub instance_id: ItemInstanceId,
}

#[cfg(test)]
mod tests {
    use super::{
        DefinitionSearchQuery, InstanceSearchQuery, ItemHashFilter, MAX_HASH_FILTERS,
        MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS, SearchTerm, definition_matches,
        instance_matches, instance_matches_with_definition, normalize_search_text,
    };
    use crate::domain::{ItemDefinition, ItemInstance, ItemLocation, PerkSummary};
    use crate::error::{AppError, ErrorCategory};
    use crate::ids::{CharacterId, DefinitionVersion, ItemHash, PerkHash};

    fn term(value: &str) -> SearchTerm {
        SearchTerm::new(value).expect("valid term")
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new("manifest-v1"),
        }
    }

    fn instance(instance_id: &str, item_hash: i64) -> ItemInstance {
        ItemInstance {
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            plugs: Vec::new(),
            perks: Vec::new(),
        }
    }

    fn query(
        text: Option<&str>,
        item_type: Option<&str>,
        tags: Vec<&str>,
    ) -> DefinitionSearchQuery {
        DefinitionSearchQuery {
            text: text.map(term),
            item_type: item_type.map(term),
            tags: tags.into_iter().map(term).collect(),
            element: None,
            slot: None,
            class_type: None,
            limit: MAX_SEARCH_LIMIT,
        }
    }

    fn instance_query(
        item_hashes: ItemHashFilter,
        perk_hash: Option<PerkHash>,
    ) -> InstanceSearchQuery {
        InstanceSearchQuery {
            item_hashes,
            perk_hash,
            perk_name: None,
            mod_name: None,
            character_id: None,
            definitions: None,
            min_power: None,
            limit: MAX_SEARCH_LIMIT,
        }
    }

    #[test]
    fn new_definition_filters_require_matching_attributes() {
        use crate::domain::{ClassType, DamageType, ItemSlot};
        let mut item = definition(7, "Solar weapon");
        item.element = Some("sOLar".into());
        item.slot = Some(ItemSlot::Energy);
        item.class_type = Some(ClassType::Hunter);
        let mut filter = query(None, None, Vec::new());
        filter.element = Some(DamageType::Solar);
        assert!(definition_matches(&filter, &item));
        filter.element = Some(DamageType::Void);
        assert!(!definition_matches(&filter, &item));
        filter.element = None;
        filter.slot = Some(ItemSlot::Energy);
        assert!(definition_matches(&filter, &item));
        filter.slot = Some(ItemSlot::Power);
        assert!(!definition_matches(&filter, &item));
        filter.slot = None;
        filter.class_type = Some(ClassType::Hunter);
        assert!(definition_matches(&filter, &item));
        filter.class_type = Some(ClassType::Titan);
        assert!(!definition_matches(&filter, &item));
        assert!(!definition_matches(&filter, &definition(8, "Unknown")));
    }

    #[test]
    fn instance_definition_filters_exclude_other_types_and_missing_definitions() {
        let item = instance("owned", 7);
        let mut weapon = definition(7, "Solar weapon");
        weapon.item_type = Some("Hand Cannon".into());
        weapon.tags = vec!["exotic".into()];
        let mut filter = instance_query(ItemHashFilter::Any, None);
        assert!(instance_matches_with_definition(&filter, &item, None));
        filter.definitions = Some(Box::new(query(None, Some("hand cannon"), Vec::new())));
        assert!(instance_matches_with_definition(
            &filter,
            &item,
            Some(&weapon)
        ));
        assert!(!instance_matches_with_definition(&filter, &item, None));
        weapon.item_type = Some("Rocket Launcher".into());
        assert!(!instance_matches_with_definition(
            &filter,
            &item,
            Some(&weapon)
        ));
        filter.definitions = Some(Box::new(query(Some("solar"), None, vec!["exotic"])));
        assert!(instance_matches_with_definition(
            &filter,
            &item,
            Some(&weapon)
        ));
        weapon.tags.clear();
        assert!(!instance_matches_with_definition(
            &filter,
            &item,
            Some(&weapon)
        ));
    }

    #[test]
    fn perk_name_rejects_exotic_armor_intrinsics_and_unknown_items() {
        use crate::domain::ItemSlot;
        let item = instance("armor", 7);
        let mut armor = definition(7, "Exotic helmet");
        armor.slot = Some(ItemSlot::Helmet);
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.perk_name = Some(term("intrinsic"));
        assert!(!instance_matches_with_definition(
            &filter,
            &item,
            Some(&armor)
        ));
        assert!(!instance_matches_with_definition(&filter, &item, None));
    }

    #[test]
    fn empty_mod_socket_placeholders_are_not_armor_mods() {
        use crate::domain::ItemSlot;
        let mut armor = definition(7, "Helmet");
        armor.slot = Some(ItemSlot::Helmet);
        let mut empty = definition(10, "Empty Mod Socket");
        empty.plug_category = Some("enhancements.v2_head".into());
        let mut real = definition(11, "Harmonic Siphon");
        real.plug_category = Some("enhancements.v2_head".into());
        let (_, mods) = super::inventory_plug_names(&armor, &[empty.clone(), real]);
        assert_eq!(mods, vec!["Harmonic Siphon".to_string()]);
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.mod_name = Some(term("empty mod socket"));
        assert!(!super::instance_matches_plug_names(&filter, &[empty]));
    }

    #[test]
    fn mod_name_requires_armor_and_exact_normalized_armor_mod() {
        use crate::domain::ItemSlot;
        let item = instance("owned", 7);
        let mut armor = definition(7, "Helmet");
        armor.slot = Some(ItemSlot::Helmet);
        let mut plug = definition(10, "  Harmonic   SIPHON ");
        plug.plug_category = Some("enhancements.v2_head".into());
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.mod_name = Some(term("harmonic siphon"));
        for slot in [
            ItemSlot::Helmet,
            ItemSlot::Gauntlets,
            ItemSlot::Chest,
            ItemSlot::Legs,
            ItemSlot::ClassItem,
        ] {
            armor.slot = Some(slot);
            assert!(instance_matches_with_definition(
                &filter,
                &item,
                Some(&armor)
            ));
        }
        for slot in [
            ItemSlot::Kinetic,
            ItemSlot::Energy,
            ItemSlot::Power,
            ItemSlot::Other,
            ItemSlot::Unknown,
        ] {
            armor.slot = Some(slot);
            assert!(!instance_matches_with_definition(
                &filter,
                &item,
                Some(&armor)
            ));
        }
        assert!(!instance_matches_with_definition(&filter, &item, None));
        assert!(super::instance_matches_plug_names(&filter, &[plug.clone()]));
        plug.name = "Harmonic Siphon Extra".into();
        assert!(!super::instance_matches_plug_names(
            &filter,
            &[plug.clone()]
        ));
        plug.name = "Harmonic Siphon".into();
        for category in ["intrinsics", "enhancements.ghosts_activity", "shader"] {
            plug.plug_category = Some(category.into());
            assert!(!super::instance_matches_plug_names(
                &filter,
                &[plug.clone()]
            ));
        }
        assert!(!super::instance_matches_plug_names(&filter, &[]));
    }

    #[test]
    fn minimum_power_is_inclusive_and_missing_power_fails() {
        let mut item = instance("owned", 7);
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.min_power = Some(1800);
        assert!(!instance_matches(&filter, &item));
        item.power = Some(1799);
        assert!(!instance_matches(&filter, &item));
        item.power = Some(1800);
        assert!(instance_matches(&filter, &item));
        item.power = Some(1801);
        assert!(instance_matches(&filter, &item));
    }

    #[test]
    fn perk_name_is_exact_weapon_only_and_socket_ordered() {
        use crate::domain::PlugSummary;
        let mut item = instance("owned", 7);
        item.plugs = vec![PlugSummary {
            plug_hash: ItemHash::new(10),
            enabled: true,
        }];
        let mut perk = definition(10, "  RAMPAGE  ");
        perk.plug_category = Some("frames".into());
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.perk_name = Some(term("rampage"));
        assert!(super::instance_matches_perk_name(&filter, &[perk.clone()]));
        assert_eq!(
            super::weapon_perk_names(&[perk.clone()]),
            vec!["  RAMPAGE  "]
        );
        perk.name = "Rampage Spec".into();
        assert!(!super::instance_matches_perk_name(&filter, &[perk.clone()]));
        perk.name = "Rampage".into();
        perk.plug_category = Some("shader".into());
        assert!(!super::instance_matches_perk_name(&filter, &[perk]));
        assert!(!super::instance_matches_perk_name(&filter, &[]));
        assert!(SearchTerm::new(" ").is_err());
        assert!(SearchTerm::new(&"a".repeat(MAX_SEARCH_TEXT_LENGTH + 1)).is_err());
    }

    #[test]
    fn hit_names_are_slot_guarded_and_socket_ordered() {
        use crate::domain::ItemSlot;
        let mut item = definition(7, "Item");
        let mut perk = definition(10, "Intrinsic");
        perk.plug_category = Some("intrinsics".into());
        let mut armor_mod = definition(11, "Harmonic Siphon");
        armor_mod.plug_category = Some("enhancements.v2_head".into());
        let plugs = vec![armor_mod.clone(), perk, armor_mod];
        for slot in [ItemSlot::Kinetic, ItemSlot::Energy, ItemSlot::Power] {
            item.slot = Some(slot);
            assert_eq!(
                super::inventory_plug_names(&item, &plugs),
                (vec!["Intrinsic".into()], vec![])
            );
        }
        item.slot = Some(ItemSlot::Helmet);
        assert_eq!(
            super::inventory_plug_names(&item, &plugs),
            (
                vec![],
                vec!["Harmonic Siphon".into(), "Harmonic Siphon".into()]
            )
        );
        item.slot = None;
        assert_eq!(super::inventory_plug_names(&item, &plugs), (vec![], vec![]));
    }

    #[test]
    fn normalization_trims_collapses_and_lowercases() {
        assert_eq!(normalize_search_text("  Gjallar   Horn \t"), "gjallar horn");
        assert_eq!(normalize_search_text("ALREADY"), "already");
        assert_eq!(normalize_search_text("   "), "");
    }

    #[test]
    fn search_terms_are_validated_and_normalized() {
        assert_eq!(term("  Ghost   Shell ").as_str(), "ghost shell");

        for invalid in ["", "   ", "bad\u{7}control"] {
            assert_eq!(
                SearchTerm::new(invalid),
                Err(AppError::new(ErrorCategory::ProviderValidation)),
                "term {invalid:?} must be rejected"
            );
        }

        let boundary = "a".repeat(MAX_SEARCH_TEXT_LENGTH);
        assert_eq!(SearchTerm::new(&boundary).unwrap().as_str(), boundary);
        let oversized = "a".repeat(MAX_SEARCH_TEXT_LENGTH + 1);
        assert_eq!(
            SearchTerm::new(&oversized),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
    }

    #[test]
    fn invalid_limits_and_tag_counts_are_rejected() {
        let mut definition_query = query(Some("ghost"), None, Vec::new());
        definition_query.limit = 0;
        assert_eq!(
            definition_query.validate(),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
        definition_query.limit = MAX_SEARCH_LIMIT + 1;
        assert_eq!(
            definition_query.validate(),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
        definition_query.limit = MAX_SEARCH_LIMIT;
        definition_query.tags = (0..=MAX_TAG_FILTERS)
            .map(|i| term(&format!("tag-{i}")))
            .collect();
        assert_eq!(
            definition_query.validate(),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        let mut instance_query = instance_query(ItemHashFilter::Any, None);
        instance_query.limit = 0;
        assert_eq!(
            instance_query.validate(),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
    }

    #[test]
    fn instance_hash_filter_cardinality_is_bounded() {
        // The boundary and the empty list are both valid; only an over-large
        // list is rejected. `AnyOf(empty)` keeps its fail-closed meaning.
        let boundary: Vec<ItemHash> = (0..MAX_HASH_FILTERS as i64).map(ItemHash::new).collect();
        let at_boundary = instance_query(ItemHashFilter::AnyOf(boundary), None);
        assert_eq!(at_boundary.validate(), Ok(()));

        let over: Vec<ItemHash> = (0..=MAX_HASH_FILTERS as i64).map(ItemHash::new).collect();
        let over_bound = instance_query(ItemHashFilter::AnyOf(over), None);
        assert_eq!(
            over_bound.validate(),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        // The empty list stays valid and remains distinct from `Any`.
        assert_eq!(
            instance_query(ItemHashFilter::AnyOf(Vec::new()), None).validate(),
            Ok(())
        );
        assert_eq!(instance_query(ItemHashFilter::Any, None).validate(), Ok(()));
    }

    #[test]
    fn definition_text_matches_normalized_name_substring() {
        let gjallarhorn = definition(1, "Gjallarhorn");
        assert!(definition_matches(
            &query(Some("gjallar"), None, Vec::new()),
            &gjallarhorn
        ));
        assert!(definition_matches(
            &query(Some("  HORN "), None, Vec::new()),
            &gjallarhorn
        ));
        assert!(!definition_matches(
            &query(Some("hornet"), None, Vec::new()),
            &gjallarhorn
        ));
    }

    #[test]
    fn definition_type_matches_exactly_and_absent_type_never_matches() {
        let mut weapon = definition(1, "Gjallarhorn");
        weapon.item_type = Some("Rocket Launcher".into());
        assert!(definition_matches(
            &query(None, Some("rocket launcher"), Vec::new()),
            &weapon
        ));
        assert!(!definition_matches(
            &query(None, Some("rocket"), Vec::new()),
            &weapon
        ));
        assert!(!definition_matches(
            &query(None, Some("Rocket Launcher"), Vec::new()),
            &definition(2, "Ghost")
        ));
    }

    #[test]
    fn definition_tags_require_every_listed_tag() {
        let mut item = definition(1, "Gjallarhorn");
        item.tags = vec!["exotic".into(), "solar".into()];
        assert!(definition_matches(
            &query(None, None, vec!["exotic"]),
            &item
        ));
        assert!(definition_matches(
            &query(None, None, vec!["EXOTIC", " solar "]),
            &item
        ));
        assert!(!definition_matches(&query(None, None, vec!["void"]), &item));
    }

    #[test]
    fn unfiltered_definition_query_matches_everything() {
        assert!(definition_matches(
            &query(None, None, Vec::new()),
            &definition(1, "Anything")
        ));
    }

    #[test]
    fn instance_hash_filter_distinguishes_any_from_empty() {
        let item = instance("instance-1", 7);

        assert!(instance_matches(
            &instance_query(ItemHashFilter::Any, None),
            &item
        ));
        assert!(instance_matches(
            &instance_query(ItemHashFilter::AnyOf(vec![ItemHash::new(7)]), None),
            &item
        ));
        assert!(!instance_matches(
            &instance_query(ItemHashFilter::AnyOf(vec![ItemHash::new(8)]), None),
            &item
        ));
        assert!(!instance_matches(
            &instance_query(ItemHashFilter::AnyOf(Vec::new()), None),
            &item
        ));
    }

    #[test]
    fn instance_perk_filter_is_exact_and_independent_of_flags() {
        let mut item = instance("instance-1", 7);
        item.perks = vec![
            PerkSummary {
                perk_hash: PerkHash::new(10),
                active: false,
                visible: false,
            },
            PerkSummary {
                perk_hash: PerkHash::new(20),
                active: true,
                visible: true,
            },
        ];

        assert!(instance_matches(
            &instance_query(ItemHashFilter::Any, Some(PerkHash::new(10))),
            &item
        ));
        assert!(instance_matches(
            &instance_query(ItemHashFilter::Any, Some(PerkHash::new(20))),
            &item
        ));
        assert!(!instance_matches(
            &instance_query(ItemHashFilter::Any, Some(PerkHash::new(30))),
            &item
        ));
    }

    #[test]
    fn instance_character_filter_excludes_vault_and_other_locations() {
        let mut on_character = instance("instance-1", 7);
        on_character.location = ItemLocation::Character(CharacterId::new("warlock"));

        let mut query = instance_query(ItemHashFilter::Any, None);
        query.character_id = Some(CharacterId::new("warlock"));
        assert!(instance_matches(&query, &on_character));

        query.character_id = Some(CharacterId::new("hunter"));
        assert!(!instance_matches(&query, &on_character));
        assert!(!instance_matches(&query, &instance("instance-2", 7)));
    }

    #[test]
    fn contract_results_reuse_shared_metadata_and_status_types() {
        use crate::ids::{DefinitionVersion, SyncRevision};
        use crate::state::{Dataset, FreshnessStatus, SourceMarker, Timestamp};

        let metadata = super::QueryMetadata {
            sync: crate::domain::SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(2),
                definition_version: DefinitionVersion::new("manifest-v1"),
            },
            freshness: FreshnessStatus::Fresh,
        };
        assert_eq!(metadata.freshness, FreshnessStatus::Fresh);
        assert_eq!(metadata.sync.source, SourceMarker::Profile);
        assert_eq!(metadata.sync.revision, SyncRevision::new(2));
    }
}
