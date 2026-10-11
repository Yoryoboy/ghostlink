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
//! The small index uses `serde_json::Value`; components stream through typed,
//! field-limited records. Optional fields whose value is unknown or wrongly
//! typed are dropped rather than rejected. Every failure is a sanitized
//! `ErrorCategory::ProviderValidation` error, so no upstream version, path,
//! hash, or identifier can escape through the failure path.
//!
//! Defense-in-depth guards tighten the accepted surface beyond the minimum
//! contract: control characters are rejected in the version and component path,
//! percent-encoding is rejected in the component path so a later URL decode
//! cannot revive encoded traversal or separators, and decimal hash keys must be
//! canonical so leading-zero aliases cannot yield duplicate semantic hashes.

pub use crate::public_milestones::decode_public_milestones;

/// Bounded, field-limited armor catalogs; redacted rows are omitted.
pub fn decode_armor_catalog(
    kind: destiny_core::armor_catalog::ArmorCatalogKind,
    bytes: &[u8],
) -> Result<Vec<destiny_core::armor_catalog::ArmorCatalogRecord>, AppError> {
    decode_armor_catalog_reader(kind, bytes)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatGroupWire {
    hash: u32,
    maximum_value: Option<i32>,
    #[serde(default)]
    redacted: bool,
    scaled_stats: Option<Vec<ScaledStatWire>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EquipableSetWire {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    display_properties: Option<CatalogNameWire>,
    set_items: Option<Vec<u32>>,
    set_perks: Option<Vec<SetPerkWire>>,
}
#[derive(Deserialize)]
struct CatalogNameWire {
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScaledStatWire {
    stat_hash: u32,
    maximum_value: i32,
    display_as_numeric: bool,
    display_interpolation: Vec<InterpolationWire>,
}
#[derive(Deserialize)]
struct InterpolationWire {
    value: i32,
    weight: i32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPerkWire {
    required_set_count: i32,
    sandbox_perk_hash: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SocketCatalogWire {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    socket_category_hash: Option<u32>,
    always_randomize_sockets: Option<bool>,
    plug_whitelist: Option<Vec<WhitelistWire>>,
    category_style: Option<u32>,
    display_properties: Option<CatalogNameWire>,
    reusable_plug_items: Option<Vec<PlugSetItemWire>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WhitelistWire {
    category_hash: u32,
    category_identifier: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlugSetItemWire {
    plug_item_hash: u32,
    currently_can_roll: bool,
}
struct ArmorCatalogSeed(destiny_core::armor_catalog::ArmorCatalogKind);
impl<'de> DeserializeSeed<'de> for ArmorCatalogSeed {
    type Value = Vec<destiny_core::armor_catalog::ArmorCatalogRecord>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for ArmorCatalogSeed {
    type Value = Vec<destiny_core::armor_catalog::ArmorCatalogRecord>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("armor catalog")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        use destiny_core::armor_catalog::*;
        let mut seen = std::collections::HashSet::new();
        let mut records = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            let hash =
                decode_decimal_u32(&key).map_err(|_| de::Error::custom("invalid catalog"))?;
            if !seen.insert(hash) || seen.len() > self.0.max_records() {
                return Err(de::Error::custom("invalid catalog"));
            }
            let missing = || de::Error::custom("invalid catalog");
            let record = match self.0 {
                ArmorCatalogKind::SocketType
                | ArmorCatalogKind::SocketCategory
                | ArmorCatalogKind::PlugSet => {
                    let wire = map.next_value::<SocketCatalogWire>()?;
                    if wire.hash != hash {
                        return Err(missing());
                    }
                    if wire.redacted {
                        continue;
                    }
                    match self.0 {
                        ArmorCatalogKind::SocketType => {
                            ArmorCatalogRecord::SocketType(SocketTypeDefinition {
                                hash,
                                socket_category_hash: wire
                                    .socket_category_hash
                                    .ok_or_else(missing)?,
                                always_randomize_sockets: wire
                                    .always_randomize_sockets
                                    .ok_or_else(missing)?,
                                plug_whitelist: wire
                                    .plug_whitelist
                                    .ok_or_else(missing)?
                                    .into_iter()
                                    .map(|entry| PlugWhitelistEntry {
                                        category_hash: entry.category_hash,
                                        category_identifier: entry.category_identifier,
                                    })
                                    .collect(),
                            })
                        }
                        ArmorCatalogKind::SocketCategory => {
                            ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
                                hash,
                                category_style: wire.category_style.ok_or_else(missing)?,
                                name: wire.display_properties.ok_or_else(missing)?.name,
                            })
                        }
                        ArmorCatalogKind::PlugSet => {
                            ArmorCatalogRecord::PlugSet(PlugSetDefinition {
                                hash,
                                reusable_plug_items: wire
                                    .reusable_plug_items
                                    .ok_or_else(missing)?
                                    .into_iter()
                                    .map(|entry| PlugSetItem {
                                        plug_item_hash: entry.plug_item_hash,
                                        currently_can_roll: entry.currently_can_roll,
                                    })
                                    .collect(),
                            })
                        }
                        _ => unreachable!(),
                    }
                }
                ArmorCatalogKind::StatGroup => {
                    let wire = map.next_value::<StatGroupWire>()?;
                    if wire.hash != hash {
                        return Err(missing());
                    }
                    if wire.redacted {
                        continue;
                    }
                    ArmorCatalogRecord::StatGroup(StatGroupDefinition {
                        hash,
                        maximum_value: wire.maximum_value,
                        scaled_stats: wire
                            .scaled_stats
                            .ok_or_else(missing)?
                            .into_iter()
                            .map(|stat| ScaledStat {
                                stat_hash: stat.stat_hash,
                                maximum_value: stat.maximum_value,
                                display_as_numeric: stat.display_as_numeric,
                                display_interpolation: stat
                                    .display_interpolation
                                    .into_iter()
                                    .map(|point| InterpolationPoint {
                                        value: point.value,
                                        weight: point.weight,
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                }
                ArmorCatalogKind::EquipableSet => {
                    let wire = map.next_value::<EquipableSetWire>()?;
                    if wire.hash != hash {
                        return Err(missing());
                    }
                    if wire.redacted {
                        continue;
                    }
                    ArmorCatalogRecord::EquipableSet(EquipableSetDefinition {
                        hash,
                        name: wire.display_properties.ok_or_else(missing)?.name,
                        set_items: wire.set_items.ok_or_else(missing)?,
                        set_perks: wire
                            .set_perks
                            .ok_or_else(missing)?
                            .into_iter()
                            .map(|perk| SetPerk {
                                required_set_count: perk.required_set_count,
                                sandbox_perk_hash: perk.sandbox_perk_hash,
                            })
                            .collect(),
                    })
                }
            };
            record
                .validate()
                .map_err(|_| de::Error::custom("invalid catalog"))?;
            records.push(record);
        }
        Ok(records)
    }
}
fn decode_armor_catalog_reader(
    kind: destiny_core::armor_catalog::ArmorCatalogKind,
    body: impl Read,
) -> Result<Vec<destiny_core::armor_catalog::ArmorCatalogRecord>, AppError> {
    let mut bounded = BoundedReader {
        inner: body,
        remaining: max_armor_catalog_bytes(kind),
        exceeded: false,
        failed: false,
    };
    let result = {
        let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(&mut bounded));
        ArmorCatalogSeed(kind)
            .deserialize(&mut deserializer)
            .and_then(|records| {
                deserializer.end()?;
                Ok(records)
            })
    };
    result.map_err(|_| {
        if bounded.failed && !bounded.exceeded {
            provider_unavailable()
        } else {
            provider_validation()
        }
    })
}
/// Live probe 2026-10-11: plug sets were 10.4 MB; allow seasonal growth to 64 MiB.
/// Other field-limited catalogs retain the 16 MiB decompressed safety ceiling.
const fn max_armor_catalog_bytes(kind: destiny_core::armor_catalog::ArmorCatalogKind) -> usize {
    match kind {
        destiny_core::armor_catalog::ArmorCatalogKind::PlugSet => 64 * 1024 * 1024,
        _ => 16 * 1024 * 1024,
    }
}

#[cfg(test)]
mod loadout_name_tests {
    use super::*;
    #[test]
    fn loadout_names_use_top_level_name_and_validate_records() {
        let table = decode_loadout_names(br#"{"3192132835":{"hash":3192132835,"index":0,"redacted":false,"name":"PvE","displayProperties":{"name":"Wrong"}},"2":{"hash":2,"name":" "},"3":{"hash":3,"redacted":true}}"#).unwrap();
        assert_eq!(table.definitions[0].hash(), 3192132835);
        assert_eq!(table.definitions[0].name(), "PvE");
        assert_eq!(table.skipped_count, 2);
        for invalid in [
            br#"{"1":{"hash":2,"name":"PvE"}}"#.as_slice(),
            br#"{"01":{"hash":1,"name":"PvE"}}"#,
            br#"{"1":{"hash":1,"name":3}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"PvE"}}}"#,
            br#"{"1":{"hash":1,"name":"PvE"},"1":{"hash":1,"name":"PvP"}}"#,
        ] {
            assert!(decode_loadout_names(invalid).is_err());
        }
        let oversized = format!("{{\"1\":{{\"hash\":1,\"name\":\"{}\"}}}}", "x".repeat(257));
        assert!(decode_loadout_names(oversized.as_bytes()).is_err());
        let records: serde_json::Map<String, Value> = (0..1000)
            .map(|hash| {
                (
                    hash.to_string(),
                    serde_json::json!({"hash":hash,"name":"PvE"}),
                )
            })
            .collect();
        assert_eq!(
            decode_loadout_names(&serde_json::to_vec(&records).unwrap())
                .unwrap()
                .definitions
                .len(),
            1000
        );
        let mut over_cap = records;
        over_cap.insert("1000".into(), serde_json::json!({"hash":1000,"name":"PvE"}));
        assert!(decode_loadout_names(&serde_json::to_vec(&over_cap).unwrap()).is_err());
    }
    #[test]
    fn manifest_plan_includes_loadout_name_path() {
        let mut en = serde_json::Map::new();
        for component in [
            "DestinyInventoryItemDefinition",
            "DestinyMilestoneDefinition",
            "DestinyActivityDefinition",
            "DestinyActivityModifierDefinition",
            "DestinyObjectiveDefinition",
            "DestinyActivityTypeDefinition",
            "DestinyStatDefinition",
            "DestinyLoadoutNameDefinition",
            "DestinySandboxPerkDefinition",
            "DestinyStatGroupDefinition",
            "DestinyEquipableItemSetDefinition",
            "DestinySocketTypeDefinition",
            "DestinySocketCategoryDefinition",
            "DestinyPlugSetDefinition",
        ] {
            en.insert(
                component.into(),
                Value::String("/common/destiny2_content/names.json".into()),
            );
        }
        let mut index = serde_json::json!({"ErrorCode":1,"Response":{"version":"v1","jsonWorldComponentContentPaths":{"en":en}}});
        assert_eq!(
            decode_manifest_index(&serde_json::to_vec(&index).unwrap())
                .unwrap()
                .loadout_name_path,
            "/common/destiny2_content/names.json"
        );
        index["Response"]["jsonWorldComponentContentPaths"]["en"]["DestinyLoadoutNameDefinition"] =
            Value::String("/common/destiny2_content/../bad.json".into());
        assert!(decode_manifest_index(&serde_json::to_vec(&index).unwrap()).is_err());
    }
}

use destiny_core::ports::{
    ReadFailure, ReadRetry, ReadRetryScope, RetryAfterHeader, parse_retry_after_header_bytes,
};
use destiny_core::{
    AppError, BucketHash, Clock, DefinitionVersion, ErrorCategory, ItemHash, ManifestAcquisition,
    ManifestDefinitionCandidate, ManifestGateway, Timestamp,
};
use serde::{
    Deserialize,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{
    fmt,
    io::{self, BufReader, Read},
    time::Duration,
};

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
    pub armor_catalog_paths: [String; 5],
    pub version: DefinitionVersion,
    pub component_path: String,
    pub milestone_path: String,
    pub activity_path: String,
    pub modifier_path: String,
    pub objective_path: String,
    pub activity_type_path: String,
    pub stat_path: String,
    pub loadout_name_path: String,
    pub sandbox_perk_path: String,
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

    crate::bungie_errors::read_error(root)?;

    let response = root
        .get("Response")
        .and_then(Value::as_object)
        .ok_or_else(provider_validation)?;

    let version = decode_version(response.get("version"))?;
    let component_path = decode_component_path(response, "DestinyInventoryItemDefinition")?;
    let milestone_path = decode_component_path(response, "DestinyMilestoneDefinition")?;
    let activity_path = decode_component_path(response, "DestinyActivityDefinition")?;
    let modifier_path = decode_component_path(response, "DestinyActivityModifierDefinition")?;

    Ok(ManifestDownloadPlan {
        armor_catalog_paths: [
            decode_component_path(response, "DestinyStatGroupDefinition")?,
            decode_component_path(response, "DestinyEquipableItemSetDefinition")?,
            decode_component_path(response, "DestinySocketTypeDefinition")?,
            decode_component_path(response, "DestinySocketCategoryDefinition")?,
            decode_component_path(response, "DestinyPlugSetDefinition")?,
        ],
        version,
        component_path,
        milestone_path,
        activity_path,
        modifier_path,
        objective_path: decode_component_path(response, "DestinyObjectiveDefinition")?,
        activity_type_path: decode_component_path(response, "DestinyActivityTypeDefinition")?,
        stat_path: decode_component_path(response, "DestinyStatDefinition")?,
        loadout_name_path: decode_component_path(response, "DestinyLoadoutNameDefinition")?,
        sandbox_perk_path: decode_component_path(response, "DestinySandboxPerkDefinition")?,
    })
}

/// Maps one validated inventory component into untrusted definition candidates.
///
/// The component must be a non-empty JSON object with at most 100,000 records.
/// Each key is a canonical decimal `u32`; each record is an object whose numeric
/// `hash` equals its key. Blank-name and redacted records are skipped and counted;
/// at least one named record must remain. The supplied `version` is attached to
/// every candidate unchanged. Records stream in payload order without sorting.
pub fn decode_inventory_items(
    version: DefinitionVersion,
    bytes: &[u8],
) -> Result<ManifestAcquisition, AppError> {
    decode_inventory_reader(version, bytes, MAX_COMPONENT_BYTES)
}

/// Counts bytes delivered after transport decompression, including ignored fields
/// and trailing whitespace. A cap breach is distinct from a transfer failure.
struct BoundedReader<R> {
    inner: R,
    remaining: usize,
    exceeded: bool,
    failed: bool,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let length = buffer.len().min(self.remaining.saturating_add(1));
        let count = self
            .inner
            .read(&mut buffer[..length])
            .inspect_err(|_| self.failed = true)?;
        if count > self.remaining {
            self.exceeded = true;
            return Err(io::Error::other("manifest size limit"));
        }
        self.remaining -= count;
        Ok(count)
    }
}

fn decode_inventory_reader(
    version: DefinitionVersion,
    body: impl Read,
    limit: usize,
) -> Result<ManifestAcquisition, AppError> {
    let mut bounded = BoundedReader {
        inner: body,
        remaining: limit,
        exceeded: false,
        failed: false,
    };
    let result = {
        let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(&mut bounded));
        InventorySeed(&version)
            .deserialize(&mut deserializer)
            .and_then(|batch| {
                deserializer.end()?;
                Ok(batch)
            })
    };
    result.map_err(|_| {
        if bounded.failed && !bounded.exceeded {
            provider_unavailable()
        } else {
            provider_validation()
        }
    })
}

// Optional values retain the existing tolerant projection semantics.
#[derive(Deserialize)]
#[serde(untagged)]
enum OptionalField<T> {
    Present(T),
    Ignored(IgnoredAny),
}
impl<T> Default for OptionalField<T> {
    fn default() -> Self {
        Self::Ignored(IgnoredAny)
    }
}
impl<T> OptionalField<T> {
    fn into_option(self) -> Option<T> {
        match self {
            Self::Present(value) => Some(value),
            Self::Ignored(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InventoryRecord {
    #[serde(default)]
    sockets: Option<SocketFields>,
    #[serde(default)]
    investment_stats: Vec<InvestmentStatFields>,
    #[serde(default)]
    stats: Option<StatGroupFields>,
    #[serde(default)]
    equipping_block: Option<EquippingFields>,
    #[serde(default)]
    non_transferrable: Option<bool>,
    hash: u32,
    #[serde(default)]
    perks: Vec<InventoryPerk>,
    #[serde(default)]
    display_properties: OptionalField<DisplayProperties>,
    #[serde(default, deserialize_with = "deserialize_inventory_fields")]
    inventory: Option<InventoryFields>,
    #[serde(default)]
    does_postmaster_pull_have_side_effects: Option<bool>,
    #[serde(default)]
    plug: Option<PlugFields>,
    #[serde(default)]
    item_type_display_name: OptionalField<String>,
    #[serde(default)]
    class_type: OptionalField<u32>,
    #[serde(default)]
    default_damage_type: OptionalField<u32>,
    #[serde(default)]
    redacted: OptionalField<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InventoryPerk {
    #[serde(default)]
    perk_hash: OptionalField<u32>,
}

#[derive(Deserialize)]
struct DisplayProperties {
    name: String,
    #[serde(default)]
    description: OptionalField<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InventoryFields {
    #[serde(default)]
    is_instance_item: Option<bool>,
    #[serde(default)]
    max_stack_size: Option<i32>,
    #[serde(default)]
    stack_unique_label: Option<String>,
    #[serde(default)]
    bucket_type_hash: OptionalField<u32>,
    #[serde(default)]
    tier_type_name: OptionalField<String>,
}

// Keep legacy tolerance for absent/non-object inventory blocks, but never let
// the tolerant outer wrapper turn malformed safety children into unknown values.
fn deserialize_inventory_fields<'de, D: de::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<InventoryFields>, D::Error> {
    let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
    if !raw.get().trim_start().starts_with('{') {
        return Ok(None);
    }
    serde_json::from_str(raw.get())
        .map(Some)
        .map_err(|_| de::Error::custom("invalid manifest"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlugFields {
    #[serde(default)]
    plug_category_identifier: Option<String>,
    #[serde(default)]
    plug_category_hash: Option<u32>,
    #[serde(default)]
    energy_cost: Option<EnergyCostFields>,
    #[serde(default)]
    energy_capacity: Option<CapacityFields>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SocketFields {
    socket_entries: Vec<SocketEntryFields>,
    socket_categories: Vec<SocketCategoryFields>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SocketEntryFields {
    socket_type_hash: u32,
    single_initial_item_hash: u32,
    #[serde(default)]
    reusable_plug_set_hash: Option<u32>,
    #[serde(default)]
    randomized_plug_set_hash: Option<u32>,
    #[serde(default)]
    reusable_plug_items: Vec<ReusablePlugFields>,
    #[serde(default)]
    plug_sources: Option<u32>,
    #[serde(default)]
    default_visible: Option<bool>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReusablePlugFields {
    plug_item_hash: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SocketCategoryFields {
    socket_category_hash: u32,
    socket_indexes: Vec<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InvestmentStatFields {
    stat_type_hash: u32,
    value: i32,
    is_conditionally_active: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatGroupFields {
    #[serde(default)]
    stat_group_hash: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EquippingFields {
    #[serde(default)]
    equipable_item_set_hash: Option<u32>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnergyCostFields {
    energy_cost: i32,
    energy_type: u32,
    energy_type_hash: u32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapacityFields {
    capacity_value: i32,
}

struct InventorySeed<'a>(&'a DefinitionVersion);
impl<'de> DeserializeSeed<'de> for InventorySeed<'_> {
    type Value = ManifestAcquisition;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de> Visitor<'de> for InventorySeed<'_> {
    type Value = ManifestAcquisition;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("inventory definitions")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut definitions = Vec::new();
        let mut skipped_count = 0;
        let mut count = 0;
        while let Some(key) = map.next_key::<String>()? {
            count += 1;
            if count > MAX_INVENTORY_RECORDS {
                return Err(de::Error::custom("invalid manifest"));
            }
            let record = map.next_value::<InventoryRecord>()?;
            match decode_record(&key, record, self.0)
                .map_err(|_| de::Error::custom("invalid manifest"))?
            {
                Some(candidate) => definitions.push(candidate),
                None => skipped_count += 1,
            }
        }
        if definitions.is_empty() {
            return Err(de::Error::custom("empty manifest"));
        }
        Ok(ManifestAcquisition {
            sandbox_perks: Vec::new(),
            version: self.0.clone(),
            definitions,
            skipped_count,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorldRecord {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    #[serde(default)]
    display_properties: Option<WorldDisplay>,
    #[serde(default)]
    milestone_type: u32,
    #[serde(default)]
    activity_type_hash: u32,
    #[serde(default)]
    activity_light_level: i32,
    #[serde(skip)]
    is_displayable: bool,
    #[serde(skip)]
    progress_description: String,
    #[serde(skip)]
    completion_value: i32,
}
#[derive(Deserialize)]
struct WorldDisplay {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
}

trait WorldKind {
    const ALLOW_EMPTY: bool = false;
    const REQUIRES_DISPLAY: bool = true;
    fn is_blank(display: &WorldDisplay) -> bool {
        display.name.trim().is_empty()
    }
    fn read_record<'de, M: MapAccess<'de>>(map: &mut M) -> Result<WorldRecord, M::Error> {
        map.next_value()
    }
}
impl WorldKind for destiny_core::MilestoneDefinition {}
impl WorldKind for destiny_core::ActivityDefinition {}
impl WorldKind for destiny_core::ActivityModifierDefinition {}
impl WorldKind for destiny_core::ActivityTypeDefinition {}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatRecord {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    #[serde(default)]
    display_properties: Option<StatDisplay>,
}
#[derive(Deserialize)]
struct StatDisplay {
    #[serde(default)]
    name: String,
}
impl WorldKind for destiny_core::StatDefinition {
    fn read_record<'de, M: MapAccess<'de>>(map: &mut M) -> Result<WorldRecord, M::Error> {
        let stat = map.next_value::<StatRecord>()?;
        Ok(WorldRecord {
            hash: stat.hash,
            is_displayable: false,
            redacted: stat.redacted,
            display_properties: stat.display_properties.map(|display| WorldDisplay {
                name: display.name,
                description: String::new(),
            }),
            milestone_type: 0,
            activity_type_hash: 0,
            activity_light_level: 0,
            progress_description: String::new(),
            completion_value: 0,
        })
    }
}
fn project_stat(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::StatDefinition, AppError> {
    destiny_core::StatDefinition::new(record.hash, display.name)
}
/// Streams at most 1,000 stat records, retaining only hash and display name.
pub fn decode_stats(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::StatDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 1000, project_stat)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxPerkRecord {
    hash: u32,
    #[serde(default)]
    redacted: OptionalField<bool>,
    #[serde(default)]
    is_displayable: OptionalField<bool>,
    #[serde(default)]
    display_properties: OptionalField<SandboxPerkDisplay>,
}
#[derive(Deserialize)]
struct SandboxPerkDisplay {
    #[serde(default)]
    description: OptionalField<String>,
}
impl WorldKind for destiny_core::ports::SandboxPerkCandidate {
    const ALLOW_EMPTY: bool = true;
    fn is_blank(display: &WorldDisplay) -> bool {
        display.description.trim().is_empty()
    }
    fn read_record<'de, M: MapAccess<'de>>(map: &mut M) -> Result<WorldRecord, M::Error> {
        let record = map.next_value::<SandboxPerkRecord>()?;
        let description = record
            .display_properties
            .into_option()
            .and_then(|display| display.description.into_option())
            .unwrap_or_default();
        Ok(WorldRecord {
            hash: record.hash,
            redacted: record.redacted.into_option() == Some(true),
            is_displayable: record.is_displayable.into_option() == Some(true),
            display_properties: Some(WorldDisplay {
                name: String::new(),
                description,
            }),
            milestone_type: 0,
            activity_type_hash: 0,
            activity_light_level: 0,
            progress_description: String::new(),
            completion_value: 0,
        })
    }
}
fn project_sandbox_perk(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::ports::SandboxPerkCandidate, AppError> {
    Ok(destiny_core::ports::SandboxPerkCandidate {
        hash: record.hash,
        is_displayable: record.is_displayable,
        description: display.description,
    })
}
/// Bounded, field-limited effect decode; blank/redacted records are omitted.
pub fn decode_sandbox_perks(
    bytes: &[u8],
) -> Result<Vec<destiny_core::ports::SandboxPerkCandidate>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 100_000, project_sandbox_perk)
        .map(|table| table.definitions)
}

#[derive(Deserialize)]
struct LoadoutNameRecord {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    #[serde(default)]
    name: String,
}
impl WorldKind for destiny_core::LoadoutNameDefinition {
    fn read_record<'de, M: MapAccess<'de>>(map: &mut M) -> Result<WorldRecord, M::Error> {
        let record = map.next_value::<LoadoutNameRecord>()?;
        Ok(WorldRecord {
            hash: record.hash,
            is_displayable: false,
            redacted: record.redacted,
            display_properties: Some(WorldDisplay {
                name: record.name,
                description: String::new(),
            }),
            milestone_type: 0,
            activity_type_hash: 0,
            activity_light_level: 0,
            progress_description: String::new(),
            completion_value: 0,
        })
    }
}
fn project_loadout_name(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::LoadoutNameDefinition, AppError> {
    destiny_core::LoadoutNameDefinition::new(record.hash, display.name)
}
/// Streams at most 1,000 loadout names from their top-level `name` field.
pub fn decode_loadout_names(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::LoadoutNameDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 1000, project_loadout_name)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObjectiveRecord {
    hash: u32,
    #[serde(default)]
    redacted: bool,
    #[serde(default)]
    progress_description: String,
    #[serde(default)]
    completion_value: i32,
}
impl WorldKind for destiny_core::ObjectiveDefinition {
    const REQUIRES_DISPLAY: bool = false;
    fn read_record<'de, M: MapAccess<'de>>(map: &mut M) -> Result<WorldRecord, M::Error> {
        let record = map.next_value::<ObjectiveRecord>()?;
        Ok(WorldRecord {
            hash: record.hash,
            is_displayable: false,
            redacted: record.redacted,
            display_properties: None,
            milestone_type: 0,
            activity_type_hash: 0,
            activity_light_level: 0,
            progress_description: record.progress_description,
            completion_value: record.completion_value,
        })
    }
}

type WorldProjection<T> = fn(WorldRecord, WorldDisplay) -> Result<T, AppError>;
struct WorldSeed<T> {
    cap: usize,
    project: WorldProjection<T>,
}
impl<'de, T: WorldKind> DeserializeSeed<'de> for WorldSeed<T> {
    type Value = destiny_core::WorldDefinitionTable<T>;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'de, T: WorldKind> Visitor<'de> for WorldSeed<T> {
    type Value = destiny_core::WorldDefinitionTable<T>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("world definitions")
    }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut definitions = Vec::new();
        let mut skipped_count = 0;
        let mut seen = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            let hash =
                decode_decimal_u32(&key).map_err(|_| de::Error::custom("invalid manifest"))?;
            if !seen.insert(hash) || seen.len() > self.cap {
                return Err(de::Error::custom("invalid manifest"));
            }
            let mut record = T::read_record(&mut map)?;
            if record.hash != hash {
                return Err(de::Error::custom("invalid manifest"));
            }
            if record.redacted {
                skipped_count += 1;
                continue;
            }
            let objective = !T::REQUIRES_DISPLAY;
            let display = if objective {
                WorldDisplay {
                    name: String::new(),
                    description: String::new(),
                }
            } else {
                record
                    .display_properties
                    .take()
                    .ok_or_else(|| de::Error::custom("invalid manifest"))?
            };
            if !objective && T::is_blank(&display) {
                skipped_count += 1;
                continue;
            }
            definitions.push(
                (self.project)(record, display)
                    .map_err(|_| de::Error::custom("invalid manifest"))?,
            );
        }
        if definitions.is_empty() && !T::ALLOW_EMPTY {
            return Err(de::Error::custom("empty manifest"));
        }
        Ok(destiny_core::WorldDefinitionTable {
            definitions,
            skipped_count,
        })
    }
}
fn decode_world_reader<T: WorldKind>(
    body: impl Read,
    limit: usize,
    cap: usize,
    project: WorldProjection<T>,
) -> Result<destiny_core::WorldDefinitionTable<T>, AppError> {
    let mut bounded = BoundedReader {
        inner: body,
        remaining: limit,
        exceeded: false,
        failed: false,
    };
    let result = {
        let mut deserializer = serde_json::Deserializer::from_reader(BufReader::new(&mut bounded));
        WorldSeed { cap, project }
            .deserialize(&mut deserializer)
            .and_then(|batch| {
                deserializer.end()?;
                Ok(batch)
            })
    };
    result.map_err(|_| {
        if bounded.failed && !bounded.exceeded {
            provider_unavailable()
        } else {
            provider_validation()
        }
    })
}
fn project_milestone(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::MilestoneDefinition, AppError> {
    destiny_core::MilestoneDefinition::new(
        record.hash,
        display.name,
        display.description,
        record.milestone_type,
    )
}
fn project_activity(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::ActivityDefinition, AppError> {
    destiny_core::ActivityDefinition::new(
        record.hash,
        display.name,
        display.description,
        record.activity_type_hash,
        record.activity_light_level,
    )
}
fn project_modifier(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::ActivityModifierDefinition, AppError> {
    destiny_core::ActivityModifierDefinition::new(record.hash, display.name, display.description)
}
/// Streams a bounded English milestone table.
pub fn decode_milestones(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::MilestoneDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 1000, project_milestone)
}
/// Streams a bounded English activity table.
pub fn decode_activities(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::ActivityDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 50000, project_activity)
}
/// Streams a bounded English activity modifier table; templates remain verbatim.
pub fn decode_activity_modifiers(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::ActivityModifierDefinition>, AppError>
{
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 10000, project_modifier)
}

fn project_objective(
    record: WorldRecord,
    _: WorldDisplay,
) -> Result<destiny_core::ObjectiveDefinition, AppError> {
    destiny_core::ObjectiveDefinition::new(
        record.hash,
        record.progress_description,
        record.completion_value,
    )
}
fn project_activity_type(
    record: WorldRecord,
    display: WorldDisplay,
) -> Result<destiny_core::ActivityTypeDefinition, AppError> {
    destiny_core::ActivityTypeDefinition::new(record.hash, display.name, display.description)
}
/// Streams objectives, retaining empty progress descriptions.
pub fn decode_objectives(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::ObjectiveDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 100000, project_objective)
}
/// Streams a bounded English activity-type table.
pub fn decode_activity_types(
    bytes: &[u8],
) -> Result<destiny_core::WorldDefinitionTable<destiny_core::ActivityTypeDefinition>, AppError> {
    decode_world_reader(bytes, MAX_COMPONENT_BYTES, 1000, project_activity_type)
}

pub(crate) fn decode_version(value: Option<&Value>) -> Result<DefinitionVersion, AppError> {
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

pub(crate) fn decode_component_path(
    response: &Map<String, Value>,
    kind: &str,
) -> Result<String, AppError> {
    let path = response
        .get("jsonWorldComponentContentPaths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get("en"))
        .and_then(Value::as_object)
        .and_then(|en| en.get(kind))
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
    record: InventoryRecord,
    version: &DefinitionVersion,
) -> Result<Option<ManifestDefinitionCandidate>, AppError> {
    if record.perks.len() > 64 {
        return Err(provider_validation());
    }
    let key_value = decode_decimal_u32(key)?;
    if record.hash != key_value {
        return Err(provider_validation());
    }
    if record.redacted.into_option() == Some(true) {
        return Ok(None);
    }
    let display = record
        .display_properties
        .into_option()
        .ok_or_else(provider_validation)?;
    let name = display.name;
    let description = display.description.into_option();
    if name.trim().is_empty() {
        return Ok(None);
    }
    if name.chars().count() > 256 {
        return Err(provider_validation());
    }
    let (socket_entries, socket_categories) = record.sockets.map_or_else(
        || (Vec::new(), Vec::new()),
        |sockets| {
            (
                sockets
                    .socket_entries
                    .into_iter()
                    .map(|entry| destiny_core::DefinitionSocketEntry {
                        socket_type_hash: entry.socket_type_hash,
                        single_initial_item_hash: entry.single_initial_item_hash,
                        reusable_plug_set_hash: entry.reusable_plug_set_hash,
                        randomized_plug_set_hash: entry.randomized_plug_set_hash,
                        reusable_plug_items: entry
                            .reusable_plug_items
                            .into_iter()
                            .map(|plug| plug.plug_item_hash)
                            .collect(),
                        plug_sources: entry.plug_sources,
                        default_visible: entry.default_visible,
                    })
                    .collect(),
                sockets
                    .socket_categories
                    .into_iter()
                    .map(|category| destiny_core::DefinitionSocketCategory {
                        socket_category_hash: category.socket_category_hash,
                        socket_indexes: category.socket_indexes,
                    })
                    .collect(),
            )
        },
    );
    let armor = destiny_core::ArmorDefinition {
        socket_entries,
        socket_categories,
        investment_stats: record
            .investment_stats
            .into_iter()
            .map(|stat| destiny_core::InvestmentStat {
                stat_type_hash: stat.stat_type_hash,
                value: stat.value,
                is_conditionally_active: stat.is_conditionally_active,
            })
            .collect(),
        stat_group_hash: record.stats.and_then(|stats| stats.stat_group_hash),
        equipable_item_set_hash: record
            .equipping_block
            .and_then(|block| block.equipable_item_set_hash),
        plug_category_hash: record
            .plug
            .as_ref()
            .and_then(|plug| plug.plug_category_hash),
        energy_cost: record
            .plug
            .as_ref()
            .and_then(|plug| plug.energy_cost.as_ref())
            .map(|cost| destiny_core::PlugEnergyCost {
                energy_cost: cost.energy_cost,
                energy_type: cost.energy_type,
                energy_type_hash: cost.energy_type_hash,
            }),
        aspect_capacity: record
            .plug
            .as_ref()
            .and_then(|plug| plug.energy_capacity.as_ref())
            .map(|capacity| capacity.capacity_value),
    };
    armor.validate()?;
    let plug_category = record.plug.and_then(|plug| plug.plug_category_identifier);
    if plug_category
        .as_ref()
        .is_some_and(|category| category.chars().count() > 256)
    {
        return Err(provider_validation());
    }
    let inventory = record.inventory;
    let is_instance_item = inventory
        .as_ref()
        .and_then(|fields| fields.is_instance_item);
    let max_stack_size = inventory.as_ref().and_then(|fields| fields.max_stack_size);
    let stack_unique_label = inventory
        .as_ref()
        .and_then(|fields| fields.stack_unique_label.clone());
    let (bucket_hash, tier) = inventory.map_or((None, None), |inventory| {
        (
            inventory
                .bucket_type_hash
                .into_option()
                .map(|bucket| BucketHash::new(i64::from(bucket))),
            inventory.tier_type_name.into_option(),
        )
    });
    Ok(Some(ManifestDefinitionCandidate {
        armor,
        non_transferrable: record.non_transferrable,
        does_postmaster_pull_have_side_effects: record.does_postmaster_pull_have_side_effects,
        is_instance_item,
        max_stack_size,
        stack_unique_label,
        perk_hashes: record
            .perks
            .into_iter()
            .filter_map(|perk| perk.perk_hash.into_option())
            .collect(),
        description,
        hash: ItemHash::new(i64::from(key_value)),
        bucket_hash,
        name,
        plug_category,
        item_type: record.item_type_display_name.into_option(),
        tier,
        element: None,
        damage_type: match record.default_damage_type.into_option() {
            Some(1) => Some(destiny_core::domain::DamageType::Kinetic),
            Some(2) => Some(destiny_core::domain::DamageType::Arc),
            Some(3) => Some(destiny_core::domain::DamageType::Solar),
            Some(4) => Some(destiny_core::domain::DamageType::Void),
            Some(6) => Some(destiny_core::domain::DamageType::Stasis),
            Some(7) => Some(destiny_core::domain::DamageType::Strand),
            _ => None,
        },
        slot: bucket_hash.and_then(destiny_core::domain::ItemSlot::from_equipment_bucket),
        class_type: match record.class_type.into_option() {
            Some(0) => Some(destiny_core::domain::ClassType::Titan),
            Some(1) => Some(destiny_core::domain::ClassType::Hunter),
            Some(2) => Some(destiny_core::domain::ClassType::Warlock),
            Some(3) => Some(destiny_core::domain::ClassType::Any),
            _ => None,
        },
        tags: Vec::new(),
        version: version.clone(),
    }))
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

/// The documented Bungie Destiny 2 Manifest index endpoint.
pub const MANIFEST_ENDPOINT: &str = "https://www.bungie.net/Platform/Destiny2/Manifest/";

/// Fixed API-key-only public milestone endpoint.
pub const MILESTONES_ENDPOINT: &str = "https://www.bungie.net/Platform/Destiny2/Milestones/";

/// The only origin the production transport is permitted to contact.
pub const MANIFEST_ORIGIN: &str = "https://www.bungie.net";

/// Strict upper bound on the Manifest index response body (1 MiB).
const MAX_INDEX_BYTES: usize = 1024 * 1024;

/// Strict upper bound on one decompressed component (512 MiB).
const MAX_COMPONENT_BYTES: usize = 512 * 1024 * 1024;

/// Total component timeout, including body transfer.
const COMPONENT_TIMEOUT: Duration = Duration::from_secs(180);

/// One total timeout covering connect, request, and body read.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A tighter bound on the connection phase only.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const CONTENT_TYPE_JSON: &str = "application/json";
const API_KEY_HEADER: &str = "x-api-key";

fn provider_unavailable() -> AppError {
    AppError::new(ErrorCategory::ProviderUnavailable)
}

fn configuration() -> AppError {
    AppError::new(ErrorCategory::ConfigurationError)
}

/// A validated Bungie API key.
///
/// The value is accepted only when it is non-empty and free of whitespace and
/// control characters, so it can never smuggle a header-injection payload or a
/// blank credential. The bytes are reachable only through the trusted
/// `expose` boundary and are never rendered by [`fmt::Debug`].
pub struct ManifestApiKey(String);

impl ManifestApiKey {
    /// Validates a candidate API key.
    ///
    /// Empty, whitespace-bearing, or control-bearing values are rejected as
    /// sanitized configuration failures.
    pub fn new(value: impl Into<String>) -> Result<Self, AppError> {
        let value = value.into();
        if value.is_empty()
            || value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(configuration());
        }
        Ok(Self(value))
    }

    /// Exposes the secret to the trusted production transport only.
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ManifestApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ManifestApiKey")
    }
}

/// The exact read-only GET request this adapter asks a transport to send.
///
/// The API key is carried by reference and is never rendered by [`fmt::Debug`].
pub struct ManifestHttpRequest<'a> {
    url: &'a str,
    api_key: Option<&'a ManifestApiKey>,
}

impl<'a> ManifestHttpRequest<'a> {
    pub fn new(url: &'a str, api_key: Option<&'a ManifestApiKey>) -> Self {
        Self { url, api_key }
    }

    pub fn url(&self) -> &'a str {
        self.url
    }

    pub fn api_key(&self) -> Option<&'a ManifestApiKey> {
        self.api_key
    }
}

impl fmt::Debug for ManifestHttpRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestHttpRequest")
            .field("url", &self.url)
            .field("api_key", &self.api_key.map(|_| "<redacted>"))
            .finish()
    }
}

/// A transport-level failure, categorized before it reaches the adapter.
///
/// The variants are intentionally coarse so that no upstream text can escape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestTransportError {
    /// Adaptive pacing refused dispatch.
    Throttled(u64),
    /// The request exceeded the total timeout.
    Timeout,
    /// DNS, connection, TLS, or body-transfer failure.
    Unavailable,
    /// Local request construction or client configuration failure.
    Configuration,
}

/// A transport response with a bounded, streaming body.
///
/// The body is a reader so the adapter can enforce the response-size cap
/// without trusting the transport to buffer everything first.
pub struct ManifestHttpResponse {
    status: u16,
    content_length: Option<u64>,
    content_type: Option<String>,
    retry_after: RetryAfterHeader,
    body: Box<dyn Read>,
    throttle: Option<std::sync::Arc<crate::bungie_throttle::BungieThrottle>>,
}

impl ManifestHttpResponse {
    /// Builds a response for a transport implementation or contract test.
    ///
    /// `content_type` is the raw upstream `Content-Type` header value. It is
    /// only inspected to accept or reject the response; it is never rendered by
    /// [`fmt::Debug`] and never reaches an error or diagnostic. The response
    /// starts with no provider pacing hint; a transport that observed a
    /// `Retry-After` header attaches one with [`Self::with_retry_after`].
    pub fn new(
        status: u16,
        content_length: Option<u64>,
        content_type: Option<String>,
        body: Box<dyn Read>,
    ) -> Self {
        Self {
            status,
            content_length,
            content_type,
            retry_after: RetryAfterHeader::Absent,
            body,
            throttle: None,
        }
    }

    /// Attaches the classified provider pacing hint, when one was observed.
    pub fn with_retry_after(mut self, retry_after: RetryAfterHeader) -> Self {
        self.retry_after = retry_after;
        self
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub fn content_type(&self) -> Option<&str> {
        self.content_type.as_deref()
    }

    /// The classified provider pacing hint the transport observed.
    ///
    /// The value only shapes a bounded retry decision and is never surfaced.
    pub const fn retry_after(&self) -> RetryAfterHeader {
        self.retry_after
    }
}

impl fmt::Debug for ManifestHttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManifestHttpResponse")
            .field("status", &self.status)
            .field("content_length", &self.content_length)
            .field(
                "content_type",
                &self.content_type.as_ref().map(|_| "<redacted>"),
            )
            .field("retry_after", &self.retry_after)
            .field("body", &"<redacted>")
            .finish()
    }
}

/// The seam between the Manifest acquisition logic and an HTTP implementation.
///
/// Production uses [`HttpManifestTransport`]; tests inject a fake that records
/// the request and returns crafted responses.
pub trait ManifestTransport {
    fn get(
        &mut self,
        request: &ManifestHttpRequest<'_>,
    ) -> Result<ManifestHttpResponse, ManifestTransportError>;
}

/// The production blocking HTTPS transport.
///
/// Construction failures are reported as `ConfigurationError` and never
/// surface transport text.
pub struct HttpManifestTransport {
    client: reqwest::blocking::Client,
    throttle: std::sync::Arc<crate::bungie_throttle::BungieThrottle>,
}

impl HttpManifestTransport {
    /// Injects isolated pacing/time for deterministic transport tests.
    pub fn with_throttle(
        mut self,
        throttle: std::sync::Arc<crate::bungie_throttle::BungieThrottle>,
    ) -> Self {
        self.throttle = throttle;
        self
    }

    /// Builds a hardened client for the fixed Manifest origin.
    pub fn new() -> Result<Self, AppError> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .gzip(true)
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|_| configuration())?;
        Ok(Self {
            client,
            throttle: crate::bungie_throttle::shared(),
        })
    }
}

impl fmt::Debug for HttpManifestTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpManifestTransport")
            .finish_non_exhaustive()
    }
}

impl ManifestTransport for HttpManifestTransport {
    fn get(
        &mut self,
        request: &ManifestHttpRequest<'_>,
    ) -> Result<ManifestHttpResponse, ManifestTransportError> {
        if !is_allowed_manifest_url(request.url()) {
            return Err(ManifestTransportError::Configuration);
        }
        self.throttle
            .before_request()
            .map_err(ManifestTransportError::Throttled)?;
        let timeout = if matches!(request.url(), MANIFEST_ENDPOINT | MILESTONES_ENDPOINT) {
            REQUEST_TIMEOUT
        } else {
            COMPONENT_TIMEOUT
        };
        let mut builder = self.client.get(request.url()).timeout(timeout);
        if let Some(api_key) = request.api_key() {
            let value = reqwest::header::HeaderValue::from_str(api_key.expose())
                .map_err(|_| ManifestTransportError::Configuration)?;
            builder = builder.header(
                reqwest::header::HeaderName::from_static(API_KEY_HEADER),
                value,
            );
        }
        let response = builder.send().map_err(classify_transport_error)?;
        let status = response.status().as_u16();
        if status == 200 && !matches!(request.url(), MANIFEST_ENDPOINT | MILESTONES_ENDPOINT) {
            // Static content has no Bungie envelope; HTTP success is its success signal.
            self.throttle.observe(&serde_json::json!({"ErrorCode":1}));
        }
        let content_length = response.content_length();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_after = classify_retry_after(response.headers());
        Ok(ManifestHttpResponse {
            status,
            content_length,
            content_type,
            retry_after,
            body: Box::new(response),
            throttle: matches!(request.url(), MANIFEST_ENDPOINT | MILESTONES_ENDPOINT)
                .then(|| self.throttle.clone()),
        })
    }
}

/// Accepts only the fixed index endpoint or a validated component URL on the
/// fixed origin, so the production transport can never be pointed elsewhere.
fn is_allowed_manifest_url(url: &str) -> bool {
    url == MANIFEST_ENDPOINT
        || url == MILESTONES_ENDPOINT
        || url
            .strip_prefix(MANIFEST_ORIGIN)
            .is_some_and(is_safe_component_path)
}

fn classify_transport_error(error: reqwest::Error) -> ManifestTransportError {
    if error.is_timeout() {
        ManifestTransportError::Timeout
    } else if error.is_builder() {
        ManifestTransportError::Configuration
    } else {
        ManifestTransportError::Unavailable
    }
}

/// Classifies the `Retry-After` header of one response from its raw bytes.
///
/// A missing header is [`RetryAfterHeader::Absent`], so the bounded policy
/// backoff applies. A present header is classified byte-for-byte, so a value
/// that is not valid UTF-8 is [`RetryAfterHeader::Invalid`] -- present but
/// unusable -- rather than silently treated as absent. The raw value is never
/// rendered.
fn classify_retry_after(headers: &reqwest::header::HeaderMap) -> RetryAfterHeader {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .map_or(RetryAfterHeader::Absent, |value| {
            parse_retry_after_header_bytes(value.as_bytes())
        })
}

/// The production Manifest gateway.
///
/// It holds a validated API key, a transport, an injected clock, and a bounded
/// read-retry runtime. The index request carries the key; the component request
/// never does. A definition acquisition retries both the explicit rate limit and
/// a transient provider-unavailable failure under the shared bounded policy. The
/// injected clock is consulted only to interpret an absolute-date `Retry-After`
/// hint; an unavailable clock makes that hint unusable, so the rate-limit
/// response is surfaced rather than repeated against an unknown instant.
pub struct BungieManifestGateway<T, C> {
    api_key: ManifestApiKey,
    transport: T,
    clock: C,
    read_retry: ReadRetry,
}

impl<T, C> BungieManifestGateway<T, C> {
    pub fn new(api_key: ManifestApiKey, transport: T, clock: C) -> Self {
        Self {
            api_key,
            transport,
            clock,
            read_retry: ReadRetry::default(),
        }
    }

    /// Replaces the bounded read-retry runtime, for composition or a
    /// deterministic test policy.
    pub fn with_read_retry(mut self, read_retry: ReadRetry) -> Self {
        self.read_retry = read_retry;
        self
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T: ManifestTransport, C: Clock> BungieManifestGateway<T, C> {
    /// Fetches fresh index evidence at call time, never reusing a prior index.
    /// Uses the existing bounded read-retry policy. This establishes observed
    /// correspondence only, not authenticity beyond the existing transport or
    /// atomicity with a later action.
    pub fn fetch_equipment_capacity_evidence(
        &mut self,
    ) -> Result<destiny_core::EquipmentCapacityEvidence, AppError> {
        let request = ManifestHttpRequest::new(MANIFEST_ENDPOINT, Some(&self.api_key));
        let bytes = fetch_with_retry(
            &mut self.transport,
            &mut self.read_retry,
            &self.clock,
            &request,
            MAX_INDEX_BYTES,
        )?;
        crate::equipment_capacity_policy::decode_equipment_capacity_evidence(&bytes)
    }

    fn load_world_table<D: WorldKind>(
        &mut self,
        path: &str,
        cap: usize,
        project: WorldProjection<D>,
    ) -> Result<destiny_core::WorldDefinitionTable<D>, AppError> {
        let url = format!("{MANIFEST_ORIGIN}{path}");
        let request = ManifestHttpRequest::new(&url, None);
        self.read_retry
            .run(ReadRetryScope::RateLimitAndTransient, || {
                let response =
                    fetch_response(&mut self.transport, &request, || self.clock.now().ok())?;
                if response
                    .content_length
                    .is_some_and(|length| length > MAX_COMPONENT_BYTES as u64)
                {
                    return Err(ReadFailure::terminal(provider_validation()));
                }
                decode_world_reader(response.body, MAX_COMPONENT_BYTES, cap, project)
                    .map_err(ReadFailure::terminal)
            })
    }
}

impl<T, C> fmt::Debug for BungieManifestGateway<T, C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BungieManifestGateway")
            .finish_non_exhaustive()
    }
}

impl<T: ManifestTransport, C: Clock> destiny_core::PublicMilestonesGateway
    for BungieManifestGateway<T, C>
{
    fn load_public_milestones(
        &mut self,
    ) -> Result<destiny_core::PublicMilestonesSnapshot, AppError> {
        let request = ManifestHttpRequest::new(MILESTONES_ENDPOINT, Some(&self.api_key));
        let bytes = fetch_with_retry(
            &mut self.transport,
            &mut self.read_retry,
            &self.clock,
            &request,
            MAX_INDEX_BYTES,
        )?;
        decode_public_milestones(&bytes)
    }
}

impl<T, C> ManifestGateway for BungieManifestGateway<T, C>
where
    T: ManifestTransport,
    C: Clock,
{
    fn fetch_equipment_capacity_evidence(
        &mut self,
    ) -> Result<destiny_core::EquipmentCapacityEvidence, AppError> {
        BungieManifestGateway::fetch_equipment_capacity_evidence(self)
    }

    fn fetch_index(&mut self) -> Result<destiny_core::ports::ManifestIndex, AppError> {
        let index_request = ManifestHttpRequest::new(MANIFEST_ENDPOINT, Some(&self.api_key));
        let index_bytes = fetch_with_retry(
            &mut self.transport,
            &mut self.read_retry,
            &self.clock,
            &index_request,
            MAX_INDEX_BYTES,
        )?;
        let plan = decode_manifest_index(&index_bytes)?;
        Ok(destiny_core::ports::ManifestIndex {
            armor_catalog_paths: plan.armor_catalog_paths,
            version: plan.version,
            component_path: plan.component_path,
            milestone_path: plan.milestone_path,
            activity_path: plan.activity_path,
            modifier_path: plan.modifier_path,
            objective_path: plan.objective_path,
            activity_type_path: plan.activity_type_path,
            stat_path: plan.stat_path,
            loadout_name_path: plan.loadout_name_path,
            sandbox_perk_path: plan.sandbox_perk_path,
        })
    }

    fn load_world_components(
        &mut self,
        plan: &destiny_core::ports::ManifestIndex,
    ) -> Result<destiny_core::WorldDefinitionsAcquisition, AppError> {
        // Validate the entire plan before issuing any content request.
        for path in [
            &plan.milestone_path,
            &plan.activity_path,
            &plan.modifier_path,
            &plan.objective_path,
            &plan.activity_type_path,
            &plan.stat_path,
            &plan.loadout_name_path,
        ] {
            if !is_safe_component_path(path) {
                return Err(provider_validation());
            }
        }
        let mut armor_catalogs = Vec::new();
        for path in &plan.armor_catalog_paths {
            if !is_safe_component_path(path) {
                return Err(provider_validation());
            }
        }
        decode_version(Some(&Value::String(plan.version.as_str().to_owned())))?;
        for (kind, path) in destiny_core::armor_catalog::ArmorCatalogKind::ALL
            .into_iter()
            .zip(&plan.armor_catalog_paths)
        {
            let url = format!("{MANIFEST_ORIGIN}{path}");
            let request = ManifestHttpRequest::new(&url, None);
            armor_catalogs.extend(self.read_retry.run(
                ReadRetryScope::RateLimitAndTransient,
                || {
                    let response =
                        fetch_response(&mut self.transport, &request, || self.clock.now().ok())?;
                    if response
                        .content_length
                        .is_some_and(|size| size > max_armor_catalog_bytes(kind) as u64)
                    {
                        return Err(ReadFailure::terminal(provider_validation()));
                    }
                    decode_armor_catalog_reader(kind, response.body).map_err(ReadFailure::terminal)
                },
            )?);
        }
        Ok(destiny_core::WorldDefinitionsAcquisition {
            armor_catalogs,
            item_descriptions: Vec::new(),
            version: plan.version.clone(),
            milestones: self.load_world_table(&plan.milestone_path, 1000, project_milestone)?,
            activities: self.load_world_table(&plan.activity_path, 50000, project_activity)?,
            modifiers: self.load_world_table(&plan.modifier_path, 10000, project_modifier)?,
            objectives: self.load_world_table(&plan.objective_path, 100000, project_objective)?,
            activity_types: self.load_world_table(
                &plan.activity_type_path,
                1000,
                project_activity_type,
            )?,
            stats: self.load_world_table(&plan.stat_path, 1000, project_stat)?,
            loadout_names: self.load_world_table(
                &plan.loadout_name_path,
                1000,
                project_loadout_name,
            )?,
        })
    }

    fn load_component(
        &mut self,
        plan: &destiny_core::ports::ManifestIndex,
    ) -> Result<ManifestAcquisition, AppError> {
        if !is_safe_component_path(&plan.component_path)
            || !is_safe_component_path(&plan.sandbox_perk_path)
        {
            return Err(provider_validation());
        }
        decode_version(Some(&Value::String(plan.version.as_str().to_owned())))?;
        let component_url = format!("{MANIFEST_ORIGIN}{}", plan.component_path);
        let component_request = ManifestHttpRequest::new(&component_url, None);
        let mut acquisition = self
            .read_retry
            .run(ReadRetryScope::RateLimitAndTransient, || {
                let response = fetch_response(&mut self.transport, &component_request, || {
                    self.clock.now().ok()
                })?;
                decode_inventory_reader(plan.version.clone(), response.body, MAX_COMPONENT_BYTES)
                    .map_err(ReadFailure::terminal)
            })?;
        acquisition.sandbox_perks = self
            .load_world_table(&plan.sandbox_perk_path, 100_000, project_sandbox_perk)?
            .definitions;
        Ok(acquisition)
    }
}

/// Performs one bounded, paced Manifest read.
///
/// A definition acquisition is a pure cache fill that never feeds a mutation
/// observation, so it may repeat both the explicit rate limit and a transient
/// provider-unavailable failure under the shared bounded policy.
fn fetch_with_retry<T, C>(
    transport: &mut T,
    read_retry: &mut ReadRetry,
    clock: &C,
    request: &ManifestHttpRequest<'_>,
    limit: usize,
) -> Result<Vec<u8>, AppError>
where
    T: ManifestTransport,
    C: Clock,
{
    read_retry.run(ReadRetryScope::RateLimitAndTransient, || {
        fetch_once(transport, request, limit, || clock.now().ok())
    })
}

/// Performs one fixed GET and reports a retry-ready failure.
///
/// This is the single-attempt primitive: it never repeats. The gateway wraps it
/// in a bounded [`ReadRetry`] run, and the failure keeps the sanitized
/// [`AppError`] plus the resolved pacing hint, so the bounded retry loop can pace
/// or suppress a repeat without ever re-reading a response body or surfacing
/// provider text. `retry_after_now` is consulted only for an absolute-date hint,
/// and an unavailable clock makes that hint unusable rather than guessed.
fn fetch_once<T, F>(
    transport: &mut T,
    request: &ManifestHttpRequest<'_>,
    limit: usize,
    retry_after_now: F,
) -> Result<Vec<u8>, ReadFailure>
where
    T: ManifestTransport,
    F: FnOnce() -> Option<Timestamp>,
{
    let mut response = fetch_response(transport, request, retry_after_now)?;
    let bytes = read_bounded_body(&mut response.body, response.content_length, limit)
        .map_err(ReadFailure::terminal)?;
    if let Some(throttle) = &response.throttle {
        throttle.observe_bytes(&bytes);
    }
    if matches!(request.url(), MANIFEST_ENDPOINT | MILESTONES_ENDPOINT) {
        crate::bungie_errors::check_read_body(&bytes)?;
    }
    Ok(bytes)
}

fn fetch_response<T: ManifestTransport, F: FnOnce() -> Option<Timestamp>>(
    transport: &mut T,
    request: &ManifestHttpRequest<'_>,
    retry_after_now: F,
) -> Result<ManifestHttpResponse, ReadFailure> {
    let response = transport.get(request).map_err(|error| {
        if let ManifestTransportError::Throttled(seconds) = error {
            ReadFailure::with_retry_after(
                AppError::bungie_rate_limit(seconds),
                destiny_core::ports::RetryAfterHint::Unusable,
            )
        } else {
            ReadFailure::terminal(map_transport_error(error))
        }
    })?;
    if response.status != 200 {
        let hint = response.retry_after().resolve(retry_after_now);
        return Err(ReadFailure::with_retry_after(
            classify_status(response.status),
            hint,
        ));
    }
    if !response.content_type().is_some_and(is_json_content_type) {
        return Err(ReadFailure::terminal(provider_validation()));
    }
    Ok(response)
}

fn map_transport_error(error: ManifestTransportError) -> AppError {
    match error {
        ManifestTransportError::Timeout | ManifestTransportError::Unavailable => {
            provider_unavailable()
        }
        ManifestTransportError::Configuration => configuration(),
        ManifestTransportError::Throttled(seconds) => AppError::bungie_rate_limit(seconds),
    }
}

fn classify_status(status: u16) -> AppError {
    match status {
        401 | 403 => AppError::new(ErrorCategory::ProviderAuthentication),
        429 => AppError::new(ErrorCategory::ProviderRateLimit),
        500..=599 => provider_unavailable(),
        _ => provider_validation(),
    }
}

/// Accepts only an `application/json` media type, case-insensitively, with an
/// optional well-formed parameter list such as `; charset=UTF-8`.
///
/// Missing, malformed, control-bearing, or non-JSON values are rejected. The
/// raw upstream value is inspected here only; it is never surfaced.
fn is_json_content_type(value: &str) -> bool {
    if value.chars().any(|character| character.is_control()) {
        return false;
    }
    let mut parts = value.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    media_type.eq_ignore_ascii_case(CONTENT_TYPE_JSON) && parts.all(is_media_parameter)
}

/// Requires one `name=value` media-type parameter with a valid token name and a
/// token or quoted-string value.
fn is_media_parameter(parameter: &str) -> bool {
    let Some((name, value)) = parameter.split_once('=') else {
        return false;
    };
    let name = name.trim();
    let value = value.trim();
    !name.is_empty() && is_token(name) && is_parameter_value(value)
}

fn is_parameter_value(value: &str) -> bool {
    if is_token(value) {
        return true;
    }
    let Some(inner) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    !inner.contains('"')
        && inner
            .chars()
            .all(|character| character == '\t' || character == ' ' || character.is_ascii_graphic())
}

fn is_token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(is_token_byte)
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

/// Reads the body under a strict cap, checking the announced length first so an
/// oversize body is rejected without reading any bytes.
fn read_bounded_body(
    body: &mut dyn Read,
    content_length: Option<u64>,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    if let Some(length) = content_length
        && length > limit as u64
    {
        return Err(provider_validation());
    }
    let mut buffer = Vec::new();
    body.take(limit as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|_| provider_unavailable())?;
    if buffer.len() > limit {
        return Err(provider_validation());
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::{decode_inventory_items, decode_manifest_index};

    #[test]
    fn postmaster_metadata_preserves_unknown_false_and_exact_values() {
        for metadata in [
            "",
            r#", "inventory": null, "doesPostmasterPullHaveSideEffects": null"#,
            r#", "inventory": {"isInstanceItem":null,"maxStackSize":null,"stackUniqueLabel":null}"#,
        ] {
            let bytes = format!(
                "{{\"1\":{{\"hash\":1,\"displayProperties\":{{\"name\":\"Item\"}}{metadata}}}}}"
            );
            let batch = decode_inventory_items(version(), bytes.as_bytes()).unwrap();
            let item = &batch.definitions[0];
            assert_eq!(item.does_postmaster_pull_have_side_effects, None);
            assert_eq!(item.is_instance_item, None);
            assert_eq!(item.max_stack_size, None);
            assert_eq!(item.stack_unique_label, None);
        }
        for flag in [false, true] {
            for capacity in [i32::MIN, -1, 0, 1, i32::MAX] {
                for label in ["", "  Exact Label\n", "Case.Sensitive"] {
                    let bytes = serde_json::json!({"1": {
                        "hash": 1, "displayProperties": {"name": "Item"},
                        "doesPostmasterPullHaveSideEffects": flag,
                        "inventory": {"isInstanceItem": flag, "maxStackSize": capacity,
                            "stackUniqueLabel": label, "bucketTypeHash": 123}
                    }})
                    .to_string();
                    let batch = decode_inventory_items(version(), bytes.as_bytes()).unwrap();
                    let item = &batch.definitions[0];
                    assert_eq!(item.does_postmaster_pull_have_side_effects, Some(flag));
                    assert_eq!(item.is_instance_item, Some(flag));
                    assert_eq!(item.max_stack_size, Some(capacity));
                    assert_eq!(item.stack_unique_label.as_deref(), Some(label));
                    assert_eq!(item.bucket_hash, Some(destiny_core::BucketHash::new(123)));
                }
            }
        }
    }

    #[test]
    fn postmaster_metadata_wrong_types_are_not_swallowed() {
        for metadata in [
            r#""doesPostmasterPullHaveSideEffects":"private-invalid""#,
            r#""doesPostmasterPullHaveSideEffects":0"#,
            r#""doesPostmasterPullHaveSideEffects":[]"#,
            r#""doesPostmasterPullHaveSideEffects":{}"#,
            r#""inventory":{"isInstanceItem":1}"#,
            r#""inventory":{"isInstanceItem":"false"}"#,
            r#""inventory":{"isInstanceItem":[]}"#,
            r#""inventory":{"maxStackSize":1.5}"#,
            r#""inventory":{"maxStackSize":1.0}"#,
            r#""inventory":{"maxStackSize":2147483648}"#,
            r#""inventory":{"maxStackSize":-2147483649}"#,
            r#""inventory":{"maxStackSize":18446744073709551616}"#,
            r#""inventory":{"maxStackSize":"1"}"#,
            r#""inventory":{"maxStackSize":true}"#,
            r#""inventory":{"maxStackSize":{}}"#,
            r#""inventory":{"stackUniqueLabel":false}"#,
            r#""inventory":{"stackUniqueLabel":1}"#,
            r#""inventory":{"stackUniqueLabel":[]}"#,
            r#""inventory":{"stackUniqueLabel":{}}"#,
        ] {
            for presentation in [
                r#""displayProperties":{"name":"Item"}"#,
                r#""displayProperties":{"name":""}"#,
                r#""redacted":true"#,
            ] {
                let bytes = format!(
                    "{{\"1\":{{\"hash\":1,{presentation},{metadata}}},\"2\":{{\"hash\":2,\"displayProperties\":{{\"name\":\"Other\"}}}}}}"
                );
                assert_validation(decode_inventory_items(version(), bytes.as_bytes()));
            }
        }
    }

    #[test]
    fn sandbox_perks_and_inventory_references_are_decoded() {
        let batch = decode_inventory_items(DefinitionVersion::new("v1"), br#"{"1":{"hash":1,"displayProperties":{"name":"Facet"},"perks":[{"perkHash":7},{"perkHash":8}]}}"#).unwrap();
        assert_eq!(batch.definitions[0].perk_hashes, vec![7, 8]);
        let perks = super::decode_sandbox_perks(br#"{"7":{"hash":7,"isDisplayable":true,"displayProperties":{"description":"First\nSecond"}},"8":{"hash":8,"isDisplayable":false,"displayProperties":{"description":"Hidden"}},"9":{"hash":9,"redacted":true},"10":{"hash":10,"displayProperties":{"description":17}}}"#).unwrap();
        assert_eq!(perks.len(), 2);
        assert_eq!(perks[0].description, "First\nSecond");
        assert!(perks[0].is_displayable);
        assert!(!perks[1].is_displayable);
        assert!(super::decode_sandbox_perks(br#"{"7":{"hash":8}}"#).is_err());
        assert!(super::decode_sandbox_perks(b"not json").is_err());
        assert!(super::decode_sandbox_perks(br#"{"7":{"hash":7},"7":{"hash":7}}"#).is_err());
        let bytes = br#"{"7":{"hash":7,"isDisplayable":true,"displayProperties":{"description":"Effect"}}} "#;
        assert!(
            super::decode_world_reader(
                bytes.as_slice(),
                bytes.len() - 1,
                100_000,
                super::project_sandbox_perk
            )
            .is_err()
        );
        assert!(
            super::decode_world_reader(
                bytes.as_slice(),
                bytes.len(),
                0,
                super::project_sandbox_perk
            )
            .is_err()
        );
        assert!(super::decode_sandbox_perks(b"{}").unwrap().is_empty());
        let oversized = format!(
            "{{\"1\":{{\"hash\":1,\"displayProperties\":{{\"name\":\"Facet\"}},\"perks\":[{}]}}}}",
            vec!["{\"perkHash\":7}"; 65].join(",")
        );
        assert!(
            decode_inventory_items(DefinitionVersion::new("v1"), oversized.as_bytes()).is_err()
        );
    }

    #[test]
    fn inventory_descriptions_are_decoded_without_affecting_item_names() {
        let batch = decode_inventory_items(DefinitionVersion::new("v1"), br#"{"1":{"hash":1,"displayProperties":{"name":"Perk","description":"First\nSecond"}},"2":{"hash":2,"displayProperties":{"name":"Other","description":17}}}"#).unwrap();
        assert_eq!(
            batch.definitions[0].description.as_deref(),
            Some("First\nSecond")
        );
        assert_eq!(batch.definitions[1].description, None);
    }

    #[test]
    fn plug_category_decode_is_optional_and_bounded() {
        for category in [None, Some("frames"), Some("intrinsics"), Some("origins")] {
            let mut record = serde_json::json!({"hash":1,"displayProperties":{"name":"Test"}});
            if let Some(category) = category {
                record["plug"] = serde_json::json!({"plugCategoryIdentifier": category});
            }
            let bytes = serde_json::json!({"1":record}).to_string();
            let batch = decode_inventory_items(version(), bytes.as_bytes()).unwrap();
            assert_eq!(batch.definitions[0].plug_category.as_deref(), category);
        }
        for category in [serde_json::json!(42), serde_json::json!("x".repeat(257))] {
            let bytes = serde_json::json!({"1":{"hash":1,"displayProperties":{"name":"Test"},"plug":{"plugCategoryIdentifier":category}}}).to_string();
            assert_validation(decode_inventory_items(version(), bytes.as_bytes()));
        }
    }
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
                    "en": { "DestinySocketTypeDefinition": path, "DestinySocketCategoryDefinition": path, "DestinyPlugSetDefinition": path, "DestinyStatGroupDefinition": path, "DestinyEquipableItemSetDefinition": path, "DestinyInventoryItemDefinition": path, "DestinyMilestoneDefinition": path, "DestinyActivityDefinition": path, "DestinyActivityModifierDefinition": path, "DestinyObjectiveDefinition": path, "DestinyActivityTypeDefinition": path, "DestinyStatDefinition": path, "DestinyLoadoutNameDefinition": path, "DestinySandboxPerkDefinition": path }
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
    fn stats_keep_only_names_and_bound_untrusted_records() {
        let table = super::decode_stats(br#"{"1":{"hash":1,"displayProperties":{"name":"Weapons","description":{}}},"2":{"hash":2,"redacted":true},"3":{"hash":3,"displayProperties":{"name":" "}}}"#).unwrap();
        assert_eq!(table.definitions[0].hash(), 1);
        assert_eq!(table.definitions[0].name(), "Weapons");
        assert_eq!(table.skipped_count, 2);
        for bytes in [
            br#"{}"#.as_slice(),
            br#"{"1":{"hash":2,"displayProperties":{"name":"Weapons"}}}"#,
            br#"{"01":{"hash":1,"displayProperties":{"name":"Weapons"}}}"#,
        ] {
            assert_validation(super::decode_stats(bytes));
        }
        let oversized = json!({"1":{"hash":1,"displayProperties":{"name":"x".repeat(257)}}});
        assert_validation(super::decode_stats(oversized.to_string().as_bytes()));
        for bytes in [
            br#"{"1":{"hash":1,"displayProperties":{"name":"bad\u0000"}}}"#.as_slice(),
            br#"{"1":{"hash":1,"redacted":true}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"Weapons"}},"1":{"hash":1,"displayProperties":{"name":"Weapons"}}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"Weapons"}}} []"#,
        ] {
            assert_validation(super::decode_stats(bytes));
        }
        assert_eq!(
            super::decode_stats(build_component(1000).as_bytes())
                .unwrap()
                .definitions
                .len(),
            1000
        );
        assert_validation(super::decode_stats(build_component(1001).as_bytes()));
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"Weapons"},"ignored":"large"}}"#;
        assert_validation(super::decode_world_reader(
            bytes.as_slice(),
            bytes.len() - 1,
            1000,
            super::project_stat,
        ));
        assert_eq!(
            super::decode_world_reader(bytes.as_slice(), bytes.len(), 1000, super::project_stat)
                .unwrap()
                .definitions
                .len(),
            1
        );
    }

    #[test]
    fn objectives_keep_blank_skip_redacted_and_enforce_bounds() {
        let table = super::decode_objectives(br#"{"1":{"hash":1,"progressDescription":"","completionValue":-1},"2":{"hash":2,"redacted":true}}"#).unwrap();
        assert_eq!(table.definitions[0].progress_description(), "");
        assert_eq!(table.definitions[0].completion_value(), -1);
        assert_eq!(table.skipped_count, 1);
        for text in ["x".repeat(513), "bad\u{1}".into()] {
            let body =
                json!({"1":{"hash":1,"progressDescription":text,"completionValue":1}}).to_string();
            assert_validation(super::decode_objectives(body.as_bytes()));
        }
        let boundary = build_component(100000);
        assert_eq!(
            super::decode_objectives(boundary.as_bytes())
                .unwrap()
                .definitions
                .len(),
            100000
        );
        assert_validation(super::decode_objectives(build_component(100001).as_bytes()));
        let skipped = build_component(100001).replace("\"hash\":", "\"redacted\":true,\"hash\":");
        assert_validation(super::decode_objectives(skipped.as_bytes()));
        let ignored_display = br#"{"1":{"hash":1,"displayProperties":{"description":42},"progressDescription":"Line\nTwo","completionValue":2147483647}}"#;
        assert_eq!(
            super::decode_objectives(ignored_display)
                .unwrap()
                .definitions[0]
                .progress_description(),
            "Line\nTwo"
        );
        let types = super::decode_activity_types(br#"{"1":{"hash":1,"displayProperties":{"name":"Raid"}},"2":{"hash":2,"redacted":true},"3":{"hash":3,"displayProperties":{"name":" "}}}"#).unwrap();
        assert_eq!(types.skipped_count, 2);
        assert_eq!(types.definitions[0].name(), "Raid");
        // Live data has displayProperties objects without a name key: skip, never fail the batch.
        let nameless = super::decode_activity_types(
            br#"{"1":{"hash":1,"displayProperties":{"name":"Raid"}},"2":{"hash":2,"displayProperties":{"description":"x"}}}"#,
        )
        .unwrap();
        assert_eq!(nameless.skipped_count, 1);
        assert_eq!(nameless.definitions.len(), 1);
        for bad in [
            br#"{"1":{"hash":2,"progressDescription":""}}"#.as_slice(),
            br#"{"1":{"hash":1},"1":{"hash":1}}"#,
            br#"{"1":{"hash":1,"completionValue":2147483648}}"#,
            br#"{"1":{"hash":1}}} []"#,
        ] {
            assert_validation(super::decode_objectives(bad));
        }
        assert_validation(super::decode_activity_types(
            build_component(1001).as_bytes(),
        ));
        assert_eq!(
            super::decode_activity_types(build_component(1000).as_bytes())
                .unwrap()
                .definitions
                .len(),
            1000
        );
    }

    #[test]
    fn world_tables_skip_blank_and_redacted_and_validate_fields() {
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"Raid","description":"Line\nTwo"},"milestoneType":3,"activityTypeHash":4,"activityLightLevel":2000},"2":{"hash":2,"redacted":true},"3":{"hash":3,"displayProperties":{"name":" "}}}"#;
        let milestones = super::decode_milestones(bytes).unwrap();
        assert_eq!(milestones.skipped_count, 2);
        assert_eq!(milestones.definitions[0].milestone_type(), 3);
        assert_eq!(milestones.definitions[0].description(), "Line\nTwo");
        let activities = super::decode_activities(bytes).unwrap();
        assert_eq!(activities.definitions[0].activity_type_hash(), 4);
        assert_eq!(activities.definitions[0].light_level(), 2000);
        assert_eq!(
            super::decode_activity_modifiers(bytes).unwrap().definitions[0].name(),
            "Raid"
        );
        for bad in [
            b"{}".as_slice(),
            br#"{"1":{"hash":1,"redacted":true}}"#,
            br#"{"01":{"hash":1,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"hash":2,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"x\u0001"}}}"#,
        ] {
            assert_validation(super::decode_milestones(bad));
            assert_validation(super::decode_activities(bad));
            assert_validation(super::decode_activity_modifiers(bad));
        }
        for (name, description) in [
            ("x".repeat(257), String::new()),
            ("x".into(), "d".repeat(2049)),
        ] {
            let body =
                json!({"1":{"hash":1,"displayProperties":{"name":name,"description":description}}})
                    .to_string();
            assert_validation(super::decode_activity_modifiers(body.as_bytes()));
        }
    }

    #[test]
    fn activities_accept_negative_light_level_and_skip_blank_names() {
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"Raid"},"activityLightLevel":-10},"2":{"hash":2,"displayProperties":{"name":" "},"activityLightLevel":-10}}"#;
        let activities = super::decode_activities(bytes).unwrap();
        assert_eq!(activities.skipped_count, 1);
        assert_eq!(activities.definitions.len(), 1);
        assert_eq!(activities.definitions[0].name(), "Raid");
        assert_eq!(activities.definitions[0].light_level(), -10);
    }

    #[test]
    fn activities_reject_unparseable_field_even_on_blank_name_record() {
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"Raid"}},"2":{"hash":2,"displayProperties":{"name":" "},"activityLightLevel":"unparseable"}}"#;
        assert_validation(super::decode_activities(bytes));
    }

    #[test]
    fn world_table_caps_include_skipped_records() {
        assert_eq!(
            super::decode_milestones(build_component(1000).as_bytes())
                .unwrap()
                .definitions
                .len(),
            1000
        );
        assert_eq!(
            super::decode_activity_modifiers(build_component(10000).as_bytes())
                .unwrap()
                .definitions
                .len(),
            10000
        );
        assert_eq!(
            super::decode_activities(build_component(50000).as_bytes())
                .unwrap()
                .definitions
                .len(),
            50000
        );
        let skipped = build_component(1001)
            .replace("displayProperties", "ignoredDisplay")
            .replace("\"hash\":", "\"redacted\":true,\"hash\":");
        assert_validation(super::decode_milestones(skipped.as_bytes()));
        assert_validation(super::decode_milestones(build_component(1001).as_bytes()));
        assert_validation(super::decode_activity_modifiers(
            build_component(10001).as_bytes(),
        ));
        assert_validation(super::decode_activities(build_component(50001).as_bytes()));
    }

    #[test]
    fn world_reader_bounds_ignored_bytes_and_rejects_duplicates_and_trailing_data() {
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"x"},"ignored":[1,2,3]}} "#;
        assert!(
            super::decode_world_reader(bytes.as_slice(), bytes.len(), 1, super::project_modifier)
                .is_ok()
        );
        assert_validation(super::decode_world_reader(
            bytes.as_slice(),
            bytes.len() - 1,
            1,
            super::project_modifier,
        ));
        for bad in [
            br#"{"1":{"hash":1,"displayProperties":{"name":" "}}}"#.as_slice(),
            br#"{"1":{"hash":1,"displayProperties":{"name":"x"}},"1":{"hash":1,"displayProperties":{"name":"x"}}}"#,
            br#"{"1":{"hash":1,"displayProperties":{"name":"x"}}} []"#,
        ] {
            assert_validation(super::decode_activity_modifiers(bad));
        }
    }

    #[test]
    fn world_paths_are_required_and_safe() {
        let mut value: serde_json::Value =
            serde_json::from_slice(&index_bytes("v1", VALID_PATH)).unwrap();
        let en = value["Response"]["jsonWorldComponentContentPaths"]["en"]
            .as_object_mut()
            .unwrap();
        for kind in [
            "DestinyMilestoneDefinition",
            "DestinyActivityDefinition",
            "DestinyActivityModifierDefinition",
            "DestinyObjectiveDefinition",
            "DestinyActivityTypeDefinition",
            "DestinyStatDefinition",
            "DestinyLoadoutNameDefinition",
            "DestinySandboxPerkDefinition",
            "DestinyStatGroupDefinition",
            "DestinyEquipableItemSetDefinition",
            "DestinySocketTypeDefinition",
            "DestinySocketCategoryDefinition",
            "DestinyPlugSetDefinition",
        ] {
            en.insert(kind.into(), json!(VALID_PATH));
        }
        for kind in [
            "DestinyMilestoneDefinition",
            "DestinyActivityDefinition",
            "DestinyActivityModifierDefinition",
            "DestinyObjectiveDefinition",
            "DestinyActivityTypeDefinition",
            "DestinyStatDefinition",
            "DestinyLoadoutNameDefinition",
            "DestinySandboxPerkDefinition",
            "DestinyStatGroupDefinition",
            "DestinyEquipableItemSetDefinition",
            "DestinySocketTypeDefinition",
            "DestinySocketCategoryDefinition",
            "DestinyPlugSetDefinition",
        ] {
            let mut missing = value.clone();
            missing["Response"]["jsonWorldComponentContentPaths"]["en"]
                .as_object_mut()
                .unwrap()
                .remove(kind);
            assert_validation(decode_manifest_index(missing.to_string().as_bytes()));
            let mut unsafe_path = value.clone();
            unsafe_path["Response"]["jsonWorldComponentContentPaths"]["en"][kind] =
                json!("/common/destiny2_content/../secret.json");
            assert_validation(decode_manifest_index(unsafe_path.to_string().as_bytes()));
        }
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
    fn decompressed_reader_counts_ignored_fields_and_trailing_bytes() {
        let bytes =
            br#"{"1":{"hash":1,"displayProperties":{"name":"sentinel"},"ignored":[1,2,3]}} "#;
        assert!(super::decode_inventory_reader(version(), bytes.as_slice(), bytes.len()).is_ok());
        let error = super::decode_inventory_reader(version(), bytes.as_slice(), bytes.len() - 1)
            .unwrap_err();
        assert_eq!(error, AppError::new(ErrorCategory::ProviderValidation));
        assert!(!format!("{error:?}").contains("sentinel"));
    }

    #[test]
    fn gzip_component_stream_decodes_and_bounds_expanded_bytes() {
        use std::io::Write;
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"kept"},"ignored":"padding padding padding padding"}}"#;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let reader = || flate2::read::GzDecoder::new(compressed.as_slice());
        assert_eq!(
            super::decode_inventory_reader(version(), reader(), bytes.len())
                .unwrap()
                .definitions
                .len(),
            1
        );
        assert_validation(super::decode_inventory_reader(
            version(),
            reader(),
            bytes.len() - 1,
        ));
        // This builder call is a compile-time guard that reqwest's gzip feature
        // remains enabled; no sockets are opened by this test.
        assert!(super::HttpManifestTransport::new().is_ok());
    }

    #[test]
    fn all_redacted_or_blank_records_and_long_names_fail() {
        assert_validation(decode_inventory_items(
            version(),
            br#"{"1":{"hash":1,"redacted":true}}"#,
        ));
        let component = json!({"1": {"hash": 1, "displayProperties": {"name": "x".repeat(257)}}});
        assert_validation(decode_inventory_items(
            version(),
            component.to_string().as_bytes(),
        ));
    }

    #[test]
    fn blank_and_redacted_records_are_skipped() {
        let bytes = br#"{"1":{"hash":1,"displayProperties":{"name":"  "}},"2":{"hash":2,"redacted":true,"displayProperties":{"name":"hidden"}},"3":{"hash":3,"displayProperties":{"name":"kept"},"unknown":{"large":[1,2,3]}}}"#;
        let batch = decode_inventory_items(version(), bytes).expect("remaining record");
        assert_eq!(batch.skipped_count, 2);
        assert_eq!(batch.definitions.len(), 1);
        assert_eq!(batch.definitions[0].hash.value(), 3);
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

#[cfg(test)]
mod transport_tests {
    use super::{
        BungieManifestGateway, MANIFEST_ENDPOINT, MANIFEST_ORIGIN, ManifestApiKey,
        ManifestHttpRequest, ManifestHttpResponse, ManifestTransport, ManifestTransportError,
        classify_retry_after, is_allowed_manifest_url, read_bounded_body,
    };
    use destiny_core::ports::{ReadRetry, ReadRetryPolicy, RetryAfterHeader, RetryDelay};
    use destiny_core::{
        AppError, Clock, ErrorCategory, ManifestAcquisition, ManifestGateway, Timestamp,
    };
    use serde_json::json;
    use std::collections::VecDeque;
    use std::io::{self, Cursor, Read};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const COMPONENT_PATH: &str =
        "/common/destiny2_content/json/en/DestinyInventoryItemDefinition-abc.json";

    const GATEWAY_NOW: u64 = 1_724_000_000;

    type QueuedResponse = Result<ManifestHttpResponse, ManifestTransportError>;

    struct RecordedRequest {
        url: String,
        has_api_key: bool,
    }

    struct FakeTransport {
        requests: Vec<RecordedRequest>,
        responses: VecDeque<QueuedResponse>,
    }

    impl FakeTransport {
        fn with(responses: Vec<QueuedResponse>) -> Self {
            Self {
                requests: Vec::new(),
                responses: responses.into(),
            }
        }
    }

    impl ManifestTransport for FakeTransport {
        fn get(
            &mut self,
            request: &ManifestHttpRequest<'_>,
        ) -> Result<ManifestHttpResponse, ManifestTransportError> {
            self.requests.push(RecordedRequest {
                url: request.url().to_owned(),
                has_api_key: request.api_key().is_some(),
            });
            self.responses
                .pop_front()
                .expect("fake transport has a queued response")
        }
    }

    fn key() -> ManifestApiKey {
        ManifestApiKey::new("test-api-key").expect("valid key")
    }

    /// A fixed clock; a definition read never consults it unless a `Retry-After`
    /// hint is an absolute date.
    struct FixedClock(Timestamp);

    impl Clock for FixedClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(self.0)
        }
    }

    fn clock() -> FixedClock {
        FixedClock(Timestamp::new(GATEWAY_NOW))
    }

    /// A clock whose every reading fails, so an absolute-date hint is unusable.
    struct FailClock;

    impl Clock for FailClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Err(AppError::new(ErrorCategory::ClockOverflow))
        }
    }

    /// Records the exact paced waits a bounded retry performed, with no sleep.
    #[derive(Clone, Default)]
    struct RecordingDelay {
        waits: Arc<Mutex<Vec<Duration>>>,
    }

    impl RecordingDelay {
        fn waits(&self) -> Vec<Duration> {
            self.waits.lock().expect("delay log lock").clone()
        }
    }

    impl RetryDelay for RecordingDelay {
        fn wait(&mut self, delay: Duration) {
            self.waits.lock().expect("delay log lock").push(delay);
        }
    }

    /// A bounded policy with no real sleep, injected through the gateway.
    fn retrying_gateway(
        responses: Vec<QueuedResponse>,
    ) -> (
        BungieManifestGateway<FakeTransport, FixedClock>,
        RecordingDelay,
    ) {
        let delay = RecordingDelay::default();
        let gateway = BungieManifestGateway::new(key(), FakeTransport::with(responses), clock())
            .with_read_retry(ReadRetry::new(
                ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_millis(100)),
                delay.clone(),
            ));
        (gateway, delay)
    }

    fn index_body(path: &str) -> String {
        json!({
            "ErrorCode": 1,
            "Response": {
                "version": "manifest-v1",
                "jsonWorldComponentContentPaths": {
                    "en": { "DestinySocketTypeDefinition": path, "DestinySocketCategoryDefinition": path, "DestinyPlugSetDefinition": path, "DestinyStatGroupDefinition": path, "DestinyEquipableItemSetDefinition": path, "DestinyInventoryItemDefinition": path, "DestinyMilestoneDefinition": path, "DestinyActivityDefinition": path, "DestinyActivityModifierDefinition": path, "DestinyObjectiveDefinition": path, "DestinyActivityTypeDefinition": path, "DestinyStatDefinition": path, "DestinyLoadoutNameDefinition": path, "DestinySandboxPerkDefinition": path }
                }
            }
        })
        .to_string()
    }

    #[test]
    fn manifest_capacity_port_acquires_once_per_call_without_cached_index() {
        let body = json!({"ErrorCode":1,"Response":{"version":"port-v1",
            "jsonWorldComponentContentPaths":{"en":{"DestinyInventoryBucketDefinition":"/common/destiny2_content/buckets.json"}}}}).to_string();
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(json_response(body.clone())),
                Ok(json_response(body.replace("port-v1", "port-v2"))),
            ]),
            clock(),
        );
        for (calls, version) in [(1, "port-v1"), (2, "port-v2")] {
            let evidence =
                ManifestGateway::fetch_equipment_capacity_evidence(&mut gateway).unwrap();
            assert_eq!(evidence.definition_version().as_str(), version);
            assert_eq!(
                evidence.bucket_artifact_identity(),
                "/common/destiny2_content/buckets.json"
            );
            assert_eq!(gateway.transport().requests.len(), calls);
            let request = gateway.transport().requests.last().unwrap();
            assert_eq!(request.url, MANIFEST_ENDPOINT);
            assert!(request.has_api_key);
        }
    }

    #[test]
    fn gateway_equipment_capacity_evidence_is_fresh_and_uses_index_request() {
        let body = json!({"ErrorCode":1,"Response":{"version":"observed-v1",
            "jsonWorldComponentContentPaths":{"en":{"DestinyInventoryBucketDefinition":"/common/destiny2_content/buckets.json"}}}}).to_string();
        let changed = body.replace("observed-v1", "observed-v2");
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(json_response(index_body(
                    "/common/destiny2_content/items.json",
                ))),
                Ok(json_response(body)),
                Ok(json_response(changed)),
            ]),
            clock(),
        );
        gateway.fetch_index().unwrap();
        assert_eq!(
            gateway
                .fetch_equipment_capacity_evidence()
                .unwrap()
                .definition_version()
                .as_str(),
            "observed-v1"
        );
        assert_eq!(
            gateway
                .fetch_equipment_capacity_evidence()
                .unwrap()
                .definition_version()
                .as_str(),
            "observed-v2"
        );
        let requests = &gateway.transport().requests;
        assert_eq!(requests.len(), 3);
        for request in requests {
            assert_eq!(request.url, MANIFEST_ENDPOINT);
            assert!(request.has_api_key);
        }
    }

    #[test]
    fn gateway_equipment_capacity_evidence_reuses_bounded_read_retry() {
        let body = json!({"ErrorCode":1,"Response":{"version":"v1",
            "jsonWorldComponentContentPaths":{"en":{"DestinyInventoryBucketDefinition":"/common/destiny2_content/buckets.json"}}}}).to_string();
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(503)),
            Ok(json_response(body.clone())),
        ]);
        assert!(gateway.fetch_equipment_capacity_evidence().is_ok());
        assert_eq!(gateway.transport().requests.len(), 2);
        assert_eq!(delay.waits(), vec![Duration::from_millis(10)]);
        // A decodable body announced over the cap isolates cap enforcement.
        let oversized = ManifestHttpResponse::new(
            200,
            Some(super::MAX_INDEX_BYTES as u64 + 1),
            Some("application/json".into()),
            Box::new(Cursor::new(body.into_bytes())),
        );
        let mut gateway =
            BungieManifestGateway::new(key(), FakeTransport::with(vec![Ok(oversized)]), clock());
        assert_eq!(
            gateway
                .fetch_equipment_capacity_evidence()
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(gateway.transport().requests.len(), 1);
    }

    fn component_body() -> String {
        json!({
            "42": { "hash": 42, "displayProperties": { "name": "Test Item" } }
        })
        .to_string()
    }

    fn json_response(body: String) -> ManifestHttpResponse {
        ManifestHttpResponse::new(
            200,
            Some(body.len() as u64),
            Some("application/json".to_owned()),
            Box::new(Cursor::new(body.into_bytes())),
        )
    }

    fn status_response(status: u16) -> ManifestHttpResponse {
        ManifestHttpResponse::new(
            status,
            Some(0),
            Some("application/json".to_owned()),
            Box::new(Cursor::new(Vec::new())),
        )
    }

    fn run_with_component_content_type(
        content_type: Option<&str>,
    ) -> Result<ManifestAcquisition, AppError> {
        let body = component_body();
        let component = ManifestHttpResponse::new(
            200,
            Some(body.len() as u64),
            content_type.map(str::to_owned),
            Box::new(Cursor::new(body.into_bytes())),
        );
        let transport = FakeTransport::with(vec![
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(component),
            Ok(json_response("{}".into())),
        ]);
        let mut gateway = BungieManifestGateway::new(key(), transport, clock());
        gateway.load_definitions()
    }

    struct CountingReader {
        remaining: usize,
        read: usize,
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Ok(0);
            }
            let count = buffer.len().min(self.remaining);
            buffer.iter_mut().take(count).for_each(|byte| *byte = b'x');
            self.remaining -= count;
            self.read += count;
            Ok(count)
        }
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("reader failure"))
        }
    }

    #[test]
    fn gateway_world_components_use_twelve_urls_without_key() {
        let mut index: serde_json::Value =
            serde_json::from_str(&index_body(COMPONENT_PATH)).unwrap();
        let paths = [
            "/common/destiny2_content/groups.json",
            "/common/destiny2_content/sets.json",
            "/common/destiny2_content/socket-types.json",
            "/common/destiny2_content/socket-categories.json",
            "/common/destiny2_content/plug-sets.json",
            "/common/destiny2_content/m.json",
            "/common/destiny2_content/a.json",
            "/common/destiny2_content/mod.json",
            "/common/destiny2_content/objectives.json",
            "/common/destiny2_content/types.json",
            "/common/destiny2_content/stats.json",
            "/common/destiny2_content/loadout-names.json",
        ];
        for (kind, path) in [
            "DestinyStatGroupDefinition",
            "DestinyEquipableItemSetDefinition",
            "DestinySocketTypeDefinition",
            "DestinySocketCategoryDefinition",
            "DestinyPlugSetDefinition",
            "DestinyMilestoneDefinition",
            "DestinyActivityDefinition",
            "DestinyActivityModifierDefinition",
            "DestinyObjectiveDefinition",
            "DestinyActivityTypeDefinition",
            "DestinyStatDefinition",
            "DestinyLoadoutNameDefinition",
        ]
        .into_iter()
        .zip(paths)
        {
            index["Response"]["jsonWorldComponentContentPaths"]["en"][kind] = json!(path);
        }
        let transport = FakeTransport::with(vec![
            Ok(json_response(index.to_string())),
            Ok(json_response(r#"{"123":{"hash":123,"scaledStats":[]}}"#.into())),
            Ok(json_response(r#"{"456":{"hash":456,"displayProperties":{"name":"Synthetic set"},"setItems":[42],"setPerks":[{"requiredSetCount":2,"sandboxPerkHash":7}]}}"#.into())),
            Ok(json_response("{}".into())),
            Ok(json_response("{}".into())),
            Ok(json_response("{}".into())),
            Ok(json_response(component_body())),
            Ok(json_response(component_body())),
            Ok(json_response(component_body())),
            Ok(json_response(component_body())),
            Ok(json_response(component_body())),
            Ok(json_response(component_body())),
        ]);
        let mut gateway = BungieManifestGateway::new(key(), transport, clock());
        let index = gateway.fetch_index().unwrap();
        let mut unsafe_plan = index.clone();
        unsafe_plan.loadout_name_path = "/common/destiny2_content/../bad.json".into();
        assert!(gateway.load_world_components(&unsafe_plan).is_err());
        assert_eq!(gateway.transport().requests.len(), 1);
        // The new component has top-level names, unlike display-based records.
        gateway.transport.responses.push_back(Ok(json_response(
            r#"{"42":{"hash":42,"name":"PvE"}}"#.into(),
        )));
        let batch = gateway.load_world_components(&index).unwrap();
        assert_eq!(batch.version.as_str(), "manifest-v1");
        assert_eq!(batch.milestones.definitions.len(), 1);
        assert_eq!(batch.activities.definitions.len(), 1);
        assert_eq!(batch.modifiers.definitions.len(), 1);
        assert_eq!(batch.stats.definitions.len(), 1);
        assert_eq!(batch.loadout_names.definitions[0].name(), "PvE");
        let requests = &gateway.transport().requests;
        assert_eq!(batch.armor_catalogs.len(), 2);
        assert_eq!(requests.len(), 13);
        assert!(requests[0].has_api_key);
        for (request, path) in requests[1..].iter().zip(paths) {
            assert_eq!(request.url, format!("{MANIFEST_ORIGIN}{path}"));
            assert!(!request.has_api_key);
        }
    }

    #[test]
    fn sandbox_failure_preserves_published_manifest_and_descriptions() {
        use destiny_core::ports::{DefinitionRepository, WorldDefinitionRepository};
        for failed_component in [json_response("not json".into()), status_response(503)] {
            let transport = FakeTransport::with(vec![
                Ok(json_response(index_body(COMPONENT_PATH))),
                Ok(json_response(r#"{"42":{"hash":42,"displayProperties":{"name":"Facet"},"perks":[{"perkHash":7}]}}"#.into())),
                Ok(json_response(r#"{"7":{"hash":7,"isDisplayable":true,"displayProperties":{"description":"Prior effect"}}}"#.into())),
                Ok(json_response(index_body(COMPONENT_PATH).replace("manifest-v1", "manifest-v2"))),
                Ok(json_response(component_body())),
                Ok(failed_component),
            ]);
            let gateway = BungieManifestGateway::new(key(), transport, clock())
                .with_read_retry(ReadRetry::single_attempt());
            let mut service = destiny_app::ManifestService::new(
                gateway,
                crate::sqlite::SqliteStore::open_in_memory().unwrap(),
            );
            service.acquire_and_publish(Timestamp::new(1)).unwrap();
            let metadata = service.repository().current_metadata().unwrap();
            let hash = destiny_core::ItemHash::new(42);
            let description = service.repository().lookup_item_description(hash).unwrap();
            assert_eq!(description, Some("Prior effect".into()));
            assert!(service.sync(Timestamp::new(2)).is_err());
            assert_eq!(service.repository().current_metadata().unwrap(), metadata);
            assert_eq!(
                service.repository().lookup_item_description(hash).unwrap(),
                description
            );
            assert!(
                matches!(service.repository().lookup_definition(hash).unwrap(), destiny_core::Resolution::Unique(item) if item.name == "Facet")
            );
        }
    }

    #[test]
    fn gateway_fetches_exact_urls_and_key_only_on_index() {
        let transport = FakeTransport::with(vec![
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(json_response(component_body())),
            Ok(json_response("{}".into())),
        ]);
        let mut gateway = BungieManifestGateway::new(key(), transport, clock());

        let acquisition = gateway.load_definitions().expect("acquisition");
        assert_eq!(acquisition.version.as_str(), "manifest-v1");
        assert_eq!(acquisition.definitions.len(), 1);
        assert_eq!(acquisition.definitions[0].hash.value(), 42);
        assert_eq!(acquisition.definitions[0].name, "Test Item");

        let requests = &gateway.transport().requests;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[2].url,
            format!("{MANIFEST_ORIGIN}{COMPONENT_PATH}")
        );
        assert!(!requests[2].has_api_key);
        assert_eq!(requests[0].url, MANIFEST_ENDPOINT);
        assert!(requests[0].has_api_key);
        assert_eq!(
            requests[1].url,
            format!("{MANIFEST_ORIGIN}{COMPONENT_PATH}")
        );
        assert!(!requests[1].has_api_key);
    }

    #[test]
    fn gateway_retries_a_rate_limited_index_with_bounded_pacing() {
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(429)),
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(json_response(component_body())),
            Ok(json_response("{}".into())),
        ]);

        let acquisition = gateway
            .load_definitions()
            .expect("the second index attempt succeeds");
        assert_eq!(acquisition.definitions.len(), 1);
        assert_eq!(gateway.transport().requests.len(), 4);
        assert_eq!(delay.waits(), vec![Duration::from_millis(10)]);
    }

    #[test]
    fn gateway_retries_a_transient_status_on_a_definition_read() {
        // A definition acquisition never feeds a mutation observation, so a
        // transient provider-unavailable failure is retried too.
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(503)),
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(json_response(component_body())),
            Ok(json_response("{}".into())),
        ]);

        gateway
            .load_definitions()
            .expect("the second index attempt succeeds");
        assert_eq!(gateway.transport().requests.len(), 4);
        assert_eq!(delay.waits(), vec![Duration::from_millis(10)]);
    }

    #[test]
    fn gateway_exhausts_a_persistent_rate_limit_at_the_attempt_ceiling() {
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(429)),
            Ok(status_response(429)),
            Ok(status_response(429)),
        ]);

        let error = gateway
            .load_definitions()
            .expect_err("a persistent rate limit is terminal after the ceiling");
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(gateway.transport().requests.len(), 3);
        // Two repeats for three attempts, paced at 10 ms then 20 ms.
        assert_eq!(
            delay.waits(),
            vec![Duration::from_millis(10), Duration::from_millis(20)]
        );
    }

    #[test]
    fn gateway_never_retries_a_validation_status() {
        let (mut gateway, delay) = retrying_gateway(vec![Ok(status_response(400))]);

        let error = gateway.load_definitions().expect_err("400 is terminal");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(gateway.transport().requests.len(), 1);
        assert!(delay.waits().is_empty());
    }

    #[test]
    fn gateway_suppresses_a_retry_after_beyond_the_policy_ceiling() {
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(429).with_retry_after(RetryAfterHeader::DeltaSeconds(5))),
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(json_response(component_body())),
        ]);

        let error = gateway
            .load_definitions()
            .expect_err("a wait beyond the ceiling must not be shortened");
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(gateway.transport().requests.len(), 1);
        assert!(delay.waits().is_empty());
    }

    #[test]
    fn gateway_honors_a_retry_after_within_the_policy_ceiling() {
        let delay = RecordingDelay::default();
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(status_response(429).with_retry_after(RetryAfterHeader::DeltaSeconds(2))),
                Ok(json_response(index_body(COMPONENT_PATH))),
                Ok(json_response(component_body())),
                Ok(json_response("{}".into())),
            ]),
            clock(),
        )
        .with_read_retry(ReadRetry::new(
            ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(5)),
            delay.clone(),
        ));

        gateway
            .load_definitions()
            .expect("the hinted repeat succeeds");
        assert_eq!(gateway.transport().requests.len(), 4);
        assert_eq!(delay.waits(), vec![Duration::from_secs(2)]);
    }

    #[test]
    fn gateway_honors_an_http_date_retry_after_with_the_injected_clock() {
        let delay = RecordingDelay::default();
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(status_response(429)
                    .with_retry_after(RetryAfterHeader::HttpDate(GATEWAY_NOW + 2))),
                Ok(json_response(index_body(COMPONENT_PATH))),
                Ok(json_response(component_body())),
                Ok(json_response("{}".into())),
            ]),
            clock(),
        )
        .with_read_retry(ReadRetry::new(
            ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(5)),
            delay.clone(),
        ));

        gateway
            .load_definitions()
            .expect("the date-hinted repeat succeeds");
        assert_eq!(gateway.transport().requests.len(), 4);
        assert_eq!(delay.waits(), vec![Duration::from_secs(2)]);
    }

    #[test]
    fn gateway_suppresses_an_http_date_retry_after_beyond_the_policy_ceiling() {
        let delay = RecordingDelay::default();
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(status_response(429)
                    .with_retry_after(RetryAfterHeader::HttpDate(GATEWAY_NOW + 600))),
                Ok(json_response(index_body(COMPONENT_PATH))),
                Ok(json_response(component_body())),
            ]),
            clock(),
        )
        .with_read_retry(ReadRetry::new(
            ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(5)),
            delay.clone(),
        ));

        let error = gateway
            .load_definitions()
            .expect_err("a date beyond the ceiling must not be shortened");
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(gateway.transport().requests.len(), 1);
        assert!(delay.waits().is_empty());
    }

    #[test]
    fn gateway_suppresses_an_http_date_when_the_clock_is_unavailable() {
        let delay = RecordingDelay::default();
        let mut gateway = BungieManifestGateway::new(
            key(),
            FakeTransport::with(vec![
                Ok(status_response(429)
                    .with_retry_after(RetryAfterHeader::HttpDate(GATEWAY_NOW + 2))),
                Ok(json_response(index_body(COMPONENT_PATH))),
                Ok(json_response(component_body())),
            ]),
            FailClock,
        )
        .with_read_retry(ReadRetry::new(
            ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(5)),
            delay.clone(),
        ));

        let error = gateway
            .load_definitions()
            .expect_err("an uninterpretable date is not repeated");
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(gateway.transport().requests.len(), 1);
        assert!(delay.waits().is_empty());
    }

    #[test]
    fn gateway_suppresses_an_invalid_retry_after() {
        let (mut gateway, delay) = retrying_gateway(vec![
            Ok(status_response(429).with_retry_after(RetryAfterHeader::Invalid)),
            Ok(json_response(index_body(COMPONENT_PATH))),
            Ok(json_response(component_body())),
        ]);

        let error = gateway
            .load_definitions()
            .expect_err("a malformed instruction must not be repeated");
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert_eq!(gateway.transport().requests.len(), 1);
        assert!(delay.waits().is_empty());
    }

    #[test]
    fn present_non_utf8_retry_after_is_invalid_not_absent() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(classify_retry_after(&headers), RetryAfterHeader::Absent);

        // Obs-text bytes are a syntactically valid header value but are not
        // valid UTF-8, so the header is present yet unusable -- never absent.
        let non_utf8 = [0xffu8, 0xfe];
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_bytes(non_utf8.as_slice())
                .expect("obs-text header bytes are a valid header value"),
        );
        assert_eq!(classify_retry_after(&headers), RetryAfterHeader::Invalid);

        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("120"),
        );
        assert_eq!(
            classify_retry_after(&headers),
            RetryAfterHeader::DeltaSeconds(120)
        );
    }

    #[test]
    fn api_key_and_debug_output_omit_secrets() {
        let sentinel = "sentinel-secret-api-key";
        let api_key = ManifestApiKey::new(sentinel).expect("valid key");
        assert_eq!(format!("{api_key:?}"), "ManifestApiKey");
        assert_eq!(api_key.expose(), sentinel);

        let request = ManifestHttpRequest::new(MANIFEST_ENDPOINT, Some(&api_key));
        let request_debug = format!("{request:?}");
        assert!(request_debug.contains(MANIFEST_ENDPOINT));
        assert!(request_debug.contains("<redacted>"));
        assert!(!request_debug.contains(sentinel));

        let response = ManifestHttpResponse::new(
            200,
            Some(12),
            Some("sentinel-content-type".to_owned()),
            Box::new(Cursor::new(b"sentinel-body".to_vec())),
        );
        let response_debug = format!("{response:?}");
        assert!(!response_debug.contains("sentinel-content-type"));
        assert!(!response_debug.contains("sentinel-body"));
    }

    #[test]
    fn invalid_api_keys_are_configuration_failures() {
        for invalid in ["", " ", "  ", "has space", "has\ttab", "has\u{1}control"] {
            let error = ManifestApiKey::new(invalid).expect_err("invalid key");
            assert_eq!(error.category(), ErrorCategory::ConfigurationError);
        }
    }

    #[test]
    fn status_codes_map_to_categories() {
        let cases = [
            (401, ErrorCategory::ProviderAuthentication),
            (403, ErrorCategory::ProviderAuthentication),
            (429, ErrorCategory::ProviderRateLimit),
            (500, ErrorCategory::ProviderUnavailable),
            (503, ErrorCategory::ProviderUnavailable),
            (400, ErrorCategory::ProviderValidation),
        ];
        for (status, expected) in cases {
            let transport = FakeTransport::with(vec![Ok(status_response(status))]);
            // A single attempt keeps this a category-mapping test rather than a
            // retry test; the bounded retry behavior has dedicated coverage.
            let mut gateway = BungieManifestGateway::new(key(), transport, clock())
                .with_read_retry(ReadRetry::single_attempt());
            let error = gateway.load_definitions().expect_err("status rejection");
            assert_eq!(error.category(), expected, "status {status}");
        }
    }

    #[test]
    fn transport_errors_map_to_categories() {
        let cases = [
            (
                ManifestTransportError::Timeout,
                ErrorCategory::ProviderUnavailable,
            ),
            (
                ManifestTransportError::Unavailable,
                ErrorCategory::ProviderUnavailable,
            ),
            (
                ManifestTransportError::Configuration,
                ErrorCategory::ConfigurationError,
            ),
        ];
        for (transport_error, expected) in cases {
            let transport = FakeTransport::with(vec![Err(transport_error)]);
            // A single attempt keeps this a category-mapping test; a manifest
            // read retries a transient transport failure with dedicated coverage.
            let mut gateway = BungieManifestGateway::new(key(), transport, clock())
                .with_read_retry(ReadRetry::single_attempt());
            let error = gateway.load_definitions().expect_err("transport rejection");
            assert_eq!(error.category(), expected, "{transport_error:?}");
        }
    }

    #[test]
    fn component_content_type_must_be_json() {
        let rejected = [
            None,
            Some("text/plain"),
            Some("application/xml"),
            Some("application/json; charset"),
            Some("application/json; =utf-8"),
            Some("application/json\u{1}"),
        ];
        for content_type in rejected {
            let error =
                run_with_component_content_type(content_type).expect_err("content type rejection");
            assert_eq!(
                error.category(),
                ErrorCategory::ProviderValidation,
                "{content_type:?}"
            );
        }

        let accepted = [
            Some("application/json"),
            Some("application/json "),
            Some("APPLICATION/JSON"),
            Some("application/json; charset=utf-8"),
            Some("application/json; charset=\"UTF-8\""),
        ];
        for content_type in accepted {
            assert!(
                run_with_component_content_type(content_type).is_ok(),
                "{content_type:?}"
            );
        }
    }

    #[test]
    fn announced_oversize_is_rejected_without_reading() {
        let mut body = CountingReader {
            remaining: 100,
            read: 0,
        };
        let error = read_bounded_body(&mut body, Some(100), 4).expect_err("announced oversize");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(body.read, 0);
    }

    #[test]
    fn unannounced_overflow_and_reader_errors_are_categorized() {
        let error = read_bounded_body(&mut Cursor::new(vec![b'x'; 5]), None, 4)
            .expect_err("unannounced overflow");
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);

        let error = read_bounded_body(&mut FailingReader, None, 8).expect_err("reader error");
        assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
    }

    #[test]
    fn bounded_body_accepts_the_exact_limit() {
        let body = read_bounded_body(&mut Cursor::new(vec![b'x'; 4]), Some(4), 4)
            .expect("announced length at limit");
        assert_eq!(body.len(), 4);

        let body = read_bounded_body(&mut Cursor::new(vec![b'x'; 4]), None, 4)
            .expect("unannounced length at limit");
        assert_eq!(body.len(), 4);
    }

    #[test]
    fn allowlist_accepts_only_index_and_validated_component_urls() {
        let component_url = format!("{MANIFEST_ORIGIN}{COMPONENT_PATH}");
        assert!(is_allowed_manifest_url(MANIFEST_ENDPOINT));
        assert!(is_allowed_manifest_url(&component_url));

        let rejected = [
            "http://www.bungie.net/Platform/Destiny2/Manifest/",
            "https://evil.example/Platform/Destiny2/Manifest/",
            "https://www.bungie.net.evil.example/Platform/Destiny2/Manifest/",
            "https://api.bungie.net/Platform/Destiny2/Manifest/",
            "https://www.bungie.net/Platform/Destiny2/Manifest/?x=1",
            "https://www.bungie.net/Platform/Destiny2/Manifest/#frag",
            "https://www.bungie.net/common/destiny2_content/x.json?y=1",
            "https://www.bungie.net/common/destiny2_content/json/en/../secret.json",
            "https://www.bungie.net/common/destiny2_content/json/en/%2e%2e/secret.json",
            "https://www.bungie.net/common/destiny2_content/json/en/a\\b.json",
            "https://www.bungie.net/other/prefix/x.json",
            "https://www.bungie.net/common/destiny2_content/x.txt",
        ];
        for url in rejected {
            assert!(!is_allowed_manifest_url(url), "should reject {url}");
        }
    }

    #[test]
    fn fixtures_publish_offline_and_replace_prior_definitions() {
        use destiny_app::manifest::ManifestService;
        use destiny_app::test_support::InMemoryDefinitionRepository;
        use destiny_core::{
            BucketHash, Dataset, DefinitionRepository, DefinitionVersion, ItemDefinition, ItemHash,
            Resolution, SourceMarker, Timestamp,
        };
        use std::cell::RefCell;
        use std::rc::Rc;

        const FIXTURE_VERSION: &str = "manifest-fixture-v1";
        const FIXTURE_CHECKED_AT: Timestamp = Timestamp::new(4_242);
        const FIXTURE_RECORD_COUNT: usize = 4;
        const PRIOR_FAKE_HASH: i64 = 77_777;
        const FIXTURE_COMPONENT_PATH: &str =
            "/common/destiny2_content/json/en/DestinyInventoryItemDefinition-fixture.json";

        struct SharedFakeTransport {
            requests: Rc<RefCell<Vec<RecordedRequest>>>,
            responses: VecDeque<QueuedResponse>,
        }

        impl ManifestTransport for SharedFakeTransport {
            fn get(
                &mut self,
                request: &ManifestHttpRequest<'_>,
            ) -> Result<ManifestHttpResponse, ManifestTransportError> {
                self.requests.borrow_mut().push(RecordedRequest {
                    url: request.url().to_owned(),
                    has_api_key: request.api_key().is_some(),
                });
                self.responses
                    .pop_front()
                    .expect("fake transport has a queued response")
            }
        }

        let fixture_response = |bytes: &[u8]| {
            ManifestHttpResponse::new(
                200,
                Some(bytes.len() as u64),
                Some("application/json".to_owned()),
                Box::new(Cursor::new(bytes.to_vec())),
            )
        };

        let index_fixture = include_bytes!("../tests/fixtures/manifest-index.json");
        let component_fixture = include_bytes!("../tests/fixtures/inventory-items.json");

        let fixture_records = serde_json::from_slice::<serde_json::Value>(component_fixture)
            .expect("inventory fixture is JSON")
            .as_object()
            .expect("inventory fixture is an object")
            .len();
        assert_eq!(fixture_records, FIXTURE_RECORD_COUNT);
        assert!(fixture_records <= 100_000);

        let requests = Rc::new(RefCell::new(Vec::new()));
        let transport = SharedFakeTransport {
            requests: Rc::clone(&requests),
            responses: VecDeque::from(vec![
                Ok(fixture_response(index_fixture)),
                Ok(fixture_response(component_fixture)),
                Ok(fixture_response(b"{}")),
            ]),
        };
        let gateway = BungieManifestGateway::new(key(), transport, clock());

        let prior_version = DefinitionVersion::new("prior-fake-v1");
        let repository = InMemoryDefinitionRepository::new(
            prior_version.clone(),
            vec![ItemDefinition {
                armor: Default::default(),
                non_transferrable: None,
                does_postmaster_pull_have_side_effects: None,
                is_instance_item: None,
                max_stack_size: None,
                stack_unique_label: None,
                hash: ItemHash::new(PRIOR_FAKE_HASH),
                bucket_hash: None,
                name: "Prior Fake".to_owned(),
                item_type: None,
                tier: None,
                element: None,
                slot: None,
                class_type: None,
                plug_category: None,
                tags: Vec::new(),
                version: prior_version,
            }],
        );

        let mut service = ManifestService::new(gateway, repository);
        let outcome = service
            .acquire_and_publish(FIXTURE_CHECKED_AT)
            .expect("offline publication");

        assert_eq!(outcome.version.as_str(), FIXTURE_VERSION);
        assert_eq!(outcome.definition_count, FIXTURE_RECORD_COUNT);
        assert_eq!(outcome.revision.value(), 1);

        let recorded = requests.borrow();
        assert_eq!(recorded.len(), 3);
        assert!(!recorded[2].has_api_key);
        assert_eq!(recorded[0].url, MANIFEST_ENDPOINT);
        assert!(recorded[0].has_api_key);
        assert_eq!(
            recorded[1].url,
            format!("{MANIFEST_ORIGIN}{FIXTURE_COMPONENT_PATH}")
        );
        assert!(!recorded[1].has_api_key);

        let repository = service.repository();
        assert_eq!(
            repository
                .current_definition_version()
                .expect("current version")
                .as_str(),
            FIXTURE_VERSION
        );
        assert_eq!(repository.definition_count(), FIXTURE_RECORD_COUNT);
        let metadata = repository
            .current_metadata()
            .expect("metadata read")
            .expect("committed metadata");
        assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
        assert_eq!(metadata.source, SourceMarker::Manifest);
        assert_eq!(metadata.checked_at, FIXTURE_CHECKED_AT);
        assert_eq!(metadata.revision.value(), 1);
        assert_eq!(metadata.definition_version.as_str(), FIXTURE_VERSION);
        assert_eq!(
            repository
                .lookup_definition(ItemHash::new(PRIOR_FAKE_HASH))
                .expect("prior lookup"),
            Resolution::Missing
        );
        assert_eq!(repository.publication_attempts(), 1);

        let unique = |hash: i64| match repository
            .lookup_definition(ItemHash::new(hash))
            .expect("definition lookup")
        {
            Resolution::Unique(definition) => definition,
            other => panic!("expected one normalized definition for {hash}: {other:?}"),
        };

        let suros = unique(2_522_695);
        assert_eq!(suros.name, "Suros Regime");
        assert_eq!(suros.item_type.as_deref(), Some("Auto Rifle"));
        assert_eq!(suros.tier.as_deref(), Some("Exotic"));
        assert_eq!(suros.bucket_hash, Some(BucketHash::new(1_498_876_634)));
        assert_eq!(suros.version.as_str(), FIXTURE_VERSION);
        assert!(suros.tags.is_empty());

        let ghost = unique(999);
        assert_eq!(ghost.name, "Ghost Shell");
        assert_eq!(ghost.item_type, None);
        assert_eq!(ghost.bucket_hash, None);
        assert_eq!(ghost.tier, None);

        let wrong_types = unique(42);
        assert_eq!(wrong_types.name, "Wrong Types");
        assert_eq!(wrong_types.item_type, None);
        assert_eq!(wrong_types.bucket_hash, None);
        assert_eq!(wrong_types.tier, None);

        let missing = unique(1709);
        assert_eq!(missing.name, "Missing Inventory Fields");
        assert_eq!(missing.item_type.as_deref(), Some("Quest Step"));
        assert_eq!(missing.bucket_hash, None);
        assert_eq!(missing.tier, None);
    }
}
