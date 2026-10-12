//! DIM process/mappers.ts:61-190 (mapDimItemToProcessItems, tuning variants),
//! loadout-utils.ts:356-405 (assumed masterwork stats) and
//! armor-upgrade-utils.ts:12-50 (assumed energy, artifice, masterwork).
use super::*;
use crate::InvestmentStat;
use crate::armor_model::{ARMOR_STATS, tuning_mod_stat};
use crate::build_capture::empty_plug;
use crate::mod_assignment::{AssignmentMod, AssignmentSocket};

/// Armor 3.0 masterwork socket (isArmor3) or legacy armor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Masterwork {
    #[default]
    Legacy,
    /// `plug_stats`: the plugged masterwork plug's stats (zeros if none plugged).
    Armor3 { plug_stats: [i32; 6] },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuningPlug {
    pub hash: u32,
    pub enabled: bool,
    /// The plug definition's investment stats. None = not found in the socket's
    /// plug set or not pluggable (DIM skips it).
    pub investment_stats: Option<Vec<InvestmentStat>>,
}

/// The DimItem facts the optimizer reads. Ghostlink-side assembly is BE-008d.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ArmorCandidate {
    pub id: String,
    pub hash: u32,
    pub name: String,
    pub bucket: i64,
    /// DestinyClass; 3 (Classified) is ignored by the strictly-worse pass.
    pub class_type: u8,
    pub is_exotic: bool,
    /// isArtifice(item): an actual, visible artifice socket (not the assumption).
    pub is_artifice: bool,
    pub masterwork: Masterwork,
    /// maxEnergyCapacity(item): tier 5 armor has 11, otherwise 10.
    pub tier5: bool,
    /// None = DIM's `!item.energy`, which assumes capacity 0.
    pub energy_capacity: Option<u32>,
    pub power: Option<u16>,
    /// Base stats in `ARMOR_STATS` order (missing DIM stats are not representable).
    pub base_stats: [i32; 6],
    /// getSpecialtySocketMetadata(item).slotTag as the activity category hash.
    pub activity_slot: Option<u32>,
    pub set_bonus: Option<u32>,
    pub has_set_bonus_mod_socket: bool,
    /// Plugged intrinsic perks: armor perk socket plus extra intrinsic sockets.
    pub intrinsic_perks: Vec<u32>,
    /// Extra intrinsic sockets only (exotic class item perks).
    pub extra_intrinsic_perks: Vec<u32>,
    /// None = no tuning socket; Some = its reusable plugs, in socket order.
    pub tuning: Option<Vec<TuningPlug>>,
    /// Bucket-mod sockets for assignBucketSpecificMods. None = not projected.
    pub bucket_sockets: Option<Vec<AssignmentSocket>>,
}

impl ArmorCandidate {
    /// DIM item-utils.ts:409-430 getArmor3TuningStat: the single stat all tuners
    /// agree on; mixed directions (exotics) or none give None.
    pub fn tuned_stat(&self) -> Option<u32> {
        let mut tuned = None;
        for plug in self.tuning.as_deref().unwrap_or(&[]) {
            let Some(stat) = tuning_mod_stat(plug.hash) else {
                continue;
            };
            if tuned.is_some_and(|previous| previous != stat) {
                return None;
            }
            tuned = Some(stat);
        }
        tuned
    }
}

/// DIM ProcessItem (process-worker/types.ts:17-60).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessItem {
    pub id: String,
    pub hash: u32,
    pub name: String,
    pub is_exotic: bool,
    pub is_artifice: bool,
    pub remaining_energy_capacity: i32,
    pub power: u16,
    pub stats: [i32; 6],
    pub compatible_activity_mod: Option<u32>,
    pub set_bonus: Option<u32>,
    pub intrinsic_perks: Option<Vec<u32>>,
    pub has_set_bonus_mod_socket: bool,
    pub included_tuning_mod: Option<u32>,
    pub tuning_variants: Option<Vec<TuningVariant>>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TuningVariant {
    pub mod_hash: u32,
    pub stats: [i32; 6],
}

pub fn is_assumed_masterworked(item: &ArmorCandidate, rules: &EnergyRules) -> bool {
    use AssumeArmorMasterwork::*;
    matches!(rules.assume_armor_masterwork, All | ArtificeExotic)
        || (rules.assume_armor_masterwork == Legendary && !item.is_exotic)
}

pub fn assumed_item_energy(item: &ArmorCandidate, rules: &EnergyRules) -> i32 {
    let Some(capacity) = item.energy_capacity else {
        return 0;
    };
    let assumed = if is_assumed_masterworked(item, rules) {
        if item.tier5 { 11 } else { 10 }
    } else {
        rules.min_item_energy
    };
    (capacity as i32).max(assumed)
}

pub fn is_assumed_artifice(item: &ArmorCandidate, rules: &EnergyRules) -> bool {
    (item.is_exotic
        && item.energy_capacity.is_some()
        && rules.assume_armor_masterwork == AssumeArmorMasterwork::ArtificeExotic
        && item.masterwork == Masterwork::Legacy)
        || item.is_artifice
}

