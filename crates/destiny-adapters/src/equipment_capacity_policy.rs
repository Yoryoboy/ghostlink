//! Pinned audited equipment capacity and public index correspondence only.

use crate::manifest::{decode_component_path, decode_version};
use destiny_core::{
    AppError, BucketHash, DefinitionVersion, EquipmentCapacityEvidence, EquipmentCapacityPolicy,
    ErrorCategory, QualifiedEquipmentCapacityPolicy,
};
use serde_json::Value;

/// Audited 2026-10-07 from the public Bungie Manifest index
/// (`https://www.bungie.net/Platform/Destiny2/Manifest/`) and its English
/// DestinyInventoryBucketDefinition artifact. All eight equipment buckets are
/// character-scoped with itemCount 10, including equipped items. A new Manifest
/// version closes qualification until explicit re-audit; never auto-accept it.
pub const PINNED_EQUIPMENT_CAPACITY_VERSION: &str = "244213.26.06.29.2000-1-bnet.65864";
/// Exact English bucket artifact path from the audit above.
pub const PINNED_EQUIPMENT_BUCKET_PATH: &str = "/common/destiny2_content/json/en/DestinyInventoryBucketDefinition-a8ba855a-93c7-4014-8f2e-d92357fdfb42.json";
/// Audited slots per character equipment bucket, including the equipped slot.
pub const PINNED_EQUIPMENT_CAPACITY: usize = 10;

const EQUIPMENT_BUCKETS: [u32; 8] = [
    1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954, 1585787867,
];

/// Builds the pinned policy through the core's complete-bucket validation.
pub fn pinned_equipment_capacity_policy() -> EquipmentCapacityPolicy {
    EquipmentCapacityPolicy::new(
        DefinitionVersion::new(PINNED_EQUIPMENT_CAPACITY_VERSION),
        PINNED_EQUIPMENT_BUCKET_PATH,
        EQUIPMENT_BUCKETS
            .into_iter()
            .map(|hash| (BucketHash::new(i64::from(hash)), PINNED_EQUIPMENT_CAPACITY))
            .collect(),
    )
    .expect("audited constants must form a complete equipment capacity policy")
}

/// Validates only the index envelope, version and required English bucket path.
/// Independent of the item-sync download plan; no artifact fetch or authenticity
/// certification is performed. Failures contain no provider-controlled text.
pub fn decode_equipment_capacity_evidence(
    index_bytes: &[u8],
) -> Result<EquipmentCapacityEvidence, AppError> {
    let invalid = || AppError::new(ErrorCategory::ProviderValidation);
    let value: Value = serde_json::from_slice(index_bytes).map_err(|_| invalid())?;
    let root = value.as_object().ok_or_else(invalid)?;
    if root.get("ErrorCode").and_then(Value::as_u64) != Some(1) {
        return Err(invalid());
    }
    let response = root
        .get("Response")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    EquipmentCapacityEvidence::new(
        decode_version(response.get("version"))?,
        decode_component_path(response, "DestinyInventoryBucketDefinition")?,
    )
}

/// Distinguishes malformed index evidence from the core's exact correspondence
/// refusal. Both wrap the original sanitized AppError (ProviderValidation);
/// qualification does not imply freshness, authenticity or dispatch authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EquipmentCapacityQualificationError {
    Decode(AppError),
    Qualification(AppError),
}

