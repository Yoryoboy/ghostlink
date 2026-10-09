//! One official batch equip attempt, separate from single-item equip.
use crate::{
    AppError, CharacterId, DestinyMembership, ErrorCategory, ItemInstanceId,
    is_canonical_loadout_id,
};
use std::collections::HashSet;

/// Equipment-slot bound; this is not an arbitrary inventory-size batch.
pub const MAX_EQUIP_ITEMS: usize = 17;

/// Items must already be on this character; the caller owns that preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipItemsRequest {
    membership: DestinyMembership,
    character: CharacterId,
    items: Vec<ItemInstanceId>,
}
impl EquipItemsRequest {
    pub fn new(
        membership: DestinyMembership,
        character: CharacterId,
        items: Vec<ItemInstanceId>,
    ) -> Result<Self, AppError> {
        let mut seen = HashSet::new();
        if items.is_empty()
            || items.len() > MAX_EQUIP_ITEMS
            || !is_canonical_loadout_id(character.as_str())
            || i32::try_from(membership.membership_type().value()).is_err()
            || items.iter().any(|id| {
                !is_canonical_loadout_id(id.as_str())
                    || id.as_str().parse::<i64>().is_err()
                    || !seen.insert(id.as_str())
            })
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            character,
            items,
        })
    }
    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn character(&self) -> &CharacterId {
        &self.character
    }
    pub fn items(&self) -> &[ItemInstanceId] {
        &self.items
    }
}

/// Only a bounded integer code survives decoding, never provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EquipItemStatus {
    Success,
    Failure(i32),
    /// Bungie omitted this item (e.g. it was not on the character).
    /// This is not evidence that it was equipped.
    NotReported,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipItemResult {
    pub item: ItemInstanceId,
    pub status: EquipItemStatus,
}

/// Acknowledged preserves well-formed reported statuses and marks absent IDs
/// NotReported. It does not mean that every item succeeded. Duplicate, extra,
/// or malformed results make the whole attempt uncertain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EquipItemsOutcome {
    Acknowledged(Vec<EquipItemResult>),
    Rejected(crate::ProviderRejection),
    Refused,
    RestrictedLocation,
    AuthenticationRefused,
    Transient,
    Uncertain,
}

impl EquipItemsOutcome {
    /// True only when every requested item explicitly reported success.
    pub fn fully_applied(&self) -> bool {
        matches!(self, Self::Acknowledged(results) if !results.is_empty()
            && results.iter().all(|result| result.status == EquipItemStatus::Success))
    }
}

/// Requires MoveEquipDestinyItems; caller paces at 0.1s between actions.
/// No retry or refresh. Err denotes pre-dispatch failure only.
pub trait EquipItemsGateway {
    fn equip_items(&mut self, request: &EquipItemsRequest) -> Result<EquipItemsOutcome, AppError>;
}
