//! One official free socket plug insertion (`InsertSocketPlugFree`), no AWA tokens.
use crate::{
    AppError, CharacterId, DestinyMembership, ErrorCategory, ItemHash, ItemInstanceId,
    ProviderRejection,
};

/// The item must be owned by `character`; the caller owns that preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsertPlugRequest {
    membership: DestinyMembership,
    item: ItemInstanceId,
    character: CharacterId,
    socket_index: u8,
    plug: ItemHash,
}
impl InsertPlugRequest {
    pub fn new(
        membership: DestinyMembership,
        item: ItemInstanceId,
        character: CharacterId,
        socket_index: u8,
        plug: ItemHash,
    ) -> Result<Self, AppError> {
        if item.as_str().is_empty() || character.as_str().is_empty() || plug.value() <= 0 {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            membership,
            item,
            character,
            socket_index,
            plug,
        })
    }
    pub const fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub const fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub const fn character(&self) -> &CharacterId {
        &self.character
    }
    pub const fn socket_index(&self) -> u8 {
        self.socket_index
    }
    pub const fn plug(&self) -> ItemHash {
        self.plug
    }
}

/// Bounded refusals; no provider text crosses the seam.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPlugRejection {
    Provider(ProviderRejection),
    InvalidRequest,
}
impl InsertPlugRejection {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Provider(reason) => reason.reason(),
            Self::InvalidRequest => "invalid_request",
        }
    }
}

/// Honest classification of exactly one insertion attempt (not verified state).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPlugOutcome {
    Inserted,
    Rejected(InsertPlugRejection),
    AuthenticationRefused,
    Transient,
    Uncertain,
}

/// Requires the same OAuth permission as the other item actions
/// (MoveEquipDestinyItems); Bungie asks for at least 0.5 s between insertions.
/// No retry or refresh. Err denotes pre-dispatch failure only.
pub trait InsertPlugGateway {
    fn insert_plug(&mut self, request: &InsertPlugRequest) -> Result<InsertPlugOutcome, AppError>;
}
