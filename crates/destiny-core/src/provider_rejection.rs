//! Provider-neutral, definitive non-application reasons shared by all actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderRejection {
    ItemNotFound,
    AlreadyInInventory,
    NotInSocialSpace,
    Unequippable,
    ExoticLimit,
    DestinationFull,
    UniquenessViolation,
    Equipped,
    NotTransferable,
    ActionForbidden,
    Throttled,
}
impl ProviderRejection {
    pub const ALL: [Self; 11] = [
        Self::ItemNotFound,
        Self::AlreadyInInventory,
        Self::NotInSocialSpace,
        Self::Unequippable,
        Self::ExoticLimit,
        Self::DestinationFull,
        Self::UniquenessViolation,
        Self::Equipped,
        Self::NotTransferable,
        Self::ActionForbidden,
        Self::Throttled,
    ];
    pub const fn reason(self) -> &'static str {
        match self {
            Self::ItemNotFound => "item_not_found",
            Self::AlreadyInInventory => "already_in_inventory",
            Self::NotInSocialSpace => "not_in_social_space",
            Self::Unequippable => "unequippable",
            Self::ExoticLimit => "exotic_limit",
            Self::DestinationFull => "destination_full",
            Self::UniquenessViolation => "uniqueness_violation",
            Self::Equipped => "equipped",
            Self::NotTransferable => "not_transferable",
            Self::ActionForbidden => "action_forbidden",
            Self::Throttled => "throttled",
        }
    }
    pub const fn message(self) -> &'static str {
        match self {
            Self::ItemNotFound => {
                "the item is no longer where Ghostlink thought; the next read refreshes local state; reassess and retry"
            }
            Self::AlreadyInInventory => {
                "the item is already there; read its location before retrying"
            }
            Self::NotInSocialSpace => "go to orbit or a social space and retry",
            Self::Unequippable => "item is not equippable on this character",
            Self::ExoticLimit => {
                "equipping this item would exceed the exotic limit; choose a different item"
            }
            Self::DestinationFull => "destination full; free a slot and retry",
            Self::UniquenessViolation => {
                "this item conflicts with a uniqueness rule; choose a different item"
            }
            Self::Equipped => "item is equipped; unequip it first",
            Self::NotTransferable => "item is not transferable; choose a different item",
            Self::ActionForbidden => "Bungie forbids this action; do not work around it",
            Self::Throttled => "Bungie is throttling; wait and retry",
        }
    }
    pub const fn category(self) -> crate::ErrorCategory {
        use crate::ErrorCategory as C;
        match self {
            Self::Throttled => C::ProviderRateLimit,
            Self::ItemNotFound | Self::AlreadyInInventory | Self::DestinationFull => {
                C::ProviderValidation
            }
            _ => C::UnsupportedOperation,
        }
    }
}
