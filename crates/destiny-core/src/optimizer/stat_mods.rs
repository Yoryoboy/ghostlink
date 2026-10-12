//! DIM process-worker/process-utils.ts (session info, slot-independent mod assignment,
//! `updateMaxStats`, `pickOptimalStatMods`, `greedyPickStatMods`) and
//! process/mappers.ts:43-59 (`mapArmor2ModToProcessMod`).
//! A `LoSessionInfo` belongs to one search thread, like DIM's worker: its memos use
//! interior mutability without locking.
use super::auto_mods::{
    AutoModData, AutoModsCache, ModsPick, build_auto_mods_map, choose_auto_mods,
};
use super::*;
use crate::ItemDefinition;
use crate::mod_assignment::{SPECIALTY_SOCKETS, generate_permutations_of_five};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// DIM `ProcessMod`; `tag` is the activity (specialty socket) plug category.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProcessMod {
    pub hash: u32,
    pub energy_cost: i32,
    pub tag: Option<u32>,
}

/// `mapArmor2ModToProcessMod`: only activity mods carry a tag. DIM's other tagged
/// categories (unknown plug categories) have no tag lookup in Ghostlink, and the
/// activity classification is the one `rank_independent_mods` already uses.
pub fn process_mod(definition: &ItemDefinition) -> ProcessMod {
    let category = definition.armor.plug_category_hash;
    ProcessMod {
        hash: definition.hash.value() as u32,
        energy_cost: definition
            .armor
            .energy_cost
            .as_ref()
            .map_or(0, |e| e.energy_cost),
        tag: category.filter(|c| SPECIALTY_SOCKETS.iter().any(|(_, activity)| activity == c)),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RejectionRate {
    pub times_failed: u64,
    pub times_checked: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FinalAssignmentStatistics {
    pub mod_assignment_attempted: u64,
    pub mods_assignment_failed: u64,
    pub auto_mods_assignment_failed: u64,
}
/// DIM `ModAssignmentStatistics` (process-worker/types.ts:113-126).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModAssignmentStatistics {
    pub early_mods_check: RejectionRate,
    pub auto_mods_pick: RejectionRate,
    pub final_assignment: FinalAssignmentStatistics,
}

/// DIM `MinMaxStat`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MinMaxStat {
    pub min_stat: i32,
    pub max_stat: i32,
}

/// Memo key for an energy profile (auto-stat-mod-utils.ts:116-156): remaining energies
/// are 0-10 (5 bits), sorted descending so equal multisets pack equally.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ContextKey {
    Packed(i64),
    Multiple(Vec<i64>, i32, i32),
}
fn pack_energy_vector(capacities: &[i32; 5]) -> i64 {
    capacities
        .iter()
        .fold(0, |packed, c| packed * 32 + i64::from(*c))
}
/// Stat needs are 0-200: 8 bits per stat packs all six exactly.
pub fn pack_stat_needs(needed: &[i32; 6]) -> i64 {
    needed
        .iter()
        .rev()
        .fold(0, |packed, n| packed * 0x100 + i64::from(*n))
}
pub fn build_context_key(energies: &[[i32; 5]], artifice: i32, total: i32) -> ContextKey {
    if let [single] = energies {
        return ContextKey::Packed(
            (pack_energy_vector(single) * 8 + i64::from(artifice)) * 64 + i64::from(total),
        );
    }
    let mut packed: Vec<_> = energies.iter().map(pack_energy_vector).collect();
    packed.sort_by(|a, b| b.cmp(a));
    ContextKey::Multiple(packed, artifice, total)
}

/// Packed stat needs to the memoized `choose_auto_mods` result (`None` = impossible).
type AutoModsMemo = HashMap<i64, Option<Vec<Arc<ModsPick>>>>;

