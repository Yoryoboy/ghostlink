//! DIM process/mappers.ts:189-249 (getAutoMods, mapAutoMods) and
//! process-worker/auto-stat-mod-utils.ts (picks cache, chooseAutoMods).
use super::stat_mods::{LoSessionInfo, build_context_key, pack_stat_needs};
use super::*;
use crate::armor_catalog::ArmorCatalogRecord;
use crate::{InvestmentStat, ItemDefinition};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AutoMod {
    pub hash: u32,
    pub cost: i32,
}
/// Major and minor always exist together or not at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneralAutoMods {
    pub major: AutoMod,
    pub minor: AutoMod,
}
/// DIM `AutoModData` (process-worker/types.ts:93-101), keyed by stat hash.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutoModData {
    pub general_mods: HashMap<u32, GeneralAutoMods>,
    pub artifice_mods: HashMap<u32, u32>,
}

/// DIM `getAutoMods` followed by `mapAutoMods`: Ghostlink maps straight to the
/// hash and energy cost that `mapAutoMods` keeps, since nothing else reads the
/// definitions. Plug sets and definitions that DIM would read from a complete
/// manifest are typed unknown here instead of being treated as empty.
pub fn auto_mod_data(
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
) -> Result<AutoModData, &'static str> {
    // mappers.ts:210-214: only pluggable definitions with investment stats.
    let plug_set = |set_hash: u32| -> Result<Vec<&ItemDefinition>, &'static str> {
        let set = catalogs
            .iter()
            .find_map(|r| match r {
                ArmorCatalogRecord::PlugSet(s) if s.hash == set_hash => Some(s),
                _ => None,
            })
            .ok_or("auto_mod_plug_set_unavailable")?;
        let mut plugs = Vec::new();
        for entry in &set.reusable_plug_items {
            let plug = definitions
                .iter()
                .find(|d| d.hash.value() == i64::from(entry.plug_item_hash))
                .ok_or("auto_mod_plug_definition_unavailable")?;
            if plug.armor.plug_category_hash.is_some() && !plug.armor.investment_stats.is_empty() {
                plugs.push(plug);
            }
        }
        Ok(plugs)
    };
    let general = plug_set(GENERAL_SOCKET_PLUG_SET)?;
    let artifice = plug_set(ARTIFICE_SOCKET_PLUG_SET)?;
    let cost = |d: &ItemDefinition| d.armor.energy_cost.as_ref().map_or(0, |e| e.energy_cost);
    let gives = |d: &ItemDefinition, stat: u32, value: i32| {
        d.armor
            .investment_stats
            .iter()
            .any(|s: &InvestmentStat| s.stat_type_hash == stat && s.value == value)
    };
    let hash = |d: &ItemDefinition| d.hash.value() as u32;
    let mut data = AutoModData::default();
    for stat in ARMOR_STATS {
        // A small boost in a single stat that costs no energy (mappers.ts:220-231).
        if let Some(mod_def) = artifice
            .iter()
            .find(|d| gives(d, stat, ARTIFICE_STAT_BOOST))
            .filter(|d| cost(d) == 0)
        {
            data.artifice_mods.insert(stat, hash(mod_def));
        }
        // Largest cost first; the second-largest is the reduced-cost variant, which
        // is used once unlocked (mappers.ts:232-239).
        let find_by_value = |value: i32| {
            let mut relevant: Vec<_> = general.iter().filter(|d| gives(d, stat, value)).collect();
            relevant.sort_by_key(|d| std::cmp::Reverse(cost(d)));
            let large = relevant.first().copied();
            let small = relevant.get(1).copied();
            match small {
                Some(small) if unlocked.contains(&hash(small)) => Some(small),
                _ => large,
            }
            .map(|d| AutoMod {
                hash: hash(d),
                cost: cost(d),
            })
        };
        if let (Some(major), Some(minor)) = (
            find_by_value(MAJOR_STAT_BOOST),
            find_by_value(MINOR_STAT_BOOST),
        ) {
            data.general_mods
                .insert(stat, GeneralAutoMods { major, minor });
        }
    }
    Ok(data)
}

/// A particular way of achieving a target stat value for a single stat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModsPick {
    pub num_artifice_mods: i32,
    pub num_general_mods: i32,
    /// Costs of the general mods, majors first (descending for a stat's own costs).
    pub general_mods_costs: Vec<i32>,
    /// Majors, then minors, then artifice mods.
    pub mod_hashes: Vec<u32>,
    pub mod_energy_cost: i32,
    pub target_stat_index: usize,
    /// The points actually provided (a request for 1 point may get an artifice +3).
    pub exact_stat_points: i32,
}
/// Precalculated picks that reach each target value of one stat.
pub type CacheForStat = HashMap<i32, Vec<Arc<ModsPick>>>;
/// Indexed by position in the stat order.
pub type AutoModsCache = Vec<CacheForStat>;

fn repeat<T: Clone>(n: i32, value: T) -> std::iter::RepeatN<T> {
    std::iter::repeat_n(value, n as usize)
}

