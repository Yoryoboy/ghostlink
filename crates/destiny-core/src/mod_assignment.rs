//! DIM bucket-specific and free mod assignment (commit 29c122e).
//! General/activity candidates use DIM's material and assignment-change ranking.
use crate::{armor_catalog::*, build::*, build_analysis::ResolvedBuildItem, *};

mod costs;
mod independent;
mod ranking;
pub use costs::cumulative_energy_costs;
pub use independent::{
    ARTIFICE_CATEGORY, FreeModAssignment, FreeModItem, SPECIALTY_SOCKETS, TUNING_CATEGORY,
    assign_free_mods, generate_mod_permutations, specialty_category,
};
pub use ranking::{
    MaterialCosts, RANKED_MATERIALS, RankedAssignment, RankingItem, rank_independent_mods,
};

pub const ARMOR_BUCKETS: [i64; 5] = [3448274439, 3551918588, 14239492, 20886954, 1585787867];

/// Shared CLI/MCP evidence. Lists preserve requested duplicates and DIM order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModFitAnalysis {
    pub evaluation: &'static str,
    pub reason: Option<&'static str>,
    pub invalid_mods: Option<Vec<u32>>,
    /// Unassigned after the analyzer's allowed energy upgrades, not as-is.
    pub unassigned_mods: Option<Vec<u32>>,
    pub needs_upgrades_for_mods: Option<bool>,
    pub uses_seasonal_mods: Option<bool>,
    pub item_mod_assignments: Option<Vec<(ItemInstanceId, Vec<u32>)>>,
    pub upgrade_costs: Option<MaterialCosts>,
}
impl Default for ModFitAnalysis {
    fn default() -> Self {
        Self::unknown("dim_independent_mod_assignment_not_implemented")
    }
}
impl ModFitAnalysis {
    fn unknown(reason: &'static str) -> Self {
        Self {
            evaluation: "not_evaluated",
            reason: Some(reason),
            invalid_mods: None,
            unassigned_mods: None,
            needs_upgrades_for_mods: None,
            uses_seasonal_mods: None,
            item_mod_assignments: None,
            upgrade_costs: None,
        }
    }
}

/// DIM mod-utils.ts:15-21 (not rarity, localized names or whitelist matching).
pub fn bucket_for_category(category: u32) -> Option<i64> {
    match category {
        2912171003 => Some(3448274439),
        3422420680 => Some(3551918588),
        1526202480 => Some(14239492),
        2111701510 => Some(20886954),
        912441879 => Some(1585787867),
        _ => None,
    }
}

