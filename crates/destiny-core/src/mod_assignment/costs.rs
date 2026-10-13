//! DIM inventory/store/energy.ts:18-49; mod-assignment-utils.ts:162-265.
use super::{MaterialCosts, RANKED_MATERIALS};
use crate::{ItemDefinition, ItemInstance, armor_catalog::*};

pub(super) fn material_costs(
    plug: &ItemDefinition,
    catalogs: &[ArmorCatalogRecord],
    first_only: bool,
) -> Result<MaterialCosts, &'static str> {
    let hash = plug
        .armor
        .insertion_material_requirement_hash
        .ok_or("insertion_material_requirement_hash_unavailable")?;
    let set = catalogs
        .iter()
        .find_map(|r| match r {
            ArmorCatalogRecord::MaterialRequirementSet(s) if s.hash == hash => Some(s),
            _ => None,
        })
        .ok_or("material_requirement_set_unavailable")?;
    let mut costs = [0_i64; 5];
    for (i, hash) in RANKED_MATERIALS.iter().enumerate() {
        for material in set.materials.iter().filter(|m| m.item_hash == *hash) {
            costs[i] += i64::from(material.count);
            if first_only {
                break;
            }
        }
    }
    Ok(costs)
}

pub fn cumulative_energy_costs(
    item: &ItemInstance,
    definition: &ItemDefinition,
    definitions: &[ItemDefinition],
    catalogs: &[ArmorCatalogRecord],
) -> Result<Vec<MaterialCosts>, &'static str> {
    let max = if item.gear_tier == Some(5) { 11 } else { 10 };
    let mut costs = vec![[0_i64; 5]; max + 1];
    // ArmorTier takes precedence; GhostTier is only a fallback. No visibility,
    // enabled, currentlyCanRoll or instance reusable-membership filter.
    let index = [760375309, 446880883].iter().find_map(|hash| {
        definition
            .armor
            .socket_categories
            .iter()
            .find(|c| c.socket_category_hash == *hash)
            .and_then(|c| c.socket_indexes.first())
            .filter(|index| {
                item.sockets
                    .iter()
                    .any(|s| u32::from(s.socket_index) == **index)
            })
    });
    let Some(index) = index else {
        return Ok(costs);
    };
    let entry = definition
        .armor
        .socket_entries
        .get(*index as usize)
        .ok_or("energy_socket_definition_unavailable")?;
    let Some(hash) = entry
        .reusable_plug_set_hash
        .or(entry.randomized_plug_set_hash)
    else {
        return Ok(costs);
    };
    let set = catalogs
        .iter()
        .find_map(|r| match r {
            ArmorCatalogRecord::PlugSet(s) if s.hash == hash => Some(s),
            _ => None,
        })
        .ok_or("energy_plug_set_unavailable")?;
    let energy_type = item
        .energy
        .as_ref()
        .and_then(|e| e.energy_type)
        .ok_or("armor_energy_type_unavailable")?;
    let mut plugs = Vec::new();
    for p in &set.reusable_plug_items {
        // Entries without a definition are skipped like DIM (sockets.ts:515-523).
        let Some(plug) = definitions
            .iter()
            .find(|d| d.hash.value() == i64::from(p.plug_item_hash))
        else {
            continue;
        };
        if plug.armor.plug_category_hash.is_none() || plug.armor.aspect_capacity.is_none() {
            continue;
        }
        if [Some(400813043), Some(4096629092)].contains(&plug.armor.plug_category_hash) {
            let kind = plug
                .armor
                .energy_capacity_type
                .ok_or("energy_plug_capacity_type_unavailable")?;
            let availability = plug
                .armor
                .plug_availability
                .ok_or("energy_plug_availability_unavailable")?;
            if kind == energy_type && availability == 2 {
                plugs.push(plug);
            }
        }
    }
    for tier in 0..=max {
        if tier > 0 {
            costs[tier] = costs[tier - 1];
        }
        if let Some(plug) = plugs
            .iter()
            .find(|p| p.armor.aspect_capacity == Some(tier as i32))
        {
            let increment = material_costs(plug, catalogs, false)?;
            for (i, cost) in increment.iter().enumerate() {
                costs[tier][i] += cost;
            }
        }
    }
    Ok(costs)
}