/// DIM `LoSessionInfo`: data that stays the same in a given run.
#[derive(Debug)]
pub struct LoSessionInfo {
    pub auto_mod_options: AutoModsCache,
    pub has_activity_mods: bool,
    /// The total cost of all user-picked general and activity mods.
    pub total_mod_energy_cost: i32,
    /// The cost of user-picked general mods, sorted descending.
    pub general_mod_costs: Vec<i32>,
    /// How many general mod slots are available for auto stat mods.
    pub num_available_general_mods: i32,
    /// All uniquely distinguishable activity mod permutations.
    pub activity_mod_permutations: Vec<Vec<Option<ProcessMod>>>,
    pub activity_tag_counts: HashMap<u32, usize>,
    pub(super) auto_mods_memo: RefCell<HashMap<ContextKey, AutoModsMemo>>,
    pub(super) max_boost_memo: RefCell<HashMap<ContextKey, HashMap<i64, i32>>>,
}
impl LoSessionInfo {
    /// DIM `precalculateStructures` (process-utils.ts:50-78).
    pub fn new(
        auto_mods: &AutoModData,
        general_mods: &[ProcessMod],
        activity_mods: &[ProcessMod],
        auto_stat_mods: bool,
        stat_order: &[u32; 6],
    ) -> Self {
        let mut general_mod_costs: Vec<_> = general_mods.iter().map(|m| m.energy_cost).collect();
        general_mod_costs.sort_by(|a, b| b.cmp(a));
        let num_available_general_mods = if auto_stat_mods {
            5 - general_mod_costs.len() as i32
        } else {
            0
        };
        let mut activity_tag_counts = HashMap::new();
        for tag in activity_mods.iter().filter_map(|m| m.tag) {
            *activity_tag_counts.entry(tag).or_insert(0) += 1;
        }
        Self {
            auto_mod_options: build_auto_mods_map(
                auto_mods,
                num_available_general_mods,
                stat_order,
            ),
            has_activity_mods: !activity_mods.is_empty(),
            total_mod_energy_cost: general_mod_costs.iter().sum::<i32>()
                + activity_mods.iter().map(|m| m.energy_cost).sum::<i32>(),
            general_mod_costs,
            num_available_general_mods,
            activity_mod_permutations: generate_process_mod_permutations(activity_mods),
            activity_tag_counts,
            auto_mods_memo: RefCell::default(),
            max_boost_memo: RefCell::default(),
        }
    }
    /// Memoized `choose_auto_mods` results (one per energy profile and stat needs).
    pub fn auto_mods_memo_size(&self) -> usize {
        self.auto_mods_memo
            .borrow()
            .values()
            .map(HashMap::len)
            .sum()
    }
    /// Memoized `update_max_stats` boosts.
    pub fn max_boost_memo_size(&self) -> usize {
        self.max_boost_memo
            .borrow()
            .values()
            .map(HashMap::len)
            .sum()
    }
}

/// DIM `generateProcessModPermutations` (:612-623): two mods that fit the same socket
/// with the same cost and tag are identical from the assignment perspective.
pub fn generate_process_mod_permutations(mods: &[ProcessMod]) -> Vec<Vec<Option<ProcessMod>>> {
    generate_permutations_of_five(mods, |permutation| {
        permutation
            .iter()
            .map(|m| m.as_ref().map(|m| (m.energy_cost, m.tag)))
            .collect::<Vec<_>>()
    })
}

/// DIM `getRemainingEnergiesPerAssignment` result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemainingEnergies {
    /// Total remaining energy capacity across the set.
    pub set_energy: i32,
    /// For each valid activity permutation, the energy left per item, sorted
    /// descending (consumers treat them as multisets).
    pub remaining_energies_per_assignment: Vec<[i32; 5]>,
}
/// Per-armor-set cache shared by `update_max_stats` and `pick_optimal_stat_mods`;
/// the caller owns one instance and must clear `result` for each new armor set.
#[derive(Clone, Debug, Default)]
pub struct SetEnergyCache {
    pub result: Option<Rc<RemainingEnergies>>,
}

