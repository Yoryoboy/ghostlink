//! Pinned audited stack capacity; uses the existing bucket index evidence decoder.

use crate::equipment_capacity_policy::{
    PINNED_EQUIPMENT_BUCKET_PATH, PINNED_EQUIPMENT_CAPACITY_VERSION,
    decode_equipment_capacity_evidence,
};
use destiny_core::{
    AppError, BucketHash, CapacityPolicyMismatch, DefinitionVersion,
    QualifiedStackBucketCapacityPolicy, StackBucketCapacityPolicy,
};

pub const PINNED_STACK_BUCKET_CAPACITY_VERSION: &str = PINNED_EQUIPMENT_CAPACITY_VERSION;
pub const PINNED_STACK_BUCKET_PATH: &str = PINNED_EQUIPMENT_BUCKET_PATH;
pub const PINNED_STACK_BUCKET_CAPACITY: u64 = 50;

/// Audited 2026-10-07: Consumables and Modifications have itemCount 50,
/// scope 1 (account), category 1, location 0, hasTransferDestination true,
/// enabled, non-FIFO. No other buckets are supported. Drift requires re-audit.
pub fn pinned_stack_bucket_capacity_policy() -> StackBucketCapacityPolicy {
    StackBucketCapacityPolicy::new(
        DefinitionVersion::new(PINNED_STACK_BUCKET_CAPACITY_VERSION),
        PINNED_STACK_BUCKET_PATH,
        [1469714392, 3313201758]
            .into_iter()
            .map(|hash| (BucketHash::new(hash), PINNED_STACK_BUCKET_CAPACITY, true))
            .collect(),
    )
    .expect("audited constants must form a supported stack bucket policy")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StackBucketCapacityQualificationError {
    Decode(AppError),
    Qualification(CapacityPolicyMismatch),
}

/// No fetch, new port, or authenticity/freshness certification. Reuses exactly
/// the same observed version and English bucket artifact as equipment capacity.
pub fn qualify_pinned_stack_bucket_capacity(
    index_bytes: &[u8],
) -> Result<QualifiedStackBucketCapacityPolicy, StackBucketCapacityQualificationError> {
    let evidence = decode_equipment_capacity_evidence(index_bytes)
        .map_err(StackBucketCapacityQualificationError::Decode)?;
    pinned_stack_bucket_capacity_policy()
        .qualify(&evidence)
        .map_err(StackBucketCapacityQualificationError::Qualification)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{ItemHash, StackPullRefusal};
    use serde_json::{Value, json};

    fn index() -> Value {
        json!({"ErrorCode":1,"Response":{"version":PINNED_STACK_BUCKET_CAPACITY_VERSION,
            "jsonWorldComponentContentPaths":{"en":{"DestinyInventoryBucketDefinition":PINNED_STACK_BUCKET_PATH}}}})
    }

    fn qualify(
        value: &Value,
    ) -> Result<QualifiedStackBucketCapacityPolicy, StackBucketCapacityQualificationError> {
        qualify_pinned_stack_bucket_capacity(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn matching_bucket_only_index_qualifies_and_covers_only_audited_buckets() {
        let policy = pinned_stack_bucket_capacity_policy();
        assert_eq!(
            policy.definition_version().as_str(),
            PINNED_STACK_BUCKET_CAPACITY_VERSION
        );
        assert_eq!(policy.bucket_artifact_identity(), PINNED_STACK_BUCKET_PATH);
        let qualified = qualify(&index()).unwrap();
        for hash in [1469714392, 3313201758] {
            assert_eq!(policy.capacity(BucketHash::new(hash)), Some((50, true)));
            let input = qualified
                .derive(BucketHash::new(hash), ItemHash::new(42), &[])
                .unwrap();
            assert_eq!(input.occupied, Some(0));
            assert_eq!(input.existing_quantity, Some(0));
            assert_eq!(input.account_wide, Some(true));
        }
        for hash in [0, 375726501, 3865314626, 1498876634] {
            assert_eq!(policy.capacity(BucketHash::new(hash)), None);
            assert_eq!(
                qualified.derive(BucketHash::new(hash), ItemHash::new(42), &[]),
                Err(StackPullRefusal::UnknownCapacity)
            );
        }
        assert!(
            crate::manifest::decode_manifest_index(&serde_json::to_vec(&index()).unwrap()).is_err()
        );
    }

    #[test]
    fn version_and_artifact_drift_fail_closed() {
        for field in ["version", "artifact"] {
            let mut value = index();
            if field == "version" {
                value["Response"]["version"] = json!("new-version");
            } else {
                value["Response"]["jsonWorldComponentContentPaths"]["en"]["DestinyInventoryBucketDefinition"] =
                    json!("/common/destiny2_content/json/en/new.json");
            }
            let Err(StackBucketCapacityQualificationError::Qualification(error)) = qualify(&value)
            else {
                panic!("expected qualification refusal");
            };
            assert_eq!(
                error,
                if field == "version" {
                    CapacityPolicyMismatch::VersionMismatch
                } else {
                    CapacityPolicyMismatch::ArtifactMismatch
                }
            );
        }
    }

    #[test]
    fn malformed_evidence_fails_in_shared_decoder() {
        assert!(matches!(
            qualify_pinned_stack_bucket_capacity(b"not json"),
            Err(StackBucketCapacityQualificationError::Decode(_))
        ));
        let mut missing = index();
        missing["Response"]["jsonWorldComponentContentPaths"]["en"] = json!({});
        assert!(matches!(
            qualify(&missing),
            Err(StackBucketCapacityQualificationError::Decode(_))
        ));
    }
}