/// Qualifies only exact audited version AND artifact correspondence.
pub fn qualify_pinned_equipment_capacity(
    index_bytes: &[u8],
) -> Result<QualifiedEquipmentCapacityPolicy, EquipmentCapacityQualificationError> {
    let evidence = decode_equipment_capacity_evidence(index_bytes)
        .map_err(EquipmentCapacityQualificationError::Decode)?;
    pinned_equipment_capacity_policy()
        .qualify(&evidence)
        .map_err(EquipmentCapacityQualificationError::Qualification)
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::domain::ItemSlot;
    use serde_json::{Value, json};

    fn index() -> Value {
        json!({"ErrorCode":1,"Response":{"version":PINNED_EQUIPMENT_CAPACITY_VERSION,
            "jsonWorldComponentContentPaths":{"en":{"DestinyInventoryBucketDefinition":PINNED_EQUIPMENT_BUCKET_PATH}}}})
    }

    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn pinned_policy_constructs_and_covers_all_equipment_slots() {
        let policy = pinned_equipment_capacity_policy();
        assert_eq!(
            policy.definition_version().as_str(),
            PINNED_EQUIPMENT_CAPACITY_VERSION
        );
        assert_eq!(
            policy.bucket_artifact_identity(),
            PINNED_EQUIPMENT_BUCKET_PATH
        );
        let expected = [
            ItemSlot::Kinetic,
            ItemSlot::Energy,
            ItemSlot::Power,
            ItemSlot::Helmet,
            ItemSlot::Gauntlets,
            ItemSlot::Chest,
            ItemSlot::Legs,
            ItemSlot::ClassItem,
        ];
        for (hash, slot) in EQUIPMENT_BUCKETS.into_iter().zip(expected) {
            let bucket = BucketHash::new(i64::from(hash));
            assert_eq!(ItemSlot::from_equipment_bucket(bucket), Some(slot));
            assert_eq!(policy.capacity(bucket), Some(10));
        }
    }

    #[test]
    fn matching_bucket_only_index_qualifies_independently_of_item_plan() {
        let payload = bytes(&index());
        let evidence = decode_equipment_capacity_evidence(&payload).unwrap();
        assert_eq!(
            evidence.definition_version().as_str(),
            PINNED_EQUIPMENT_CAPACITY_VERSION
        );
        assert_eq!(
            evidence.bucket_artifact_identity(),
            PINNED_EQUIPMENT_BUCKET_PATH
        );
        assert!(qualify_pinned_equipment_capacity(&payload).is_ok());
        assert!(crate::manifest::decode_manifest_index(&payload).is_err());
    }

    #[test]
    fn changed_version_or_safe_artifact_refuses_qualification_not_decoding() {
        for field in ["version", "artifact"] {
            let mut value = index();
            if field == "version" {
                value["Response"]["version"] = json!("new-version");
            } else {
                value["Response"]["jsonWorldComponentContentPaths"]["en"]["DestinyInventoryBucketDefinition"] =
                    json!("/common/destiny2_content/json/en/new.json");
            }
            let payload = bytes(&value);
            assert!(decode_equipment_capacity_evidence(&payload).is_ok());
            let Err(EquipmentCapacityQualificationError::Qualification(error)) =
                qualify_pinned_equipment_capacity(&payload)
            else {
                panic!("expected correspondence refusal")
            };
            assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        }
    }

    #[test]
    fn missing_unsafe_paths_and_bad_envelopes_refuse_decoding() {
        let mut cases = vec![
            json!(null),
            json!([]),
            json!({}),
            json!({"ErrorCode":1,"Response":[]}),
        ];
        for code in [json!(2), json!("1"), json!(null)] {
            let mut value = index();
            value["ErrorCode"] = code;
            cases.push(value);
        }
        for version in ["", "bad version", "bad\nversion", &"v".repeat(129)] {
            let mut value = index();
            value["Response"]["version"] = json!(version);
            cases.push(value);
        }
        for path in [
            Value::Null,
            json!("https://example.invalid/buckets.json"),
            json!("/common/destiny2_content/../bad.json"),
            json!("/common/destiny2_content/%2e%2e/bad.json"),
        ] {
            let mut value = index();
            value["Response"]["jsonWorldComponentContentPaths"]["en"]["DestinyInventoryBucketDefinition"] =
                path;
            cases.push(value);
        }
        let mut missing = index();
        missing["Response"]["jsonWorldComponentContentPaths"]["en"] = json!({});
        cases.push(missing);
        for value in cases {
            let payload = bytes(&value);
            assert_eq!(
                decode_equipment_capacity_evidence(&payload)
                    .unwrap_err()
                    .category(),
                ErrorCategory::ProviderValidation
            );
            assert!(matches!(
                qualify_pinned_equipment_capacity(&payload),
                Err(EquipmentCapacityQualificationError::Decode(_))
            ));
        }
        assert!(decode_equipment_capacity_evidence(b"not json").is_err());
    }
}
