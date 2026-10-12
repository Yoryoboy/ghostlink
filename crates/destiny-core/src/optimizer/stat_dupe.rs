//! DIM utils/stats-set.ts and utils/stats.ts:15-80 computeStatDupeLower.
use super::ArmorCandidate;
use crate::armor_model::ARMOR_STATS;
use std::collections::{BTreeMap, BTreeSet};

/// "Are there stats strictly better than these?" over equal-length stat mixes. DIM
/// stores a trie; this is the same relation (every stat >= and not identical).
#[derive(Clone, Debug, Default)]
pub struct StatsSet(Vec<Vec<i32>>);

impl StatsSet {
    pub fn insert(&mut self, stats: Vec<i32>) {
        self.0.push(stats);
    }
    pub fn do_better_stats_exist(&self, stats: &[i32]) -> bool {
        self.0.iter().any(|known| {
            known.len() == stats.len()
                && known != stats
                && known.iter().zip(stats).all(|(k, s)| k >= s)
        })
    }
}

/// Ids of armor for which another item has strictly better stat mixes in every
/// configuration. `get_stats` may return extra dummy entries (they are compared as
/// stat mixes but never adjusted by tuning/artifice), like DIM's LO callback.
pub fn compute_stat_dupe_lower(
    items: &[ArmorCandidate],
    get_stats: &dyn Fn(&ArmorCandidate) -> Vec<(i64, i32)>,
) -> BTreeSet<String> {
    let relevant = ARMOR_STATS.map(i64::from);
    let mut mixes: Vec<Option<Vec<Vec<i32>>>> = Vec::with_capacity(items.len());
    for item in items {
        // Classified (class 3) armor is disregarded; so is armor without stats/power.
        if item.class_type == 3 || item.power.is_none_or(|p| p == 0) {
            mixes.push(None);
            continue;
        }
        let stats = get_stats(item);
        let adjusted = |tuned: Option<(i64, i64)>, artifice: Option<i64>, balanced: &[i64]| {
            stats
                .iter()
                .map(|(hash, value)| {
                    value
                        + match (tuned, artifice) {
                            (Some((up, _)), _) if *hash == up => 5,
                            (Some((_, down)), _) if *hash == down => -5,
                            (_, Some(boost)) if *hash == boost => 3,
                            _ if balanced.contains(hash) => 1,
                            _ => 0,
                        }
                })
                .collect::<Vec<_>>()
        };
        let mut item_mixes = vec![stats.iter().map(|(_, v)| *v).collect::<Vec<_>>()];
        if let Some(tuned) = item.tuned_stat() {
            let tuned = i64::from(tuned);
            for lowered in relevant.iter().filter(|h| **h != tuned) {
                item_mixes.push(adjusted(Some((tuned, *lowered)), None, &[]));
            }
            // Balanced Tuning raises every entry sitting at 5 (masterworked base 0); DIM
            // matches on value alone, so dummy entries equal to 5 move too.
            let balanced: Vec<i64> = stats
                .iter()
                .filter(|(_, value)| *value == 5)
                .map(|(hash, _)| *hash)
                .collect();
            if relevant.iter().any(|h| balanced.contains(h)) {
                item_mixes.push(adjusted(None, None, &balanced));
            }
        } else if item.is_artifice {
            for boost in relevant {
                item_mixes.push(adjusted(None, Some(boost), &[]));
            }
        }
        mixes.push(Some(item_mixes));
    }
    let group_key = |item: &ArmorCandidate| {
        (
            item.bucket,
            item.class_type,
            item.is_exotic.then_some(item.hash),
        )
    };
    let mut sets: BTreeMap<_, StatsSet> = BTreeMap::new();
    for (item, item_mixes) in items.iter().zip(&mixes) {
        let set = sets.entry(group_key(item)).or_default();
        for mix in item_mixes.iter().flatten() {
            set.insert(mix.clone());
        }
    }
    let mut dupes = BTreeSet::new();
    for (item, own) in items.iter().zip(&mixes) {
        let Some(own) = own else { continue };
        if own
            .iter()
            .all(|mix| sets[&group_key(item)].do_better_stats_exist(mix))
        {
            dupes.insert(item.id.clone());
        }
    }
    dupes
}