/// :85-134.
fn remaining_energies_per_assignment(
    permutations: &[Vec<Option<ProcessMod>>],
    items: &[ProcessItem],
) -> RemainingEnergies {
    let set_energy = items.iter().map(|i| i.remaining_energy_capacity).sum();
    let mut per_assignment = Vec::new();
    'permutations: for permutation in permutations {
        let mut capacities = [0; 5];
        for (i, item) in items.iter().enumerate() {
            capacities[i] = item.remaining_energy_capacity;
            if let Some(Some(activity)) = permutation.get(i) {
                if activity.energy_cost > item.remaining_energy_capacity
                    || item.compatible_activity_mod != activity.tag
                {
                    continue 'permutations;
                }
                capacities[i] -= activity.energy_cost;
            }
        }
        capacities.sort_by(|a, b| b.cmp(a));
        per_assignment.push(capacities);
    }
    RemainingEnergies {
        set_energy,
        remaining_energies_per_assignment: per_assignment,
    }
}
fn compute_remaining_energies(
    info: &LoSessionInfo,
    items: &[ProcessItem],
    cache: Option<&mut SetEnergyCache>,
) -> Rc<RemainingEnergies> {
    if let Some(result) = cache.as_deref().and_then(|c| c.result.clone()) {
        return result;
    }
    let result = Rc::new(remaining_energies_per_assignment(
        &info.activity_mod_permutations,
        items,
    ));
    if let Some(cache) = cache {
        cache.result = Some(result.clone());
    }
    result
}

/// DIM `updateMaxStats` (:170-313): raises `stat_ranges` by trying to individually get
/// the highest value of each stat. Returns true if at least one stat can exceed the
/// desired range's minimum.
pub fn update_max_stats(
    info: &LoSessionInfo,
    armor: &[ProcessItem],
    set_stats: &[i32; 6],
    num_artifice_mods: i32,
    desired: &[DesiredStatRange],
    stat_ranges: &mut [MinMaxStat],
    mut energy_cache: Option<&mut SetEnergyCache>,
) -> bool {
    let mut found_any_improvement = false;
    // How many extra points each stat needs to reach its minimum.
    let mut required = [0; 6];
    for (i, filter) in desired.iter().enumerate() {
        let (min, max) = (i32::from(filter.min_stat), i32::from(filter.max_stat));
        let value = set_stats[i];
        let range = &mut stat_ranges[i];
        if range.max_stat < min {
            // Only called with sets that satisfy the constraints, so bump optimistically.
            range.max_stat = min;
        }
        if value > range.max_stat {
            range.max_stat = value;
            // A range with min == max cannot be improved within the desired range.
            found_any_improvement |= min < max;
        }
        required[i] = (min - value).max(0);
    }
    if info.num_available_general_mods == 0 && num_artifice_mods == 0 {
        return found_any_improvement;
    }
    // Cheap upper bound on what auto mods can add to one stat (ignoring energy and slots).
    let max_single_stat_bonus = info.num_available_general_mods * MAJOR_STAT_BOOST
        + num_artifice_mods * ARTIFICE_STAT_BOOST;
    let mut remaining: Option<Rc<RemainingEnergies>> = None;
    let mut context: Option<ContextKey> = None;
    for (i, filter) in desired.iter().enumerate() {
        let (min, max) = (i32::from(filter.min_stat), i32::from(filter.max_stat));
        let value = set_stats[i];
        if stat_ranges[i].max_stat >= MAX_STAT
            || value + max_single_stat_bonus <= stat_ranges[i].max_stat
        {
            continue;
        }
        let energies = remaining
            .get_or_insert_with(|| {
                compute_remaining_energies(info, armor, energy_cache.as_deref_mut())
            })
            .clone();
        let budget = energies.set_energy - info.total_mod_energy_cost;
        let context = context.get_or_insert_with(|| {
            build_context_key(
                &energies.remaining_energies_per_assignment,
                num_artifice_mods,
                budget,
            )
        });
        // The largest boost holding the other stats at their minimums does not depend on
        // this stat's value or the running max, so it is memoized per energy profile.
        let previous = required[i];
        required[i] = 0;
        let boost_key = i as i64 * 0x1000000000000 + pack_stat_needs(&required);
        let cached = info
            .max_boost_memo
            .borrow()
            .get(context)
            .and_then(|memo| memo.get(&boost_key).copied());
        let max_boost = cached.unwrap_or_else(|| {
            // chooseAutoMods is monotonic in one stat's requirement, so a binary search is
            // exact. -1 means even the base minimums cannot be met.
            let (mut good, mut high) = (-1, max_single_stat_bonus);
            while good < high {
                let mid = (good + high + 1) >> 1;
                required[i] = mid;
                let feasible = choose_auto_mods(
                    info,
                    &required,
                    num_artifice_mods,
                    &energies.remaining_energies_per_assignment,
                    budget,
                );
                if feasible.is_some() {
                    good = mid;
                } else {
                    high = mid - 1;
                }
            }
            info.max_boost_memo
                .borrow_mut()
                .entry(context.clone())
                .or_default()
                .insert(boost_key, good);
            good
        });
        required[i] = previous;
        let new_value = value + max_boost.min(MAX_STAT - value);
        if new_value > stat_ranges[i].max_stat {
            found_any_improvement |= min < max && new_value > min;
            stat_ranges[i].max_stat = new_value;
        }
    }
    found_any_improvement
}