pub fn assumed_masterwork_stats(item: &ArmorCandidate, rules: &EnergyRules) -> [i32; 6] {
    let capacity = assumed_item_energy(item, rules);
    let assumed = is_assumed_masterworked(item, rules);
    let mut stats = item.base_stats;
    for (i, value) in stats.iter_mut().enumerate() {
        match item.masterwork {
            // Legacy bonus uses the 10 threshold even for tier 5 armor.
            Masterwork::Legacy if capacity >= 10 => *value += 2,
            Masterwork::Armor3 { .. } if assumed && *value == 0 => *value = 5,
            Masterwork::Armor3 { plug_stats } if !assumed && *value == 0 => *value += plug_stats[i],
            _ => {}
        }
    }
    stats
}

/// One item, or one per tuning mod (non-exotics) / one with variants (exotics).
pub fn map_armor_to_process_items(
    item: &ArmorCandidate,
    rules: &EnergyRules,
    bucket_mods: &[AssignmentMod],
    ranges: &[DesiredStatRange],
    auto_stat_mods: bool,
) -> Vec<ProcessItem> {
    let stats = assumed_masterwork_stats(item, rules);
    let mods_cost: i32 = bucket_mods.iter().map(|m| m.cost).sum();
    let process = ProcessItem {
        id: item.id.clone(),
        hash: item.hash,
        name: item.name.clone(),
        is_exotic: item.is_exotic,
        is_artifice: is_assumed_artifice(item, rules),
        remaining_energy_capacity: assumed_item_energy(item, rules) - mods_cost,
        power: item.power.unwrap_or(0),
        stats,
        compatible_activity_mod: item.activity_slot,
        set_bonus: item.set_bonus,
        intrinsic_perks: (!item.intrinsic_perks.is_empty()).then(|| item.intrinsic_perks.clone()),
        has_set_bonus_mod_socket: item.has_set_bonus_mod_socket,
        included_tuning_mod: None,
        tuning_variants: None,
    };
    let Some(plugs) = item
        .tuning
        .as_deref()
        .filter(|p| auto_stat_mods && !p.is_empty())
    else {
        return vec![process];
    };
    let variants = tuning_variants(item, &stats, plugs, ranges);
    if item.is_exotic {
        // Exotics keep their variants on one item; the search picks per set.
        return vec![if variants.is_empty() {
            process
        } else {
            ProcessItem {
                tuning_variants: Some(variants),
                ..process
            }
        }];
    }
    variants
        .into_iter()
        .map(|v| ProcessItem {
            included_tuning_mod: Some(v.mod_hash),
            stats: v.stats,
            ..process.clone()
        })
        .collect()
}

/// DIM stats-conditional.ts:190-210 merges duplicate stats, keeping the first flag;
/// :62-78 makes Balanced Tuning's conditional stats apply to base-zero stats only.
fn active_plug_stats(
    item: &ArmorCandidate,
    plug: &TuningPlug,
    definition: &[InvestmentStat],
) -> Vec<(usize, i32)> {
    let mut merged: Vec<(u32, i32, bool)> = Vec::new();
    for s in definition {
        match merged.iter_mut().find(|m| m.0 == s.stat_type_hash) {
            Some(existing) => existing.1 += s.value,
            None => merged.push((s.stat_type_hash, s.value, s.is_conditionally_active)),
        }
    }
    merged
        .into_iter()
        .filter_map(|(hash, value, conditional)| {
            let index = ARMOR_STATS.iter().position(|h| *h == hash)?;
            let active =
                !conditional || plug.hash != BALANCED_TUNING_PLUG || item.base_stats[index] == 0;
            active.then_some((index, value))
        })
        .collect()
}

/// DIM mappers.ts:132-190.
fn tuning_variants(
    item: &ArmorCandidate,
    stats: &[i32; 6],
    plugs: &[TuningPlug],
    ranges: &[DesiredStatRange],
) -> Vec<TuningVariant> {
    // Sacrifice the last ignored stat, else the last of the lowest maximums.
    let default_dump = ranges
        .iter()
        .rev()
        .find(|r| r.max_stat == 0)
        .or_else(|| {
            ranges
                .iter()
                .enumerate()
                .min_by_key(|(i, r)| i64::from(r.max_stat) * 1000 - *i as i64)
                .map(|(_, r)| r)
        })
        .map(|r| r.stat_hash);
    let mut variants = Vec::new();
    for plug in plugs {
        let Some(definition) = plug.investment_stats.as_deref().filter(|d| !d.is_empty()) else {
            continue;
        };
        if !plug.enabled || empty_plug(i64::from(plug.hash)) {
            continue;
        }
        let mut tuned = *stats;
        let mut dump = None;
        for (index, value) in active_plug_stats(item, plug, definition) {
            tuned[index] = (tuned[index] + value).min(MAX_STAT);
            if value < 0 {
                dump = Some(ARMOR_STATS[index]);
            }
        }
        let desired_max = dump
            .and_then(|hash| ranges.iter().find(|r| r.stat_hash == hash))
            .map_or(0, |r| r.max_stat);
        if dump.is_none() || default_dump == dump || (desired_max > 0 && desired_max <= 175) {
            variants.push(TuningVariant {
                mod_hash: plug.hash,
                stats: tuned,
            });
        }
    }
    variants
}
