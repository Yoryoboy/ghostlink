//! DIM 29c122e mod-assignment-utils.ts:162-265,437-528,952-989,1022-1036.
use super::{AssignmentMod, costs, cumulative_energy_costs, generate_mod_permutations};
use crate::{ItemDefinition, ItemInstance, armor_catalog::*, build::AssumeArmorMasterwork};

pub const RANKED_MATERIALS: [u32; 5] = [3467984096, 4257549985, 4257549984, 3853748946, 3159615086];
pub type MaterialCosts = [i64; 5];

/// Inputs in caller item order. Costs are cumulative and cached by rarity by
/// the caller, as in DIM. Enhancement cost includes reaching maximum energy.
#[derive(Clone, Debug)]
pub struct RankingItem {
    pub original_capacity: i32,
    pub derived_capacity: i32,
    pub used: i32,
    pub specialty: Option<u32>,
    pub current_mods: Vec<u32>,
    pub costs: Vec<MaterialCosts>,
    pub enhancement_cost: Option<MaterialCosts>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankedAssignment {
    pub assigned: Vec<Vec<u32>>,
    pub unassigned: Vec<u32>,
    pub unassigned_by_item: Vec<Vec<u32>>,
    pub upgrade_costs: MaterialCosts,
}

#[allow(clippy::too_many_arguments)] // Pure request-local inputs, no hidden state.
pub(super) fn rank_armor(
    armor: &[(&ItemInstance, &ItemDefinition)],
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
    eligible: &[&ItemDefinition],
    assigned: &[Vec<u32>],
    enhancements: &[bool],
    assumption: Option<AssumeArmorMasterwork>,
) -> Result<RankedAssignment, &'static str> {
    let mut by_rarity = std::collections::BTreeMap::new();
    let mut items = Vec::new();
    let has_activity = eligible.iter().any(|m| {
        super::SPECIALTY_SOCKETS
            .iter()
            .any(|(_, c)| Some(*c) == m.armor.plug_category_hash)
    });
    for (index, (item, definition)) in armor.iter().enumerate() {
        let current = item
            .energy
            .as_ref()
            .expect("checked energy")
            .energy_capacity as i32;
        let rarity = definition.tier.as_deref().expect("checked rarity");
        if !by_rarity.contains_key(rarity) {
            by_rarity.insert(
                rarity,
                cumulative_energy_costs(item, definition, definitions, catalogs)?,
            );
        }
        let costs: Vec<MaterialCosts> = by_rarity[rarity].clone();
        let assumed = matches!(
            assumption,
            Some(AssumeArmorMasterwork::All | AssumeArmorMasterwork::ArtificeExotic)
        ) || (assumption == Some(AssumeArmorMasterwork::Legendary)
            && rarity != "Exotic");
        let maximum = if item.gear_tier == Some(5) { 11 } else { 10 };
        let derived_capacity = current.max(if assumption.is_none() {
            1
        } else if assumed {
            maximum
        } else {
            9
        });
        let enhancement_cost = if enhancements[index] {
            let mut extra = [0; 5];
            // DIM getExoticArtificeCosts: absent/non-pluggable plug contributes
            // no additional materials; an unresolved requirement stays unknown.
            if let Some(plug) = definitions
                .iter()
                .find(|d| d.hash.value() == 720825311 && d.armor.plug_category_hash.is_some())
            {
                extra = costs::material_costs(plug, catalogs, true)?;
            }
            let target = costs
                .get(maximum as usize)
                .ok_or("energy_cost_target_capacity_out_of_range")?;
            let paid = costs
                .get(current as usize)
                .ok_or("energy_cost_original_capacity_out_of_range")?;
            for i in 0..5 {
                extra[i] += target[i] - paid[i];
            }
            Some(extra)
        } else {
            None
        };
        let category = definition
            .armor
            .socket_categories
            .iter()
            .find(|c| c.socket_category_hash == 590099826)
            .expect("checked sockets");
        let current_mods = item
            .sockets
            .iter()
            .filter(|s| category.socket_indexes.contains(&u32::from(s.socket_index)))
            .filter_map(|s| s.plug_hash.map(|h| h.value() as u32))
            .collect();
        let used: i32 = assigned[index]
            .iter()
            .map(|hash| {
                definitions
                    .iter()
                    .find(|d| d.hash.value() == i64::from(*hash))
                    .expect("assigned definition")
                    .armor
                    .energy_cost
                    .as_ref()
                    .map_or(0, |e| e.energy_cost)
            })
            .try_fold(0_i32, |sum, cost| sum.checked_add(cost))
            .ok_or("mod_energy_cost_overflow")?;
        items.push(RankingItem {
            original_capacity: current,
            derived_capacity,
            used,
            specialty: if has_activity {
                super::specialty_category(item, definition, definitions, catalogs)?
            } else {
                None
            },
            current_mods,
            costs,
            enhancement_cost,
        });
    }
    let make_mod = |m: &&ItemDefinition| AssignmentMod {
        hash: m.hash.value() as u32,
        cost: m.armor.energy_cost.as_ref().map_or(0, |e| e.energy_cost),
    };
    let general = eligible
        .iter()
        .filter(|m| m.armor.plug_category_hash == Some(2487827355))
        .map(make_mod)
        .collect::<Vec<_>>();
    let activity = eligible
        .iter()
        .filter(|m| {
            super::SPECIALTY_SOCKETS
                .iter()
                .any(|(_, c)| Some(*c) == m.armor.plug_category_hash)
        })
        .map(|m| (make_mod(m), m.armor.plug_category_hash.unwrap()))
        .collect::<Vec<_>>();
    rank_independent_mods(&items, &general, &activity)
}