/// Assemble only active ArmorMods sockets. Missing data never proves nonmembership.
/// DIM socket-utils.ts:225-237; sockets.ts:658-687,792-825.
pub fn assignment_sockets(
    item: &ItemInstance,
    definition: &ItemDefinition,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    assignment_only: bool,
) -> Result<Vec<AssignmentSocket>, &'static str> {
    let category = definition
        .armor
        .socket_categories
        .iter()
        .find(|c| c.socket_category_hash == 590099826)
        .ok_or("armor_mod_socket_categories_unavailable")?;
    let mut result = Vec::new();
    for index in &category.socket_indexes {
        let socket = item
            .sockets
            .iter()
            .find(|s| u32::from(s.socket_index) == *index)
            .ok_or("armor_mod_socket_state_unavailable")?;
        let entry = definition
            .armor
            .socket_entries
            .get(*index as usize)
            .ok_or("armor_mod_socket_definition_unavailable")?;
        if assignment_only && (!socket.visible || entry.socket_type_hash == 1843767421) {
            continue;
        }
        let Some(plug_hash) = socket.plug_hash else {
            continue;
        };
        let plug = definitions
            .iter()
            .find(|d| d.hash == plug_hash)
            .ok_or("inserted_plug_definition_unavailable")?;
        if plug.armor.plug_category_hash.is_none() {
            continue;
        }
        let socket_type = catalogs
            .iter()
            .find_map(|c| match c {
                ArmorCatalogRecord::SocketType(t) if t.hash == entry.socket_type_hash => Some(t),
                _ => None,
            })
            .ok_or("socket_type_unavailable")?;
        if !catalogs.iter().any(|c| {
            matches!(c, ArmorCatalogRecord::SocketCategory(t)
            if t.hash == socket_type.socket_category_hash)
        }) {
            return Err("socket_category_unavailable");
        }
        let set_hash = entry
            .reusable_plug_set_hash
            .or(entry.randomized_plug_set_hash);
        let set = set_hash
            .map(|hash| {
                catalogs
                    .iter()
                    .find_map(|c| match c {
                        ArmorCatalogRecord::PlugSet(s) if s.hash == hash => Some(s),
                        _ => None,
                    })
                    .ok_or("plug_set_unavailable")
            })
            .transpose()?;
        if entry.plug_sources.is_none() {
            return Err("socket_plug_sources_unavailable");
        }
        // DIM's cached set includes all resolved pluggable entries, regardless
        // of currentlyCanRoll. Raw instance membership ignores both flags.
        let mut fits = Vec::new();
        if let Some(set) = set {
            for p in &set.reusable_plug_items {
                let d = definitions
                    .iter()
                    .find(|d| d.hash.value() == i64::from(p.plug_item_hash))
                    .ok_or("plug_set_item_definition_unavailable")?;
                if d.armor.plug_category_hash.is_some() {
                    fits.push(p.plug_item_hash);
                }
            }
        }
        let size = set.map(|_| fits.len());
        if let Some(empty) =
            crate::build_capture::socket_empty_plug(entry, socket_type, set, socket)
        {
            fits.push(empty);
        }
        if let Some(raw) = &socket.reusable_plugs {
            fits.extend(raw.iter().map(|p| p.plug_hash.value() as u32));
        } else {
            // The projected set can prove positive membership, but cannot prove
            // the missing raw list contains no additional matching plugs.
            return Err("raw_reusable_plugs_not_projected");
        }
        result.push(AssignmentSocket {
            index: socket.socket_index,
            plug_set_size: size,
            fits,
        });
    }
    Ok(result)
}

/// Categorize against all owned armor, then fit valid mods against the resolved
/// set with as-is/upgraded rules. Bucket/free choices are DIM-greedy; general and
/// activity permutations rank by unassigned count, materials and insertion changes.
pub fn analyze_bucket_mods(
    build: &BuildDefinition,
    resolved: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
) -> ModFitAnalysis {
    analyze_bucket_mods_with_unlocks(build, resolved, inventory, definitions, catalogs, &[])
}

/// Unlock hashes must be scoped to the target character plus account-wide entries.
/// DIM defaults missing profile plug sets to an empty set (selectors.ts:333-346).
pub fn analyze_bucket_mods_with_unlocks(
    build: &BuildDefinition,
    resolved: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
) -> ModFitAnalysis {
    analyze_with_rules(
        build,
        resolved,
        inventory,
        definitions,
        catalogs,
        unlocked,
        None,
        false,
    )
}

/// DIM getLoadoutStats always fits with the supplied rules, independently of
/// the mod-problem analyzer's as-is-first shortcut. Invalid-for-set mods do not
/// contribute stats (fitMostMods returns them separately from unassignedMods).
pub fn stat_mod_assignments(
    build: &BuildDefinition,
    resolved: &[ResolvedBuildItem],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    upgraded: bool,
) -> ModFitAnalysis {
    let inventory: Vec<_> = resolved
        .iter()
        .filter(|r| r.should_equip)
        .filter_map(|r| r.inventory.clone())
        .collect();
    analyze_with_rules(
        build,
        resolved,
        &inventory,
        definitions,
        catalogs,
        unlocked,
        Some(upgraded),
        false,
    )
}

