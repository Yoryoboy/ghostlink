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
//! matches by [`ItemInstanceId`] ascending unless a stat sort is requested.
//! Stat sorting puts missing values last and breaks ties by instance ID;
//! the limit truncates the ordered
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
pub const MAX_SEARCH_LIMIT: usize = 1000;

/// Maximum number of ordered owned matches a search may skip.
pub const MAX_SEARCH_OFFSET: usize = 10_000;

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
pub struct SearchTerm(String, Vec<String>);

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
        Ok(Self(normalized, Vec::new()))
    }

    /// The normalized term used for matching.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches_name(&self, name: &str) -> bool {
        let normalized = normalize_search_text(name);
        if self.1.is_empty() {
            normalized == self.0
        } else {
            self.1.contains(&normalized)
        }
    }

    pub fn resolve(
        &mut self,
        catalog: &crate::name_matching::NameCatalog,
        kind: &str,
    ) -> Result<Vec<String>, AppError> {
        let names = catalog.resolve(kind, &self.0)?;
        self.1 = names
            .iter()
            .map(|name| normalize_search_text(name))
            .collect();
        Ok(names)
    }
}

fn validate_search_limit(limit: usize) -> Result<(), AppError> {
    if limit == 0 || limit > MAX_SEARCH_LIMIT {
        return Err(AppError::invalid_argument(
            "limit",
            format!("must be between 1 and {MAX_SEARCH_LIMIT}"),
        ));
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
    /// Exact normalized stored tier; unknown names match nothing.
    pub tier: Option<SearchTerm>,
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
            || self.tier.is_some()
            || !self.tags.is_empty()
            || self.element.is_some()
            || self.slot.is_some()
            || self.class_type.is_some()
    }

    /// Rejects an out-of-range limit or too many tag filters.
    pub fn validate(&self) -> Result<(), AppError> {
        validate_search_limit(self.limit)?;
        if self.tags.len() > MAX_TAG_FILTERS {
            return Err(AppError::invalid_argument(
                "tag",
                format!("at most {MAX_TAG_FILTERS} filters are allowed"),
            ));
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

/// Maximum number of stat conditions in one search.
pub const MAX_STAT_CONDITIONS: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatOp {
    AtLeast,
    AtMost,
    GreaterThan,
    LessThan,
    Equals,
}

impl StatOp {
    pub(crate) fn matches(self, value: i64, target: i64) -> bool {
        match self {
            Self::AtLeast => value >= target,
            Self::AtMost => value <= target,
            Self::GreaterThan => value > target,
            Self::LessThan => value < target,
            Self::Equals => value == target,
        }
    }
}

/// A validated exact normalized stat name and signed integer comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatCondition {
    pub name: SearchTerm,
    pub op: StatOp,
    pub value: i32,
}

impl std::str::FromStr for StatCondition {
    type Err = AppError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid_stat = || {
            AppError::invalid_argument(
                "stat",
                "expected Name>=N, Name<=N, Name>N, Name<N or Name=N (N must be a signed 32-bit integer)",
            )
        };
        reject_prohibited_text(input, MAX_SEARCH_TEXT_LENGTH + 32).map_err(|_| invalid_stat())?;
        let index = input.find(['<', '>', '=']).ok_or_else(invalid_stat)?;
        let raw_name = input[..index].trim();
        if raw_name.contains('!') {
            return Err(invalid_stat());
        }
        let name = SearchTerm::new(raw_name).map_err(|_| invalid_stat())?;
        let tail = &input[index..];
        let (op, number) = if let Some(number) = tail.strip_prefix(">=") {
            (StatOp::AtLeast, number)
        } else if let Some(number) = tail.strip_prefix("<=") {
            (StatOp::AtMost, number)
        } else if let Some(number) = tail.strip_prefix('>') {
            (StatOp::GreaterThan, number)
        } else if let Some(number) = tail.strip_prefix('<') {
            (StatOp::LessThan, number)
        } else if let Some(number) = tail.strip_prefix('=') {
            (StatOp::Equals, number)
        } else {
            return Err(invalid_stat());
        };
        let value = number.trim().parse().map_err(|_| invalid_stat())?;
        Ok(Self { name, op, value })
    }
}

/// All conditions must hold; any hash with the same normalized name may satisfy
/// each condition. Missing or unnamed stats never satisfy a condition.
pub fn instance_matches_stats(
    query: &InstanceSearchQuery,
    instance: &ItemInstance,
    definitions: &[crate::StatDefinition],
) -> bool {
    query.stats.iter().all(|condition| {
        instance.stats.iter().any(|stat| {
            definitions.iter().any(|definition| {
                definition.hash() == stat.stat_hash
                    && condition.name.matches_name(definition.name())
            }) && condition
                .op
                .matches(i64::from(stat.value), i64::from(condition.value))
        })
    })
}

/// Stat ordering for instance hits. Descending by default; missing values last.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatSort {
    pub name: SearchTerm,
    pub ascending: bool,
}

