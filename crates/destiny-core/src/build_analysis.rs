//! Pure DIM loadout resolution and explicitly bounded analyzer coverage.
use crate::{build::*, domain::ClassType, search_query, *};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FindingKind {
    MissingItems,
    InvalidMods,
    EmptyFragmentSlots,
    TooManyFragments,
    NeedsArmorUpgrades,
    UsesSeasonalMods,
    BetterStatsAvailable,
    NotAFullArmorSet,
    DoesNotRespectExotic,
    ModsDontFit,
    DoesNotSatisfyStatConstraints,
    InvalidSearchQuery,
    ItemsDoNotMatchSearchQuery,
}
impl FindingKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingItems => "missing_items",
            Self::InvalidMods => "invalid_mods",
            Self::EmptyFragmentSlots => "empty_fragment_slots",
            Self::TooManyFragments => "too_many_fragments",
            Self::NeedsArmorUpgrades => "needs_armor_upgrades",
            Self::UsesSeasonalMods => "uses_seasonal_mods",
            Self::BetterStatsAvailable => "better_stats_available",
            Self::NotAFullArmorSet => "not_a_full_armor_set",
            Self::DoesNotRespectExotic => "does_not_respect_exotic",
            Self::ModsDontFit => "mods_dont_fit",
            Self::DoesNotSatisfyStatConstraints => "does_not_satisfy_stat_constraints",
            Self::InvalidSearchQuery => "invalid_search_query",
            Self::ItemsDoNotMatchSearchQuery => "items_do_not_match_search_query",
        }
    }
    pub const fn blocks_optimization(self) -> bool {
        matches!(
            self,
            Self::NotAFullArmorSet | Self::DoesNotRespectExotic | Self::ModsDontFit
        )
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildFinding {
    pub kind: FindingKind,
    /// evaluated, search_only, or not_evaluated. None presence never means clean.
    pub evaluation: &'static str,
    pub present: Option<bool>,
    pub reason: Option<&'static str>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBuildItem {
    pub spec: BuildItem,
    pub should_equip: bool,
    pub inventory: Option<ItemInstance>,
    pub class_compatible: Option<bool>,
    pub resolution_reason: Option<&'static str>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildAnalysis {
    pub target_character: CharacterId,
    pub items: Vec<ResolvedBuildItem>,
    pub specified: usize,
    pub resolved: usize,
    pub missing: usize,
    pub findings: Vec<BuildFinding>,
    pub not_evaluated: Vec<&'static str>,
}
fn definition(defs: &[ItemDefinition], hash: ItemHash) -> Option<&ItemDefinition> {
    defs.iter().find(|d| d.hash == hash)
}
pub fn migrated_hash(hash: ItemHash) -> ItemHash {
    ItemHash::new(match hash.value() {
        1334959255 => 2328211300,
        2958378809 => 2932390016,
        1751782730 => 3168997075,
        3635991036 => 2240888816,
        3105935002 => 2550323932,
        3481861797 => 3941205951,
        3225959819 => 2453351420,
        3382391785 => 2842471112,
        3887892656 => 2849050827,
        hash => hash,
    })
}
/// DIM loadout-utils.ts matchByHash, including its D1 horn/shader entries.
pub fn resolves_by_hash(d: &ItemDefinition) -> bool {
    d.is_instance_item != Some(true)
        || d.bucket_hash.is_some_and(|b| {
            matches!(
                b.value(),
                3284755031 | 4274335291 | 1093090108 | 3796357825 | 2973005342 | 1506418338
            )
        })
}
fn owner(location: &ItemLocation) -> Option<&CharacterId> {
    match location {
        ItemLocation::Character(id) | ItemLocation::Postmaster(id) => Some(id),
        _ => None,
    }
}
fn compatible(class: Option<ClassType>, build_class: u8) -> Option<bool> {
    match (class, build_class) {
        (Some(ClassType::Unknown) | None, _) => None,
        (Some(ClassType::Any), _) | (_, 3) => Some(true),
        (Some(class), expected) => Some(matches!(
            (class, expected),
            (ClassType::Titan, 0) | (ClassType::Hunter, 1) | (ClassType::Warlock, 2)
        )),
    }
}
fn armor(d: &ItemDefinition) -> bool {
    d.bucket_hash.is_some_and(|b| {
        matches!(
            b.value(),
            3448274439 | 3551918588 | 14239492 | 20886954 | 1585787867
        )
    })
}
fn exotic(d: &ItemDefinition) -> bool {
    d.tier.as_deref() == Some("Exotic")
}
fn evaluated(kind: FindingKind, present: bool) -> BuildFinding {
    BuildFinding {
        kind,
        evaluation: "evaluated",
        present: Some(present),
        reason: None,
    }
}
fn skipped(kind: FindingKind, reason: &'static str) -> BuildFinding {
    BuildFinding {
        kind,
        evaluation: "not_evaluated",
        present: None,
        reason: Some(reason),
    }
}
fn crafting_unavailable(
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    objectives: &[ObjectiveDefinition],
) -> bool {
    inventory.iter().any(|item| {
        let Some(d) = definition(defs, item.item_hash) else {
            return true;
        };
        let Some(category) = d
            .armor
            .socket_categories
            .iter()
            .find(|c| c.socket_category_hash == 3583996951)
        else {
            return false;
        };
        let raw = category
            .socket_indexes
            .first()
            .and_then(|index| {
                item.sockets
                    .iter()
                    .find(|s| u32::from(s.socket_index) == *index)
            })
            .and_then(|s| s.plug_objectives.as_ref());
        raw.is_none_or(|raw| {
            // Observed empty objectives identify a non-crafted item. A nonempty
            // but incomplete crafting tuple cannot establish a known date.
            (!raw.is_empty()
                && crate::build_capture::crafted_date_from_objectives(raw, objectives).is_none())
                || raw.iter().any(|(hash, _)| {
                    objectives
                        .iter()
                        .find(|d| d.hash() == *hash)
                        .and_then(|d| d.ui_style())
                        .is_none()
                })
        })
    })
}

/// Inventory order is the projection's canonical order. Like DIM, fallback examines
/// only its first hash candidate, never a later transferable substitute.
pub fn analyze_build(
    build: &BuildDefinition,
    target: &CharacterId,
    inventory: &[ItemInstance],
    defs: &[ItemDefinition],
    stat_defs: &[StatDefinition],
    characters: &[Character],
    objectives: &[ObjectiveDefinition],
) -> BuildAnalysis {
    let items: Vec<_> = build
        .equipped
        .iter()
        .map(|s| (s, true))
        .chain(build.unequipped.iter().map(|s| (s, false)))
        .map(|(spec, should_equip)| {
            let info = definition(defs, migrated_hash(spec.hash));
            let found = info.and_then(|d| {
                if resolves_by_hash(d) {
                    let candidates: Vec<_> =
                        inventory.iter().filter(|i| i.item_hash == d.hash).collect();
                    candidates
                        .iter()
                        .find(|i| owner(&i.location) == Some(target))
                        .copied()
                        .or_else(|| candidates.first().copied().filter(|i| !i.notransfer(d)))
                } else {
                    spec.instance_id
                        .as_ref()
                        .and_then(|id| {
                            inventory
                                .iter()
                                .find(|i| &i.instance_id == id && id.as_str() != "0")
                        })
                        .or_else(|| {
                            let date = spec.crafted_date.filter(|date| *date != 0)?;
                            // DIM keyBy keeps the last instanced crafted-date match;
                            // neither hash nor target ownership constrains this fallback.
                            inventory.iter().rev().find(|item| {
                                definition(defs, item.item_hash).is_some_and(|d| {
                                    d.is_instance_item == Some(true)
                                        && crate::build_capture::crafted_date(item, d, objectives)
                                            == Some(date)
                                })
                            })
                        })
                }
            });
            ResolvedBuildItem {
                spec: spec.clone(),
                should_equip,
                class_compatible: found
                    .and_then(|i| definition(defs, i.item_hash))
                    .and_then(|d| compatible(d.class_type, build.class_type)),
                resolution_reason: if found.is_some() {
                    None
                } else if info.is_none() {
                    Some("item_definition_unavailable")
                } else if spec.crafted_date.is_some_and(|date| date != 0)
                    && info.is_some_and(|d| !resolves_by_hash(d))
                    && crafting_unavailable(inventory, defs, objectives)
                {
                    Some("crafted_dates_unavailable")
                } else {
                    Some("item_not_found")
                },
                inventory: found.cloned(),
            }
        })
        .collect();
    let missing = items.iter().filter(|i| i.inventory.is_none()).count();
    let equipped_armor: Vec<_> = items
        .iter()
        .filter(|i| i.should_equip)
        .filter_map(|i| i.inventory.as_ref())
        .filter_map(|i| definition(defs, i.item_hash).map(|d| (i, d)))
        .filter(|(_, d)| armor(d))
        .collect();
    let mut findings = vec![
        evaluated(FindingKind::MissingItems, missing > 0),
        evaluated(
            FindingKind::NotAFullArmorSet,
            !equipped_armor.is_empty() && equipped_armor.len() < 5,
        ),
    ];
    let parameter = build.parameters.as_ref();
    let mut exotic_finding = evaluated(FindingKind::DoesNotRespectExotic, false);
    // DIM evaluates this only if some equipped armor successfully resolved.
    if !equipped_armor.is_empty() {
        let selected_exotic = equipped_armor
            .iter()
            .map(|(_, d)| *d)
            .find(|d| exotic(d))
            .or_else(|| {
                items
                    .iter()
                    .filter(|i| i.should_equip && i.inventory.is_none())
                    .filter_map(|i| definition(defs, i.spec.hash))
                    .find(|d| exotic(d))
            });
        match parameter.and_then(|p| p.exotic_armor_hash) {
            Some(ExoticArmor::None) => exotic_finding.present = Some(selected_exotic.is_some()),
            Some(ExoticArmor::Any) => exotic_finding.present = Some(selected_exotic.is_none()),
            Some(ExoticArmor::Item(hash)) => match definition(defs, hash) {
                Some(expected) => {
                    exotic_finding.present =
                        Some(selected_exotic.is_none_or(|d| d.name != expected.name))
                }
                None => {
                    exotic_finding = skipped(
                        FindingKind::DoesNotRespectExotic,
                        "exotic_definition_unavailable",
                    )
                }
            },
            None => {}
        }
    }
    findings.push(exotic_finding);
    let mut invalid = evaluated(FindingKind::InvalidSearchQuery, false);
    let mut nonmatching = BuildFinding {
        kind: FindingKind::ItemsDoNotMatchSearchQuery,
        evaluation: "search_only",
        present: Some(false),
        reason: Some("optimizer_filtering_and_suppression_not_evaluated"),
    };
    if !equipped_armor.is_empty()
        && let Some(query) = parameter
            .and_then(|p| p.query.as_deref())
            .filter(|q| !q.is_empty())
    {
        match search_query::compile(query) {
            Err(e)
                if matches!(
                    e.kind,
                    search_query::QueryParseErrorKind::UnsupportedFilter
                        | search_query::QueryParseErrorKind::UnknownFilter
                        | search_query::QueryParseErrorKind::QueryTooLong
                ) =>
            {
                invalid = skipped(
                    FindingKind::InvalidSearchQuery,
                    "search_engine_capability_unavailable",
                );
                nonmatching = skipped(
                    FindingKind::ItemsDoNotMatchSearchQuery,
                    "search_engine_capability_unavailable",
                );
            }
            Err(_) => invalid.present = Some(true),
            Ok(query) => {
                let mut execution = search_query::ResolvedQuery::new(&query);
                execution.stats = stat_defs.to_vec();
                let mut unavailable = false;
                for name in search_query::referenced_characters(&query) {
                    let candidates: Vec<_> = characters
                        .iter()
                        .filter(|c| {
                            c.id.as_str() == name
                                || c.class
                                    .as_deref()
                                    .is_some_and(|s| s.eq_ignore_ascii_case(&name))
                        })
                        .collect();
                    if let [character] = candidates.as_slice() {
                        execution.characters.insert(name, character.id.clone());
                    } else {
                        unavailable = true;
                    }
                }
                if search_query::referenced_stats(&query).iter().any(|name| {
                    !stat_defs
                        .iter()
                        .any(|d| query::normalize_search_text(d.name()) == *name)
                }) {
                    unavailable = true;
                }
                if execution.needs_plugs()
                    && items.iter().any(|i| !i.spec.socket_overrides.is_empty())
                {
                    unavailable = true;
                }
                if unavailable {
                    nonmatching = skipped(
                        FindingKind::ItemsDoNotMatchSearchQuery,
                        "query_context_unavailable",
                    );
                } else {
                    nonmatching.present = Some(
                        equipped_armor
                            .iter()
                            .filter(|(_, d)| {
                                compatible(d.class_type, build.class_type) == Some(true)
                            })
                            .any(|(i, d)| {
                                let selected: Vec<_> = i
                                    .plugs
                                    .iter()
                                    .filter_map(|p| definition(defs, p.plug_hash).cloned())
                                    .collect();
                                let available: Vec<_> = i
                                    .available_plugs
                                    .iter()
                                    .filter_map(|p| definition(defs, p.plug_hash).cloned())
                                    .collect();
                                !execution.matches(i, Some(d), &selected, &available, true)
                            }),
                    );
                }
            }
        }
    }
    findings.extend([invalid, nonmatching]);
    for (kind, reason) in [
        (FindingKind::InvalidMods, "mod_assignment_data_unavailable"),
        (
            FindingKind::EmptyFragmentSlots,
            "fragment_capacity_unavailable",
        ),
        (
            FindingKind::TooManyFragments,
            "fragment_capacity_unavailable",
        ),
        (
            FindingKind::NeedsArmorUpgrades,
            "armor_upgrade_data_unavailable",
        ),
        (
            FindingKind::UsesSeasonalMods,
            "seasonal_mod_costs_unavailable",
        ),
        (FindingKind::BetterStatsAvailable, "optimizer_unavailable"),
        (
            FindingKind::ModsDontFit,
            "armor_energy_and_socket_data_unavailable",
        ),
        (
            FindingKind::DoesNotSatisfyStatConstraints,
            "armor_base_stats_unavailable",
        ),
    ] {
        if kind == FindingKind::InvalidMods
            && parameter.is_some_and(|p| p.artifact_unlocks.is_some())
        {
            let mut finding = evaluated(kind, true);
            finding.reason = Some("artifact_unlocks_present");
            findings.push(finding);
        } else {
            findings.push(skipped(kind, reason));
        }
    }
    BuildAnalysis {
        target_character: target.clone(),
        specified: items.len(),
        resolved: items.len() - missing,
        missing,
        items,
        findings,
        not_evaluated: vec!["socket_overrides_not_evaluated"],
    }
}
