//! Field-limited Manifest catalogs used by DIM's armor model. No derivations.
use crate::{AppError, ErrorCategory};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArmorCatalogKind {
    StatGroup,
    EquipableSet,
}
impl ArmorCatalogKind {
    pub const ALL: [Self; 2] = [Self::StatGroup, Self::EquipableSet];
    pub const fn component(self) -> &'static str {
        match self {
            Self::StatGroup => "DestinyStatGroupDefinition",
            Self::EquipableSet => "DestinyEquipableItemSetDefinition",
        }
    }
    pub const fn id(self) -> u32 {
        match self {
            Self::StatGroup => 0,
            Self::EquipableSet => 1,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArmorCatalogRecord {
    StatGroup(StatGroupDefinition),
    EquipableSet(EquipableSetDefinition),
}
impl ArmorCatalogRecord {
    pub const fn kind(&self) -> ArmorCatalogKind {
        match self {
            Self::StatGroup(_) => ArmorCatalogKind::StatGroup,
            Self::EquipableSet(_) => ArmorCatalogKind::EquipableSet,
        }
    }
    pub const fn hash(&self) -> u32 {
        match self {
            Self::StatGroup(value) => value.hash,
            Self::EquipableSet(value) => value.hash,
        }
    }
    pub fn validate(&self) -> Result<(), AppError> {
        let valid = match self {
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
    let mut counts = [0; 2];
    for record in records {
        record.validate()?;
        counts[record.kind().id() as usize] += 1;
        if !seen.insert((record.kind().id(), record.hash()))
            || counts[record.kind().id() as usize] > 10_000
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
