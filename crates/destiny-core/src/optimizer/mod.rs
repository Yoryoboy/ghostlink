//! Pure Loadout Optimizer model, pre-filter and search (BE-008a/b/c), following DIM 29c122e
//! `loadout-builder/`. Inputs that Ghostlink does not project stay typed unknown.
use crate::armor_model::ARMOR_STATS;
use crate::build::{AssumeArmorMasterwork, StatConstraint};

mod auto_mods;
mod driver;
mod filter;
mod items;
mod search;
mod set_tracker;
mod stat_dupe;
mod stat_mods;
pub use auto_mods::{
    AutoMod, AutoModData, AutoModsCache, CacheForStat, GeneralAutoMods, ModsPick, auto_mod_data,
    build_auto_mods_map, choose_auto_mods,
};
pub use driver::{PROGRESS_CADENCE, SearchBudget, SearchOptions, SearchResult, search};
pub use filter::{
    BucketFilterInfo, FilterError, FilterInfo, FilterParams, ItemsByBucket, filter_items,
};
pub use items::{
    ArmorCandidate, Masterwork, ProcessItem, TuningPlug, TuningVariant, assumed_item_energy,
    assumed_masterwork_stats, is_assumed_artifice, is_assumed_masterworked,
    map_armor_to_process_items,
};
pub use search::{
    ArmorSet, ArmorSetData, ProcessStatistics, SearchError, SearchInputs, SkipReasons,
};
pub use set_tracker::{HeapEntry, HeapSetTracker, RETURNED_ARMOR_SETS, TieBreak, encode_stat_mix};
pub use stat_dupe::{StatsSet, compute_stat_dupe_lower};
pub use stat_mods::{
    ContextKey, FinalAssignmentStatistics, LoSessionInfo, MinMaxStat, ModAssignmentStatistics,
    ProcessMod, RejectionRate, RemainingEnergies, SetEnergyCache, StatModPick,
    generate_process_mod_permutations, greedy_pick_stat_mods,
    pick_and_assign_slot_independent_mods, pick_optimal_stat_mods, process_mod, update_max_stats,
};

// DIM loadout-builder/types.ts:105-155 and loadout/known-values.ts:39.
pub const MIN_LO_ITEM_ENERGY: i32 = 9;
pub const MAX_STAT: i32 = 200;
pub const GENERAL_SOCKET_PLUG_SET: u32 = 731468111;
pub const ARTIFICE_SOCKET_PLUG_SET: u32 = 4285066582;
pub const TUNING_SOCKET_PLUG_SET: u32 = 1155052024;
pub const ARTIFICE_STAT_BOOST: i32 = 3;
pub const MINOR_STAT_BOOST: i32 = 5;
pub const MAJOR_STAT_BOOST: i32 = 10;
pub const TUNING_STAT_BOOST: i32 = 5;
pub const BALANCED_TUNING_STAT_BOOST: i32 = 1;
/// DIM d2-known-values.ts:439 ModsWithConditionalStats.BalancedTuning.
pub const BALANCED_TUNING_PLUG: u32 = 3122197216;

/// DIM `ArmorEnergyRules` (types.ts:164-173).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EnergyRules {
    pub assume_armor_masterwork: AssumeArmorMasterwork,
    pub min_item_energy: i32,
}
/// `loDefaultArmorEnergyRules`.
impl Default for EnergyRules {
    fn default() -> Self {
        Self {
            assume_armor_masterwork: AssumeArmorMasterwork::None,
            min_item_energy: MIN_LO_ITEM_ENERGY,
        }
    }
}

/// DIM `DesiredStatRange`: ignored stats are a range with max 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DesiredStatRange {
    pub stat_hash: u32,
    pub min_stat: u16,
    pub max_stat: u16,
}

/// DIM loadout-params.ts:14-34 (constrained stats first in priority order, then
/// ignored stats in game order) with loadout-analyzer/utils.ts:32-33 (ignored => 0/0).
pub fn desired_stat_ranges(constraints: &[StatConstraint]) -> Vec<DesiredStatRange> {
    let mut resolved: Vec<_> = ARMOR_STATS
        .iter()
        .enumerate()
        .map(|(game_index, hash)| {
            // keyBy: the last constraint for a hash wins; ordering uses the first (findIndex).
            let c = constraints.iter().rev().find(|c| c.stat_hash == *hash);
            let priority = constraints
                .iter()
                .position(|c| c.stat_hash == *hash)
                .unwrap_or(100 + game_index);
            let range = DesiredStatRange {
                stat_hash: *hash,
                min_stat: c.map_or(0, |c| c.min_stat.unwrap_or(0)),
                max_stat: c.map_or(0, |c| c.max_stat.unwrap_or(MAX_STAT as u16)),
            };
            (priority, range)
        })
        .collect();
    resolved.sort_by_key(|(priority, _)| *priority);
    resolved.into_iter().map(|(_, range)| range).collect()
}

/// DIM lockedExoticHash: undefined (None), LOCKED_EXOTIC_NO_EXOTIC (-1),
/// LOCKED_EXOTIC_ANY_EXOTIC (-2), or a positive item hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockedExotic {
    None,
    Any,
    Item {
        hash: u32,
        /// DIM `defs.InventoryItem.get`. None = definition not projected; DIM
        /// then treats the lock as having no bucket and no name.
        definition: Option<ExoticDefinition>,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExoticDefinition {
    pub bucket: i64,
    pub name: String,
}
