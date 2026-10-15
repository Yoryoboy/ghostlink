//! DIM 29c122e loadout/stats.ts:101-214 and loadout-utils.ts:284-413.
use crate::{armor_model::*, build::*, mod_assignment::ModFitAnalysis, *};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildStats {
    pub evaluation: &'static str,
    pub reason: Option<&'static str>,
    pub as_is: Option<[i32; 6]>,
    pub assumed: Option<[i32; 6]>,
    pub satisfies_constraints: Option<bool>,
    pub needs_upgrades_for_stats: Option<bool>,
}
impl Default for BuildStats {
    fn default() -> Self {
        Self::unknown("dim_loadout_stat_aggregation_not_implemented")
    }
}
impl BuildStats {
    pub fn unknown(reason: &'static str) -> Self {
        Self {
            evaluation: "not_evaluated",
            reason: Some(reason),
            as_is: None,
            assumed: None,
            satisfies_constraints: None,
            needs_upgrades_for_stats: None,
        }
    }
}

pub struct StatArmor<'a> {
    pub item: &'a ItemInstance,
    pub definition: &'a ItemDefinition,
    pub stats: &'a ArmorStats,
    /// None is legacy armor; Some is the Armor 3 masterwork socket's
    /// projected contribution, including disabled inserted plugs as in DIM.
    pub masterwork: Option<[i32; 6]>,
}

/// resolveStatConstraints keys by hash (last wins), fills ignored stats, and
/// getStatProblems compares clamped minima only. Unknown hashes are ignored.
pub fn satisfies_minima(values: &[i32; 6], constraints: &[StatConstraint]) -> bool {
    ARMOR_STATS.iter().enumerate().all(|(i, hash)| {
        constraints
            .iter()
            .rev()
            .find(|c| c.stat_hash == *hash)
            .is_none_or(|c| values[i].max(0) >= i32::from(c.min_stat.unwrap_or(0)))
    })
}

/// DIM stats-conditional.ts:52-107,155-174,190-210. Duplicate stats use
/// the first entry's activation flag. Assigned base-zero, not current totals,
/// activates Balanced Tuning; unassigned Balanced Tuning contributes nothing.
pub fn add_plug_stats(
    totals: &mut [i32; 6],
    hash: u32,
    plug: &ArmorDefinition,
    class: u8,
    base: Option<&[i32; 6]>,
) -> Result<(), &'static str> {
    for (i, stat_hash) in ARMOR_STATS.iter().enumerate() {
        let matching: Vec<_> = plug
            .investment_stats
            .iter()
            .filter(|s| s.stat_type_hash == *stat_hash)
            .collect();
        let Some(first) = matching.first() else {
            continue;
        };
        if first.is_conditionally_active {
            if (hash == 3122197216 || plug.plug_category_hash == Some(ARMOR3_MASTERWORK_PLUG))
                && base.is_none_or(|b| b[i] != 0)
            {
                continue;
            }
            if [3511092054, 711234314].contains(&hash) {
                continue;
            }
            if [2272984671, 1727069360].contains(&hash) {
                let required = match i {
                    0 => Some(1),
                    1 => Some(0),
                    2 => Some(2),
                    _ => None,
                };
                if required.is_some_and(|required| required != class) {
                    continue;
                }
            }
        }
        for stat in matching {
            totals[i] = totals[i]
                .checked_add(stat.value)
                .ok_or("loadout_stat_overflow")?;
        }
    }
    Ok(())
}

/// DIM analysis.ts:158-190; intentionally energy-based, not Armor 3 tier-based.
pub fn inferred_masterwork(
    build: &BuildDefinition,
    armor: &[StatArmor<'_>],
) -> AssumeArmorMasterwork {
    let pairs: Vec<_> = armor.iter().map(|a| (a.item, a.definition)).collect();
    infer_masterwork_assumption(
        build
            .parameters
            .as_ref()
            .and_then(|p| p.assume_armor_masterwork)
            .unwrap_or(AssumeArmorMasterwork::None),
        &pairs,
    )
}

/// The same inference for any resolved armor, given the saved assumption.
pub fn infer_masterwork_assumption(
    mut assumption: AssumeArmorMasterwork,
    armor: &[(&ItemInstance, &ItemDefinition)],
) -> AssumeArmorMasterwork {
    let low = |item: &ItemInstance| item.energy.as_ref().is_some_and(|e| e.energy_capacity < 10);
    let is_exotic = |definition: &ItemDefinition| definition.tier.as_deref() == Some("Exotic");
    let legendary_low = armor.iter().any(|(i, d)| !is_exotic(d) && low(i));
    let exotic_low = armor.iter().any(|(i, d)| is_exotic(d) && low(i));
    if !legendary_low && assumption != AssumeArmorMasterwork::ArtificeExotic {
        assumption = if exotic_low && assumption != AssumeArmorMasterwork::All {
            AssumeArmorMasterwork::Legendary
        } else {
            AssumeArmorMasterwork::All
        };
    }
    assumption
}

/// DIM loadout/stats.ts getFontMods: "Font of ..." mods per stat in `ARMOR_STATS` order.
const FONT_MODS: [u32; 6] = [
    4046357305, 686455429, 1193713026, 1781551382, 1130820873, 633101315,
];

/// The runtime stat bonus of the Font mods among `mods` (cost-pair variants count as the
/// same mod): none, 20, 40, then 50 from three of them. Same pinned table as mod assignment.
pub fn font_bonuses(mods: &[u32]) -> [i32; 6] {
    std::array::from_fn(|i| {
        let count = mods
            .iter()
            .filter(|m| crate::mod_assignment::resolve_mod_cost_variant(**m, &[]) == FONT_MODS[i])
            .count();
        [0, 20, 40, 50][count.min(3)]
    })
}

/// DIM stats.ts:102-214 `getTotalModStatChanges` without item assignments: the subclass
/// plugs, then the locked mods (no item, so item-conditional stats stay off), then Fonts.
pub fn locked_mod_stat_totals(
    definitions: &[ItemDefinition],
    subclass_plugs: &[u32],
    mods: &[&ItemDefinition],
    class: u8,
    runtime: bool,
) -> Result<[i32; 6], &'static str> {
    let mut totals = [0_i32; 6];
    for hash in subclass_plugs {
        let plug = definitions
            .iter()
            .find(|d| d.hash.value() == i64::from(*hash) && d.armor.plug_category_hash.is_some())
            .ok_or("stat_plug_definition_unavailable")?;
        add_plug_stats(&mut totals, *hash, &plug.armor, class, None)?;
    }
    for plug in mods {
        add_plug_stats(
            &mut totals,
            plug.hash.value() as u32,
            &plug.armor,
            class,
            None,
        )?;
    }
    if runtime {
        let hashes: Vec<u32> = mods.iter().map(|m| m.hash.value() as u32).collect();
        for (total, bonus) in totals.iter_mut().zip(font_bonuses(&hashes)) {
            *total = total.checked_add(bonus).ok_or("loadout_stat_overflow")?;
        }
    }
    Ok(totals)
}

