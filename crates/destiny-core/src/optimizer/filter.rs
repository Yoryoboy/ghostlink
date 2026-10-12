//! DIM loadout-builder/item-filter.ts:67-292, in DIM's order: unassigned mods ->
//! empty; pinned / locked exotic / no exotic; set-bonus >= 4 with a locked exotic;
//! excluded; bucket-specific mod fit; search; strictly-worse dedupe.
use super::*;
use crate::mod_assignment::{ARMOR_BUCKETS, AssignmentMod, assign_bucket_specific_mods};
use std::collections::BTreeMap;

/// DIM inventory/store/exotic-class-item.ts:6-36 keys (the perk lists come from
/// the candidates' own sockets).
const EXOTIC_CLASS_ITEMS_WITH_PERKS: [u32; 3] = [266021826, 2273643087, 2809120022];

pub type ItemsByBucket = BTreeMap<i64, Vec<ArmorCandidate>>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BucketFilterInfo {
    pub total_considered: usize,
    pub cant_fit_mods: usize,
    pub final_valid: usize,
    pub removed_strictly_worse: usize,
    pub removed_by_search_filter: usize,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilterInfo {
    pub search_query_effective: bool,
    pub exotic_does_not_exist: bool,
    pub per_bucket: BTreeMap<i64, BucketFilterInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FilterError {
    /// Locked bucket mods exist but the item's mod sockets are not projected.
    BucketSocketsUnavailable { item_id: String },
}

#[derive(Default)]
pub struct FilterParams<'a> {
    pub items: &'a [ArmorCandidate],
    pub pinned: BTreeMap<i64, ArmorCandidate>,
    pub excluded: BTreeMap<i64, Vec<String>>,
    pub bucket_mods: BTreeMap<i64, Vec<AssignmentMod>>,
    /// DIM `unassignedMods.length > 0`.
    pub has_unassigned_mods: bool,
    /// Activity slot tags required by locked activity mods.
    pub activity_mod_slots: Vec<u32>,
    pub locked_exotic: Option<LockedExotic>,
    pub energy_rules: EnergyRules,
    pub set_bonuses: BTreeMap<u32, u32>,
    pub perks: Vec<u32>,
}

pub fn filter_items(
    p: &FilterParams,
    search: &dyn Fn(&ArmorCandidate) -> bool,
) -> Result<(ItemsByBucket, FilterInfo), FilterError> {
    let mut filtered: ItemsByBucket = ARMOR_BUCKETS.iter().map(|b| (*b, Vec::new())).collect();
    let mut info = FilterInfo {
        per_bucket: ARMOR_BUCKETS
            .iter()
            .map(|b| (*b, Default::default()))
            .collect(),
        ..Default::default()
    };
    if p.items.is_empty() || p.has_unassigned_mods {
        return Ok((filtered, info));
    }
    // Any exotic requested but none matches the search: exotics skip the search.
    let exclude_exotics_from_filter = p.locked_exotic == Some(LockedExotic::Any)
        && !p.items.iter().any(|i| i.is_exotic && search(i));
    let (locked_hash, locked_def) = match &p.locked_exotic {
        Some(LockedExotic::Item { hash, definition }) => (Some(*hash), definition.as_ref()),
        _ => (None, None),
    };
    let mut required_slots = p.activity_mod_slots.clone();
    required_slots.sort();
    required_slots.dedup();
    // 2 + 2 + 1 = 5: with a locked exotic and 4+ set pieces, legendaries must be in those sets.
    let include_only_sets: Option<Vec<u32>> = (p.set_bonuses.values().sum::<u32>() >= 4
        && locked_def.is_some())
    .then(|| p.set_bonuses.keys().copied().collect());

    for bucket in ARMOR_BUCKETS {
        let locked_mods = p.bucket_mods.get(&bucket).map_or(&[][..], Vec::as_slice);
        let in_bucket = || p.items.iter().filter(|i| i.bucket == bucket);
        let locked_applicable = locked_def.is_some_and(|d| d.bucket == bucket);
        let exotics: Option<Vec<&ArmorCandidate>> = locked_applicable.then(|| {
            let (hash, def) = (locked_hash.unwrap(), locked_def.unwrap());
            in_bucket()
                .filter(|i| i.hash == hash || i.name == def.name)
                .collect()
        });
        if exotics.as_ref().is_some_and(Vec::is_empty) {
            info.exotic_does_not_exist = true;
        }
        let mut first_pass: Vec<&ArmorCandidate> = if let Some(pinned) = p.pinned.get(&bucket) {
            vec![pinned]
        } else if let Some(exotics) = exotics {
            let class_perks =
                locked_hash.is_some_and(|h| EXOTIC_CLASS_ITEMS_WITH_PERKS.contains(&h));
            if class_perks && !p.perks.is_empty() {
                // Only perks that belong to these class items count.
                let relevant: Vec<u32> = p
                    .perks
                    .iter()
                    .copied()
                    .filter(|perk| {
                        exotics
                            .iter()
                            .any(|i| i.extra_intrinsic_perks.contains(perk))
                    })
                    .collect();
                exotics
                    .into_iter()
                    .filter(|i| relevant.iter().all(|r| i.extra_intrinsic_perks.contains(r)))
                    .collect()
            } else {
                exotics
            }
        } else if p.locked_exotic == Some(LockedExotic::None) || locked_def.is_some() {
            in_bucket().filter(|i| !i.is_exotic).collect()
        } else {
            in_bucket().collect()
        };
        let total_considered = first_pass.len();
        if let Some(sets) = include_only_sets.as_ref().filter(|_| !locked_applicable) {
            first_pass.retain(|i| {
                i.set_bonus.is_some_and(|s| sets.contains(&s)) || i.has_set_bonus_mod_socket
            });
        }
        let excluded = p.excluded.get(&bucket);
        first_pass.retain(|i| !excluded.is_some_and(|ids| ids.contains(&i.id)));
        let without_excluded = first_pass.len();
        let mut fits = Vec::new();
        for item in first_pass {
            let fit = if locked_mods.is_empty() {
                true
            } else {
                let sockets = item.bucket_sockets.as_deref().ok_or_else(|| {
                    FilterError::BucketSocketsUnavailable {
                        item_id: item.id.clone(),
                    }
                })?;
                let capacity = assumed_item_energy(item, &p.energy_rules);
                assign_bucket_specific_mods(sockets, locked_mods, capacity)
                    .unassigned
                    .is_empty()
            };
            if fit {
                fits.push(item);
            }
        }
        let searched: Vec<&ArmorCandidate> = fits
            .iter()
            .copied()
            .filter(|i| (i.is_exotic && exclude_exotics_from_filter) || search(i))
            .collect();
        // An empty result, or exotic-only matches where legendaries exist, ignores
        // the search so buckets are not stuck exotic-only (double exotic).
        let usable = !searched.is_empty()
            && (searched.iter().any(|i| !i.is_exotic) || !fits.iter().any(|i| !i.is_exotic));
        let mut finals = if usable { searched } else { fits.clone() };
        let removed_by_search = if usable { fits.len() - finals.len() } else { 0 };
        info.search_query_effective |= removed_by_search > 0;

        let mods_cost: i32 = locked_mods.iter().map(|m| m.cost).sum();
        let mut removed_strictly_worse = 0;
        if finals.len() > 1 {
            let candidates: Vec<ArmorCandidate> = finals.iter().map(|i| (*i).clone()).collect();
            let stats = |item: &ArmorCandidate| -> Vec<(i64, i32)> {
                let mut stats: Vec<(i64, i32)> = ARMOR_STATS
                    .iter()
                    .zip(assumed_masterwork_stats(item, &p.energy_rules))
                    .map(|(h, v)| (i64::from(*h), v))
                    .collect();
                stats.push((-2, assumed_item_energy(item, &p.energy_rules) - mods_cost));
                stats.extend(
                    required_slots
                        .iter()
                        .map(|tag| (-3, i32::from(item.activity_slot == Some(*tag)))),
                );
                stats.extend(p.set_bonuses.keys().map(|set| {
                    (
                        -10 - i64::from(*set),
                        i32::from(item.set_bonus == Some(*set)),
                    )
                }));
                stats
            };
            let worse = compute_stat_dupe_lower(&candidates, &stats);
            finals.retain(|i| !worse.contains(&i.id));
            removed_strictly_worse = worse.len();
        }
        info.per_bucket.insert(
            bucket,
            BucketFilterInfo {
                total_considered,
                cant_fit_mods: without_excluded - fits.len(),
                final_valid: finals.len(),
                removed_strictly_worse,
                removed_by_search_filter: removed_by_search,
            },
        );
        filtered.insert(bucket, finals.into_iter().cloned().collect());
    }
    Ok((filtered, info))
}