/// DIM auto-stat-mod-utils.ts:273-353. The cache is separated by stat; picks for
/// different stats are pieced together by `choose_auto_mods`.
fn build_cache_for_stat(
    data: &AutoModData,
    stat_hash: u32,
    stat_index: usize,
    available_general_mods: i32,
) -> CacheForStat {
    let mut cache = CacheForStat::new();
    let artifice = data.artifice_mods.get(&stat_hash).copied();
    let general = data.general_mods.get(&stat_hash);
    let minor = general.map(|g| g.minor);
    let major = general.map(|g| g.major);
    for artifice_count in 0..=if artifice.is_some() { 5 } else { 0 } {
        for minors in 0..=if minor.is_some() {
            available_general_mods
        } else {
            0
        } {
            for majors in 0..=if major.is_some() {
                available_general_mods - minors
            } else {
                0
            } {
                let value = artifice_count * ARTIFICE_STAT_BOOST
                    + minors * MINOR_STAT_BOOST
                    + majors * MAJOR_STAT_BOOST;
                if value == 0 {
                    continue;
                }
                // Overshooting is allowed within reason, keeping picks Pareto-optimal
                // (comment at auto-stat-mod-utils.ts:306-315).
                let lower = value
                    - if artifice_count > 0 {
                        ARTIFICE_STAT_BOOST - 1
                    } else {
                        MINOR_STAT_BOOST - 1
                    };
                let none = AutoMod { hash: 0, cost: 0 };
                let (minor_mod, major_mod) = (minor.unwrap_or(none), major.unwrap_or(none));
                let pick = Arc::new(ModsPick {
                    num_artifice_mods: artifice_count,
                    num_general_mods: minors + majors,
                    general_mods_costs: repeat(majors, major_mod.cost)
                        .chain(repeat(minors, minor_mod.cost))
                        .collect(),
                    mod_hashes: repeat(majors, major_mod.hash)
                        .chain(repeat(minors, minor_mod.hash))
                        .chain(repeat(artifice_count, artifice.unwrap_or(0)))
                        .collect(),
                    mod_energy_cost: minors * minor_mod.cost + majors * major_mod.cost,
                    target_stat_index: stat_index,
                    exact_stat_points: value,
                });
                for achievable in lower..=value {
                    cache.entry(achievable).or_default().push(pick.clone());
                }
            }
        }
    }
    // Prefer picks that use artifice mods, since they are free (stable sort).
    for picks in cache.values_mut() {
        picks.sort_by_key(|p| (-p.num_artifice_mods, -p.num_general_mods));
    }
    cache
}

/// DIM `buildAutoModsMap` (auto-stat-mod-utils.ts:355-366).
pub fn build_auto_mods_map(
    data: &AutoModData,
    available_general_mods: i32,
    stat_order: &[u32; 6],
) -> AutoModsCache {
    stat_order
        .iter()
        .enumerate()
        .map(|(index, hash)| build_cache_for_stat(data, *hash, index, available_general_mods))
        .collect()
}

/// DIM `chooseAutoMods` (auto-stat-mod-utils.ts:55-114): picks auto mods that provide
/// at least `needed_stats`. The result is a pure function of the arguments for a
/// fixed session, so single energy vectors with a non-negative budget are memoized.
pub fn choose_auto_mods(
    info: &LoSessionInfo,
    needed_stats: &[i32; 6],
    num_artifice_mods: i32,
    energies: &[[i32; 5]],
    total_energy: i32,
) -> Option<Vec<Arc<ModsPick>>> {
    let solve = || {
        let mut picked = Vec::new();
        recursively_choose_mods(
            info,
            needed_stats,
            0,
            info.num_available_general_mods,
            num_artifice_mods,
            energies,
            total_energy,
            &mut picked,
        )
        .then_some(picked)
    };
    if total_energy < 0 || energies.len() > 1 {
        return solve();
    }
    let context = build_context_key(energies, num_artifice_mods, total_energy);
    let needs = pack_stat_needs(needed_stats);
    if let Some(cached) = info
        .auto_mods_memo
        .borrow()
        .get(&context)
        .and_then(|memo| memo.get(&needs))
    {
        return cached.clone();
    }
    let result = solve();
    info.auto_mods_memo
        .borrow_mut()
        .entry(context)
        .or_default()
        .insert(needs, result.clone());
    result
}

/// DIM `doGeneralModsFit` (:162-183): the user-picked and picked general mod costs,
/// sorted descending, must fit the descending remaining energies of some assignment.
fn do_general_mods_fit(costs: &[i32], energies: &[[i32; 5]], picked: &[Arc<ModsPick>]) -> bool {
    let mut costs = costs.to_vec();
    for pick in picked {
        costs.extend(&pick.general_mods_costs);
    }
    costs.sort_by(|a, b| b.cmp(a));
    energies.iter().any(|capacities| {
        costs
            .iter()
            .enumerate()
            .all(|(i, cost)| capacities.get(i).is_some_and(|c| cost <= c))
    })
}

/// DIM `recursivelyChooseMods` (:191-255); `picked` holds the earlier stats' picks and
/// is left holding the solution when this returns true.
#[allow(clippy::too_many_arguments)] // Mirrors DIM's explicit recursion state.
fn recursively_choose_mods(
    info: &LoSessionInfo,
    needed: &[i32; 6],
    mut stat_index: usize,
    general_slots: i32,
    artifice_slots: i32,
    energies: &[[i32; 5]],
    total_energy: i32,
    picked: &mut Vec<Arc<ModsPick>>,
) -> bool {
    while stat_index < needed.len() && needed[stat_index] == 0 {
        stat_index += 1;
    }
    if stat_index == needed.len() {
        return do_general_mods_fit(&info.general_mod_costs, energies, picked);
    }
    let Some(possible) = info.auto_mod_options[stat_index].get(&needed[stat_index]) else {
        return false;
    };
    for pick in possible {
        if pick.num_artifice_mods > artifice_slots
            || pick.num_general_mods > general_slots
            || pick.mod_energy_cost > total_energy
        {
            continue;
        }
        picked.push(pick.clone());
        if recursively_choose_mods(
            info,
            needed,
            stat_index + 1,
            general_slots - pick.num_general_mods,
            artifice_slots - pick.num_artifice_mods,
            energies,
            total_energy - pick.mod_energy_cost,
            picked,
        ) {
            return true;
        }
        picked.pop();
    }
    false
}
