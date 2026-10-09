//! Execution primitives for typed queries; no catalog resolution or I/O.
use std::collections::{BTreeMap, BTreeSet};

use super::{Comparison, Leaf, TypedQuery, TypedQueryKind};
use crate::domain::{ItemDefinition, ItemInstance, ItemLocation};
use crate::ids::{CharacterId, ItemHash, PerkHash};
use crate::query::{
    self, DefinitionSearchQuery, InstanceSearchQuery, ItemHashFilter, LocationFilter, SearchTerm,
    StatCondition, StatOp,
};

/// Borrowed execution data. Plug definitions must come from this instance's
/// selected/available plugs. Their hashes and categories remain available for
/// the existing core name matchers. A missing or mismatched item definition
/// cannot satisfy definition criteria or name slot guards.
///
/// Stats are catalog-resolved values keyed by normalized name. Keep *all* values
/// for same-name hashes: any value may satisfy a comparison, as in structured
/// stat matching. Character selectors are normalized keys resolved by the caller
/// with the application selector (including its missing/ambiguous errors).
pub struct ItemView<'a> {
    pub instance: &'a ItemInstance,
    pub definition: Option<&'a ItemDefinition>,
    pub socketed: &'a [ItemDefinition],
    pub available: &'a [ItemDefinition],
    pub include_available: bool,
    pub stats: &'a BTreeMap<String, Vec<i32>>,
    pub characters: &'a BTreeMap<String, CharacterId>,
}

/// Request-local catalog resolution shared by repository candidate scans.
/// Keep this separate from pagination criteria so intersecting single-valued
/// structured fields never overwrites either side of the AND.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedQuery {
    pub prefilter: InstanceSearchQuery,
    pub predicate: Option<TypedQuery>,
    pub characters: BTreeMap<String, CharacterId>,
    pub stats: Vec<crate::StatDefinition>,
}

impl ResolvedQuery {
    pub fn new(query: &TypedQuery) -> Self {
        let (prefilter, predicate) = match lower(query) {
            Lowering::Full(criteria) => (criteria, None),
            Lowering::Residual {
                prefilter,
                predicate,
            } => (prefilter, Some(predicate)),
        };
        Self {
            prefilter,
            predicate,
            characters: BTreeMap::new(),
            stats: Vec::new(),
        }
    }

    /// Candidate reconstruction only loads definition/plug data used by the AST.
    pub fn needs_definition(&self) -> bool {
        self.prefilter
            .definitions
            .as_ref()
            .is_some_and(|q| q.has_filters())
            || self.predicate.as_ref().is_some_and(|q| {
                any_leaf(q, |leaf| {
                    !matches!(
                        leaf,
                        Leaf::Locked
                            | Leaf::Unlocked
                            | Leaf::Equipped
                            | Leaf::Vault
                            | Leaf::Inventory
                            | Leaf::Postmaster
                            | Leaf::Hash(_)
                            | Leaf::PerkHash(_)
                            | Leaf::Power(_)
                            | Leaf::Stat { .. }
                            | Leaf::Character(_)
                    )
                })
            })
    }

    pub fn needs_plugs(&self) -> bool {
        self.predicate.as_ref().is_some_and(|q| {
            any_leaf(q, |leaf| {
                matches!(
                    leaf,
                    Leaf::Perk(_)
                        | Leaf::PerkName(_)
                        | Leaf::ExactPerk(_)
                        | Leaf::Mod(_)
                        | Leaf::Archetype(_)
                )
            })
        })
    }

    pub fn needs_available_plugs(&self) -> bool {
        self.predicate.as_ref().is_some_and(|q| {
            any_leaf(q, |leaf| {
                matches!(
                    leaf,
                    Leaf::Perk(_) | Leaf::PerkName(_) | Leaf::ExactPerk(_) | Leaf::Mod(_)
                )
            })
        })
    }

