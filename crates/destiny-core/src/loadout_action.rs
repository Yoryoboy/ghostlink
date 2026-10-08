//! One official saved-loadout action, separate from the live read port.
use crate::{
    AppError, CharacterId, DestinyMembership, ErrorCategory, MAX_CHARACTER_LOADOUTS,
    is_canonical_loadout_id,
};

/// Validated zero-based saved slot and character; no item-by-item emulation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipLoadoutRequest {
    membership: DestinyMembership,
    character: CharacterId,
    loadout_index: i32,
}

impl EquipLoadoutRequest {
    pub fn new(
        membership: DestinyMembership,
        character: CharacterId,
        loadout_index: i32,
    ) -> Result<Self, AppError> {
        if usize::try_from(loadout_index).map_or(true, |i| i >= MAX_CHARACTER_LOADOUTS)
            || !is_canonical_loadout_id(character.as_str())
            || i32::try_from(membership.membership_type().value()).is_err()
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            character,
            loadout_index,
        })
    }

    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn character(&self) -> &CharacterId {
        &self.character
    }
    pub const fn loadout_index(&self) -> i32 {
        self.loadout_index
    }
}

/// Bounded reasons, never raw provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutActionRejection {
    /// Bungie requires orbit, a social space, or an offline character.
    RestrictedLocation,
    CharacterNotFound,
    InvalidRequest,
}

impl LoadoutActionRejection {
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::RestrictedLocation => ErrorCategory::UnsupportedOperation,
            Self::CharacterNotFound | Self::InvalidRequest => ErrorCategory::ProviderValidation,
        }
    }
}

/// Receipt for a single attempt, not proof of refreshed inventory state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadoutActionOutcome {
    Applied,
    Rejected(LoadoutActionRejection),
    AuthenticationRefused,
    Transient,
    Uncertain,
}

impl LoadoutActionOutcome {
    pub const fn applied(self) -> bool {
        matches!(self, Self::Applied)
    }
    pub const fn rejection(self) -> Option<LoadoutActionRejection> {
        match self {
            Self::Rejected(reason) => Some(reason),
            _ => None,
        }
    }
}

/// One dispatch, no retry or verification. Callers pace at one action/second.
/// `Err` means pre-dispatch failure; post-dispatch ambiguity is `Uncertain`.
pub trait LoadoutActionGateway {
    fn equip_loadout(
        &mut self,
        request: &EquipLoadoutRequest,
    ) -> Result<LoadoutActionOutcome, AppError>;
}