pub fn total_stats(
    armor: &[StatArmor<'_>],
    fit: &ModFitAnalysis,
    definitions: &[ItemDefinition],
    subclass_plugs: &[u32],
    class: u8,
    runtime: bool,
    assumption: Option<AssumeArmorMasterwork>,
) -> Result<[i32; 6], &'static str> {
    let assigned = fit
        .item_mod_assignments
        .as_ref()
        .ok_or(fit.reason.unwrap_or("stat_mod_assignments_unavailable"))?;
    let unassigned = fit
        .unassigned_mods
        .as_ref()
        .ok_or("stat_mod_assignments_unavailable")?;
    let mut totals = [0_i32; 6];
    for a in armor {
        let energy = a
            .item
            .energy
            .as_ref()
            .ok_or("armor_energy_unavailable")?
            .energy_capacity;
        let assumed = assumption.is_some_and(|s| {
            matches!(
                s,
                AssumeArmorMasterwork::All | AssumeArmorMasterwork::ArtificeExotic
            ) || s == AssumeArmorMasterwork::Legendary
                && a.definition.tier.as_deref() != Some("Exotic")
        });
        let capacity = energy.max(if assumed {
            if a.item.gear_tier == Some(5) { 11 } else { 10 }
        } else if assumption.is_some() {
            9
        } else {
            1
        });
        for (i, hash) in ARMOR_STATS.iter().enumerate() {
            let stat = a
                .stats
                .stats
                .iter()
                .find(|s| s.stat_hash == *hash)
                .ok_or("armor_base_stats_unavailable")?;
            let bonus = match a.masterwork {
                None if capacity >= 10 => 2,
                Some(_) if assumed && stat.base == 0 => 5,
                Some(mw) if !assumed && stat.base == 0 => mw[i],
                _ => 0,
            };
            totals[i] = totals[i]
                .checked_add(stat.base)
                .and_then(|v| v.checked_add(bonus))
                .ok_or("loadout_stat_overflow")?;
        }
    }
    let definition = |hash: u32| {
        definitions
            .iter()
            .find(|d| d.hash.value() == i64::from(hash))
            .filter(|d| d.armor.plug_category_hash.is_some())
            .ok_or("stat_plug_definition_unavailable")
    };
    for hash in subclass_plugs {
        add_plug_stats(&mut totals, *hash, &definition(*hash)?.armor, class, None)?;
    }
    let mut mods = Vec::new();
    for (id, hashes) in assigned {
        let Some(a) = armor.iter().find(|a| a.item.instance_id == *id) else {
            continue;
        };
        let mut base = [0; 6];
        for (i, hash) in ARMOR_STATS.iter().enumerate() {
            base[i] = a
                .stats
                .stats
                .iter()
                .find(|s| s.stat_hash == *hash)
                .ok_or("armor_base_stats_unavailable")?
                .base;
        }
        for hash in hashes {
            add_plug_stats(
                &mut totals,
                *hash,
                &definition(*hash)?.armor,
                class,
                Some(&base),
            )?;
            mods.push(*hash);
        }
    }
    for hash in unassigned {
        add_plug_stats(&mut totals, *hash, &definition(*hash)?.armor, class, None)?;
        mods.push(*hash);
    }
    if runtime {
        for (total, bonus) in totals.iter_mut().zip(font_bonuses(&mods)) {
            *total = total.checked_add(bonus).ok_or("loadout_stat_overflow")?;
        }
    }
    Ok(totals)
}
