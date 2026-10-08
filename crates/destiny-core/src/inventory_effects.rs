//! Pure local projection effects for acknowledged instance actions.
//!
//! Callers supply acknowledgment and resolve equipment buckets. This module
//! performs no I/O and leaves snapshot identity, freshness and revision alone.

use crate::{CharacterId, InventorySnapshot, ItemInstance, ItemInstanceId, ItemLocation};

/// One acknowledged action, not a request or authorization to call a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryEffect {
    /// One transfer leg to Vault or character inventory; always clears equipped.
    Relocate {
        item: ItemInstanceId,
        to: ItemLocation,
    },
    SetLocked {
        item: ItemInstanceId,
        locked: bool,
    },
    /// The caller resolves previously equipped items in this character's bucket.
    /// No bucket inference or implicit unequip occurs here.
    Equip {
        item: ItemInstanceId,
        character: CharacterId,
        unequip: Vec<ItemInstanceId>,
    },
    /// Only instanced holdings; stack pulls are not represented.
    PullFromPostmaster {
        item: ItemInstanceId,
        character: CharacterId,
    },
}

/// A subject could not be resolved uniquely or its state contradicted the effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InventoryEffectError {
    UnknownInstance { item: ItemInstanceId },
    DuplicateInstance { item: ItemInstanceId },
    UnsupportedDestination,
    InvalidPostmasterSource { item: ItemInstanceId },
    InvalidUnequipSubject { item: ItemInstanceId },
}

/// Updated projection with a net content-change flag, not a revision allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryEffectsResult {
    pub snapshot: InventorySnapshot,
    pub changed: bool,
}

/// Applies effects in order to a copy, returning no partial projection on error.
///
/// Only location, equipped and locked can change. `changed` compares the final
/// projection with the input, so a sequence with no net change reports false.
/// Duplicate IDs are rejected only for referenced subjects, including unequip
/// subjects. The input and every unrelated field remain untouched.
///
/// Replay is accepted when the complete postcondition already holds: a pull is
/// already carried and unequipped on its owner; an equip target is equipped on
/// its character and all unequip subjects are there and unequipped. Otherwise,
/// pulls require that owner's Postmaster and unequip subjects must be equipped
/// on that character. A target cannot also be its own unequip subject.
/// Equip destinations cannot be Other/Postmaster because they are CharacterId.
pub fn apply_inventory_effects(
    snapshot: &InventorySnapshot,
    effects: &[InventoryEffect],
) -> Result<InventoryEffectsResult, InventoryEffectError> {
    let mut updated = snapshot.clone();
    for effect in effects {
        match effect {
            InventoryEffect::Relocate { item, to } => {
                let index = subject_index(&updated.items, item)?;
                if !matches!(to, ItemLocation::Vault | ItemLocation::Character(_)) {
                    return Err(InventoryEffectError::UnsupportedDestination);
                }
                updated.items[index].location = to.clone();
                updated.items[index].equipped = false;
            }
            InventoryEffect::SetLocked { item, locked } => {
                let index = subject_index(&updated.items, item)?;
                updated.items[index].locked = *locked;
            }
            InventoryEffect::Equip {
                item,
                character,
                unequip,
            } => {
                let index = subject_index(&updated.items, item)?;
                let indices = unequip
                    .iter()
                    .map(|subject| subject_index(&updated.items, subject))
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = ItemLocation::Character(character.clone());
                let already_satisfied = updated.items[index].location == destination
                    && updated.items[index].equipped
                    && indices.iter().all(|&other| {
                        other != index
                            && updated.items[other].location == destination
                            && !updated.items[other].equipped
                    });
                if already_satisfied {
                    continue;
                }
                for &other in &indices {
                    if other == index
                        || updated.items[other].location != destination
                        || !updated.items[other].equipped
                    {
                        return Err(InventoryEffectError::InvalidUnequipSubject {
                            item: updated.items[other].instance_id.clone(),
                        });
                    }
                }
                for other in indices {
                    updated.items[other].equipped = false;
                }
                updated.items[index].location = destination;
                updated.items[index].equipped = true;
            }
            InventoryEffect::PullFromPostmaster { item, character } => {
                let index = subject_index(&updated.items, item)?;
                let subject = &mut updated.items[index];
                let destination = ItemLocation::Character(character.clone());
                if subject.location == destination && !subject.equipped {
                    continue;
                }
                if subject.location != ItemLocation::Postmaster(character.clone()) {
                    return Err(InventoryEffectError::InvalidPostmasterSource {
                        item: item.clone(),
                    });
                }
                subject.location = destination;
                subject.equipped = false;
            }
        }
    }
    Ok(InventoryEffectsResult {
        changed: updated != *snapshot,
        snapshot: updated,
    })
}

fn subject_index(
    items: &[ItemInstance],
    subject: &ItemInstanceId,
) -> Result<usize, InventoryEffectError> {
    let mut matches = items
        .iter()
        .enumerate()
        .filter(|(_, item)| &item.instance_id == subject);
    let Some((index, _)) = matches.next() else {
        return Err(InventoryEffectError::UnknownInstance {
            item: subject.clone(),
        });
    };
    if matches.next().is_some() {
        return Err(InventoryEffectError::DuplicateInstance {
            item: subject.clone(),
        });
    }
    Ok(index)
}