    pub fn matches(
        &self,
        instance: &ItemInstance,
        definition: Option<&ItemDefinition>,
        socketed: &[ItemDefinition],
        available: &[ItemDefinition],
        include_available: bool,
    ) -> bool {
        if !query::instance_matches_with_definition(&self.prefilter, instance, definition)
            || !query::instance_matches_stats(&self.prefilter, instance, &self.stats)
            || !query::instance_matches_available_names(&self.prefilter, socketed, available)
        {
            return false;
        }
        let Some(predicate) = &self.predicate else {
            return true;
        };
        let mut stats: BTreeMap<String, Vec<i32>> = BTreeMap::new();
        for stat in &instance.stats {
            if let Some(definition) = self.stats.iter().find(|d| d.hash() == stat.stat_hash) {
                stats
                    .entry(query::normalize_search_text(definition.name()))
                    .or_default()
                    .push(stat.value);
            }
        }
        matches(
            predicate,
            &ItemView {
                instance,
                definition,
                socketed,
                available,
                include_available,
                stats: &stats,
                characters: &self.characters,
            },
        )
    }
}

fn any_leaf(query: &TypedQuery, predicate: fn(&Leaf) -> bool) -> bool {
    match &query.kind {
        TypedQueryKind::Leaf(leaf) => predicate(leaf),
        TypedQueryKind::And(children) | TypedQueryKind::Or(children) => {
            children.iter().any(|q| any_leaf(q, predicate))
        }
        TypedQueryKind::Not(child) => any_leaf(child, predicate),
        TypedQueryKind::Noop => false,
    }
}

fn empty_query() -> InstanceSearchQuery {
    InstanceSearchQuery {
        offset: 0,
        sort: None,
        include_available: false,
        item_hashes: ItemHashFilter::Any,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        character_id: None,
        location: None,
        equipped: None,
        definitions: None,
        min_power: None,
        stats: vec![],
        limit: query::MAX_SEARCH_LIMIT,
    }
}
fn empty_definitions() -> DefinitionSearchQuery {
    DefinitionSearchQuery {
        text: None,
        item_type: None,
        tier: None,
        tags: vec![],
        element: None,
        slot: None,
        class_type: None,
        limit: query::MAX_SEARCH_LIMIT,
    }
}
fn compare(value: i64, cmp: Comparison) -> bool {
    cmp.op.matches(value, i64::from(cmp.value))
}