impl std::str::FromStr for StatSort {
    type Err = AppError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || AppError::invalid_argument("sort", "expected Stat, Stat:asc or Stat:desc");
        let (name, ascending) = match input.rsplit_once(':') {
            Some((name, "asc")) => (name, true),
            Some((name, "desc")) => (name, false),
            Some(_) => return Err(invalid()),
            None => (input, false),
        };
        Ok(Self {
            name: SearchTerm::new(name).map_err(|_| invalid())?,
            ascending,
        })
    }
}

impl StatSort {
    pub fn order(&self) -> &'static str {
        if self.ascending { "asc" } else { "desc" }
    }

    /// Reject multiple display names; identical-name hashes use their maximum value.
    pub fn resolve(
        &mut self,
        catalog: &crate::name_matching::NameCatalog,
    ) -> Result<String, AppError> {
        let names = self.name.resolve(catalog, "stat")?;
        if names.len() != 1 {
            return Err(AppError::name_resolution(
                ErrorCategory::AmbiguousSearchName,
                format!(
                    "sort stat matches several names; be more specific: {}",
                    names.join(", ")
                ),
            ));
        }
        Ok(names[0].clone())
    }
}

/// Shared total ordering: stat value, missing last in either direction, then ID.
pub fn compare_instances(
    sort: Option<&StatSort>,
    left: &ItemInstance,
    right: &ItemInstance,
    definitions: &[crate::StatDefinition],
) -> std::cmp::Ordering {
    let by_id = || left.instance_id.cmp(&right.instance_id);
    let Some(sort) = sort else { return by_id() };
    let value = |item: &ItemInstance| {
        item.stats
            .iter()
            .filter(|stat| {
                definitions.iter().any(|definition| {
                    definition.hash() == stat.stat_hash && sort.name.matches_name(definition.name())
                })
            })
            .map(|stat| stat.value)
            .max()
    };
    let ordering = match (value(left), value(right)) {
        (Some(a), Some(b)) if sort.ascending => a.cmp(&b),
        (Some(a), Some(b)) => b.cmp(&a),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    };
    ordering.then_with(by_id)
}

/// Instance location class. Unknown provider locations match neither class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocationFilter {
    Vault,
    Character,
}

impl std::str::FromStr for LocationFilter {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "vault" => Ok(Self::Vault),
            "character" => Ok(Self::Character),
            _ => Err(AppError::invalid_argument(
                "location",
                "expected one of vault, character",
            )),
        }
    }
}

