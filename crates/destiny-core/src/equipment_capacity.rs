//! Pure equipment capacity correspondence and occupancy contracts; no I/O.

use crate::domain::ItemSlot;
use crate::{
    AppError, BucketHash, CharacterId, DefinitionVersion, ErrorCategory, ItemLocation,
    ProfileAcquisition, StackContainer,
};

/// Exact capacity-policy correspondence failure; version takes precedence when
/// both identities differ. Contains no provider-controlled text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityPolicyMismatch {
    VersionMismatch,
    ArtifactMismatch,
}

/// Audited capacities including the equipped slot, bound to exact definition artifacts.
/// No default policy or mutable fields: callers must supply all eight supported buckets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipmentCapacityPolicy {
    definition_version: DefinitionVersion,
    bucket_artifact_identity: String,
    capacities: Vec<(BucketHash, usize)>,
}

impl EquipmentCapacityPolicy {
    pub fn new(
        definition_version: DefinitionVersion,
        bucket_artifact_identity: impl Into<String>,
        capacities: Vec<(BucketHash, usize)>,
    ) -> Result<Self, AppError> {
        let bucket_artifact_identity = bucket_artifact_identity.into();
        validate_identity(&definition_version, &bucket_artifact_identity)?;
        // The shared classifier defines exactly eight supported buckets. Eight
        // distinct accepted keys therefore cover its entire supported set.
        if capacities.len() != 8
            || capacities
                .iter()
                .enumerate()
                .any(|(index, (bucket, capacity))| {
                    *capacity == 0
                        || ItemSlot::from_equipment_bucket(*bucket).is_none()
                        || capacities[..index]
                            .iter()
                            .any(|(earlier, _)| earlier == bucket)
                })
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            definition_version,
            bucket_artifact_identity,
            capacities,
        })
    }

    pub fn definition_version(&self) -> &DefinitionVersion {
        &self.definition_version
    }
    pub fn bucket_artifact_identity(&self) -> &str {
        &self.bucket_artifact_identity
    }
    pub fn capacity(&self, bucket: BucketHash) -> Option<usize> {
        self.capacities
            .iter()
            .find_map(|(key, capacity)| (*key == bucket).then_some(*capacity))
    }

    /// Proves correspondence to supplied index evidence, NOT its freshness or
    /// authenticity. The caller must acquire and authenticate that evidence.
    pub fn qualify(
        &self,
        evidence: &EquipmentCapacityEvidence,
    ) -> Result<QualifiedEquipmentCapacityPolicy, CapacityPolicyMismatch> {
        if self.definition_version != evidence.definition_version {
            return Err(CapacityPolicyMismatch::VersionMismatch);
        }
        if self.bucket_artifact_identity != evidence.bucket_artifact_identity {
            return Err(CapacityPolicyMismatch::ArtifactMismatch);
        }
        Ok(QualifiedEquipmentCapacityPolicy {
            policy: self.clone(),
        })
    }
}

/// Identity supplied from an observed public Manifest index. Core does not fetch
/// or certify it; qualification only compares these exact strings to the policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipmentCapacityEvidence {
    definition_version: DefinitionVersion,
    bucket_artifact_identity: String,
}
impl EquipmentCapacityEvidence {
    pub fn new(
        definition_version: DefinitionVersion,
        bucket_artifact_identity: impl Into<String>,
    ) -> Result<Self, AppError> {
        let bucket_artifact_identity = bucket_artifact_identity.into();
        validate_identity(&definition_version, &bucket_artifact_identity)?;
        Ok(Self {
            definition_version,
            bucket_artifact_identity,
        })
    }
    pub fn definition_version(&self) -> &DefinitionVersion {
        &self.definition_version
    }
    pub fn bucket_artifact_identity(&self) -> &str {
        &self.bucket_artifact_identity
    }
}

fn validate_identity(version: &DefinitionVersion, artifact: &str) -> Result<(), AppError> {
    if version.as_str().is_empty() || artifact.is_empty() {
        return Err(AppError::new(ErrorCategory::ProviderValidation));
    }
    Ok(())
}