/// Empty queries match everything. NOT is plain negation, including missing
/// data. Keywords/name search only the normalized item name (not DIM's other
/// fields). Perk/perkname use substring matching; exactperk uses equality.
/// Mod/archetype reuse structured exact normalized name matching; alternatives
/// affect perk/mod names only, never archetypes. Power missing/zero fails all
/// comparisons; absent stats fail. Perkhash means summarized perks, not sockets.
pub fn matches(query: &TypedQuery, view: &ItemView<'_>) -> bool {
    match &query.kind {
        TypedQueryKind::Noop => true,
        TypedQueryKind::And(children) => children.iter().all(|q| matches(q, view)),
        TypedQueryKind::Or(children) => children.iter().any(|q| matches(q, view)),
        TypedQueryKind::Not(child) => !matches(child, view),
        TypedQueryKind::Leaf(leaf) => matches_leaf(leaf, view),
    }
}
fn matches_leaf(leaf: &Leaf, view: &ItemView<'_>) -> bool {
    let item = view.instance;
    let definition = view.definition.filter(|d| d.hash == item.item_hash);
    match leaf {
        Leaf::Locked => item.locked,
        Leaf::Unlocked => !item.locked,
        Leaf::Postmaster => matches!(item.location, ItemLocation::Postmaster(_)),
        Leaf::Weapon => definition.is_some_and(|d| query::is_weapon_slot(d.slot)),
        Leaf::Armor => definition.is_some_and(|d| query::is_armor_slot(d.slot)),
        Leaf::ExactName(term) => {
            definition.is_some_and(|d| query::normalize_search_text(&d.name) == *term)
        }
        Leaf::Power(cmp) => item
            .power
            .is_some_and(|power| power != 0 && compare(i64::from(power), *cmp)),
        Leaf::Stat { name, cmp } => view
            .stats
            .get(name)
            .is_some_and(|values| values.iter().any(|v| compare(i64::from(*v), *cmp))),
        Leaf::Character(selector) => view.characters.get(selector).is_some_and(|id| {
            let mut criteria = empty_query();
            criteria.character_id = Some(id.clone());
            query::instance_matches(&criteria, item)
        }),
        Leaf::Perk(term) | Leaf::PerkName(term) => definition.is_some_and(|d| {
            let (mut names, _) = query::inventory_plug_names(d, view.socketed);
            if view.include_available {
                names.extend(query::inventory_plug_names(d, view.available).0);
            }
            names
                .iter()
                .any(|name| query::normalize_search_text(name).contains(term))
        }),
        Leaf::ExactPerk(term) | Leaf::Mod(term) | Leaf::Archetype(term) => {
            let mut criteria = empty_query();
            let Ok(term) = SearchTerm::new(term) else {
                return false;
            };
            match leaf {
                Leaf::ExactPerk(_) => criteria.perk_name = Some(vec![term]),
                Leaf::Mod(_) => criteria.mod_name = Some(vec![term]),
                _ => criteria.archetype = Some(term),
            }
            criteria.include_available = view.include_available;
            query::instance_matches_with_definition(&criteria, item, definition)
                && query::instance_matches_available_names(&criteria, view.socketed, view.available)
        }
        _ => {
            let mut criteria = empty_query();
            // All remaining leaves have an exact structured representation.
            if !add_leaf(leaf, &mut criteria) {
                return false;
            }
            query::instance_matches_with_definition(&criteria, item, definition)
        }
    }
}

