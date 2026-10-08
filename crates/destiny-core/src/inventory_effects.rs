//! Pure local projection effects for acknowledged inventory actions.
//!
//! Callers supply acknowledgment and resolve equipment buckets. This module
//! performs no I/O and leaves snapshot identity, freshness and revision alone.

use crate::{
    BucketHash, CharacterId, InventorySnapshot, ItemHash, ItemInstance, ItemInstanceId,
    ItemLocation, StackContainer, StackObservation,
};

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
    /// Pulls one anonymous Postmaster stack into profile inventory.
    /// The caller resolves the destination bucket and max stack size from definitions.
    /// Apply exactly once per acknowledgement: replay pulls the quantity again.
    PullStack {
        item_hash: ItemHash,
        owner: CharacterId,
        quantity: u32,
        destination_bucket: BucketHash,
        max_stack_size: i32,
    },
    /// Only instanced holdings; anonymous stacks use `PullStack`.
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
    InvalidStackParameters,
    NoEligiblePostmasterStack,
    StackArithmeticOverflow,
}

/// Updated projection with a net content-change flag, not a revision allocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryEffectsResult {
    pub snapshot: InventorySnapshot,
    pub changed: bool,
}

/// Applies effects in order to a copy, returning no partial projection on error.
///
/// Instance location, equipped and locked, and stack rows can change.
/// `changed` compares the final
/// projection with the input, so a sequence with no net change reports false.
/// Duplicate IDs are rejected only for referenced subjects, including unequip
/// subjects. The input and every unrelated field remain untouched.
///
/// Stack pulls are NOT idempotent: replay pulls again. Callers must apply them
/// exactly once per acknowledgement. Stack effects restore canonical row order.
/// Instance replay is accepted when the complete postcondition already holds: a pull is
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
            InventoryEffect::PullStack {
                item_hash,
                owner,
                quantity,
                destination_bucket,
                max_stack_size,
            } => pull_stack(
                &mut updated.stacks,
                *item_hash,
                owner,
                *quantity,
                *destination_bucket,
                *max_stack_size,
            )?,
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

fn pull_stack(
    stacks: &mut Vec<StackObservation>,
    item_hash: ItemHash,
    owner: &CharacterId,
    quantity: u32,
    destination_bucket: BucketHash,
    max_stack_size: i32,
) -> Result<(), InventoryEffectError> {
    use InventoryEffectError::{InvalidStackParameters, StackArithmeticOverflow};
    if quantity == 0 || max_stack_size <= 0 {
        return Err(InvalidStackParameters);
    }
    let quantity = i32::try_from(quantity).map_err(|_| StackArithmeticOverflow)?;
    // Validate definition-supplied hashes even when no new destination row is needed.
    let destination = StackObservation::new(
        item_hash,
        destination_bucket,
        1,
        StackContainer::ProfileInventory,
    )
    .map_err(|_| InvalidStackParameters)?;
    stacks.sort();
    let source = stacks
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.item_hash() == item_hash
                && row.container() == &StackContainer::Postmaster(owner.clone())
                && row.quantity() >= quantity
        })
        .min_by_key(|(index, row)| (row.quantity(), *index))
        .map(|(index, _)| index)
        .ok_or(InventoryEffectError::NoEligiblePostmasterStack)?;
    let remaining = stacks[source]
        .quantity()
        .checked_sub(quantity)
        .ok_or(StackArithmeticOverflow)?;
    if remaining == 0 {
        stacks.remove(source);
    } else {
        stacks[source] = stack_with_quantity(&stacks[source], remaining)?;
    }
    let mut remaining = quantity;
    for row in stacks.iter_mut().filter(|row| {
        row.item_hash() == item_hash
            && row.bucket_hash() == destination_bucket
            && row.container() == &StackContainer::ProfileInventory
            && row.quantity() < max_stack_size
    }) {
        let capacity = max_stack_size
            .checked_sub(row.quantity())
            .ok_or(StackArithmeticOverflow)?;
        let added = remaining.min(capacity);
        let total = row
            .quantity()
            .checked_add(added)
            .ok_or(StackArithmeticOverflow)?;
        *row = stack_with_quantity(row, total)?;
        remaining = remaining
            .checked_sub(added)
            .ok_or(StackArithmeticOverflow)?;
        if remaining == 0 {
            break;
        }
    }
    while remaining > 0 {
        let added = remaining.min(max_stack_size);
        stacks.push(stack_with_quantity(&destination, added)?);
        remaining = remaining
            .checked_sub(added)
            .ok_or(StackArithmeticOverflow)?;
    }
    stacks.sort();
    Ok(())
}

fn stack_with_quantity(
    row: &StackObservation,
    quantity: i32,
) -> Result<StackObservation, InventoryEffectError> {
    StackObservation::new(
        row.item_hash(),
        row.bucket_hash(),
        i64::from(quantity),
        row.container().clone(),
    )
    .map_err(|_| InventoryEffectError::StackArithmeticOverflow)
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
