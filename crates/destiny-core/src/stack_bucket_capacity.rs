//! Pure stack destination capacity correspondence and profile-row derivation.

use crate::{
    AppError, BucketHash, CapacityPolicyMismatch, DefinitionVersion, EquipmentCapacityEvidence,
    ErrorCategory, ItemHash, StackContainer, StackObservation, StackPullCapacityInput,
    StackPullRefusal,
};

/// Reuses the existing Manifest version + bucket artifact evidence contract.
pub type StackBucketCapacityEvidence = EquipmentCapacityEvidence;

/// Immutable audited bucket table. Only the two supported account buckets may
/// appear; no default capacities or inference from arbitrary inventory buckets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackBucketCapacityPolicy {
    identity: StackBucketCapacityEvidence,
    capacities: Vec<(BucketHash, u64, bool)>,
}

impl StackBucketCapacityPolicy {
    /// Each entry is (runtime bucket hash, capacity in slots, account-wide).
    pub fn new(
        definition_version: DefinitionVersion,
        bucket_artifact_identity: impl Into<String>,
        capacities: Vec<(BucketHash, u64, bool)>,
    ) -> Result<Self, AppError> {
        let identity =
            StackBucketCapacityEvidence::new(definition_version, bucket_artifact_identity)?;
        if capacities.len() != 2
            || capacities
                .iter()
                .enumerate()
                .any(|(index, (bucket, slots, account))| {
                    *slots == 0
                        || !account
                        || !matches!(bucket.value(), 1469714392 | 3313201758)
                        || capacities[..index].iter().any(|(key, _, _)| key == bucket)
                })
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            identity,
            capacities,
        })
    }

    pub fn definition_version(&self) -> &DefinitionVersion {
        self.identity.definition_version()
    }

    pub fn bucket_artifact_identity(&self) -> &str {
        self.identity.bucket_artifact_identity()
    }

    pub fn capacity(&self, bucket: BucketHash) -> Option<(u64, bool)> {
        self.capacities
            .iter()
            .find_map(|(key, slots, account)| (*key == bucket).then_some((*slots, *account)))
    }

    /// Exact correspondence only, not authenticity or freshness certification.
    pub fn qualify(
        &self,
        evidence: &StackBucketCapacityEvidence,
    ) -> Result<QualifiedStackBucketCapacityPolicy, CapacityPolicyMismatch> {
        if self.definition_version() != evidence.definition_version() {
            return Err(CapacityPolicyMismatch::VersionMismatch);
        }
        if self.bucket_artifact_identity() != evidence.bucket_artifact_identity() {
            return Err(CapacityPolicyMismatch::ArtifactMismatch);
        }
        Ok(QualifiedStackBucketCapacityPolicy {
            policy: self.clone(),
        })
    }
}

/// Produced only by exact version AND bucket artifact qualification.
/// ```compile_fail
/// use destiny_core::QualifiedStackBucketCapacityPolicy;
/// let policy = QualifiedStackBucketCapacityPolicy { policy: todo!() };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedStackBucketCapacityPolicy {
    policy: StackBucketCapacityPolicy,
}

impl QualifiedStackBucketCapacityPolicy {
    /// Caller must supply complete, fresh observations corresponding to the
    /// qualified definitions. Counts rows (including duplicates) as slots and
    /// sums same-hash quantities, solely within the profile destination bucket.
    /// Postmaster, character inventory and currency summaries never contribute.
    /// Definition metadata stays unknown for the caller to qualify separately;
    /// this input alone cannot permit a pull or authorize dispatch.
    pub fn derive(
        &self,
        destination_bucket: BucketHash,
        item_hash: ItemHash,
        observations: &[StackObservation],
    ) -> Result<StackPullCapacityInput, StackPullRefusal> {
        let (capacity, account_wide) = self
            .policy
            .capacity(destination_bucket)
            .ok_or(StackPullRefusal::UnknownCapacity)?;
        let mut occupied = 0;
        let mut existing = 0;
        for row in observations.iter().filter(|row| {
            row.bucket_hash() == destination_bucket
                && row.container() == &StackContainer::ProfileInventory
        }) {
            occupied = checked_add(occupied, 1)?;
            if row.item_hash() == item_hash {
                existing = checked_add(existing, row.quantity() as u64)?;
            }
        }
        Ok(StackPullCapacityInput {
            capacity: Some(capacity),
            occupied: Some(occupied),
            existing_quantity: Some(existing),
            account_wide: Some(account_wide),
            max_stack_size: None,
            unique_stack: None,
            side_effects: None,
        })
    }
}

fn checked_add(total: u64, amount: u64) -> Result<u64, StackPullRefusal> {
    total
        .checked_add(amount)
        .ok_or(StackPullRefusal::ArithmeticOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real observations have int32 quantities: reaching u64 overflow would
    // require billions of rows. Exercise the exact aggregation primitive.
    #[test]
    fn occupied_and_quantity_additions_refuse_overflow() {
        assert_eq!(checked_add(u64::MAX - 1, 1), Ok(u64::MAX));
        assert_eq!(
            checked_add(u64::MAX, 1),
            Err(StackPullRefusal::ArithmeticOverflow)
        );
        assert_eq!(
            checked_add(u64::MAX - 2, 3),
            Err(StackPullRefusal::ArithmeticOverflow)
        );
    }
}