/// DIM `pickAndAssignSlotIndependentMods` (:333-432): can all user-chosen general and
/// activity mods be assigned to the set, and can auto stat mods provide `needed_stats`?
/// Returns the picked auto mods (empty when none are needed), `None` on failure.
pub fn pick_and_assign_slot_independent_mods(
    info: &LoSessionInfo,
    statistics: &mut ModAssignmentStatistics,
    items: &[ProcessItem],
    needed_stats: Option<&[i32; 6]>,
    num_artifice: i32,
) -> Option<Vec<Arc<ModsPick>>> {
    statistics.early_mods_check.times_checked += 1;
    let set_energy: i32 = items.iter().map(|i| i.remaining_energy_capacity).sum();
    if set_energy < info.total_mod_energy_cost {
        statistics.early_mods_check.times_failed += 1;
        return None;
    }
    // Early check: every activity tag needs enough items with a matching socket.
    if info.has_activity_mods {
        for (tag, count) in &info.activity_tag_counts {
            let sockets = items
                .iter()
                .filter(|i| i.compatible_activity_mod == Some(*tag))
                .count();
            if sockets < *count {
                statistics.early_mods_check.times_failed += 1;
                return None;
            }
        }
    }
    let mut assigned_at_least_once = false;
    statistics.final_assignment.mod_assignment_attempted += 1;
    'permutations: for permutation in &info.activity_mod_permutations {
        for (i, item) in items.iter().enumerate() {
            let Some(Some(activity)) = permutation.get(i) else {
                continue;
            };
            if activity.energy_cost > item.remaining_energy_capacity
                || item.compatible_activity_mod != activity.tag
            {
                continue 'permutations;
            }
        }
        assigned_at_least_once = true;
        // A valid activity assignment: see how much energy is left per piece, descending.
        let mut capacities = [0; 5];
        for (i, item) in items.iter().enumerate() {
            let used = permutation
                .get(i)
                .and_then(Option::as_ref)
                .map_or(0, |m| m.energy_cost);
            capacities[i] = item.remaining_energy_capacity - used;
        }
        capacities.sort_by(|a, b| b.cmp(a));
        if let Some(needed) = needed_stats {
            let result = choose_auto_mods(
                info,
                needed,
                num_artifice,
                &[capacities],
                set_energy - info.total_mod_energy_cost,
            );
            if result.is_some() {
                return result;
            }
        } else if info
            .general_mod_costs
            .iter()
            .enumerate()
            .all(|(i, cost)| capacities.get(i).is_some_and(|c| cost <= c))
        {
            return Some(Vec::new());
        }
    }
    if assigned_at_least_once && needed_stats.is_some() {
        statistics.final_assignment.auto_mods_assignment_failed += 1;
    } else {
        statistics.final_assignment.mods_assignment_failed += 1;
    }
    None
}

/// The auto stat mods picked for a set and the stat points they add.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatModPick {
    pub mods: Vec<u32>,
    pub bonus_stats: [i32; 6],
}

