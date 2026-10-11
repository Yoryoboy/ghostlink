//! Provider-neutral build definitions. Optional values retain DIM's unset semantics.
use crate::{AppError, BucketHash, ErrorCategory, ItemHash, ItemInstanceId, PerkHash};
use std::collections::BTreeMap;

pub const MAX_BUILD_ITEMS: usize = 256;
pub const MAX_BUILD_SOCKETS: usize = 128;
pub const MAX_BUILD_NAME_BYTES: usize = 1024;
pub const MAX_BUILD_TEXT_BYTES: usize = 65536;
pub const MAX_BUILD_PARAMETERS: usize = 1024;
pub const MAX_BUILD_STAT: u16 = 200;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildDefinition {
    pub id: String,
    pub name: String,
    pub notes: Option<String>,
    /// DestinyClass: Titan 0, Hunter 1, Warlock 2, unrestricted 3.
    pub class_type: u8,
    pub equipped: Vec<BuildItem>,
    pub unequipped: Vec<BuildItem>,
    pub parameters: Option<BuildParameters>,
    pub created_at: Option<u64>,
    pub last_updated_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildItem {
    pub instance_id: Option<ItemInstanceId>,
    pub hash: ItemHash,
    pub amount: Option<u32>,
    pub socket_overrides: Vec<(u16, ItemHash)>,
    /// UTC epoch seconds, unlike API-created timestamps.
    pub crafted_date: Option<u64>,
}

impl BuildItem {
    pub fn normalize(&mut self) {
        self.socket_overrides.sort_by_key(|entry| entry.0);
        // DIM's missing-item IDs are UI placeholders, not real instance IDs.
        if self
            .instance_id
            .as_ref()
            .is_some_and(|id| id.as_str() == "0")
        {
            self.instance_id = None;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExoticArmor {
    None,
    Any,
    Item(ItemHash),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssumeArmorMasterwork {
    None,
    Legendary,
    All,
    ArtificeExotic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatConstraint {
    pub stat_hash: u32,
    pub min_stat: Option<u16>,
    pub max_stat: Option<u16>,
}

impl StatConstraint {
    /// Clamp each legacy/modern pair separately, then prefer modern values.
    pub fn normalize(
        stat_hash: u32,
        min_stat: Option<i64>,
        max_stat: Option<i64>,
        min_tier: Option<i64>,
        max_tier: Option<i64>,
    ) -> Self {
        let max_tier = max_tier.map(|v| v.clamp(0, 10));
        let min_tier = min_tier.map(|v| v.clamp(0, max_tier.unwrap_or(10)));
        let max_stat = max_stat.map(|v| v.clamp(0, i64::from(MAX_BUILD_STAT)));
        let min_stat = min_stat.map(|v| v.clamp(0, max_stat.unwrap_or(i64::from(MAX_BUILD_STAT))));
        Self {
            stat_hash,
            min_stat: min_stat.or(min_tier.map(|v| v * 10)).map(|v| v as u16),
            max_stat: max_stat.or(max_tier.map(|v| v * 10)).map(|v| v as u16),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildArtifactUnlocks {
    pub unlocked_item_hashes: Vec<ItemHash>,
    pub season_number: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildInGameIdentifiers {
    pub color_hash: u32,
    pub icon_hash: u32,
    pub name_hash: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildParameters {
    pub stat_constraints: Option<Vec<StatConstraint>>,
    pub mods: Option<Vec<ItemHash>>,
    pub mods_by_bucket: Option<BTreeMap<BucketHash, Vec<ItemHash>>>,
    pub perks: Option<Vec<PerkHash>>,
    pub set_bonuses: Option<BTreeMap<u32, u32>>,
    pub exotic_armor_hash: Option<ExoticArmor>,
    pub query: Option<String>,
    pub assume_armor_masterwork: Option<AssumeArmorMasterwork>,
    pub auto_stat_mods: Option<bool>,
    pub include_runtime_stat_benefits: Option<bool>,
    pub clear_mods: Option<bool>,
    pub clear_weapons: Option<bool>,
    pub clear_armor: Option<bool>,
    pub artifact_unlocks: Option<BuildArtifactUnlocks>,
    pub in_game_identifiers: Option<BuildInGameIdentifiers>,
    /// Unknown parameter fields, as canonical JSON. Interpreted only by adapters.
    pub extra: Option<String>,
}

fn valid_hash(hash: ItemHash) -> bool {
    u32::try_from(hash.value()).is_ok()
}

fn valid_hashes(hashes: &[ItemHash]) -> bool {
    hashes.len() <= MAX_BUILD_PARAMETERS && hashes.iter().copied().all(valid_hash)
}

impl BuildDefinition {
    pub fn validate(&self) -> Result<(), AppError> {
        let invalid = || AppError::new(ErrorCategory::ProviderValidation);
        if self.id.is_empty()
            || self.id.len() > 128
            || self.name.len() > MAX_BUILD_NAME_BYTES
            || self
                .notes
                .as_ref()
                .is_some_and(|v| v.len() > MAX_BUILD_TEXT_BYTES)
            || self.class_type > 3
            || self.equipped.len() + self.unequipped.len() > MAX_BUILD_ITEMS
        {
            return Err(invalid());
        }
        for item in self.equipped.iter().chain(&self.unequipped) {
            if !valid_hash(item.hash)
                || item.instance_id.as_ref().is_some_and(|id| {
                    !crate::is_canonical_loadout_id(id.as_str()) || id.as_str() == "0"
                })
                || item.socket_overrides.len() > MAX_BUILD_SOCKETS
                || item.socket_overrides.iter().any(|(index, hash)| {
                    usize::from(*index) >= MAX_BUILD_SOCKETS || !valid_hash(*hash)
                })
                || item.socket_overrides.windows(2).any(|w| w[0].0 >= w[1].0)
            {
                return Err(invalid());
            }
        }
        if let Some(parameters) = &self.parameters {
            parameters.validate()?;
        }
        Ok(())
    }
}

impl BuildParameters {
    pub fn validate(&self) -> Result<(), AppError> {
        let invalid = || AppError::new(ErrorCategory::ProviderValidation);
        if self.stat_constraints.as_ref().is_some_and(|values| {
            values.len() > MAX_BUILD_PARAMETERS
                || values.iter().any(|c| {
                    c.min_stat.is_some_and(|v| v > MAX_BUILD_STAT)
                        || c.max_stat.is_some_and(|v| v > MAX_BUILD_STAT)
                })
        }) || self.mods.as_ref().is_some_and(|v| !valid_hashes(v))
            || self
                .perks
                .as_ref()
                .is_some_and(|v| v.len() > MAX_BUILD_PARAMETERS)
            || self
                .set_bonuses
                .as_ref()
                .is_some_and(|v| v.len() > MAX_BUILD_PARAMETERS)
            || self.mods_by_bucket.as_ref().is_some_and(|buckets| {
                buckets.len() > MAX_BUILD_PARAMETERS
                    || buckets.iter().any(|(hash, mods)| {
                        u32::try_from(hash.value()).is_err() || !valid_hashes(mods)
                    })
            })
            || self.exotic_armor_hash.is_some_and(|v| match v {
                ExoticArmor::Item(hash) => !valid_hash(hash),
                _ => false,
            })
            || self
                .query
                .as_ref()
                .is_some_and(|v| v.len() > MAX_BUILD_TEXT_BYTES)
            || self
                .extra
                .as_ref()
                .is_some_and(|v| v.len() > MAX_BUILD_TEXT_BYTES)
            || self
                .artifact_unlocks
                .as_ref()
                .is_some_and(|v| !valid_hashes(&v.unlocked_item_hashes))
        {
            return Err(invalid());
        }
        Ok(())
    }
}
