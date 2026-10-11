//! Offline DIM JSON/URL codec. Decode preserves identity; import assigns a fresh UUID.
use destiny_core::build::*;
use destiny_core::{BucketHash, ItemHash, ItemInstanceId, PerkHash};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_DIM_INPUT_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DimLoadoutError {
    InvalidJson,
    InvalidUrl,
    Validation,
    InputTooLarge,
    ShareLinkRequiresOnlineImport,
    RandomUnavailable,
}

impl fmt::Display for DimLoadoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidJson => "invalid DIM loadout JSON",
            Self::InvalidUrl => "invalid DIM loadout URL",
            Self::Validation => "invalid DIM build definition",
            Self::InputTooLarge => "DIM input exceeds size limit",
            Self::ShareLinkRequiresOnlineImport => "share link requires online import",
            Self::RandomUnavailable => "could not generate build identifier",
        })
    }
}

impl std::error::Error for DimLoadoutError {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimLoadout {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub class_type: u8,
    #[serde(default)]
    pub equipped: Vec<DimLoadoutItem>,
    #[serde(default)]
    pub unequipped: Vec<DimLoadoutItem>,
    /// Historical DIM field, migrated to the two clear flags on decode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_space: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<DimLoadoutParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimLoadoutItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub hash: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_overrides: Option<BTreeMap<String, u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crafted_date: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimStatConstraint {
    pub stat_hash: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_stat: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_stat: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_tier: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tier: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimArtifactUnlocks {
    pub unlocked_item_hashes: Vec<u32>,
    pub season_number: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimInGameIdentifiers {
    pub color_hash: u32,
    pub icon_hash: u32,
    pub name_hash: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimLoadoutParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat_constraints: Option<Vec<DimStatConstraint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mods: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mods_by_bucket: Option<BTreeMap<String, Vec<u32>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perks: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub set_bonuses: Option<BTreeMap<String, u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exotic_armor_hash: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assume_armor_masterwork: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_stat_mods: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_runtime_stat_benefits: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_mods: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_weapons: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clear_armor: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_unlocks: Option<DimArtifactUnlocks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_game_identifiers: Option<DimInGameIdentifiers>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn decimal_key(key: &str) -> Result<u32, DimLoadoutError> {
    let value = key
        .parse::<u32>()
        .map_err(|_| DimLoadoutError::Validation)?;
    if value.to_string() != key {
        return Err(DimLoadoutError::Validation);
    }
    Ok(value)
}

fn item_hash(value: u32) -> ItemHash {
    ItemHash::new(i64::from(value))
}

impl TryFrom<DimLoadoutItem> for BuildItem {
    type Error = DimLoadoutError;

    fn try_from(value: DimLoadoutItem) -> Result<Self, Self::Error> {
        let mut item = Self {
            instance_id: value.id.map(ItemInstanceId::new),
            hash: item_hash(value.hash),
            amount: value.amount,
            socket_overrides: value
                .socket_overrides
                .unwrap_or_default()
                .into_iter()
                .map(|(index, hash)| {
                    let index = u16::try_from(decimal_key(&index)?)
                        .map_err(|_| DimLoadoutError::Validation)?;
                    Ok((index, item_hash(hash)))
                })
                .collect::<Result<_, DimLoadoutError>>()?,
            crafted_date: value.crafted_date,
        };
        item.normalize();
        Ok(item)
    }
}

// serde_json object ordering may change with workspace features; canonicalize recursively.
fn canonical_json(value: &Value) -> Result<String, DimLoadoutError> {
    match value {
        Value::Object(map) => {
            let ordered: BTreeMap<_, _> = map.iter().collect();
            let mut fields = Vec::with_capacity(ordered.len());
            for (key, value) in ordered {
                let key = serde_json::to_string(key).map_err(|_| DimLoadoutError::InvalidJson)?;
                fields.push(format!("{key}:{}", canonical_json(value)?));
            }
            Ok(format!("{{{}}}", fields.join(",")))
        }
        Value::Array(values) => Ok(format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        _ => serde_json::to_string(value).map_err(|_| DimLoadoutError::InvalidJson),
    }
}

impl TryFrom<DimLoadoutParameters> for BuildParameters {
    type Error = DimLoadoutError;

    fn try_from(p: DimLoadoutParameters) -> Result<Self, Self::Error> {
        let extra = if p.extra.is_empty() {
            None
        } else {
            Some(canonical_json(
                &serde_json::to_value(&p.extra).map_err(|_| DimLoadoutError::InvalidJson)?,
            )?)
        };
        Ok(Self {
            stat_constraints: p.stat_constraints.map(|values| {
                values
                    .into_iter()
                    .map(|c| {
                        StatConstraint::normalize(
                            c.stat_hash,
                            c.min_stat,
                            c.max_stat,
                            c.min_tier,
                            c.max_tier,
                        )
                    })
                    .collect()
            }),
            mods: p.mods.map(|v| v.into_iter().map(item_hash).collect()),
            mods_by_bucket: p
                .mods_by_bucket
                .map(|v| {
                    v.into_iter()
                        .map(|(key, hashes)| {
                            Ok((
                                BucketHash::new(i64::from(decimal_key(&key)?)),
                                hashes.into_iter().map(item_hash).collect(),
                            ))
                        })
                        .collect::<Result<_, DimLoadoutError>>()
                })
                .transpose()?,
            perks: p.perks.map(|v| v.into_iter().map(PerkHash::new).collect()),
            set_bonuses: p
                .set_bonuses
                .map(|v| {
                    v.into_iter()
                        .map(|(key, count)| Ok((decimal_key(&key)?, count)))
                        .collect::<Result<_, DimLoadoutError>>()
                })
                .transpose()?,
            exotic_armor_hash: p
                .exotic_armor_hash
                .map(|hash| match hash {
                    -1 => Ok(ExoticArmor::None),
                    -2 => Ok(ExoticArmor::Any),
                    value => u32::try_from(value)
                        .map(|v| ExoticArmor::Item(item_hash(v)))
                        .map_err(|_| DimLoadoutError::Validation),
                })
                .transpose()?,
            query: p.query,
            assume_armor_masterwork: p
                .assume_armor_masterwork
                .map(|v| match v {
                    1 => Ok(AssumeArmorMasterwork::None),
                    2 => Ok(AssumeArmorMasterwork::Legendary),
                    3 => Ok(AssumeArmorMasterwork::All),
                    4 => Ok(AssumeArmorMasterwork::ArtificeExotic),
                    _ => Err(DimLoadoutError::Validation),
                })
                .transpose()?,
            auto_stat_mods: p.auto_stat_mods,
            include_runtime_stat_benefits: p.include_runtime_stat_benefits,
            clear_mods: p.clear_mods,
            clear_weapons: p.clear_weapons,
            clear_armor: p.clear_armor,
            artifact_unlocks: p.artifact_unlocks.map(|a| BuildArtifactUnlocks {
                unlocked_item_hashes: a.unlocked_item_hashes.into_iter().map(item_hash).collect(),
                season_number: a.season_number,
            }),
            in_game_identifiers: p.in_game_identifiers.map(|i| BuildInGameIdentifiers {
                color_hash: i.color_hash,
                icon_hash: i.icon_hash,
                name_hash: i.name_hash,
            }),
            extra,
        })
    }
}

impl TryFrom<DimLoadout> for BuildDefinition {
    type Error = DimLoadoutError;

    fn try_from(value: DimLoadout) -> Result<Self, Self::Error> {
        let mut parameters = value
            .parameters
            .map(BuildParameters::try_from)
            .transpose()?;
        if value.clear_space == Some(true)
            && parameters
                .as_ref()
                .is_none_or(|p| p.clear_armor.is_none() && p.clear_weapons.is_none())
        {
            let p = parameters.get_or_insert_with(BuildParameters::default);
            p.clear_armor = Some(true);
            p.clear_weapons = Some(true);
        }
        let build = Self {
            id: value.id,
            name: value.name,
            notes: value.notes,
            class_type: value.class_type,
            equipped: value
                .equipped
                .into_iter()
                .map(BuildItem::try_from)
                .collect::<Result<_, _>>()?,
            unequipped: value
                .unequipped
                .into_iter()
                .map(BuildItem::try_from)
                .collect::<Result<_, _>>()?,
            parameters,
            created_at: value.created_at,
            last_updated_at: value.last_updated_at,
        };
        build.validate().map_err(|_| DimLoadoutError::Validation)?;
        Ok(build)
    }
}

fn is_share_link(input: &str) -> bool {
    let value = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
        .unwrap_or(input);
    let value = value.strip_prefix("dim.gg/").unwrap_or(value);
    let id = value.split('/').next().unwrap_or_default();
    id.len() >= 7
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Decode and normalize a DIM API object or an offline /loadouts?loadout= URL.
/// Identity is retained for storage/export round trips; use import for external input.
pub fn parse_dim_loadout(input: &str) -> Result<BuildDefinition, DimLoadoutError> {
    if input.len() > MAX_DIM_INPUT_BYTES {
        return Err(DimLoadoutError::InputTooLarge);
    }
    let input = input.trim();
    if is_share_link(input) {
        return Err(DimLoadoutError::ShareLinkRequiresOnlineImport);
    }
    let json = if input.starts_with('{') {
        input.to_owned()
    } else {
        let url = url::Url::parse(input).map_err(|_| DimLoadoutError::InvalidUrl)?;
        if !url.path().ends_with("/loadouts") {
            return Err(DimLoadoutError::InvalidUrl);
        }
        url.query_pairs()
            .find(|(key, _)| key == "loadout")
            .map(|(_, json)| json.into_owned())
            .filter(|v| !v.is_empty())
            .ok_or(DimLoadoutError::InvalidUrl)?
    };
    let dto: DimLoadout = serde_json::from_str(&json).map_err(|_| DimLoadoutError::InvalidJson)?;
    dto.try_into()
}

/// Import normalization assigns a fresh RFC 4122 version-4 UUID without new dependencies.
pub fn import_dim_loadout(input: &str) -> Result<BuildDefinition, DimLoadoutError> {
    let mut build = parse_dim_loadout(input)?;
    // The app's API-to-UI conversion uses amount || 1.
    for item in build.equipped.iter_mut().chain(&mut build.unequipped) {
        if item.amount.is_none_or(|amount| amount == 0) {
            item.amount = Some(1);
        }
    }
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| DimLoadoutError::RandomUnavailable)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    build.id = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            let prefix = if matches!(index, 4 | 6 | 8 | 10) {
                "-"
            } else {
                ""
            };
            format!("{prefix}{byte:02x}")
        })
        .collect();
    Ok(build)
}

impl From<&BuildItem> for DimLoadoutItem {
    fn from(item: &BuildItem) -> Self {
        Self {
            id: item.instance_id.as_ref().map(|v| v.as_str().to_owned()),
            hash: item.hash.value() as u32,
            amount: item.amount,
            socket_overrides: (!item.socket_overrides.is_empty()).then(|| {
                item.socket_overrides
                    .iter()
                    .map(|(index, hash)| (index.to_string(), hash.value() as u32))
                    .collect()
            }),
            crafted_date: item.crafted_date,
        }
    }
}

fn export_constraint(c: &StatConstraint) -> Result<DimStatConstraint, DimLoadoutError> {
    let mut dto = DimStatConstraint {
        stat_hash: c.stat_hash,
        min_stat: c.min_stat.map(i64::from),
        max_stat: c.max_stat.map(i64::from),
        min_tier: None,
        max_tier: None,
    };
    // DIM clamps modern and legacy pairs independently before resolving them.
    // A mixed pair can consequently have min > max. Retain the legacy side
    // for these pairs, otherwise re-importing would clamp away the original goal.
    if let (Some(min), Some(max)) = (c.min_stat, c.max_stat)
        && min > max
    {
        if max <= 100 && max.is_multiple_of(10) {
            dto.max_stat = None;
            dto.max_tier = Some(i64::from(max / 10));
        } else if min <= 100 && min.is_multiple_of(10) {
            dto.min_stat = None;
            dto.min_tier = Some(i64::from(min / 10));
        } else {
            return Err(DimLoadoutError::Validation);
        }
    }
    Ok(dto)
}

impl TryFrom<&BuildParameters> for DimLoadoutParameters {
    type Error = DimLoadoutError;

    fn try_from(p: &BuildParameters) -> Result<Self, Self::Error> {
        p.validate().map_err(|_| DimLoadoutError::Validation)?;
        let extra = match &p.extra {
            None => BTreeMap::new(),
            Some(json) => serde_json::from_str::<BTreeMap<String, Value>>(json)
                .map_err(|_| DimLoadoutError::Validation)?,
        };
        // Flatten must never emit duplicate keys that override typed fields.
        const KNOWN: &[&str] = &[
            "statConstraints",
            "mods",
            "modsByBucket",
            "perks",
            "setBonuses",
            "exoticArmorHash",
            "query",
            "assumeArmorMasterwork",
            "autoStatMods",
            "includeRuntimeStatBenefits",
            "clearMods",
            "clearWeapons",
            "clearArmor",
            "artifactUnlocks",
            "inGameIdentifiers",
        ];
        if extra.keys().any(|key| KNOWN.contains(&key.as_str())) {
            return Err(DimLoadoutError::Validation);
        }
        Ok(Self {
            stat_constraints: p
                .stat_constraints
                .as_ref()
                .map(|v| v.iter().map(export_constraint).collect::<Result<_, _>>())
                .transpose()?,
            mods: p
                .mods
                .as_ref()
                .map(|v| v.iter().map(|h| h.value() as u32).collect()),
            mods_by_bucket: p.mods_by_bucket.as_ref().map(|v| {
                v.iter()
                    .map(|(h, mods)| {
                        (
                            h.value().to_string(),
                            mods.iter().map(|h| h.value() as u32).collect(),
                        )
                    })
                    .collect()
            }),
            perks: p
                .perks
                .as_ref()
                .map(|v| v.iter().map(|h| h.value()).collect()),
            set_bonuses: p
                .set_bonuses
                .as_ref()
                .map(|v| v.iter().map(|(h, count)| (h.to_string(), *count)).collect()),
            exotic_armor_hash: p.exotic_armor_hash.map(|v| match v {
                ExoticArmor::None => -1,
                ExoticArmor::Any => -2,
                ExoticArmor::Item(h) => h.value(),
            }),
            query: p.query.clone(),
            assume_armor_masterwork: p.assume_armor_masterwork.map(|v| match v {
                AssumeArmorMasterwork::None => 1,
                AssumeArmorMasterwork::Legendary => 2,
                AssumeArmorMasterwork::All => 3,
                AssumeArmorMasterwork::ArtificeExotic => 4,
            }),
            auto_stat_mods: p.auto_stat_mods,
            include_runtime_stat_benefits: p.include_runtime_stat_benefits,
            clear_mods: p.clear_mods,
            clear_weapons: p.clear_weapons,
            clear_armor: p.clear_armor,
            artifact_unlocks: p.artifact_unlocks.as_ref().map(|a| DimArtifactUnlocks {
                unlocked_item_hashes: a
                    .unlocked_item_hashes
                    .iter()
                    .map(|h| h.value() as u32)
                    .collect(),
                season_number: a.season_number,
            }),
            in_game_identifiers: p
                .in_game_identifiers
                .as_ref()
                .map(|i| DimInGameIdentifiers {
                    color_hash: i.color_hash,
                    icon_hash: i.icon_hash,
                    name_hash: i.name_hash,
                }),
            extra,
        })
    }
}

impl TryFrom<&BuildDefinition> for DimLoadout {
    type Error = DimLoadoutError;

    fn try_from(build: &BuildDefinition) -> Result<Self, Self::Error> {
        build.validate().map_err(|_| DimLoadoutError::Validation)?;
        Ok(Self {
            id: build.id.clone(),
            name: build.name.clone(),
            notes: build.notes.clone(),
            class_type: build.class_type,
            equipped: build.equipped.iter().map(DimLoadoutItem::from).collect(),
            unequipped: build.unequipped.iter().map(DimLoadoutItem::from).collect(),
            clear_space: None,
            parameters: build
                .parameters
                .as_ref()
                .map(DimLoadoutParameters::try_from)
                .transpose()?,
            created_at: build.created_at,
            last_updated_at: build.last_updated_at,
        })
    }
}

pub fn export_dim_loadout(build: &BuildDefinition) -> Result<String, DimLoadoutError> {
    serde_json::to_string(&DimLoadout::try_from(build)?).map_err(|_| DimLoadoutError::InvalidJson)
}