/// DIM `pickOptimalStatMods` (:441-498): assumes the stat minimums can be hit and
/// maximizes the stats, earlier stats in the order first. `None` when no activity
/// mod assignment is valid.
pub fn pick_optimal_stat_mods(
    info: &LoSessionInfo,
    items: &[ProcessItem],
    set_stats: &[i32; 6],
    desired: &[DesiredStatRange],
    num_artifice_mods: Option<i32>,
    energy_cache: Option<&mut SetEnergyCache>,
) -> Option<StatModPick> {
    let energies = compute_remaining_energies(info, items, energy_cache);
    if energies.remaining_energies_per_assignment.is_empty() {
        return None;
    }
    // Points after which stats no longer benefit, and the points needed for the minimums.
    let mut max_added = [0; 6];
    let mut exploration = [0; 6];
    for i in (0..set_stats.len()).rev() {
        let filter = &desired[i];
        if filter.max_stat > 0 {
            let value = set_stats[i];
            if filter.min_stat > 0 {
                let needed = i32::from(filter.min_stat) - value;
                if needed > 0 {
                    exploration[i] = needed;
                }
            }
            max_added[i] = i32::from(filter.max_stat) - value;
        }
    }
    let num_artifice =
        num_artifice_mods.unwrap_or_else(|| items.iter().filter(|i| i.is_artifice).count() as i32);
    let picks = greedy_pick_stat_mods(
        info,
        &mut exploration,
        &max_added,
        num_artifice,
        &energies.remaining_energies_per_assignment,
        energies.set_energy - info.total_mod_energy_cost,
    );
    let mut bonus_stats = [0; 6];
    for pick in &picks {
        bonus_stats[pick.target_stat_index] += pick.exact_stat_points;
    }
    Some(StatModPick {
        mods: picks
            .iter()
            .flat_map(|p| p.mod_hashes.iter().copied())
            .collect(),
        bonus_stats,
    })
}

/// DIM `greedyPickStatMods` (:529-610): there are no stat tiers, so each stat in order
/// is binary-searched for the largest boost that still keeps the earlier boosts and
/// the minimums achievable. `exploration` is mutated like DIM's array.
pub fn greedy_pick_stat_mods(
    info: &LoSessionInfo,
    exploration: &mut [i32; 6],
    max_added: &[i32; 6],
    num_artifice_mods: i32,
    energies: &[[i32; 5]],
    total_energy: i32,
) -> Vec<Arc<ModsPick>> {
    // Vectors are sorted descending, so a 0 in front means all-zero.
    if energies.first().is_some_and(|e| e[0] == 0) && num_artifice_mods == 0 {
        return Vec::new();
    }
    let Some(mut picks) =
        choose_auto_mods(info, exploration, num_artifice_mods, energies, total_energy)
    else {
        // Cannot even hit the minimums.
        return Vec::new();
    };
    for i in 0..exploration.len() {
        if max_added[i] <= 0 {
            continue;
        }
        let original = exploration[i];
        let mut candidate = None;
        let mut last_good = None;
        let mut last_good_exploration = 0;
        let (mut min_boost, mut max_boost) = (ARTIFICE_STAT_BOOST.max(original), max_added[i] - 1);
        while min_boost < max_boost {
            exploration[i] = (min_boost + max_boost).div_euclid(2);
            if exploration[i] <= 0 {
                break;
            }
            candidate =
                choose_auto_mods(info, exploration, num_artifice_mods, energies, total_energy);
            if candidate.is_some() {
                min_boost = exploration[i] + 1;
                last_good = candidate.clone();
                last_good_exploration = exploration[i];
            } else {
                max_boost = exploration[i];
            }
        }
        // As in DIM, the last candidate tried wins when it succeeded.
        if let Some(candidate) = candidate {
            picks = candidate;
        } else if let Some(last_good) = last_good {
            picks = last_good;
            exploration[i] = last_good_exploration;
        } else {
            exploration[i] = original;
        }
    }
    picks
}