/// Can only be produced through exact policy/evidence qualification.
/// ```compile_fail
/// use destiny_core::QualifiedEquipmentCapacityPolicy;
/// let qualified = QualifiedEquipmentCapacityPolicy { policy: todo!() };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedEquipmentCapacityPolicy {
    policy: EquipmentCapacityPolicy,
}

impl QualifiedEquipmentCapacityPolicy {
    /// Destination is the item definition's normal `bucketTypeHash`, never its
    /// Postmaster runtime bucket. Complete acquisition pairing guarantees each
    /// instance occurs once; equipped and unequipped Character rows both count.
    pub fn prove(
        &self,
        acquisition: &ProfileAcquisition,
        owner: &CharacterId,
        destination_bucket: Option<BucketHash>,
    ) -> Result<EquipmentCapacityProof, CapacityRefusal> {
        let bucket = destination_bucket.ok_or(CapacityRefusal::MissingDestinationBucket)?;
        let capacity = self
            .policy
            .capacity(bucket)
            .ok_or(CapacityRefusal::UnsupportedDestinationBucket)?;
        let observations = acquisition
            .observations()
            .ok_or(CapacityRefusal::ObservationsUnavailable)?;
        if acquisition.input().definition_version != self.policy.definition_version {
            return Err(CapacityRefusal::DefinitionVersionMismatch);
        }
        if acquisition.input().stacks.iter().any(|stack| {
            stack.bucket_hash() == bucket
                && matches!(stack.container(), StackContainer::CharacterInventory(id) if id == owner)
        }) {
            return Err(CapacityRefusal::DestinationStackObserved);
        }
        let occupied = observations
            .iter()
            .filter(|row| {
                row.bucket_hash() == bucket
                    && matches!(row.location(), ItemLocation::Character(id) if id == owner)
            })
            .try_fold(0usize, |count, _| increment_occupied(count))?;
        if occupied >= capacity {
            return Err(CapacityRefusal::AtCapacity { occupied, capacity });
        }
        Ok(EquipmentCapacityProof {
            destination_bucket: bucket,
            owner: owner.clone(),
            occupied,
            capacity,
            definition_version: self.policy.definition_version.clone(),
        })
    }
}

/// Capacity is unproven, not necessarily full (except the explicit occupancy case).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityRefusal {
    MissingDestinationBucket,
    UnsupportedDestinationBucket,
    ObservationsUnavailable,
    DefinitionVersionMismatch,
    DestinationStackObserved,
    ArithmeticOverflow,
    AtCapacity { occupied: usize, capacity: usize },
}

fn increment_occupied(count: usize) -> Result<usize, CapacityRefusal> {
    count
        .checked_add(1)
        .ok_or(CapacityRefusal::ArithmeticOverflow)
}

/// Read-only occupancy evidence, NOT dispatch authority, freshness certification,
/// or atomic with a later pull. Reacquire observations and reprove before acting.
/// ```compile_fail
/// use destiny_core::{BucketHash, CharacterId, DefinitionVersion, EquipmentCapacityProof};
/// let proof = EquipmentCapacityProof {
///     destination_bucket: BucketHash::new(1498876634), owner: CharacterId::new("owner"),
///     occupied: 0, capacity: 10, definition_version: DefinitionVersion::new("v1"),
/// };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EquipmentCapacityProof {
    destination_bucket: BucketHash,
    owner: CharacterId,
    occupied: usize,
    capacity: usize,
    definition_version: DefinitionVersion,
}
impl EquipmentCapacityProof {
    pub fn destination_bucket(&self) -> BucketHash {
        self.destination_bucket
    }
    pub fn owner(&self) -> &CharacterId {
        &self.owner
    }
    pub fn occupied(&self) -> usize {
        self.occupied
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn definition_version(&self) -> &DefinitionVersion {
        &self.definition_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_count_refuses_overflow_with_adjacent_valid_control() {
        assert_eq!(increment_occupied(usize::MAX - 1), Ok(usize::MAX));
        assert_eq!(
            increment_occupied(usize::MAX),
            Err(CapacityRefusal::ArithmeticOverflow)
        );
    }
}