/// Activity is tried first; only the first five entries of each permutation
/// participate, matching DIM even for oversized lists. Exact ties keep Heap order.
pub fn rank_independent_mods(
    items: &[RankingItem],
    general: &[AssignmentMod],
    activity: &[(AssignmentMod, u32)],
) -> Result<RankedAssignment, &'static str> {
    let general_permutations =
        generate_mod_permutations(&general.iter().map(|m| m.hash).collect::<Vec<_>>());
    let activity_permutations =
        generate_mod_permutations(&activity.iter().map(|(m, _)| m.hash).collect::<Vec<_>>());
    let mut best = None;
    let mut best_key = None;
    for activity_permutation in &activity_permutations {
        for general_permutation in &general_permutations {
            let mut assigned = vec![vec![]; items.len()];
            let mut unassigned = Vec::new();
            let mut unassigned_by_item = vec![vec![]; items.len()];
            let mut costs = [0_i64; 5];
            let mut changes = 0;
            for (i, item) in items.iter().enumerate() {
                let mut used = i64::from(item.used);
                if let Some(hash) = activity_permutation.get(i).copied().flatten() {
                    let (m, category) = activity.iter().find(|(m, _)| m.hash == hash).unwrap();
                    if item.original_capacity > 0
                        && item.specialty == Some(*category)
                        && used + i64::from(m.cost) <= i64::from(item.derived_capacity)
                    {
                        assigned[i].push(hash);
                        used += i64::from(m.cost);
                    } else {
                        unassigned.push(hash);
                        unassigned_by_item[i].push(hash);
                    }
                }
                if let Some(hash) = general_permutation.get(i).copied().flatten() {
                    let m = general.iter().find(|m| m.hash == hash).unwrap();
                    if item.original_capacity > 0
                        && used + i64::from(m.cost) <= i64::from(item.derived_capacity)
                    {
                        assigned[i].push(hash);
                        used += i64::from(m.cost);
                    } else {
                        unassigned.push(hash);
                        unassigned_by_item[i].push(hash);
                    }
                }
                let paid = item
                    .costs
                    .get(item.original_capacity as usize)
                    .ok_or("energy_cost_original_capacity_out_of_range")?;
                let target = item
                    .costs
                    .get(used.max(i64::from(item.original_capacity)) as usize)
                    .ok_or("energy_cost_target_capacity_out_of_range")?;
                for j in 0..5 {
                    let cost = item.enhancement_cost.map_or(target[j] - paid[j], |v| v[j]);
                    costs[j] = costs[j].checked_add(cost).ok_or("material_cost_overflow")?;
                }
                changes += assigned[i]
                    .iter()
                    .filter(|hash| !item.current_mods.contains(hash))
                    .count();
            }
            let key = (unassigned.len(), costs, changes);
            if best_key.is_none_or(|k| key < k) {
                best_key = Some(key);
                best = Some(RankedAssignment {
                    assigned,
                    unassigned,
                    unassigned_by_item,
                    upgrade_costs: costs,
                });
            }
        }
    }
    Ok(best.expect("Heap permutations always include the initial permutation"))
}