/// Maximum independent names per perk/mod filter.
pub const MAX_NAME_TERMS: usize = 8;

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
    /// Number of ordered owned matches to skip, bounded to [`MAX_SEARCH_OFFSET`].
    pub offset: usize,
    pub sort: Option<StatSort>,
    pub include_available: bool,
    pub item_hashes: ItemHashFilter,
    pub perk_hash: Option<PerkHash>,
    /// Independently resolved weapon-perk terms; all terms must match.
    pub perk_name: Option<Vec<SearchTerm>>,
    /// Independently resolved armor-mod terms; all terms must match.
    pub mod_name: Option<Vec<SearchTerm>>,
    /// Exact normalized armor-archetype name.
    pub archetype: Option<SearchTerm>,
    pub character_id: Option<CharacterId>,
    pub location: Option<LocationFilter>,
    pub equipped: Option<bool>,
    /// Definition criteria applied before the instance limit; its limit is ignored.
    pub definitions: Option<Box<DefinitionSearchQuery>>,
    pub min_power: Option<u16>,
    pub stats: Vec<StatCondition>,
    pub limit: usize,
}

impl InstanceSearchQuery {
    /// Rejects an out-of-range limit or an over-large definition-hash list.
    pub fn validate(&self) -> Result<(), AppError> {
        validate_search_limit(self.limit)?;
        if self.offset > MAX_SEARCH_OFFSET {
            return Err(AppError::invalid_argument(
                "offset",
                "must be between 0 and 10000",
            ));
        }
        for (kind, terms) in [("perk", &self.perk_name), ("mod", &self.mod_name)] {
            if terms
                .as_ref()
                .is_some_and(|terms| terms.len() > MAX_NAME_TERMS)
            {
                return Err(AppError::invalid_argument(
                    kind,
                    format!("at most {MAX_NAME_TERMS} {kind} names"),
                ));
            }
        }
        if self.stats.len() > MAX_STAT_CONDITIONS {
            return Err(AppError::invalid_argument(
                "stat",
                format!("at most {MAX_STAT_CONDITIONS} conditions are allowed"),
            ));
        }
        if let Some(sort) = &self.sort {
            crate::normalize_stat_name(sort.name.as_str()).map_err(|_| {
                AppError::invalid_argument("sort", "stat name must satisfy Manifest name bounds")
            })?;
        }
        for condition in &self.stats {
            crate::normalize_stat_name(condition.name.as_str()).map_err(|_| {
                AppError::invalid_argument(
                    "stat",
                    "stat name must satisfy the Manifest name length and character bounds",
                )
            })?;
        }
        if let Some(definitions) = &self.definitions {
            definitions.validate()?;
        }
        if let ItemHashFilter::AnyOf(hashes) = &self.item_hashes
            && hashes.len() > MAX_HASH_FILTERS
        {
            return Err(AppError::invalid_argument(
                "item-hash",
                format!("at most {MAX_HASH_FILTERS} hashes are allowed"),
            ));
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

    if let Some(tier) = &query.tier
        && !definition
            .tier
            .as_deref()
            .is_some_and(|candidate| tier.matches_name(candidate))
    {
        return false;
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

    if query
        .equipped
        .is_some_and(|equipped| instance.equipped != equipped)
        || query.location.is_some_and(|location| {
            !matches!(
                (location, &instance.location),
                (LocationFilter::Vault, ItemLocation::Vault)
                    | (LocationFilter::Character, ItemLocation::Character(_))
            )
        })
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
        && (query.perk_name.as_ref().is_none_or(Vec::is_empty)
            || definition.is_some_and(|definition| {
                definition.hash == instance.item_hash && is_weapon_slot(definition.slot)
            }))
        && (query.archetype.is_none()
            || definition.is_some_and(|definition| {
                definition.hash == instance.item_hash && is_armor_slot(definition.slot)
            }))
        && (query.mod_name.as_ref().is_none_or(Vec::is_empty)
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
    query.perk_name.iter().flatten().all(|term| {
        plugs.iter().any(|definition| {
            definition
                .plug_category
                .as_deref()
                .is_some_and(crate::domain::is_weapon_perk_plug)
                && term.matches_name(&definition.name)
        })
    })
}

pub(crate) fn is_weapon_slot(slot: Option<ItemSlot>) -> bool {
    matches!(
        slot,
        Some(ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power)
    )
}

pub(crate) fn is_armor_slot(slot: Option<ItemSlot>) -> bool {
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
        && query.archetype.as_ref().is_none_or(|term| {
            plugs
                .iter()
                .any(|plug| is_armor_archetype(plug) && term.matches_name(&plug.name))
        })
        && query.mod_name.iter().flatten().all(|term| {
            plugs.iter().any(|definition| {
                is_named_armor_mod(definition) && term.matches_name(&definition.name)
            })
        })
}

/// Alternatives affect perk/mod filters only, never archetype matching.
pub fn instance_matches_available_names(
    query: &InstanceSearchQuery,
    socketed: &[ItemDefinition],
    available: &[ItemDefinition],
) -> bool {
    if !query.include_available {
        return instance_matches_plug_names(query, socketed);
    }
    let mut names_query = query.clone();
    names_query.archetype = None;
    let combined: Vec<_> = socketed.iter().chain(available.iter()).cloned().collect();
    instance_matches_plug_names(&names_query, &combined)
        && query.archetype.as_ref().is_none_or(|term| {
            socketed
                .iter()
                .any(|plug| is_armor_archetype(plug) && term.matches_name(&plug.name))
        })
}

/// Alternative names are unique, socket ordered, and omit already selected names.
pub fn inventory_available_names(
    definition: &ItemDefinition,
    socketed: &[ItemDefinition],
    available: &[ItemDefinition],
) -> (Vec<String>, Vec<String>) {
    let (selected_perks, selected_mods) = inventory_plug_names(definition, socketed);
    let (mut perks, mut mods) = inventory_plug_names(definition, available);
    for (names, selected) in [(&mut perks, selected_perks), (&mut mods, selected_mods)] {
        let mut seen: std::collections::HashSet<_> = selected
            .iter()
            .map(|name| normalize_search_text(name))
            .collect();
        names.retain(|name| seen.insert(normalize_search_text(name)));
    }
    (perks, mods)
}

pub fn is_armor_archetype(definition: &ItemDefinition) -> bool {
    definition.plug_category.as_deref().is_some_and(|category| {
        crate::domain::classify_plug(category) == crate::domain::PlugKind::ArmorArchetype
    })
}

/// First resolved armor archetype in socket order; non-armor exposes none.
pub fn inventory_archetype(
    definition: &ItemDefinition,
    plugs: &[ItemDefinition],
) -> Option<String> {
    is_armor_slot(definition.slot)
        .then(|| plugs.iter().find(|plug| is_armor_archetype(plug)))
        .flatten()
        .map(|plug| plug.name.clone())
}

/// An armor-mod plug that is not an empty-socket placeholder. The provider
/// fills unused mod sockets with "Empty Mod Socket" plugs that share the
/// armor-mod categories, so they are recognized by their display name.
pub fn is_named_armor_mod(definition: &ItemDefinition) -> bool {
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

/// Unlock flags are reliable only for the most recently played character's class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionStatus {
    Unlocked,
    Locked,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubclassOptionsResult {
    pub reference_character_id: Option<crate::CharacterId>,
    pub characters: Vec<CharacterSubclassOptions>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterSubclassOptions {
    pub character_id: crate::CharacterId,
    pub class: Option<String>,
    pub status_reliable: bool,
    pub elements: Vec<ElementOptions>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElementOptions {
    /// `None` means element-independent shared options.
    pub element: Option<crate::SubclassElement>,
    pub groups: Vec<OptionGroup>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionGroup {
    pub kind: crate::SubclassPlugKind,
    pub options: Vec<SubclassOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubclassOption {
    pub plug_hash: ItemHash,
    pub name: Option<String>,
    pub description: Option<String>,
    pub clarity: Option<String>,
    pub clarity_match: Option<String>,
    pub status: OptionStatus,
    pub can_insert: bool,
    pub enabled: bool,
}

/// Manifest bucket for equipped subclass items.
pub const SUBCLASS_BUCKET_HASH: crate::BucketHash = crate::BucketHash::new(3284755031);
/// Equipped seasonal artifact inventory bucket.
pub const ARTIFACT_BUCKET_HASH: crate::BucketHash = crate::BucketHash::new(1506418338);

/// Equipped subclass configurations in canonical character order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubclassResult {
    pub characters: Vec<CharacterSubclass>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterSubclass {
    pub character_id: crate::CharacterId,
    pub class: Option<String>,
    pub subclass: Option<SubclassConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubclassConfig {
    pub instance_id: crate::ItemInstanceId,
    pub item_hash: ItemHash,
    pub name: String,
    /// Normalized element display value from the item definition, when available.
    pub element: Option<String>,
    pub slots: Vec<SubclassSlot>,
}

/// A classified selected plug, in stored socket (ordinal) order.
/// Unknown definitions and non-subclass categories are omitted because their
/// subclass kind cannot be derived. Resolved slots currently always have a name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubclassSlot {
    pub kind: crate::SubclassPlugKind,
    pub plug_hash: ItemHash,
    pub name: Option<String>,
    pub description: Option<String>,
    pub clarity: Option<String>,
    pub clarity_match: Option<String>,
    pub empty: bool,
}

/// One concrete instance paired with the definition it references.
///
/// The pairing is explicit and sanitized: `definition` is the normalized
/// Manifest record for `instance.item_hash`, and `instance` is the current
/// runtime state. Both sides are bounded domain records with no provider payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySearchHit {
    pub available_perks: Vec<String>,
    pub available_mods: Vec<String>,
    pub definition: ItemDefinition,
    pub instance: ItemInstance,
    /// Resolved weapon-perk names in stored socket order; no plug hashes.
    pub perk_names: Vec<String>,
    /// Resolved armor-mod names for armor only, in stored socket order.
    pub mod_names: Vec<String>,
    /// First resolved archetype in socket order, for armor only.
    pub archetype: Option<String>,
    pub stats: Vec<NamedStat>,
}

/// Resolved stat display data; hashes remain internal to ordering and matching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedStat {
    pub name: String,
    pub value: i32,
}

/// A bounded inventory search answer.
///
/// `candidates` are the definition-level matches in [`ItemHash`] order, and
/// `hits` are the concrete instance matches in requested stat order, or
/// [`ItemInstanceId`] order without sorting. Both
/// lists are independently truncated to the requested limit, so a caller always
/// sees a bounded, deterministic answer. An empty list is the explicit
/// "no match" outcome: a search never silently selects a single concrete
/// instance, and it never reports a definition as an instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySearchResult {
    pub next_offset: Option<usize>,
    pub resolved: crate::name_matching::ResolvedNames,
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
    /// Same resolved, socket-ordered enrichment as inventory search hits.
    pub perk_names: Vec<String>,
    pub mod_names: Vec<String>,
    pub archetype: Option<String>,
    pub available_perks: Vec<String>,
    pub available_mods: Vec<String>,
    pub stats: Vec<NamedStat>,
    pub definition: ItemDefinition,
    pub instance: ItemInstance,
    pub metadata: QueryMetadata,
    /// Unique resolved plugs by hash and role; socketed intrinsics come first.
    pub plug_details: Vec<InspectionPlug>,
}

/// A resolved socketed intrinsic or plug exposed by an inspection name field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionPlug {
    pub hash: ItemHash,
    pub name: String,
    pub role: InspectionPlugRole,
    pub description: Option<String>,
    pub clarity: Option<String>,
    pub clarity_match: Option<String>,
}

/// Inspection-only roles; alternatives exclude names already equipped.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InspectionPlugRole {
    Intrinsic,
    Perk,
    Mod,
    AvailablePerk,
    AvailableMod,
    Archetype,
}

impl InspectionPlugRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intrinsic => "intrinsic",
            Self::Perk => "perk",
            Self::Mod => "mod",
            Self::AvailablePerk => "available_perk",
            Self::AvailableMod => "available_mod",
            Self::Archetype => "archetype",
        }
    }
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

    #[test]
    fn stat_sort_missing_last_ties_and_resolution() {
        let definitions = vec![crate::StatDefinition::new(1, "Range".into()).unwrap()];
        let mut a = instance("a", 1);
        let mut b = instance("b", 1);
        let missing = instance("0", 1);
        a.stats = vec![crate::domain::StatSummary {
            stat_hash: 1,
            value: 20,
        }];
        b.stats = vec![crate::domain::StatSummary {
            stat_hash: 1,
            value: 30,
        }];
        for (expression, expected) in [
            ("Range", vec!["b", "a", "0"]),
            ("Range:asc", vec!["a", "b", "0"]),
        ] {
            let sort: super::StatSort = expression.parse().unwrap();
            let mut items = [missing.clone(), a.clone(), b.clone()];
            items.sort_by(|a, b| super::compare_instances(Some(&sort), a, b, &definitions));
            assert_eq!(
                items
                    .iter()
                    .map(|i| i.instance_id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            b.stats = a.stats.clone();
            assert!(super::compare_instances(Some(&sort), &a, &b, &definitions).is_lt());
            b.stats[0].value = 30;
        }
        let catalog = crate::name_matching::NameCatalog::new(["Range", "Range Bonus"]);
        let mut sort: super::StatSort = "rang".parse().unwrap();
        assert_eq!(
            sort.resolve(&catalog).unwrap_err().category(),
            ErrorCategory::AmbiguousSearchName
        );
        let mut sort: super::StatSort = "Rnage".parse().unwrap();
        assert!(
            sort.resolve(&catalog)
                .unwrap_err()
                .to_string()
                .contains("Range")
        );
        assert!("Range:up".parse::<super::StatSort>().is_err());
        let mut exact: super::StatSort = " RANGE ".parse().unwrap();
        assert_eq!(exact.resolve(&catalog).unwrap(), "Range");
        let mut partial: super::StatSort = "bonus:asc".parse().unwrap();
        assert_eq!(partial.resolve(&catalog).unwrap(), "Range Bonus");
        let mut duplicates = definitions.clone();
        duplicates.push(crate::StatDefinition::new(2, "Range".into()).unwrap());
        a.stats.push(crate::domain::StatSummary {
            stat_hash: 2,
            value: 40,
        });
        assert!(super::compare_instances(Some(&exact), &a, &b, &duplicates).is_lt());
        exact.ascending = true;
        assert!(super::compare_instances(Some(&exact), &a, &b, &duplicates).is_gt());
        assert!(super::compare_instances(None, &a, &b, &duplicates).is_lt());
        assert!(query(None, None, vec![]).validate().is_ok());
        let mut q = query(None, None, vec![]);
        q.limit = 1000;
        assert!(q.validate().is_ok());
        q.limit = 1001;
        assert!(q.validate().unwrap_err().to_string().contains("1 and 1000"));
    }

    #[test]
    fn strict_stat_comparisons_exclude_the_boundary_and_missing_stats() {
        let definitions = vec![crate::StatDefinition::new(1, "Range".into()).unwrap()];
        for (expression, boundary, matching, rejected) in
            [("Range>60", 60, 61, 59), ("Range<30", 30, 29, 31)]
        {
            let mut query = instance_query(ItemHashFilter::Any, None);
            query.stats = vec![expression.parse().expect("strict operator is valid")];
            let mut item = instance("one", 1);
            assert!(!super::instance_matches_stats(&query, &item, &definitions));
            for (value, expected) in [(boundary, false), (matching, true), (rejected, false)] {
                item.stats = vec![crate::domain::StatSummary {
                    stat_hash: 1,
                    value,
                }];
                assert_eq!(
                    super::instance_matches_stats(&query, &item, &definitions),
                    expected
                );
            }
        }
    }

    fn term(value: &str) -> SearchTerm {
        SearchTerm::new(value).expect("valid term")
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            non_transferrable: None,
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
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
            transfer_status: None,
            available_plugs: Vec::new(),
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            stats: Vec::new(),
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
            tier: None,
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
            offset: 0,
            sort: None,
            include_available: false,
            item_hashes,
            perk_hash,
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: Vec::new(),
            character_id: None,
            location: None,
            equipped: None,
            definitions: None,
            min_power: None,
            limit: MAX_SEARCH_LIMIT,
        }
    }

    #[test]
    fn owned_offset_has_an_independent_bound() {
        let mut query = instance_query(ItemHashFilter::Any, None);
        query.offset = 10_000;
        assert!(query.validate().is_ok());
        query.offset = 10_001;
        let error = query.validate().unwrap_err();
        assert!(error.to_string().contains("must be between 0 and 10000"));
    }

    #[test]
    fn tier_location_and_equipped_filters_match_exactly() {
        let mut definitions = query(None, None, vec![]);
        definitions.tier = Some(term("EXOTIC"));
        assert!(definitions.has_filters());
        let mut item = definition(1, "Weapon");
        assert!(!definition_matches(&definitions, &item));
        item.tier = Some("Exotic".into());
        assert!(definition_matches(&definitions, &item));
        item.tier = Some("Exotic Extra".into());
        assert!(!definition_matches(&definitions, &item));
        item.tier = Some("Exotic".into());
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.definitions = Some(Box::new(definitions));
        filter.location = Some(super::LocationFilter::Vault);
        filter.equipped = Some(false);
        let mut owned = instance("a", 1);
        assert!(super::instance_matches_with_definition(
            &filter,
            &owned,
            Some(&item)
        ));
        assert!(!super::instance_matches_with_definition(
            &filter, &owned, None
        ));
        owned.equipped = true;
        assert!(!instance_matches(&filter, &owned));
        filter.equipped = Some(true);
        assert!(instance_matches(&filter, &owned));
        filter.character_id = Some(CharacterId::new("123"));
        assert!(!instance_matches(&filter, &owned));
        owned.location = ItemLocation::Character(CharacterId::new("123"));
        assert!(!instance_matches(&filter, &owned));
        filter.location = Some(super::LocationFilter::Character);
        assert!(instance_matches(&filter, &owned));
        owned.location = ItemLocation::other("unknown");
        assert!(!instance_matches(&filter, &owned));
        assert_eq!(
            "orbit"
                .parse::<super::LocationFilter>()
                .unwrap_err()
                .to_string(),
            "invalid location: expected one of vault, character"
        );
    }

    #[test]
    fn repeated_plug_terms_require_each_term_and_available_is_per_term() {
        use super::{instance_matches_available_names, instance_matches_plug_names};
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.perk_name = Some(vec![term("rampage"), term("kill clip")]);
        let mut rampage = definition(10, "Rampage");
        rampage.plug_category = Some("frames".into());
        let mut kill_clip = definition(11, "Kill Clip");
        kill_clip.plug_category = Some("frames".into());
        assert!(!instance_matches_plug_names(&filter, &[rampage.clone()]));
        assert!(!instance_matches_plug_names(&filter, &[kill_clip.clone()]));
        assert!(instance_matches_plug_names(
            &filter,
            &[rampage.clone(), kill_clip.clone()]
        ));
        assert!(!instance_matches_available_names(
            &filter,
            &[rampage.clone()],
            &[kill_clip.clone()]
        ));
        filter.include_available = true;
        assert!(instance_matches_available_names(
            &filter,
            &[rampage],
            &[kill_clip]
        ));
        filter.perk_name = Some(vec![term("rampage"), term("rampage")]);
        let mut rampage = definition(10, "Rampage");
        rampage.plug_category = Some("frames".into());
        assert!(instance_matches_plug_names(&filter, &[rampage]));
        filter.perk_name = None;
        filter.mod_name = Some(vec![term("siphon"), term("targeting")]);
        let mut siphon = definition(20, "Siphon");
        siphon.plug_category = Some("enhancements.v2_head".into());
        let mut targeting = definition(21, "Targeting");
        targeting.plug_category = Some("enhancements.v2_head".into());
        assert!(!instance_matches_plug_names(&filter, &[siphon.clone()]));
        assert!(instance_matches_available_names(
            &filter,
            &[siphon.clone()],
            &[targeting.clone()]
        ));
        filter.archetype = Some(term("paragon"));
        let mut paragon = definition(30, "Paragon");
        paragon.plug_category = Some("armor_archetypes".into());
        assert!(!instance_matches_available_names(
            &filter,
            &[siphon.clone()],
            &[targeting.clone(), paragon.clone()]
        ));
        assert!(instance_matches_available_names(
            &filter,
            &[siphon, paragon],
            &[targeting]
        ));
        filter.mod_name = Some(vec![term("siphon"); 9]);
        assert_eq!(
            filter.validate().unwrap_err().to_string(),
            "invalid mod: at most 8 mod names"
        );
        filter.mod_name = None;
        filter.perk_name = Some(vec![term("rampage"); 9]);
        assert_eq!(
            filter.validate().unwrap_err().to_string(),
            "invalid perk: at most 8 perk names"
        );
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
        filter.perk_name = Some(vec![term("intrinsic")]);
        assert!(!instance_matches_with_definition(
            &filter,
            &item,
            Some(&armor)
        ));
        assert!(!instance_matches_with_definition(&filter, &item, None));
    }

    #[test]
    fn archetype_names_are_exact_category_and_armor_guarded_with_first_output() {
        use crate::domain::ItemSlot;
        let item = instance("owned", 7);
        let mut armor = definition(7, "Helmet");
        let mut paragon = definition(10, "Paragon");
        paragon.plug_category = Some("armor_archetypes".into());
        let mut specialist = definition(11, "Specialist");
        specialist.plug_category = Some("armor_archetypes".into());
        let mut shader = definition(12, "Paragon");
        shader.plug_category = Some("shader".into());
        let mut filter = instance_query(ItemHashFilter::Any, None);
        filter.archetype = Some(term("  PARAGON "));
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
            assert_eq!(
                super::inventory_archetype(
                    &armor,
                    &[shader.clone(), specialist.clone(), paragon.clone()]
                ),
                Some("Specialist".into())
            );
        }
        assert!(super::instance_matches_plug_names(
            &filter,
            &[paragon.clone()]
        ));
        assert!(!super::instance_matches_plug_names(&filter, &[shader]));
        assert!(!super::instance_matches_plug_names(&filter, &[]));
        filter.archetype = Some(term("para"));
        assert!(!super::instance_matches_plug_names(
            &filter,
            &[paragon.clone()]
        ));
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
            assert_eq!(super::inventory_archetype(&armor, &[paragon.clone()]), None);
        }
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
        filter.mod_name = Some(vec![term("empty mod socket")]);
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
        filter.mod_name = Some(vec![term("harmonic siphon")]);
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
        filter.perk_name = Some(vec![term("rampage")]);
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
        for limit in [0, MAX_SEARCH_LIMIT + 1] {
            definition_query.limit = limit;
            assert_eq!(
                definition_query.validate().unwrap_err().to_string(),
                "invalid limit: must be between 1 and 1000"
            );
        }
        definition_query.limit = MAX_SEARCH_LIMIT;
        definition_query.tags = (0..=MAX_TAG_FILTERS)
            .map(|i| term(&format!("tag-{i}")))
            .collect();
        assert_eq!(
            definition_query.validate().unwrap_err().to_string(),
            "invalid tag: at most 16 filters are allowed"
        );

        let mut instance_query = instance_query(ItemHashFilter::Any, None);
        instance_query.limit = 0;
        assert_eq!(
            instance_query.validate().unwrap_err().to_string(),
            "invalid limit: must be between 1 and 1000"
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
            over_bound.validate().unwrap_err().to_string(),
            "invalid item-hash: at most 64 hashes are allowed"
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
