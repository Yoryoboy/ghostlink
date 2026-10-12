//! DIM 29c122e: mod-permutations.ts, specialty-modslots.ts and free mod assignment.
//! General/activity candidate ranking is deliberately not implemented in this slice.
use crate::{ItemDefinition, ItemInstance, armor_model};
use std::collections::HashSet;

pub const ARTIFICE_CATEGORY: u32 = 3773173029;
pub const TUNING_CATEGORY: u32 = 3481777685;

/// DIM mod-permutations.ts:3-64. Hash-key deduplication preserves Heap order.
/// As in DIM, lists longer than five retain their trailing entries unchanged.
pub fn generate_mod_permutations(mods: &[u32]) -> Vec<Vec<Option<u32>>> {
    generate_permutations_of_five(mods, |permutation| permutation.to_vec())
}

/// DIM mod-permutations.ts:16-57 `generatePermutationsOfFive`: Heap's algorithm over
/// the items padded with `None` to five, keeping the first permutation per key.
pub fn generate_permutations_of_five<T: Clone, K: Eq + std::hash::Hash>(
    items: &[T],
    key: impl Fn(&[Option<T>]) -> K,
) -> Vec<Vec<Option<T>>> {
    let mut items: Vec<_> = items.iter().cloned().map(Some).collect();
    items.resize(items.len().max(5), None);
    let mut result = vec![items.clone()];
    let mut seen = HashSet::from([key(&items)]);
    let mut cursors = [0; 5];
    let mut i = 0;
    while i < 5 {
        if cursors[i] < i {
            items.swap(if i % 2 == 0 { 0 } else { cursors[i] }, i);
            if seen.insert(key(&items)) {
                result.push(items.clone());
            }
            cursors[i] += 1;
            i = 0;
        } else {
            cursors[i] = 0;
            i += 1;
        }
    }
    result
}

// DIM specialty-modslots.ts:43-150; tags are equivalent iff these categories match.
// Order matches DIM's metadata table; item socket order determines the first match.
pub const SPECIALTY_SOCKETS: [(u32, u32); 10] = [
    (1444083081, 13646368),
    (1764679361, 1486918022),
    (1269555732, 1703496685),
    (3372624220, 2274750776),
    (2381877427, 2207493141),
    (3344538838, 2173937871),
    (1956816524, 2106680364),
    (2804745000, 2140235634),
    (1252302330, 125494331),
    (2701840022, 1081029832),
];

/// DIM item-utils.ts:69-100: first visible, plugged specialty socket, not enabled.
/// Returns its compatible activity category (the exact DIM slot-tag equivalence).
pub fn specialty_category(
    item: &ItemInstance,
    definition: &ItemDefinition,
    definitions: &[ItemDefinition],
) -> Result<Option<u32>, &'static str> {
    for socket in &item.sockets {
        if !socket.visible {
            continue;
        }
        let Some(hash) = socket.plug_hash else {
            continue;
        };
        let plug = definitions
            .iter()
            .find(|d| d.hash == hash)
            .ok_or("inserted_plug_definition_unavailable")?;
        if plug.armor.plug_category_hash.is_none() {
            continue;
        }
        let entry = definition
            .armor
            .socket_entries
            .get(usize::from(socket.socket_index))
            .ok_or("armor_socket_definition_unavailable")?;
        if let Some((_, category)) = SPECIALTY_SOCKETS
            .iter()
            .find(|(socket_type, _)| *socket_type == entry.socket_type_hash)
        {
            return Ok(Some(*category));
        }
    }
    Ok(None)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeModItem {
    pub bucket: i64,
    pub exotic: bool,
    pub artifice: bool,
    /// None = no tuning socket; Some(None) = no unique directional restriction.
    pub tuning: Option<Option<u32>>,
    /// All plugged sockets, including non-ArmorMods sockets (DIM's artifice preference).
    pub current_plugs: Vec<u32>,
}

