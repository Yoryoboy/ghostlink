//! Non-instanced inventory observations, not provider action targets.
//!
//! Membership and revision belong to the enclosing snapshot. Rows have no
//! instance or persistent stack identifier: equal rows may describe distinct
//! stacks, and neither their values nor their sorted position identifies a
//! provider stack across refreshes. Same-hash rows must not be coalesced.
//!
//! Sources may overlap (for example, currency summaries and inventory rows).
//! Container distinctions preserve provenance but do not prove disjoint
//! coverage. Acquisition must establish coverage before combining quantities;
//! collecting both sources is not permission to count the same holdings twice.

use crate::{AppError, BucketHash, CharacterId, ErrorCategory, ItemHash};

/// Explicit qualification of the profile currency source in one snapshot.
/// Holdings never imply availability; an available source may be observed-empty.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProfileCurrenciesCoverage {
    /// Legacy snapshot or acquisition that never observed this component.
    #[default]
    NotObserved,
    /// The component was observed but its data was unavailable.
    Unavailable,
    /// The component supplied data, including an empty set of balances.
    Available,
}

/// Source/container context for a non-instanced observation.
///
/// Character inventory and Postmaster require an owner structurally; profile
/// scopes cannot carry one. This context does not assert action addressability
/// or determine an equipment destination from an item's definition bucket.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum StackContainer {
    /// Profile inventory, including profile-level Vault buckets and account-wide
    /// materials. This source alone does not mean Vault equipment; retain the
    /// actual runtime bucket on each observation.
    ProfileInventory,
    /// Inventory reported for this character, distinct from its Postmaster.
    CharacterInventory(CharacterId),
    /// Postmaster holdings for this character, including materials/consumables.
    Postmaster(CharacterId),
    /// Profile currency summaries, kept separate for coverage reconciliation.
    ProfileCurrencies,
}

/// One validated observation of a non-instanced inventory stack.
///
/// This record is not an action selector and carries no [`crate::ItemInstanceId`].
/// No instance identifier is fabricated, even when two observations are equal.
/// Duplicate rows are legal and significant. Sort a collection using [`Ord`]
/// for order-independent **multiset** comparison: sorting preserves duplicates
/// and quantity differences; deduplication or hash-keyed maps do not.
/// Ordering is only a normalization convention, not provider stack identity.
///
/// There is deliberately no conversion into an instance identifier:
/// ```compile_fail
/// use destiny_core::{ItemInstanceId, StackObservation};
/// fn action_id(stack: StackObservation) -> ItemInstanceId {
///     stack.into()
/// }
/// ```
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StackObservation {
    item_hash: ItemHash,
    bucket_hash: BucketHash,
    quantity: i32,
    container: StackContainer,
}

impl StackObservation {
    /// Validates the provider-width numeric fields without changing existing
    /// hash constructors. Hashes accept the full unsigned 32-bit range,
    /// including zero; the canonical hash types do not reserve zero.
    /// Physical quantities must be in `1..=i32::MAX`; profile currency balances
    /// additionally allow zero. The wider input prevents narrowing an
    /// overflowing quantity before validation. Invalid input yields a sanitized
    /// [`ErrorCategory::ProviderValidation`] error, with no payload values.
    /// Ownership uses canonical [`CharacterId`] semantics unchanged.
    pub fn new(
        item_hash: ItemHash,
        bucket_hash: BucketHash,
        quantity: i64,
        container: StackContainer,
    ) -> Result<Self, AppError> {
        let minimum = if container == StackContainer::ProfileCurrencies {
            0
        } else {
            1
        };
        if u32::try_from(item_hash.value()).is_err()
            || u32::try_from(bucket_hash.value()).is_err()
            || !(minimum..=i64::from(i32::MAX)).contains(&quantity)
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            item_hash,
            bucket_hash,
            quantity: quantity as i32,
            container,
        })
    }

    /// Item definition reference, never an instance identifier.
    pub const fn item_hash(&self) -> ItemHash {
        self.item_hash
    }

    /// Actual runtime container bucket, not the item's definition bucket.
    pub const fn bucket_hash(&self) -> BucketHash {
        self.bucket_hash
    }

    /// Signed int32 balance (possibly zero) for profile currencies, or positive
    /// physical stack size for all other containers.
    pub const fn quantity(&self) -> i32 {
        self.quantity
    }

    /// Source/container with ownership encoded only where required.
    pub const fn container(&self) -> &StackContainer {
        &self.container
    }
}
