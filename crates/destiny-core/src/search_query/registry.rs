use std::ops::Range;

use crate::domain::{ClassType, DamageType, ItemSlot};
use crate::query::{SearchTerm, StatOp};

use super::{QueryNode, QueryNodeKind, QueryParseError, QueryParseErrorKind, parse_query};

/// Unsigned comparisons in the search language (no decimal or signed operands).
/// Reuses the structured query's comparison operators, but not its signed value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Comparison {
    pub op: StatOp,
    pub value: u32,
}

/// Validated leaves. Tier names reuse normalized stored definition tier text.
/// Location/category flags need no fabricated character IDs or bucket hashes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Leaf {
    Locked,
    Unlocked,
    Equipped,
    Vault,
    Inventory,
    Postmaster,
    Weapon,
    Armor,
    Slot(ItemSlot),
    Element(DamageType),
    Tier(&'static str),
    Class(ClassType),
    Keyword(String),
    Name(String),
    ExactName(String),
    Type(String),
    Tag(String),
    Perk(String),
    PerkName(String),
    ExactPerk(String),
    Mod(String),
    Archetype(String),
    Hash(u32),
    PerkHash(u32),
    Power(Comparison),
    Stat { name: String, cmp: Comparison },
    Character(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedQuery {
    pub kind: TypedQueryKind,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedQueryKind {
    Noop,
    And(Vec<TypedQuery>),
    Or(Vec<TypedQuery>),
    Not(Box<TypedQuery>),
    Leaf(Leaf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterFormat {
    /// Keys include `is:`; `not:` is the parser's negated spelling.
    Simple,
    Freeform,
    /// An unsigned integer without a comparison operator (definition hashes).
    Unsigned,
    Range,
    Stat,
    Keyword,
    /// Listed explicitly for help, but never produces a typed leaf.
    Unsupported,
}

/// Construction policy stored in the registry, not a second keyword dispatch.
#[derive(Clone, Debug)]
pub enum ValueKind {
    Fixed(Leaf),
    Text(fn(String) -> Leaf),
    Unsigned(fn(u32) -> Leaf),
    Range(fn(Comparison) -> Leaf),
    Stat,
    Unsupported,
}

#[derive(Clone, Debug)]
pub struct FilterSpec {
    /// Canonical key first, followed by aliases. Simple keys include `is:`.
    pub keys: &'static [&'static str],
    pub format: FilterFormat,
    pub description: &'static str,
    pub examples: &'static [&'static str],
    pub value_kind: ValueKind,
}

macro_rules! spec {
    ($key:literal, $format:ident, $description:literal, $example:literal, $kind:expr) => {
        FilterSpec {
            keys: &[$key],
            format: FilterFormat::$format,
            description: $description,
            examples: &[$example],
            value_kind: $kind,
        }
    };
}

// The only filter vocabulary. Keep canonical keys sorted for deterministic help.
// Unsupported entries are intentionally visible and clearly labeled by format.
// One row per filter keeps this declarative vocabulary easy to scan and compare.
#[rustfmt::skip]
static FILTERS: &[FilterSpec] = &[
    spec!("archetype", Freeform, "Armor archetype name", "archetype:bulwark", ValueKind::Text(Leaf::Archetype)),
    spec!("basestat", Unsupported, "Unsupported: base stats are not stored", "basestat:mobility:3", ValueKind::Unsupported),
    spec!("character", Freeform, "Character name or identifier", "character:hunter", ValueKind::Text(Leaf::Character)),
    spec!("exactname", Freeform, "Exact normalized item name", "exactname:\"Sun Shot\"", ValueKind::Text(Leaf::ExactName)),
    spec!("exactperk", Freeform, "Exact normalized weapon perk name", "exactperk:dragonfly", ValueKind::Text(Leaf::ExactPerk)),
    spec!("hash", Unsigned, "Unsigned item definition hash", "hash:123", ValueKind::Unsigned(Leaf::Hash)),
    spec!("inloadout", Unsupported, "Unsupported: loadout membership is not searchable", "inloadout:raid", ValueKind::Unsupported),
    spec!("is:arc", Simple, "Arc damage, not an inventory slot", "is:arc", ValueKind::Fixed(Leaf::Element(DamageType::Arc))),
    spec!("is:armor", Simple, "Armor equipment", "is:armor", ValueKind::Fixed(Leaf::Armor)),
    spec!("is:chest", Simple, "Chest armor slot", "is:chest", ValueKind::Fixed(Leaf::Slot(ItemSlot::Chest))),
    spec!("is:classitem", Simple, "Class item slot", "is:classitem", ValueKind::Fixed(Leaf::Slot(ItemSlot::ClassItem))),
    spec!("is:common", Simple, "Common tier", "is:common", ValueKind::Fixed(Leaf::Tier("common"))),
    spec!("is:crafted", Unsupported, "Unsupported: crafting state is not stored", "is:crafted", ValueKind::Unsupported),
    spec!("is:dupe", Unsupported, "Unsupported: duplicate analysis is not available", "is:dupe", ValueKind::Unsupported),
    spec!("is:energy", Simple, "Energy weapon slot", "is:energy", ValueKind::Fixed(Leaf::Slot(ItemSlot::Energy))),
    spec!("is:equipped", Simple, "Currently equipped", "is:equipped", ValueKind::Fixed(Leaf::Equipped)),
    spec!("is:exotic", Simple, "Exotic tier", "is:exotic", ValueKind::Fixed(Leaf::Tier("exotic"))),
    spec!("is:gauntlets", Simple, "Gauntlets armor slot", "is:gauntlets", ValueKind::Fixed(Leaf::Slot(ItemSlot::Gauntlets))),
    spec!("is:helmet", Simple, "Helmet armor slot", "is:helmet", ValueKind::Fixed(Leaf::Slot(ItemSlot::Helmet))),
    spec!("is:hunter", Simple, "Hunter class restriction", "is:hunter", ValueKind::Fixed(Leaf::Class(ClassType::Hunter))),
    spec!("is:ininventory", Simple, "Carried character inventory, excluding Postmaster", "is:ininventory", ValueKind::Fixed(Leaf::Inventory)),
    spec!("is:inloadout", Unsupported, "Unsupported: loadout membership is not searchable", "is:inloadout", ValueKind::Unsupported),
    spec!("is:inpostmaster", Simple, "Owned Postmaster holdings", "is:inpostmaster", ValueKind::Fixed(Leaf::Postmaster)),
    spec!("is:invault", Simple, "Vault holdings", "is:invault", ValueKind::Fixed(Leaf::Vault)),
    spec!("is:kinetic", Simple, "Kinetic damage, not the kinetic weapon slot", "is:kinetic", ValueKind::Fixed(Leaf::Element(DamageType::Kinetic))),
    spec!("is:kineticslot", Simple, "Kinetic weapon slot regardless of damage", "is:kineticslot", ValueKind::Fixed(Leaf::Slot(ItemSlot::Kinetic))),
    spec!("is:leg", Simple, "Leg armor slot", "is:leg", ValueKind::Fixed(Leaf::Slot(ItemSlot::Legs))),
    spec!("is:legendary", Simple, "Legendary tier", "is:legendary", ValueKind::Fixed(Leaf::Tier("legendary"))),
    spec!("is:locked", Simple, "Locked item", "is:locked", ValueKind::Fixed(Leaf::Locked)),
    spec!("is:patternunlocked", Unsupported, "Unsupported: crafting patterns are not stored", "is:patternunlocked", ValueKind::Unsupported),
    spec!("is:power", Simple, "Power weapon slot", "is:power", ValueKind::Fixed(Leaf::Slot(ItemSlot::Power))),
    spec!("is:prismatic", Unsupported, "Unsupported: prismatic is not a weapon damage type", "is:prismatic", ValueKind::Unsupported),
    spec!("is:rare", Simple, "Rare tier", "is:rare", ValueKind::Fixed(Leaf::Tier("rare"))),
    spec!("is:solar", Simple, "Solar damage", "is:solar", ValueKind::Fixed(Leaf::Element(DamageType::Solar))),
    spec!("is:stasis", Simple, "Stasis damage", "is:stasis", ValueKind::Fixed(Leaf::Element(DamageType::Stasis))),
    spec!("is:strand", Simple, "Strand damage", "is:strand", ValueKind::Fixed(Leaf::Element(DamageType::Strand))),
    spec!("is:titan", Simple, "Titan class restriction", "is:titan", ValueKind::Fixed(Leaf::Class(ClassType::Titan))),
    spec!("is:uncommon", Simple, "Uncommon tier", "is:uncommon", ValueKind::Fixed(Leaf::Tier("uncommon"))),
    spec!("is:unlocked", Simple, "Unlocked item", "is:unlocked", ValueKind::Fixed(Leaf::Unlocked)),
    spec!("is:void", Simple, "Void damage", "is:void", ValueKind::Fixed(Leaf::Element(DamageType::Void))),
    spec!("is:warlock", Simple, "Warlock class restriction", "is:warlock", ValueKind::Fixed(Leaf::Class(ClassType::Warlock))),
    spec!("is:weapon", Simple, "Weapon equipment", "is:weapon", ValueKind::Fixed(Leaf::Weapon)),
    spec!("is:wishlist", Unsupported, "Unsupported: wishlists are not stored", "is:wishlist", ValueKind::Unsupported),
    spec!("keyword", Keyword, "Bare words or quoted phrases, normalized text", "\"Sun Shot\"", ValueKind::Text(Leaf::Keyword)),
    spec!("kills", Unsupported, "Unsupported: kill trackers are not stored", "kills:3", ValueKind::Unsupported),
    spec!("mod", Freeform, "Armor mod name", "mod:resilience", ValueKind::Text(Leaf::Mod)),
    spec!("name", Freeform, "Item name text", "name:sun", ValueKind::Text(Leaf::Name)),
    spec!("notes", Unsupported, "Unsupported: DIM user notes are not stored", "notes:raid", ValueKind::Unsupported),
    spec!("perk", Freeform, "Weapon perk text", "perk:dragonfly", ValueKind::Text(Leaf::Perk)),
    spec!("perkhash", Unsigned, "Unsigned summarized perk definition hash", "perkhash:123", ValueKind::Unsigned(Leaf::PerkHash)),
    spec!("perkname", Freeform, "Weapon perk name text", "perkname:dragonfly", ValueKind::Text(Leaf::PerkName)),
    FilterSpec {
        keys: &["power", "light"],
        format: FilterFormat::Range,
        description: "Power comparison with an unsigned integer (=, <, <=, >, >=)",
        examples: &["power:>=2000", "light:2000"],
        value_kind: ValueKind::Range(Leaf::Power),
    },
    spec!("sort", Unsupported, "Unsupported: sorting belongs outside the query", "sort:power", ValueKind::Unsupported),
    spec!("sortby", Unsupported, "Unsupported: sorting belongs outside the query", "sortby:power", ValueKind::Unsupported),
    spec!("stat", Stat, "Stat name and unsigned comparison; catalog validation occurs at execution; arithmetic unsupported", "stat:mobility:>=30", ValueKind::Stat),
    spec!("tag", Freeform, "Exact stored definition tag, not a DIM user annotation; DIM tag words are accepted as text", "tag:favorite", ValueKind::Text(Leaf::Tag)),
    spec!("type", Freeform, "Item type text", "type:\"Hand Cannon\"", ValueKind::Text(Leaf::Type)),
    spec!("wishlist", Unsupported, "Unsupported: wishlists are not stored", "wishlist:good", ValueKind::Unsupported),
];

/// Stable canonical-key order; includes explicitly unsupported filters for help.
pub fn list_filters() -> &'static [FilterSpec] {
    FILTERS
}

fn unsigned(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn comparison(value: &str) -> Option<Comparison> {
    let (op, number) = if let Some(number) = value.strip_prefix("<=") {
        (StatOp::AtMost, number)
    } else if let Some(number) = value.strip_prefix(">=") {
        (StatOp::AtLeast, number)
    } else if let Some(number) = value.strip_prefix('<') {
        (StatOp::LessThan, number)
    } else if let Some(number) = value.strip_prefix('>') {
        (StatOp::GreaterThan, number)
    } else {
        (StatOp::Equals, value.strip_prefix('=').unwrap_or(value))
    };
    Some(Comparison {
        op,
        value: unsigned(number)?,
    })
}

fn filter_error(
    kind: QueryParseErrorKind,
    span: Range<usize>,
    spec: Option<&FilterSpec>,
) -> QueryParseError {
    let mut error = QueryParseError::new(kind, span);
    // Only static vocabulary may appear in diagnostics, never the input key/value.
    if let Some(spec) = spec {
        error.message = format!("filter '{}': {}", spec.keys[0], error.message);
    }
    error
}

fn typed_leaf(key: &str, value: &str, span: Range<usize>) -> Result<Leaf, QueryParseError> {
    let lookup = if key == "is" {
        format!("is:{}", value.to_ascii_lowercase())
    } else {
        key.to_owned()
    };
    let spec = FILTERS
        .iter()
        .find(|spec| spec.keys.contains(&lookup.as_str()));
    let Some(spec) = spec else {
        let kind = if key == "is" {
            QueryParseErrorKind::InvalidFilterValue
        } else {
            QueryParseErrorKind::UnknownFilter
        };
        return Err(filter_error(kind, span, None));
    };
    let invalid = || {
        filter_error(
            QueryParseErrorKind::InvalidFilterValue,
            span.clone(),
            Some(spec),
        )
    };
    let unsupported = || {
        filter_error(
            QueryParseErrorKind::UnsupportedFilter,
            span.clone(),
            Some(spec),
        )
    };
    match &spec.value_kind {
        ValueKind::Fixed(leaf) => Ok(leaf.clone()),
        ValueKind::Text(build) => {
            let term = SearchTerm::new(value).map_err(|_| invalid())?;
            Ok(build(term.as_str().to_owned()))
        }
        ValueKind::Unsigned(build) => unsigned(value).map(build).ok_or_else(invalid),
        ValueKind::Range(build) => comparison(value).map(build).ok_or_else(invalid),
        ValueKind::Stat => {
            let (name, number) = value.split_once(':').ok_or_else(invalid)?;
            if name.contains(['+', '&', '/', '*']) {
                return Err(unsupported());
            }
            let name = SearchTerm::new(name).map_err(|_| invalid())?;
            let name = crate::normalize_stat_name(name.as_str()).map_err(|_| invalid())?;
            let cmp = comparison(number).ok_or_else(invalid)?;
            Ok(Leaf::Stat { name, cmp })
        }
        ValueKind::Unsupported => Err(unsupported()),
    }
}

fn validate(node: QueryNode, errors: &mut Vec<QueryParseError>) -> TypedQuery {
    let span = node.span;
    let kind = match node.kind {
        QueryNodeKind::Noop => TypedQueryKind::Noop,
        QueryNodeKind::And(children) => TypedQueryKind::And(
            children
                .into_iter()
                .map(|child| validate(child, errors))
                .collect(),
        ),
        QueryNodeKind::Or(children) => TypedQueryKind::Or(
            children
                .into_iter()
                .map(|child| validate(child, errors))
                .collect(),
        ),
        QueryNodeKind::Not(child) => TypedQueryKind::Not(Box::new(validate(*child, errors))),
        QueryNodeKind::Filter { key, value } => match typed_leaf(&key, &value, span.clone()) {
            Ok(leaf) => TypedQueryKind::Leaf(leaf),
            Err(error) => {
                errors.push(error);
                // Placeholder cannot escape: compile returns Err if any leaf failed.
                TypedQueryKind::Noop
            }
        },
    };
    TypedQuery { kind, span }
}

/// Parses and validates every leaf, including every OR and negated branch.
/// On failure, returns the first diagnostic in source order after full validation.
/// No inventory reads, stat catalog resolution, or predicate execution occurs.
pub fn compile(query: &str) -> Result<TypedQuery, QueryParseError> {
    let parsed = parse_query(query)?;
    let mut errors = Vec::new();
    let typed = validate(parsed, &mut errors);
    match errors.into_iter().next() {
        Some(error) => Err(error),
        None => Ok(typed),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn validation_collects_errors_in_all_branches_in_source_order() {
        let parsed = super::parse_query("power:abc or (unknown:value -is:banana)").unwrap();
        let mut errors = Vec::new();
        super::validate(parsed, &mut errors);
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].kind.as_str(), "invalid_filter_value");
        assert_eq!(errors[1].kind.as_str(), "unknown_filter");
        assert_eq!(errors[2].kind.as_str(), "invalid_filter_value");
    }
}