/// DIM item-utils.ts:298-325, socket-utils.ts:677-686, armor-upgrade-utils.ts:29-36.
/// Missing raw membership is unknown, unlike known empty/conflicting directions.
pub(super) fn free_mod_items(
    armor: &[(&ItemInstance, &ItemDefinition)],
    definitions: &[ItemDefinition],
    assume_artifice_exotic: bool,
    needs_tuning: bool,
) -> Result<Vec<FreeModItem>, &'static str> {
    armor
        .iter()
        .map(|(item, definition)| {
            if item.sockets.is_empty() {
                return Err("armor_socket_state_unavailable");
            }
            let mut armor3 = false;
            let mut artifice = false;
            let mut tuning = None;
            let mut current_plugs = Vec::new();
            for socket in &item.sockets {
                let Some(hash) = socket.plug_hash else {
                    continue;
                };
                let plug = definitions
                    .iter()
                    .find(|d| d.hash == hash)
                    .ok_or("inserted_plug_definition_unavailable")?;
                let Some(category) = plug.armor.plug_category_hash else {
                    continue;
                };
                current_plugs.push(hash.value() as u32);
                armor3 |= category == armor_model::ARMOR3_MASTERWORK_PLUG;
                artifice |=
                    socket.visible && (category == ARTIFICE_CATEGORY || hash.value() == 3727270518);
                if needs_tuning && tuning.is_none() && socket.visible && category == TUNING_CATEGORY
                {
                    tuning = Some(armor_model::tuned_stat(
                        socket
                            .reusable_plugs
                            .as_deref()
                            .ok_or("raw_reusable_plugs_not_projected")?,
                    ));
                }
            }
            let exotic = definition.tier.as_deref() == Some("Exotic");
            Ok(FreeModItem {
                bucket: definition.bucket_hash.expect("armor bucket").value(),
                exotic,
                artifice: artifice
                    || (assume_artifice_exotic && exotic && item.energy.is_some() && !armor3),
                tuning,
                current_plugs,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeModAssignment {
    /// Indexed in caller item order; artifice then tuning, as in DIM.
    pub assigned: Vec<Vec<u32>>,
    pub unassigned: Vec<u32>,
}

/// DIM mod-assignment-utils.ts:359-420. Each free slot can be used once.
pub fn assign_free_mods(
    items: &[FreeModItem],
    artifice_mods: &[u32],
    tuning_mods: &[u32],
) -> FreeModAssignment {
    let mut result = FreeModAssignment {
        assigned: vec![vec![]; items.len()],
        unassigned: Vec::new(),
    };
    let mut candidates: Vec<_> = (0..items.len()).filter(|i| items[*i].artifice).collect();
    for hash in artifice_mods {
        let position = candidates
            .iter()
            .position(|i| items[*i].current_plugs.contains(hash))
            .or_else(|| candidates.iter().position(|i| !items[*i].exotic))
            .or_else(|| (!candidates.is_empty()).then_some(0));
        if let Some(position) = position {
            let index = candidates.remove(position);
            result.assigned[index].push(*hash);
        } else {
            result.unassigned.push(*hash);
        }
    }
    let mut candidates: Vec<_> = (0..items.len())
        .filter(|i| items[*i].tuning.is_some())
        .collect();
    candidates.sort_by_key(|i| {
        super::ARMOR_BUCKETS
            .iter()
            .position(|bucket| *bucket == items[*i].bucket)
            .map_or(-1, |index| index as i32)
    });
    let legendary_count = candidates.iter().filter(|i| !items[**i].exotic).count();
    if tuning_mods.len() <= legendary_count {
        candidates.retain(|i| !items[*i].exotic);
    }
    for hash in tuning_mods {
        let mod_stat = armor_model::tuning_mod_stat(*hash);
        let position = candidates.iter().position(|i| {
            let item_stat = items[*i].tuning.flatten();
            item_stat.is_none() || mod_stat.is_none() || item_stat == mod_stat
        });
        if let Some(position) = position {
            let index = candidates.remove(position);
            result.assigned[index].push(*hash);
        } else {
            result.unassigned.push(*hash);
        }
    }
    result
}
