//! Pure mapping from Bungie Manifest payloads to the provider-neutral Manifest
//! contracts in `destiny-core`.
//!
//! This module performs no I/O, no scheduling, and no normalization beyond the
//! documented projection. [`decode_manifest_index`] validates one index
//! envelope into a download plan, and [`decode_inventory_items`] validates one
//! English `DestinyInventoryItemDefinition` component into untrusted definition
//! candidates. The application Manifest service still decides whether the
//! candidates are bounded, complete, and internally consistent, and it owns
//! ordering.
//!
//! Input is handled as `serde_json::Value` only; the adapter never deserializes
//! into provider structs. Optional fields whose value is unknown or wrongly
//! typed are dropped rather than rejected. Every failure is a sanitized
//! `ErrorCategory::ProviderValidation` error, so no upstream version, path,
//! hash, or identifier can escape through the failure path.
//!
//! Defense-in-depth guards tighten the accepted surface beyond the minimum
//! contract: control characters are rejected in the version and component path,
//! percent-encoding is rejected in the component path so a later URL decode
//! cannot revive encoded traversal or separators, and decimal hash keys must be
//! canonical so leading-zero aliases cannot yield duplicate semantic hashes.

use destiny_core::{
    AppError, BucketHash, DefinitionVersion, ErrorCategory, ItemHash, ManifestAcquisition,
    ManifestDefinitionCandidate,
};
use serde_json::{Map, Value};

/// The only success code Bungie returns for a fulfilled request.
const SUCCESS_ERROR_CODE: u64 = 1;

/// Upper bound on the accepted manifest version length.
const MAX_VERSION_CHARS: usize = 128;

/// The content-path prefix every accepted component path must use.
const COMPONENT_PATH_PREFIX: &str = "/common/destiny2_content/";

/// The content-path suffix every accepted component path must use.
const COMPONENT_PATH_SUFFIX: &str = ".json";

/// Upper bound on the number of records one inventory component may contain.
const MAX_INVENTORY_RECORDS: usize = 100_000;

fn provider_validation() -> AppError {
    AppError::new(ErrorCategory::ProviderValidation)
}

/// One validated Manifest index, naming the component to download next.
///
/// The version is the exact string the provider reported, and the path is a
/// validated relative content path: safe for a caller to fetch, never a URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestDownloadPlan {
    pub version: DefinitionVersion,
    pub component_path: String,
}

/// Validates a Bungie Manifest index envelope into a download plan.
///
/// The envelope must report `ErrorCode == 1`, carry a non-empty, whitespace-
/// and control-free version of at most 128 characters, and expose a safe
/// English `DestinyInventoryItemDefinition` content path. Any deviation is a
/// sanitized provider-validation failure.
pub fn decode_manifest_index(bytes: &[u8]) -> Result<ManifestDownloadPlan, AppError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| provider_validation())?;
    let root = value.as_object().ok_or_else(provider_validation)?;

    if root.get("ErrorCode").and_then(Value::as_u64) != Some(SUCCESS_ERROR_CODE) {
        return Err(provider_validation());
    }

    let response = root
        .get("Response")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;

    let version = decode_version(response.get("version"))?;
    let component_path = decode_component_path(response)?;

    Ok(ManifestDownloadPlan {
        version,
        component_path,
    })
}

/// Maps one validated inventory component into untrusted definition candidates.
///
/// The component must be a non-empty JSON object with at most 100,000 records.
/// Each key is a canonical decimal `u32`; each record is an object whose numeric
/// `hash` equals its key and whose `displayProperties.name` is non-blank. The supplied
/// `version` is attached to every candidate unchanged, and candidates are
/// emitted in object iteration order: this function never sorts.
pub fn decode_inventory_items(
    version: DefinitionVersion,
    bytes: &[u8],
) -> Result<ManifestAcquisition, AppError> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| provider_validation())?;
    let records = value.as_object().ok_or_else(provider_validation)?;

    if records.is_empty() || records.len() > MAX_INVENTORY_RECORDS {
        return Err(provider_validation());
    }

    let mut definitions = Vec::with_capacity(records.len());
    for (key, record) in records {
        definitions.push(decode_record(key, record, &version)?);
    }

    Ok(ManifestAcquisition {
        version,
        definitions,
    })
}

fn decode_version(value: Option<&Value>) -> Result<DefinitionVersion, AppError> {
    let raw = value
        .and_then(Value::as_str)
        .ok_or_else(provider_validation)?;

    if raw.is_empty()
        || raw.chars().count() > MAX_VERSION_CHARS
        || raw
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(provider_validation());
    }

    Ok(DefinitionVersion::new(raw))
}

