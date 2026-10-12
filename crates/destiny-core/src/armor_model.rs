//! DIM armor derivations. Missing persisted inputs stay unknown, never API totals.
use crate::armor_catalog::{ScaledStat, StatGroupDefinition};
use crate::{ArmorDefinition, ItemEnergy};

// DIM data/d2/generated-enums.ts:36,171,614,1422-1467.
pub const ARMOR_STATS: [u32; 6] = [
    2996146975, 392767087, 1943323491, 1735777505, 144602215, 4244567218,
];
pub const ARMOR_STATS_PLUG: u32 = 748854354;
pub const INTRINSIC_PLUG: u32 = 1744546145;
pub const ARMOR3_MASTERWORK_PLUG: u32 = 2198080209;

pub struct ModelPlug<'a> {
    pub hash: u32,
    pub enabled: bool,
    pub definition: Option<&'a ArmorDefinition>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmorStat {
    pub stat_hash: u32,
    pub base: i32,
    pub current: i32,
    pub base_masterworked: Option<i32>,
    pub maximum_value: Option<i32>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmorStats {
    pub stats: Vec<ArmorStat>,
    pub total_base: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmorModel {
    pub stats: Option<ArmorStats>,
    pub masterwork_tier: Option<i32>,
    /// Raw unknown is preserved alongside DIM's consumer fallback.
    pub gear_tier: Option<u32>,
    pub effective_gear_tier: u32,
    pub energy: Option<ItemEnergy>,
}

/// DIM inventory/store/stats.ts:614-651. Armor deliberately bypasses interpolation/clamping.
pub fn interpolate_stat(value: i32, display: &ScaledStat) -> Option<i32> {
    if ARMOR_STATS.contains(&display.stat_hash) {
        return Some(value);
    }
    let points = &display.display_interpolation;
    if points.is_empty() {
        return None;
    }
    let value = value.min(display.maximum_value);
    let end = points
        .iter()
        .position(|point| point.value > value)
        .unwrap_or(points.len() - 1);
    let start = end.saturating_sub(1);
    let a = &points[start];
    let b = &points[end];
    let range = f64::from(b.value) - f64::from(a.value);
    if range == 0.0 {
        return Some(a.weight);
    }
    let t = (f64::from(value) - f64::from(a.value)) / range;
    let result = f64::from(a.weight) + t * (f64::from(b.weight) - f64::from(a.weight));
    // Math.round is floor(x + .5), including negative ties. Other stats use ties-to-even.
    Some(if display.stat_hash == 3871231066 {
        (result + 0.5).floor()
    } else {
        result.round_ties_even()
    } as i32)
}

/// DIM stats.ts:134-169,224-264,318-390,405-414; stats-custom.ts:52-72.
/// `None` sockets means a legacy/unobserved projection, not an observed empty list.
pub fn build_armor_stats(
    item: Option<&ArmorDefinition>,
    group: Option<&StatGroupDefinition>,
    plugs: Option<&[ModelPlug<'_>]>,
    has_energy: bool,
) -> Option<ArmorStats> {
    let (item, group, plugs) = (item?, group?, plugs?);
    if plugs
        .iter()
        .any(|plug| plug.enabled && plug.definition.is_none())
    {
        return None;
    }
    let armor3 = plugs.iter().any(|plug| {
        plug.definition
            .is_some_and(|definition| definition.plug_category_hash == Some(ARMOR3_MASTERWORK_PLUG))
    });
    let mut stats = Vec::with_capacity(6);
    for hash in ARMOR_STATS {
        let display = group
            .scaled_stats
            .iter()
            .find(|display| display.stat_hash == hash)?;
        let mut investment = item
            .investment_stats
            .iter()
            .find(|stat| stat.stat_type_hash == hash)
            .map_or(0, |stat| stat.value);
        let mut base = interpolate_stat(investment, display)?;
        // Intrinsics must all run before non-base sockets, regardless of socket order.
        for affects_base in [true, false] {
            for plug in plugs.iter().filter(|plug| plug.enabled) {
                let definition = plug.definition?;
                let intrinsic = matches!(
                    definition.plug_category_hash,
                    Some(INTRINSIC_PLUG | ARMOR_STATS_PLUG)
                );
                if intrinsic != affects_base {
                    continue;
                }
                // DIM stats-conditional.ts:190-210 merges duplicate stats before choosing
                // the activation rule, preserving the first entry's conditional flag.
                let matching: Vec<_> = definition
                    .investment_stats
                    .iter()
                    .filter(|stat| stat.stat_type_hash == hash)
                    .collect();
                let Some(first) = matching.first() else {
                    continue;
                };
                // DIM stats-conditional.ts:70-78,155-160: MW and balanced tuning
                // only affect base-zero stats. Unrecognized conditionals remain active.
                if first.is_conditionally_active
                    && (definition.plug_category_hash == Some(ARMOR3_MASTERWORK_PLUG)
                        || plug.hash == 3122197216)
                    && base != 0
                {
                    continue;
                }
                for stat in matching {
                    investment = investment.checked_add(stat.value)?;
                }
                if affects_base {
                    base = interpolate_stat(investment, display)?;
                }
            }
        }
        // DIM stats.ts:282-291: display max (including endpoint weights), with group fallback.
        let maximum_value = match (
            display.display_interpolation.first(),
            display.display_interpolation.last(),
        ) {
            (Some(first), Some(last)) => {
                Some(display.maximum_value.max(first.weight).max(last.weight))
            }
            _ => group.maximum_value,
        };
        stats.push(ArmorStat {
            stat_hash: hash,
            base,
            current: interpolate_stat(investment, display)?,
            maximum_value,
            base_masterworked: if has_energy && plugs.iter().all(|plug| plug.definition.is_some()) {
                Some(if armor3 {
                    if base == 0 { 5 } else { base }
                } else {
                    base.checked_add(2)?
                })
            } else {
                None
            },
        });
    }
    let total_base = stats
        .iter()
        .try_fold(0i32, |sum, stat| sum.checked_add(stat.base))?;
    Some(ArmorStats { stats, total_base })
}

/// DIM masterwork.ts:43-104. Energy capacity is not the Armor 3.0 masterwork tier.
pub fn armor3_masterwork_tier(
    plugs: Option<&[ModelPlug<'_>]>,
    stats: Option<&ArmorStats>,
) -> Option<i32> {
    let plugs = plugs?;
    // An unresolved plug could be the first masterwork socket: fail closed.
    for plug in plugs {
        let definition = plug.definition?;
        if definition.plug_category_hash == Some(ARMOR3_MASTERWORK_PLUG) {
            // DIM attaches plug stats even when disabled. Conditional MW stats only
            // project onto known base-zero stats; unconditional stats (including
            // zero-valued energy) still make the projected map nonempty.
            let has_projected_stats = definition.investment_stats.iter().any(|stat| {
                let first = definition
                    .investment_stats
                    .iter()
                    .find(|candidate| candidate.stat_type_hash == stat.stat_type_hash)
                    .expect("stat came from this definition");
                !first.is_conditionally_active
                    || stats.is_some_and(|stats| {
                        stats.stats.iter().any(|existing| {
                            existing.stat_hash == stat.stat_type_hash && existing.base == 0
                        })
                    })
            });
            if !has_projected_stats {
                return None;
            }
            return Some(
                definition
                    .investment_stats
                    .iter()
                    .find(|stat| stat.is_conditionally_active)
                    .map_or(0, |stat| stat.value),
            );
        }
    }
    None
}