/// Unique normalized selectors, including NOT and every OR branch.
pub fn referenced_characters(query: &TypedQuery) -> Vec<String> {
    fn visit(query: &TypedQuery, names: &mut BTreeSet<String>) {
        match &query.kind {
            TypedQueryKind::Leaf(Leaf::Character(name)) => {
                names.insert(name.clone());
            }
            TypedQueryKind::And(children) | TypedQueryKind::Or(children) => {
                for child in children {
                    visit(child, names);
                }
            }
            TypedQueryKind::Not(child) => visit(child, names),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    visit(query, &mut names);
    names.into_iter().collect()
}

/// Unique stat names in deterministic normalized order, including negated and
/// OR branches. The caller validates these against the catalog at execution.
pub fn referenced_stats(query: &TypedQuery) -> Vec<String> {
    fn visit(query: &TypedQuery, names: &mut BTreeSet<String>) {
        match &query.kind {
            TypedQueryKind::Leaf(Leaf::Stat { name, .. }) => {
                names.insert(name.clone());
            }
            TypedQueryKind::And(children) | TypedQueryKind::Or(children) => {
                for child in children {
                    visit(child, names);
                }
            }
            TypedQueryKind::Not(child) => visit(child, names),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    visit(query, &mut names);
    names.into_iter().collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Lowering {
    Full(InstanceSearchQuery),
    Residual {
        prefilter: InstanceSearchQuery,
        predicate: TypedQuery,
    },
}

/// Only positive conjunctions can lower fully. Exact leaves: keyword/name,
/// type, tag, tier, element, slot, class, hash, perkhash, equipped, vault,
/// inventory, and representable stat comparisons. Power >=N or >N lowers only
/// when the resulting u16 minimum is positive (zero must remain excluded).
/// Single-valued fields may be written only once; tags/stats retain all terms
/// within structured bounds. Plug names remain residual: their structured
/// fields may undergo catalog expansion, and available-plug policy is external.
/// Character leaves likewise need caller resolution and remain residual.
///
/// Residual prefilters contain only successfully representable leaves reached
/// through AND, never through OR or NOT. Conflicts/bounds leave the previous
/// necessary constraint intact. Fragments carry default pagination, not limits
/// on predicate evaluation; the execution caller must apply pagination last.
/// Stat names in fragments are already normalized exact names: execution must
/// validate them, not broaden them via fuzzy catalog resolution.
pub fn lower(query: &TypedQuery) -> Lowering {
    fn visit(node: &TypedQuery, criteria: &mut InstanceSearchQuery) -> bool {
        match &node.kind {
            TypedQueryKind::Noop => true,
            TypedQueryKind::And(children) => {
                let mut full = true;
                for child in children {
                    full &= visit(child, criteria);
                }
                full
            }
            TypedQueryKind::Leaf(leaf) => {
                let mut candidate = criteria.clone();
                if add_leaf(leaf, &mut candidate) && candidate.validate().is_ok() {
                    *criteria = candidate;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }
    let mut prefilter = empty_query();
    if visit(query, &mut prefilter) {
        Lowering::Full(prefilter)
    } else {
        Lowering::Residual {
            prefilter,
            predicate: query.clone(),
        }
    }
}
fn set<T>(field: &mut Option<T>, value: T) -> bool {
    if field.is_some() {
        false
    } else {
        *field = Some(value);
        true
    }
}
fn add_leaf(leaf: &Leaf, criteria: &mut InstanceSearchQuery) -> bool {
    match leaf {
        Leaf::Equipped => set(&mut criteria.equipped, true),
        Leaf::Vault => set(&mut criteria.location, LocationFilter::Vault),
        Leaf::Inventory => set(&mut criteria.location, LocationFilter::Character),
        Leaf::Hash(hash) if matches!(criteria.item_hashes, ItemHashFilter::Any) => {
            criteria.item_hashes = ItemHashFilter::AnyOf(vec![ItemHash::new(i64::from(*hash))]);
            true
        }
        Leaf::PerkHash(hash) => set(&mut criteria.perk_hash, PerkHash::new(*hash)),
        Leaf::Power(cmp) => {
            let minimum = match cmp.op {
                StatOp::AtLeast => Some(cmp.value),
                StatOp::GreaterThan => cmp.value.checked_add(1),
                _ => None,
            }
            .and_then(|v| u16::try_from(v).ok())
            .filter(|v| *v != 0);
            minimum.is_some_and(|v| set(&mut criteria.min_power, v))
        }
        Leaf::Stat { name, cmp } => {
            let (Ok(name), Ok(value)) = (SearchTerm::new(name), i32::try_from(cmp.value)) else {
                return false;
            };
            criteria.stats.push(StatCondition {
                name,
                op: cmp.op,
                value,
            });
            true
        }
        Leaf::Keyword(_)
        | Leaf::Name(_)
        | Leaf::Type(_)
        | Leaf::Tag(_)
        | Leaf::Tier(_)
        | Leaf::Slot(_)
        | Leaf::Element(_)
        | Leaf::Class(_) => {
            let definitions = criteria
                .definitions
                .get_or_insert_with(|| Box::new(empty_definitions()));
            match leaf {
                Leaf::Keyword(text) | Leaf::Name(text) => {
                    SearchTerm::new(text).is_ok_and(|v| set(&mut definitions.text, v))
                }
                Leaf::Type(text) => {
                    SearchTerm::new(text).is_ok_and(|v| set(&mut definitions.item_type, v))
                }
                Leaf::Tier(text) => {
                    SearchTerm::new(text).is_ok_and(|v| set(&mut definitions.tier, v))
                }
                Leaf::Tag(text) => SearchTerm::new(text).is_ok_and(|v| {
                    definitions.tags.push(v);
                    true
                }),
                Leaf::Slot(slot) => set(&mut definitions.slot, *slot),
                Leaf::Element(element) => set(&mut definitions.element, *element),
                Leaf::Class(class) => set(&mut definitions.class_type, *class),
                _ => unreachable!(),
            }
        }
        _ => false,
    }
}
