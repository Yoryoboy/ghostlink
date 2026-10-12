//! Field-limited Manifest catalogs used by DIM's armor model. No derivations.
use crate::{AppError, ErrorCategory};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArmorCatalogKind {
    StatGroup,
    EquipableSet,
    SocketType,
    SocketCategory,
    PlugSet,
    MaterialRequirementSet,
}
impl ArmorCatalogKind {
    pub const ALL: [Self; 6] = [
        Self::StatGroup,
        Self::EquipableSet,
        Self::SocketType,
        Self::SocketCategory,
        Self::PlugSet,
        Self::MaterialRequirementSet,
    ];
    pub const fn component(self) -> &'static str {
        match self {
            Self::StatGroup => "DestinyStatGroupDefinition",
            Self::EquipableSet => "DestinyEquipableItemSetDefinition",
            Self::SocketType => "DestinySocketTypeDefinition",
            Self::SocketCategory => "DestinySocketCategoryDefinition",
            Self::PlugSet => "DestinyPlugSetDefinition",
            Self::MaterialRequirementSet => "DestinyMaterialRequirementSetDefinition",
        }
    }
    /// Safety ceilings with seasonal headroom, not DIM/gameplay restrictions.
    /// Live probe 2026-10-11: 5,422 plug sets, 1,874 socket types; other kinds <= 112.
    pub const fn max_records(self) -> usize {
        match self {
            Self::PlugSet => 50_000,
            Self::SocketType => 20_000,
            _ => 10_000,
        }
    }
    pub const fn id(self) -> u32 {
        match self {
            Self::StatGroup => 0,
            Self::EquipableSet => 1,
            Self::SocketType => 2,
            Self::SocketCategory => 3,
            Self::PlugSet => 4,
            Self::MaterialRequirementSet => 5,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterpolationPoint {
    pub value: i32,
    pub weight: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScaledStat {
    pub stat_hash: u32,
    pub maximum_value: i32,
    pub display_as_numeric: bool,
    pub display_interpolation: Vec<InterpolationPoint>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatGroupDefinition {
    pub hash: u32,
    pub maximum_value: Option<i32>,
    pub scaled_stats: Vec<ScaledStat>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetPerk {
    /// Sandbox perk display projection; unresolved/legacy references stay unknown.
    pub name: Option<String>,
    pub icon: Option<String>,
    pub required_set_count: i32,
    pub sandbox_perk_hash: u32,
}
#[derive(Clone, Eq, PartialEq)]
pub struct EquipableSetDefinition {
    pub hash: u32,
    pub name: String,
    pub set_items: Vec<u32>,
    pub set_perks: Vec<SetPerk>,
}
impl std::fmt::Debug for EquipableSetDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EquipableSetDefinition")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}
/// Generous safety ceilings, not gameplay limits: shared cosmetic sets can contain thousands.
pub const MAX_PLUG_SET_ITEMS: usize = 65_536;
pub const MAX_PLUG_WHITELIST: usize = 1024;
#[derive(Clone, Eq, PartialEq)]
pub struct PlugWhitelistEntry {
    pub category_hash: u32,
    pub category_identifier: String,
}
impl std::fmt::Debug for PlugWhitelistEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlugWhitelistEntry")
            .field("category_hash", &self.category_hash)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketTypeDefinition {
    pub hash: u32,
    pub socket_category_hash: u32,
    pub always_randomize_sockets: bool,
    pub plug_whitelist: Vec<PlugWhitelistEntry>,
}
#[derive(Clone, Eq, PartialEq)]
pub struct SocketCategoryDefinition {
    pub hash: u32,
    pub category_style: u32,
    pub name: String,
}
impl std::fmt::Debug for SocketCategoryDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SocketCategoryDefinition")
            .field("hash", &self.hash)
            .field("category_style", &self.category_style)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlugSetItem {
    pub plug_item_hash: u32,
    pub currently_can_roll: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlugSetDefinition {
    pub hash: u32,
    pub reusable_plug_items: Vec<PlugSetItem>,
}
/// Live Manifest 244213.26.06.29.2000-1-bnet.65864: 332 sets / max 4 materials.
/// Safety headroom, not a gameplay limit.
pub const MAX_MATERIAL_REQUIREMENTS: usize = 256;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialRequirement {
    pub item_hash: u32,
    pub count: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialRequirementSetDefinition {
    pub hash: u32,
    pub materials: Vec<MaterialRequirement>,
}
fn bounded_text(value: &str, limit: usize) -> bool {
    value.chars().count() <= limit && !value.chars().any(|c| c.is_control() && !c.is_whitespace())
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArmorCatalogRecord {
    StatGroup(StatGroupDefinition),
    EquipableSet(EquipableSetDefinition),
    SocketType(SocketTypeDefinition),
    SocketCategory(SocketCategoryDefinition),
    PlugSet(PlugSetDefinition),
    MaterialRequirementSet(MaterialRequirementSetDefinition),
}
impl ArmorCatalogRecord {
    pub const fn kind(&self) -> ArmorCatalogKind {
        match self {
            Self::StatGroup(_) => ArmorCatalogKind::StatGroup,
            Self::EquipableSet(_) => ArmorCatalogKind::EquipableSet,
            Self::SocketType(_) => ArmorCatalogKind::SocketType,
            Self::SocketCategory(_) => ArmorCatalogKind::SocketCategory,
            Self::PlugSet(_) => ArmorCatalogKind::PlugSet,
            Self::MaterialRequirementSet(_) => ArmorCatalogKind::MaterialRequirementSet,
        }
    }
    pub const fn hash(&self) -> u32 {
        match self {
            Self::StatGroup(value) => value.hash,
            Self::EquipableSet(value) => value.hash,
            Self::SocketType(value) => value.hash,
            Self::SocketCategory(value) => value.hash,
            Self::PlugSet(value) => value.hash,
            Self::MaterialRequirementSet(value) => value.hash,
        }
    }
    pub fn validate(&self) -> Result<(), AppError> {
        let valid = match self {
            Self::SocketType(value) => {
                value.plug_whitelist.len() <= MAX_PLUG_WHITELIST
                    && value
                        .plug_whitelist
                        .iter()
                        .all(|entry| bounded_text(&entry.category_identifier, 256))
            }
            Self::MaterialRequirementSet(value) => {
                value.materials.len() <= MAX_MATERIAL_REQUIREMENTS
            }
            Self::SocketCategory(value) => bounded_text(&value.name, 256),
            Self::PlugSet(value) => value.reusable_plug_items.len() <= MAX_PLUG_SET_ITEMS,
            Self::StatGroup(value) => {
                value.scaled_stats.len() <= 64
                    && value
                        .scaled_stats
                        .iter()
                        .all(|stat| stat.display_interpolation.len() <= 256)
            }
            Self::EquipableSet(value) => {
                !value.name.trim().is_empty()
                    && value.name.chars().count() <= 256
                    && !value
                        .name
                        .chars()
                        .any(|c| c.is_control() && !c.is_whitespace())
                    && value.set_items.len() <= 1024
                    && value.set_perks.len() <= 64
                    && value.set_perks.iter().all(|perk| {
                        perk.name
                            .as_deref()
                            .is_none_or(|name| bounded_text(name, 256))
                            && perk
                                .icon
                                .as_deref()
                                .is_none_or(|icon| bounded_text(icon, 1024))
                    })
            }
        };
        if valid {
            Ok(())
        } else {
            Err(AppError::new(ErrorCategory::ProviderValidation))
        }
    }
}
/// Validate complete batches before publication; empty catalogs are legitimate.
pub fn validate_catalogs(records: &[ArmorCatalogRecord]) -> Result<(), AppError> {
    let mut seen = std::collections::HashSet::new();
    let mut counts = [0; 6];
    for record in records {
        record.validate()?;
        counts[record.kind().id() as usize] += 1;
        if !seen.insert((record.kind().id(), record.hash()))
            || counts[record.kind().id() as usize] > record.kind().max_records()
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
    }
    Ok(())
}
/// Single-hash and complete-kind reads for downstream pure derivations.
pub trait ArmorCatalogRepository {
    fn armor_catalog(
        &self,
        kind: ArmorCatalogKind,
        hash: Option<u32>,
    ) -> Result<Vec<ArmorCatalogRecord>, AppError>;
    fn lookup_armor_catalog(
        &self,
        kind: ArmorCatalogKind,
        hash: u32,
    ) -> Result<Option<ArmorCatalogRecord>, AppError> {
        Ok(self.armor_catalog(kind, Some(hash))?.into_iter().next())
    }
    fn armor_catalog_batch(
        &self,
        kind: ArmorCatalogKind,
        hashes: &[u32],
    ) -> Result<Vec<ArmorCatalogRecord>, AppError> {
        if hashes.len() > 1024 {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let mut records = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for hash in hashes {
            if seen.insert(*hash)
                && let Some(record) = self.lookup_armor_catalog(kind, *hash)?
            {
                records.push(record);
            }
        }
        Ok(records)
    }
}