fn decode_component_path(response: &Map<String, Value>) -> Result<String, AppError> {
    let path = response
        .get("jsonWorldComponentContentPaths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get("en"))
        .and_then(Value::as_object)
        .and_then(|en| en.get("DestinyInventoryItemDefinition"))
        .and_then(Value::as_str)
        .ok_or_else(provider_validation)?;

    if !is_safe_component_path(path) {
        return Err(provider_validation());
    }

    Ok(path.to_owned())
}

/// Accepts only a relative content path under the fixed Destiny 2 prefix.
///
/// A path with a scheme, query, fragment, Windows separator, percent-encoding,
/// control character, or a `..` path segment is rejected so the plan can never
/// escape the content root, even if a later consumer URL-decodes it.
fn is_safe_component_path(path: &str) -> bool {
    path.starts_with(COMPONENT_PATH_PREFIX)
        && path.ends_with(COMPONENT_PATH_SUFFIX)
        && !path.contains("://")
        && !path.contains('?')
        && !path.contains('#')
        && !path.contains('%')
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && !path.split('/').any(|segment| segment == "..")
}

fn decode_record(
    key: &str,
    record: &Value,
    version: &DefinitionVersion,
) -> Result<ManifestDefinitionCandidate, AppError> {
    let key_value = decode_decimal_u32(key)?;
    let record = record.as_object().ok_or_else(provider_validation)?;

    if record.get("hash").and_then(Value::as_u64) != Some(u64::from(key_value)) {
        return Err(provider_validation());
    }

    let name = record
        .get("displayProperties")
        .and_then(Value::as_object)
        .and_then(|properties| properties.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(provider_validation)?;

    let inventory = record.get("inventory").and_then(Value::as_object);
    let bucket_hash = inventory
        .and_then(|inventory| inventory.get("bucketTypeHash"))
        .and_then(Value::as_u64)
        .and_then(|bucket| u32::try_from(bucket).ok())
        .map(|bucket| BucketHash::new(i64::from(bucket)));
    let tier = inventory
        .and_then(|inventory| inventory.get("tierTypeName"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(ManifestDefinitionCandidate {
        hash: ItemHash::new(i64::from(key_value)),
        bucket_hash,
        name: name.to_owned(),
        item_type: record
            .get("itemTypeDisplayName")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tier,
        element: None,
        tags: Vec::new(),
        version: version.clone(),
    })
}

/// Requires a canonical unsigned decimal representation within `u32` range.
///
/// Rejecting non-canonical keys such as `"01"` or `"007"` prevents leading-zero
/// aliases from mapping two distinct keys onto the same semantic hash.
fn decode_decimal_u32(key: &str) -> Result<u32, AppError> {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(provider_validation());
    }

    key.parse::<u32>()
        .ok()
        .filter(|value| value.to_string() == key)
        .ok_or_else(provider_validation)
}

#[cfg(test)]
mod tests {
    use super::{decode_inventory_items, decode_manifest_index};
    use destiny_core::{AppError, BucketHash, DefinitionVersion, ErrorCategory};
    use serde_json::json;

    const VALID_PATH: &str =
        "/common/destiny2_content/json/en/DestinyInventoryItemDefinition-abc.json";

    fn version() -> DefinitionVersion {
        DefinitionVersion::new("manifest-v1")
    }

    fn index_bytes(version: &str, path: &str) -> Vec<u8> {
        json!({
            "ErrorCode": 1,
            "Response": {
                "version": version,
                "jsonWorldComponentContentPaths": {
                    "en": { "DestinyInventoryItemDefinition": path }
                }
            }
        })
        .to_string()
        .into_bytes()
    }

    fn assert_validation<T: std::fmt::Debug>(result: Result<T, AppError>) {
        let error = result.expect_err("expected a provider-validation failure");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
    }

    #[test]
    fn valid_index_produces_download_plan() {
        let plan = decode_manifest_index(&index_bytes("manifest-2024-01-01", VALID_PATH))
            .expect("valid index");
        assert_eq!(plan.version.as_str(), "manifest-2024-01-01");
        assert_eq!(plan.component_path, VALID_PATH);
    }

    #[test]
    fn unsafe_component_paths_are_rejected() {
        let attacks = [
            "",
            "/etc/passwd.json",
            "https://evil.example/common/destiny2_content/x.json",
            "/common/destiny2_content/a://b.json",
            "/common/destiny2_content/a?b.json",
            "/common/destiny2_content/a#b.json",
            "/common/destiny2_content/a\\b.json",
            "/common/destiny2_content/../secret.json",
            "/common/destiny2_content/x.txt",
            "//common/destiny2_content/x.json",
            "/common/destiny2_content/%2e%2e/secret.json",
            "/common/destiny2_content/a%2fb.json",
            "/common/destiny2_content/a\u{1}b.json",
        ];

        for path in attacks {
            assert_validation(decode_manifest_index(&index_bytes("manifest-v1", path)));
        }

        assert!(decode_manifest_index(&index_bytes("manifest-v1", VALID_PATH)).is_ok());
    }

    #[test]
    fn index_envelope_failures_are_provider_validation() {
        assert_validation(decode_manifest_index(b"{not json"));
        assert_validation(decode_manifest_index(b"[]"));
        assert_validation(decode_manifest_index(b"{}"));

        let error_code_zero = json!({
            "ErrorCode": 0,
            "Response": {
                "version": "manifest-v1",
                "jsonWorldComponentContentPaths": {
                    "en": { "DestinyInventoryItemDefinition": VALID_PATH }
                }
            }
        });
        assert_validation(decode_manifest_index(
            error_code_zero.to_string().as_bytes(),
        ));

        let error_code_string = json!({
            "ErrorCode": "1",
            "Response": {
                "version": "manifest-v1",
                "jsonWorldComponentContentPaths": {
                    "en": { "DestinyInventoryItemDefinition": VALID_PATH }
                }
            }
        });
        assert_validation(decode_manifest_index(
            error_code_string.to_string().as_bytes(),
        ));

        let missing_response = json!({ "ErrorCode": 1 });
        assert_validation(decode_manifest_index(
            missing_response.to_string().as_bytes(),
        ));

        let missing_paths = json!({ "ErrorCode": 1, "Response": { "version": "manifest-v1" } });
        assert_validation(decode_manifest_index(missing_paths.to_string().as_bytes()));

        let missing_definition = json!({
            "ErrorCode": 1,
            "Response": {
                "version": "manifest-v1",
                "jsonWorldComponentContentPaths": { "en": {} }
            }
        });
        assert_validation(decode_manifest_index(
            missing_definition.to_string().as_bytes(),
        ));
    }

    #[test]
    fn index_version_failures_are_provider_validation() {
        let over_limit = "v".repeat(129);
        for invalid in [
            "",
            " ",
            "  ",
            "man ifest",
            "man\u{1}ifest",
            over_limit.as_str(),
        ] {
            assert_validation(decode_manifest_index(&index_bytes(invalid, VALID_PATH)));
        }

        let boundary = "v".repeat(128);
        let plan = decode_manifest_index(&index_bytes(&boundary, VALID_PATH))
            .expect("128-character version is accepted");
        assert_eq!(plan.version.as_str(), boundary);

        let numeric_version = json!({
            "ErrorCode": 1,
            "Response": {
                "version": 42,
                "jsonWorldComponentContentPaths": {
                    "en": { "DestinyInventoryItemDefinition": VALID_PATH }
                }
            }
        });
        assert_validation(decode_manifest_index(
            numeric_version.to_string().as_bytes(),
        ));
    }

    #[test]
    fn valid_component_maps_candidates() {
        let component = json!({
            "2522695": {
                "hash": 2522695,
                "displayProperties": { "name": "Suros Regime" },
                "itemTypeDisplayName": "Auto Rifle",
                "inventory": {
                    "bucketTypeHash": 1498876634,
                    "tierTypeName": "Exotic"
                }
            },
            "999": {
                "hash": 999,
                "displayProperties": { "name": "Ghost Shell" }
            }
        });

        let acquisition = decode_inventory_items(version(), component.to_string().as_bytes())
            .expect("valid component");

        assert_eq!(acquisition.version.as_str(), "manifest-v1");
        assert_eq!(acquisition.definitions.len(), 2);

        let rifle = acquisition
            .definitions
            .iter()
            .find(|definition| definition.hash.value() == 2522695)
            .expect("rifle candidate");
        assert_eq!(rifle.name, "Suros Regime");
        assert_eq!(rifle.item_type.as_deref(), Some("Auto Rifle"));
        assert_eq!(rifle.tier.as_deref(), Some("Exotic"));
        assert_eq!(
            rifle.bucket_hash.map(BucketHash::value),
            Some(1_498_876_634)
        );
        assert_eq!(rifle.element, None);
        assert!(rifle.tags.is_empty());
        assert_eq!(rifle.version.as_str(), "manifest-v1");

        let ghost = acquisition
            .definitions
            .iter()
            .find(|definition| definition.hash.value() == 999)
            .expect("ghost candidate");
        assert_eq!(ghost.name, "Ghost Shell");
        assert_eq!(ghost.item_type, None);
        assert_eq!(ghost.bucket_hash, None);
        assert_eq!(ghost.tier, None);
    }

    #[test]
    fn component_shape_failures_are_provider_validation() {
        let cases: [&[u8]; 14] = [
            b"[]",
            b"{",
            b"{}",
            br#"{"abc":{"hash":1,"displayProperties":{"name":"x"}}}"#,
            br#"{"-1":{"hash":1,"displayProperties":{"name":"x"}}}"#,
            br#"{"4294967296":{"hash":4294967296,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":[]}"#,
            br#"{"1":{"hash":2,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"hash":1.5,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"   "}}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":5}}}"#,
            br#"{"01":{"hash":1,"displayProperties":{"name":"x"}}}"#,
            br#"{"007":{"hash":7,"displayProperties":{"name":"x"}}}"#,
        ];

        for bytes in cases {
            assert_validation(decode_inventory_items(version(), bytes));
        }

        assert_validation(decode_inventory_items(
            version(),
            br#"{"1":{"hash":1,"displayProperties":{}}}"#,
        ));

        let canonical_zero = decode_inventory_items(
            version(),
            br#"{"0":{"hash":0,"displayProperties":{"name":"canonical"}}}"#,
        )
        .expect("canonical zero key is accepted");
        assert_eq!(canonical_zero.definitions[0].hash.value(), 0);
    }

    #[test]
    fn component_record_count_boundaries_are_enforced() {
        let at_limit = build_component(100_000);
        let acquisition =
            decode_inventory_items(version(), at_limit.as_bytes()).expect("boundary component");
        assert_eq!(acquisition.definitions.len(), 100_000);

        let over_limit = build_component(100_001);
        assert_validation(decode_inventory_items(version(), over_limit.as_bytes()));
    }

    #[test]
    fn unknown_or_wrong_typed_optional_fields_are_dropped() {
        let component = json!({
            "7": {
                "hash": 7,
                "displayProperties": { "name": "Tolerant", "extra": "ignored" },
                "itemTypeDisplayName": 12,
                "inventory": {
                    "bucketTypeHash": "not-a-number",
                    "tierTypeName": ["array"],
                    "other": 1
                },
                "element": "Solar",
                "tags": ["should-not-map"],
                "instanceId": "sentinel-instance"
            },
            "8": {
                "hash": 8,
                "displayProperties": { "name": "No Inventory" },
                "inventory": "wrong-type"
            },
            "9": {
                "hash": 9,
                "displayProperties": { "name": "Out Of Range" },
                "inventory": { "bucketTypeHash": 4294967296_u64 }
            }
        });

        let acquisition =
            decode_inventory_items(version(), component.to_string().as_bytes()).expect("tolerant");

        let candidate = acquisition
            .definitions
            .iter()
            .find(|definition| definition.hash.value() == 7)
            .expect("tolerant candidate");
        assert_eq!(candidate.name, "Tolerant");
        assert_eq!(candidate.item_type, None);
        assert_eq!(candidate.bucket_hash, None);
        assert_eq!(candidate.tier, None);
        assert_eq!(candidate.element, None);
        assert!(candidate.tags.is_empty());
        assert_eq!(candidate.version.as_str(), "manifest-v1");

        let no_inventory = acquisition
            .definitions
            .iter()
            .find(|definition| definition.hash.value() == 8)
            .expect("no-inventory candidate");
        assert_eq!(no_inventory.bucket_hash, None);
        assert_eq!(no_inventory.tier, None);

        let out_of_range = acquisition
            .definitions
            .iter()
            .find(|definition| definition.hash.value() == 9)
            .expect("out-of-range candidate");
        assert_eq!(out_of_range.bucket_hash, None);
    }

    #[test]
    fn failures_never_leak_input_sentinels() {
        let sentinel = "sentinel-secret-value";
        let index = json!({
            "ErrorCode": 0,
            "Response": {
                "version": sentinel,
                "jsonWorldComponentContentPaths": {
                    "en": {
                        "DestinyInventoryItemDefinition": format!("/common/destiny2_content/{sentinel}.json")
                    }
                }
            }
        });
        let error = decode_manifest_index(index.to_string().as_bytes())
            .expect_err("index with a bad error code");
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));

        let component = json!({
            "1": { "hash": 2, "displayProperties": { "name": sentinel } }
        });
        let error = decode_inventory_items(version(), component.to_string().as_bytes())
            .expect_err("component with a hash mismatch");
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }

    fn build_component(records: usize) -> String {
        let mut body = String::from("{");
        for index in 0..records {
            if index > 0 {
                body.push(',');
            }
            body.push_str(&format!(
                r#""{index}":{{"hash":{index},"displayProperties":{{"name":"item-{index}"}}}}"#
            ));
        }
        body.push('}');
        body
    }
}
