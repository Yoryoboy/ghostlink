//! Ephemeral runtime bucket associations, separate from inventory revision identity.

use crate::{
    AppError, BucketHash, ErrorCategory, InventorySnapshotInput, ItemInstanceId, ItemLocation,
};
use std::collections::HashMap;

/// A gateway-supplied actual runtime bucket for one exact instance and location.
/// This is not a definition bucket, capacity fact, or action-target authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstancedBucketObservation {
    instance_id: ItemInstanceId,
    bucket_hash: BucketHash,
    location: ItemLocation,
}

impl InstancedBucketObservation {
    pub fn new(
        instance_id: ItemInstanceId,
        bucket_hash: BucketHash,
        location: ItemLocation,
    ) -> Result<Self, AppError> {
        if u32::try_from(bucket_hash.value()).is_err() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            instance_id,
            bucket_hash,
            location,
        })
    }

    pub fn instance_id(&self) -> &ItemInstanceId {
        &self.instance_id
    }

    pub fn bucket_hash(&self) -> BucketHash {
        self.bucket_hash
    }

    pub fn location(&self) -> &ItemLocation {
        &self.location
    }
}

/// One acquired input paired with unavailable or complete runtime observations.
///
/// `None` explicitly means not observed/unavailable; `Some([])` means observed
/// and complete only for an input with no instances. Stacks, quantities, item
/// hashes, currency coverage and metadata remain unchanged in the paired input.
/// Construction validates structural association, NOT authenticated provenance,
/// freshness, successful publication, or durability. A gateway must supply actual
/// runtime buckets; validation cannot establish how it acquired them.
///
/// Observations do not participate in inventory revision identity. Future planning
/// AND execution must reacquire; there is no revision-based reuse or dispatch
/// authority here. Future sync must validate requested membership and release
/// observations only after successful `replace_current`, alongside THAT returned
/// snapshot, never an unrelated reload. This contract does not implement sync.
///
/// Paired values cannot be mutated in place; consumption explicitly ends pairing.
/// ```compile_fail
/// use destiny_core::ProfileAcquisition;
/// fn mutate(pair: &mut ProfileAcquisition) {
///     pair.input().items.clear();
/// }
/// ```
/// ```compile_fail
/// use destiny_core::ProfileAcquisition;
/// fn replace(pair: &mut ProfileAcquisition) {
///     pair.observations = None;
/// }
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileAcquisition {
    input: InventorySnapshotInput,
    observations: Option<Vec<InstancedBucketObservation>>,
    minted_at: Option<crate::ProfileMintedAt>,
}

impl ProfileAcquisition {
    /// Validates the existing input invariants and an exact, complete association.
    /// Instance IDs retain the input's opaque syntax; stricter pull-request syntax
    /// belongs to that request, not to acquisition. Locations include owner identity.
    pub fn new(
        input: InventorySnapshotInput,
        observations: Option<Vec<InstancedBucketObservation>>,
    ) -> Result<Self, AppError> {
        if !input.is_structurally_valid() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        if let Some(rows) = &observations {
            let mut remaining: HashMap<_, _> = input
                .items
                .iter()
                .map(|item| (&item.instance_id, &item.location))
                .collect();
            for row in rows {
                if remaining.remove(&row.instance_id) != Some(&row.location) {
                    return Err(AppError::new(ErrorCategory::ProviderValidation));
                }
            }
            if !remaining.is_empty() {
                return Err(AppError::new(ErrorCategory::ProviderValidation));
            }
        }
        Ok(Self {
            input,
            observations,
            minted_at: None,
        })
    }

    pub fn with_minted_at(mut self, minted: Option<crate::ProfileMintedAt>) -> Self {
        self.minted_at = minted;
        self
    }

    pub fn minted_at(&self) -> Option<&crate::ProfileMintedAt> {
        self.minted_at.as_ref()
    }

    pub fn input(&self) -> &InventorySnapshotInput {
        &self.input
    }

    pub fn observations(&self) -> Option<&[InstancedBucketObservation]> {
        self.observations.as_deref()
    }

    /// Ends the immutable association, returning input and observations unchanged.
    /// Read `minted_at` before consumption when publishing; this compatibility
    /// method deliberately does not change the existing two-part return shape.
    /// Neither part becomes publication or action authorization evidence.
    pub fn into_parts(
        self,
    ) -> (
        InventorySnapshotInput,
        Option<Vec<InstancedBucketObservation>>,
    ) {
        (self.input, self.observations)
    }
}