/// Build apply fits with the in-game rules against whatever armor the apply collected:
/// DIM's `fitMostMods` accepts any number of pieces (`loadout-apply.ts:1282-1330`,
/// `mod-assignment-utils.ts:286-345`); a mod whose bucket has no piece is not assigned.
pub fn apply_mod_assignments(
    build: &BuildDefinition,
    resolved: &[ResolvedBuildItem],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
) -> ModFitAnalysis {
    let inventory: Vec<_> = resolved
        .iter()
        .filter(|r| r.should_equip)
        .filter_map(|r| r.inventory.clone())
        .collect();
    analyze_with_rules(
        build,
        resolved,
        &inventory,
        definitions,
        catalogs,
        unlocked,
        Some(false),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn analyze_with_rules(
    build: &BuildDefinition,
    resolved: &[ResolvedBuildItem],
    inventory: &[ItemInstance],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    unlocked: &[u32],
    stat_rules: Option<bool>,
    partial_set: bool,
) -> ModFitAnalysis {
    let requested = build
        .parameters
        .as_ref()
        .and_then(|p| p.mods.as_deref())
        .unwrap_or(&[]);
    let mut mods = Vec::new();
    for hash in requested {
        let hash = resolve_mod_cost_variant(hash.value() as u32, unlocked);
        // DIM loadout-utils.ts:906-923: non-pluggable/missing definitions become
        // the deprecated placeholder, which is subsequently categorized normally.
        let Some(d) = definitions
            .iter()
            .find(|d| d.hash.value() == i64::from(hash) && d.armor.plug_category_hash.is_some())
            .or_else(|| {
                definitions.iter().find(|d| {
                    d.hash.value() == i64::from(DEPRECATED_MOD_PLACEHOLDER)
                        && d.armor.plug_category_hash.is_some()
                })
            })
        else {
            return ModFitAnalysis::unknown("deprecated_mod_placeholder_definition_unavailable");
        };
        mods.push(d);
    }
    let mut invalid = Vec::new();
    let mut valid = Vec::new();
    for m in mods {
        let mut found = false;
        let mut unknown = None;
        for item in inventory {
            let Some(d) = definitions.iter().find(|d| d.hash == item.item_hash) else {
                unknown = Some("inventory_item_definition_unavailable");
                continue;
            };
            if !d
                .bucket_hash
                .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
            {
                continue;
            }
            match assignment_sockets(item, d, definitions, catalogs, false) {
                Ok(sockets) => {
                    found |= sockets
                        .iter()
                        .any(|s| s.fits.contains(&(m.hash.value() as u32)))
                }
                Err(reason) => unknown = Some(reason),
            }
            if found {
                break;
            }
        }
        if found {
            let category = m.armor.plug_category_hash.expect("resolved pluggable");
            if category == 2487827355
                || SPECIALTY_SOCKETS.iter().any(|(_, c)| *c == category)
                || bucket_for_category(category).is_some()
                || [ARTIFICE_CATEGORY, TUNING_CATEGORY].contains(&category)
            {
                valid.push(m);
            } else {
                // DIM categorizeArmorMods: unknown categories are invalid, even
                // when a socket admits the plug (not guessed independent mods).
                invalid.push(m.hash.value() as u32);
            }
        } else if let Some(reason) = unknown {
            return ModFitAnalysis::unknown(reason);
        } else {
            invalid.push(m.hash.value() as u32);
        }
    }
    let armor: Vec<_> = resolved
        .iter()
        .filter(|r| r.should_equip)
        .filter_map(|r| r.inventory.as_ref())
        .filter_map(|i| {
            definitions
                .iter()
                .find(|d| d.hash == i.item_hash)
                .map(|d| (i, d))
        })
        .filter(|(_, d)| {
            d.bucket_hash
                .is_some_and(|b| ARMOR_BUCKETS.contains(&b.value()))
        })
        .collect();
    // DIM analysis.ts:454 only assigns a full set. Excess free mods are left
    // unassigned by DIM's single-use candidate lists. Apply fits any set.
    if armor.len() != 5 && !partial_set {
        return ModFitAnalysis {
            evaluation: "not_evaluated",
            reason: Some("full_resolved_armor_set_required"),
            invalid_mods: Some(invalid),
            unassigned_mods: None,
            needs_upgrades_for_mods: None,
            uses_seasonal_mods: None,
            item_mod_assignments: None,
            upgrade_costs: None,
        };
    }
    if armor.iter().any(|(i, _)| i.energy.is_none()) {
        return ModFitAnalysis::unknown("armor_energy_unavailable");
    }
    if armor.iter().any(|(_, d)| d.tier.is_none()) {
        return ModFitAnalysis::unknown("armor_rarity_unavailable");
    }
    let mut assumption = build
        .parameters
        .as_ref()
        .and_then(|p| p.assume_armor_masterwork)
        .unwrap_or(AssumeArmorMasterwork::None);
    // DIM analysis.ts:158-190. The threshold is 10 even for Tier 5 armor.
    let exotic_low = armor.iter().any(|(i, d)| {
        d.tier.as_deref() == Some("Exotic")
            && i.energy.as_ref().is_some_and(|e| e.energy_capacity < 10)
    });
    let legendary_low = armor.iter().any(|(i, d)| {
        d.tier.as_deref() != Some("Exotic")
            && i.energy.as_ref().is_some_and(|e| e.energy_capacity < 10)
    });
    if !legendary_low && assumption != AssumeArmorMasterwork::ArtificeExotic {
        assumption = if exotic_low && assumption != AssumeArmorMasterwork::All {
            AssumeArmorMasterwork::Legendary
        } else {
            AssumeArmorMasterwork::All
        };
    }
    let fit = |upgraded: bool, valid: &[&ItemDefinition]| {
        let mut unassigned = Vec::new();
        let mut assigned = vec![vec![]; armor.len()];
        let mut bucket_unassigned = vec![vec![]; armor.len()];
        let mut enhancements = vec![false; armor.len()];
        // Re-categorization in fitMostMods marks valid-in-inventory mods that
        // cannot fit this set as invalid-for-set. Both prevent a successful fit.
        let eager: Vec<_> = armor
            .iter()
            .map(|(i, d)| assignment_sockets(i, d, definitions, catalogs, false))
            .collect::<Result<_, _>>()?;
        let mut eligible = Vec::new();
        for m in valid {
            if eager
                .iter()
                .flatten()
                .any(|s| s.fits.contains(&(m.hash.value() as u32)))
            {
                eligible.push(*m);
            } else {
                unassigned.push(m.hash.value() as u32);
            }
        }
        let hashes_for = |category| {
            eligible
                .iter()
                .filter(|m| m.armor.plug_category_hash == Some(category))
                .map(|m| m.hash.value() as u32)
                .collect::<Vec<_>>()
        };
        let artifice = hashes_for(ARTIFICE_CATEGORY);
        let tuning = hashes_for(TUNING_CATEGORY);
        if !artifice.is_empty() || !tuning.is_empty() {
            let items = independent::free_mod_items(
                &armor,
                definitions,
                upgraded && assumption == AssumeArmorMasterwork::ArtificeExotic,
                !tuning.is_empty(),
            )?;
            let actual = independent::free_mod_items(&armor, definitions, false, false)?;
            let free = assign_free_mods(&items, &artifice, &tuning);
            for (i, mods) in free.assigned.into_iter().enumerate() {
                enhancements[i] = items[i].exotic
                    && !actual[i].artifice
                    && mods.iter().any(|hash| artifice.contains(hash));
                assigned[i].extend(mods);
            }
            unassigned.extend(free.unassigned);
        }
        // Object.entries numeric bucket keys are enumerated ascending in JS.
        let mut buckets = ARMOR_BUCKETS;
        buckets.sort();
        for bucket in buckets {
            let mods: Vec<_> = eligible
                .iter()
                .filter(|m| {
                    m.armor.plug_category_hash.and_then(bucket_for_category) == Some(bucket)
                })
                .map(|m| AssignmentMod {
                    hash: m.hash.value() as u32,
                    cost: m.armor.energy_cost.as_ref().map_or(0, |e| e.energy_cost),
                })
                .collect();
            if mods.is_empty() {
                continue;
            }
            let Some((item, definition)) = armor
                .iter()
                .find(|(_, d)| d.bucket_hash.is_some_and(|b| b.value() == bucket))
            else {
                unassigned.extend(mods.iter().map(|m| m.hash));
                continue;
            };
            let sockets = assignment_sockets(item, definition, definitions, catalogs, true)?;
            let current = item
                .energy
                .as_ref()
                .expect("checked energy")
                .energy_capacity as i32;
            let assumed_mw = matches!(
                assumption,
                AssumeArmorMasterwork::All | AssumeArmorMasterwork::ArtificeExotic
            ) || (assumption == AssumeArmorMasterwork::Legendary
                && definition.tier.as_deref() != Some("Exotic"));
            let capacity = if upgraded {
                current.max(if assumed_mw {
                    if item.gear_tier == Some(5) { 11 } else { 10 }
                } else {
                    9
                })
            } else {
                current.max(1)
            };
            let bucket_fit = assign_bucket_specific_mods(&sockets, &mods, capacity);
            let index = armor
                .iter()
                .position(|(i, _)| i.instance_id == item.instance_id)
                .unwrap();
            assigned[index].splice(0..0, bucket_fit.assigned);
            bucket_unassigned[index] = bucket_fit.unassigned;
        }
        let ranked = ranking::rank_armor(
            &armor,
            definitions,
            catalogs,
            &eligible,
            &assigned,
            &enhancements,
            if upgraded { Some(assumption) } else { None },
        )?;
        for (independent, bucket) in ranked.unassigned_by_item.iter().zip(bucket_unassigned) {
            unassigned.extend(independent);
            unassigned.extend(bucket);
        }
        for (i, independent) in ranked.assigned.into_iter().enumerate() {
            assigned[i].splice(0..0, independent);
        }
        Ok::<_, &'static str>((unassigned, assigned, ranked.upgrade_costs))
    };
    if let Some(upgraded) = stat_rules {
        let (unassigned, assigned, costs) = match fit(upgraded, &valid) {
            Ok(v) => v,
            Err(reason) => return ModFitAnalysis::unknown(reason),
        };
        return ModFitAnalysis {
            evaluation: "evaluated",
            reason: None,
            invalid_mods: Some(invalid),
            unassigned_mods: Some(unassigned),
            needs_upgrades_for_mods: None,
            uses_seasonal_mods: None,
            item_mod_assignments: Some(
                armor
                    .iter()
                    .zip(assigned)
                    .map(|((item, _), mods)| (item.instance_id.clone(), mods))
                    .collect(),
            ),
            upgrade_costs: Some(costs),
        };
    }
    let as_is = match fit(false, &valid) {
        Ok(v) => v,
        Err(r) => return ModFitAnalysis::unknown(r),
    };
    let upgraded = if as_is.0.is_empty() {
        as_is.clone()
    } else {
        match fit(true, &valid) {
            Ok(v) => v,
            Err(r) => return ModFitAnalysis::unknown(r),
        }
    };
    // DIM analysis.ts:456-500: direct seasonal list, then expensive variants
    // with no unlocked plugs. Re-categorization uses this set, not all inventory.
    let direct_seasonal = valid.iter().any(|m| {
        [110793779, 2077016094, 2812559045, 2831374162, 3458160468]
            .contains(&(m.hash.value() as u32))
    });
    let (seasonal, seasonal_reason) = if direct_seasonal || !upgraded.0.is_empty() {
        (Some(direct_seasonal), None)
    } else {
        let expensive = valid
            .iter()
            .map(|m| {
                let hash = resolve_mod_cost_variant(m.hash.value() as u32, &[]);
                definitions
                    .iter()
                    .find(|d| {
                        d.hash.value() == i64::from(hash) && d.armor.plug_category_hash.is_some()
                    })
                    .or_else(|| {
                        definitions.iter().find(|d| {
                            d.hash.value() == i64::from(DEPRECATED_MOD_PLACEHOLDER)
                                && d.armor.plug_category_hash.is_some()
                        })
                    })
            })
            .collect::<Option<Vec<_>>>();
        match expensive {
            Some(mods) => match fit(true, &mods) {
                Ok(v) => (Some(!v.0.is_empty()), None),
                Err(reason) => (None, Some(reason)),
            },
            None => (None, Some("expensive_variant_fit_inputs_unavailable")),
        }
    };
    ModFitAnalysis {
        evaluation: "evaluated",
        reason: seasonal_reason,
        invalid_mods: Some(invalid),
        needs_upgrades_for_mods: Some(!as_is.0.is_empty() && upgraded.0.is_empty()),
        unassigned_mods: Some(upgraded.0),
        uses_seasonal_mods: seasonal,
        item_mod_assignments: Some(
            armor
                .iter()
                .zip(upgraded.1)
                .map(|((item, _), mods)| (item.instance_id.clone(), mods))
                .collect(),
        ),
        upgrade_costs: Some(upgraded.2),
    }
}

pub const DEPRECATED_MOD_PLACEHOLDER: u32 = 444600262;

/// Legacy stat-mod migration, then available-cost resolution, preserving order
/// and duplicates at the call site. DIM loadout-drawer/loadout-utils.ts:880-910
/// and loadout/mod-utils.ts:120-132, commit 29c122e.
pub fn resolve_mod_cost_variant(hash: u32, unlocked: &[u32]) -> u32 {
    let hash = match hash {
        204137529 => 1703647492,
        3961599962 => 4183296050,
        3682186345 => 2532323436,
        2850583378 => 1180408010,
        555005975 => 1237786518,
        2645858828 => 4204488676,
        2623485440 => 4021790309,
        4048838440 => 1435557120,
        1227870362 => 350061697,
        3355995799 => 2724608735,
        3699676109 => 2639422088,
        3253038666 => 4287799666,
        _ => hash,
    };
    // Static DIM src/data/d2/reduced-cost-mod-mappings.ts:1-117, commit 29c122e.
    // Tuple order is (normal, reduced); the reverse lookup is derived in DIM.
    #[rustfmt::skip] // Keep the pinned mapping compact and easy to compare.
    const PAIRS: &[(u32, u32)] = &[
        (14520248,2318667184),(40751621,4149682173),(48578555,3047946307),(84503918,1208761894),
        (193878019,2267311547),(319908131,792400107),(335129856,2801811288),(350061697,2519597513),
        (377010989,1153260021),(450381139,4283953067),(467550918,1017385934),(531057500,830369300),
        (534479613,2436471653),(554409585,644105),(633101315,2771425787),(686455429,3075302157),
        (688956976,56663992),(707237917,657773637),(721001747,1801153435),(802695661,2815817957),
        (837201397,3188328909),(856936828,2136310244),(953234331,3539253011),(967052942,96682422),
        (1024379611,422994787),(1039115606,3791691774),(1079896271,634608391),(1097608874,579997810),
        (1125523126,3046678542),(1130820873,3461249873),(1180408010,2568808786),(1193713026,1672155562),
        (1237786518,1124184622),(1262438062,2325151798),(1301391064,877723168),(1305536863,1891463783),
        (1388734897,897335593),(1435557120,3896141096),(1553790504,1019574576),(1589556860,2888195476),
        (1669792723,2413278875),(1677180919,2479297167),(1755737153,1783952505),(1763668984,2562645296),
        (1763780622,1139671158),(1781551382,1255614814),(1834163303,2246316031),(1971149752,1103878128),
        (2059068466,2794359402),(2076329105,1561736585),(2257238439,1305848463),(2319885414,2283894334),
        (2407398462,2305736470),(2414626352,1604394872),(2447449706,1709236482),(2452545487,2634786903),
        (2467203039,2214424583),(2485657760,965934024),(2526773280,411014648),(2532323436,2113881316),
        (2577472338,3181984586),(2586562813,2237975061),(2595839237,3775800797),(2657604783,1702273159),
        (2719698929,331268185),(2724068510,3847471926),(2724608735,1866564759),(2734674728,3174771856),
        (2773358872,1210012576),(2793473444,3184690956),(2793548555,703902595),(2921714558,2526922422),
        (2959504464,1118428792),(3000428062,3013778406),(3067648983,531665167),(3094620656,293178904),
        (3112965625,1501094193),(3149307605,2982306509),(3160387771,1044888195),(3194530172,3846931924),
        (3323910164,3979300428),(3410844187,2788997987),(3437323171,3887037435),(3462414552,3294892432),
        (3467460423,3914973263),(3518670115,2899505723),(3573031954,3276278122),(3581696649,2805854721),
        (3599522901,2031584061),(3685945823,3657186535),(3712696020,2996369932),(3719981603,930759851),
        (3726719281,3245543337),(3775916472,3675553168),(3926119246,3279257734),(3938489430,1036557198),
        (3979621113,3598972737),(3980769162,3224649746),(3994043492,1627901452),(4039026690,4267244538),
        (4046357305,1901221009),(4081595582,2245839670),(4182064480,1924584408),(4183296050,3808902618),
        (4204488676,2493161484),(4244246940,95934356),(4255093903,1086997255),(4287799666,1763607626),
        (4287822553,2303417969),(4294909663,3798468567),
    ];
    if let Some(&(normal, reduced)) = PAIRS.iter().find(|(a, b)| hash == *a || hash == *b) {
        if unlocked.contains(&reduced) {
            reduced
        } else if unlocked.contains(&hash) {
            hash
        } else {
            normal
        }
    } else {
        hash
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentMod {
    pub hash: u32,
    pub cost: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentSocket {
    pub index: u8,
    pub plug_set_size: Option<usize>,
    pub fits: Vec<u32>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BucketAssignment {
    pub assigned: Vec<u32>,
    pub unassigned: Vec<u32>,
}
pub fn assign_bucket_specific_mods(
    sockets: &[AssignmentSocket],
    mods: &[AssignmentMod],
    mut capacity: i32,
) -> BucketAssignment {
    // DIM mod-assignment-utils.ts:576-675. Both sorts are stable. Do not
    // optimize this greedy choice by energy, hash, or already-inserted mods.
    let mut sockets: Vec<_> = sockets.iter().collect();
    sockets.sort_by_key(|s| s.plug_set_size.unwrap_or(999));
    let mut mods: Vec<_> = mods.iter().collect();
    mods.sort_by_key(|m| sockets.iter().filter(|s| s.fits.contains(&m.hash)).count());
    let mut result = BucketAssignment::default();
    let mut exclusions = Vec::new();
    for m in mods {
        let position = sockets.iter().position(|s| s.fits.contains(&m.hash));
        let group = exclusion_group(m.hash);
        if let Some(position) = position
            && m.cost <= capacity
            && group.is_none_or(|g| !exclusions.contains(&g))
        {
            result.assigned.push(m.hash);
            capacity -= m.cost;
            sockets.remove(position);
            if let Some(group) = group {
                exclusions.push(group);
            }
        } else {
            result.unassigned.push(m.hash);
        }
    }
    result
}

pub(crate) fn exclusion_group(hash: u32) -> Option<&'static str> {
    // DIM data/d2/mutually-exclusive-mods.json:2-13, including duplicates.
    match hash {
        84503918 | 1125986156 | 1170405455 | 1208761894 | 3064687909 | 4004774872 | 4004774873
        | 4004774874 | 4004774875 | 4004774876 | 4004774877 => Some("finisher"),
        2649291407 => Some("fastball"),
        _ => None,
    }
}
