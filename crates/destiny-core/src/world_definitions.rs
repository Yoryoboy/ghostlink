//! Bounded provider-neutral world definitions, without provider payloads.
use crate::{AppError, DefinitionVersion, ErrorCategory};

/// Shared bounded text policy: whitespace controls (including newlines) survive.
pub fn validate_description(description: &str) -> Result<(), AppError> {
    validate_bounded_text(description, 2048)
}

fn validate_bounded_text(text: &str, cap: usize) -> Result<(), AppError> {
    if text.chars().count() > cap || text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err(AppError::new(ErrorCategory::ProviderValidation));
    }
    Ok(())
}

fn validate_text(name: &str, description: &str) -> Result<(), AppError> {
    if name.trim().is_empty()
        || validate_bounded_text(name, 256).is_err()
        || validate_description(description).is_err()
    {
        return Err(AppError::new(ErrorCategory::ProviderValidation));
    }
    Ok(())
}

/// Official inventory tooltip, separate from item identity and omitted from Debug.
#[derive(Clone, Eq, PartialEq)]
pub struct ItemDescription {
    hash: crate::ItemHash,
    description: String,
}
impl ItemDescription {
    pub fn new(hash: crate::ItemHash, description: String) -> Result<Self, AppError> {
        validate_description(&description)?;
        if description.trim().is_empty() || !(0..=i64::from(u32::MAX)).contains(&hash.value()) {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self { hash, description })
    }
    pub const fn hash(&self) -> crate::ItemHash {
        self.hash
    }
    pub fn description(&self) -> &str {
        &self.description
    }
}
/// Reject duplicate or orphan tooltip hashes before any publication mutation.
pub fn validate_item_descriptions(
    items: &crate::ManifestDefinitions,
    descriptions: &[ItemDescription],
) -> Result<(), AppError> {
    let hashes: std::collections::HashSet<_> =
        items.definitions.iter().map(|item| item.hash).collect();
    let mut seen = std::collections::HashSet::new();
    if descriptions.len() > 100_000
        || descriptions
            .iter()
            .any(|d| !hashes.contains(&d.hash()) || !seen.insert(d.hash()))
    {
        return Err(AppError::new(ErrorCategory::ProviderValidation));
    }
    Ok(())
}

impl std::fmt::Debug for ItemDescription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemDescription")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}

macro_rules! definition {
    ($kind:ident $(, $field:ident: $type:ty)*) => {
        #[derive(Clone, Eq, PartialEq)]
        pub struct $kind {
            hash: u32,
            name: String,
            description: String,
            $($field: $type,)*
        }
        impl std::fmt::Debug for $kind {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter
                    .debug_struct(stringify!($kind))
                    .field("hash", &self.hash)
                    $(.field(stringify!($field), &self.$field))*
                    .finish_non_exhaustive()
            }
        }
        impl $kind {
            pub fn new(hash: u32, name: String, description: String $(, $field: $type)*) -> Result<Self, AppError> {
                validate_text(&name, &description)?;
                Ok(Self { hash, name, description, $($field,)* })
            }
            pub const fn hash(&self) -> u32 { self.hash }
            pub fn name(&self) -> &str { &self.name }
            pub fn description(&self) -> &str { &self.description }
            $(pub const fn $field(&self) -> $type { self.$field })*
        }
    };
}
definition!(MilestoneDefinition, milestone_type: u32);
definition!(ActivityDefinition, activity_type_hash: u32, light_level: i32);
definition!(ActivityModifierDefinition);
definition!(ActivityTypeDefinition);

/// Minimal named loadout identity; provider text is omitted from Debug.
#[derive(Clone, Eq, PartialEq)]
pub struct LoadoutNameDefinition {
    hash: u32,
    name: String,
}
impl std::fmt::Debug for LoadoutNameDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadoutNameDefinition")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}
impl LoadoutNameDefinition {
    pub fn new(hash: u32, name: String) -> Result<Self, AppError> {
        validate_text(&name, "")?;
        Ok(Self { hash, name })
    }
    pub const fn hash(&self) -> u32 {
        self.hash
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Minimal named stat identity; no other provider fields are retained.
#[derive(Clone, Eq, PartialEq)]
pub struct StatDefinition {
    hash: u32,
    name: String,
}
impl std::fmt::Debug for StatDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatDefinition")
            .field("hash", &self.hash)
            .finish_non_exhaustive()
    }
}
impl StatDefinition {
    pub fn new(hash: u32, name: String) -> Result<Self, AppError> {
        normalize_stat_name(&name)?;
        Ok(Self { hash, name })
    }
    pub const fn hash(&self) -> u32 {
        self.hash
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Exact, case-insensitive stat-name matching with collapsed whitespace.
pub fn normalize_stat_name(name: &str) -> Result<String, AppError> {
    validate_text(name, "")?;
    let normalized = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    validate_text(&normalized, "")?;
    Ok(normalized)
}

#[derive(Clone, Eq, PartialEq)]
pub struct ObjectiveDefinition {
    ui_style: Option<u32>,
    hash: u32,
    progress_description: String,
    completion_value: i32,
}
impl std::fmt::Debug for ObjectiveDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectiveDefinition")
            .field("hash", &self.hash)
            .field("completion_value", &self.completion_value)
            .finish_non_exhaustive()
    }
}
impl ObjectiveDefinition {
    pub fn new(
        hash: u32,
        progress_description: String,
        completion_value: i32,
    ) -> Result<Self, AppError> {
        if progress_description.chars().count() > 512
            || progress_description
                .chars()
                .any(|c| c.is_control() && !c.is_whitespace())
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Self {
            hash,
            progress_description,
            completion_value,
            ui_style: None,
        })
    }
    pub const fn hash(&self) -> u32 {
        self.hash
    }
    pub fn with_ui_style(mut self, ui_style: Option<u32>) -> Self {
        self.ui_style = ui_style;
        self
    }
    pub const fn ui_style(&self) -> Option<u32> {
        self.ui_style
    }
    pub fn progress_description(&self) -> &str {
        &self.progress_description
    }
    pub const fn completion_value(&self) -> i32 {
        self.completion_value
    }
}

/// One component's accepted definitions and omitted blank/redacted records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldDefinitionTable<T> {
    pub definitions: Vec<T>,
    pub skipped_count: usize,
}

/// The seven non-item components acquired against one exact index version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldDefinitionsAcquisition {
    pub armor_catalogs: Vec<crate::armor_catalog::ArmorCatalogRecord>,
    /// Optional inventory tooltip catalog, published under the shared Manifest metadata.
    pub item_descriptions: Vec<ItemDescription>,
    pub version: DefinitionVersion,
    pub milestones: WorldDefinitionTable<MilestoneDefinition>,
    pub activities: WorldDefinitionTable<ActivityDefinition>,
    pub modifiers: WorldDefinitionTable<ActivityModifierDefinition>,
    pub objectives: WorldDefinitionTable<ObjectiveDefinition>,
    pub activity_types: WorldDefinitionTable<ActivityTypeDefinition>,
    pub stats: WorldDefinitionTable<StatDefinition>,
    pub loadout_names: WorldDefinitionTable<LoadoutNameDefinition>,
}

impl WorldDefinitionsAcquisition {
    /// Validate a complete, bounded, duplicate-free publication against the item version.
    pub fn validate_for(&self, version: &DefinitionVersion) -> Result<(), AppError> {
        crate::armor_catalog::validate_catalogs(&self.armor_catalogs)?;
        fn valid(hashes: impl Iterator<Item = u32>, count: usize, cap: usize) -> bool {
            count > 0
                && count <= cap
                && hashes.collect::<std::collections::HashSet<_>>().len() == count
        }
        if self.item_descriptions.len() > 100_000
            || self
                .item_descriptions
                .iter()
                .map(ItemDescription::hash)
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.item_descriptions.len()
            || (!self.loadout_names.definitions.is_empty()
                && !valid(
                    self.loadout_names
                        .definitions
                        .iter()
                        .map(LoadoutNameDefinition::hash),
                    self.loadout_names.definitions.len(),
                    1000,
                ))
            || !valid(
                self.stats.definitions.iter().map(StatDefinition::hash),
                self.stats.definitions.len(),
                1000,
            )
            || &self.version != version
            || !valid(
                self.milestones
                    .definitions
                    .iter()
                    .map(MilestoneDefinition::hash),
                self.milestones.definitions.len(),
                1000,
            )
            || !valid(
                self.activities
                    .definitions
                    .iter()
                    .map(ActivityDefinition::hash),
                self.activities.definitions.len(),
                50000,
            )
            || !valid(
                self.modifiers
                    .definitions
                    .iter()
                    .map(ActivityModifierDefinition::hash),
                self.modifiers.definitions.len(),
                10000,
            )
            || !valid(
                self.objectives
                    .definitions
                    .iter()
                    .map(ObjectiveDefinition::hash),
                self.objectives.definitions.len(),
                100000,
            )
            || !valid(
                self.activity_types
                    .definitions
                    .iter()
                    .map(ActivityTypeDefinition::hash),
                self.activity_types.definitions.len(),
                1000,
            )
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }

    pub fn counts(&self) -> [usize; 5] {
        [
            self.milestones.definitions.len(),
            self.activities.definitions.len(),
            self.modifiers.definitions.len(),
            self.objectives.definitions.len(),
            self.activity_types.definitions.len(),
        ]
    }

    pub fn skipped_counts(&self) -> [usize; 5] {
        [
            self.milestones.skipped_count,
            self.activities.skipped_count,
            self.modifiers.skipped_count,
            self.objectives.skipped_count,
            self.activity_types.skipped_count,
        ]
    }

    /// Canonicalize set order and discard acquisition-only skip counts.
    pub fn canonicalize(&mut self) {
        self.armor_catalogs
            .sort_by_key(|record| (record.kind().id(), record.hash()));
        self.item_descriptions.sort_by_key(ItemDescription::hash);
        self.milestones
            .definitions
            .sort_by_key(MilestoneDefinition::hash);
        self.activities
            .definitions
            .sort_by_key(ActivityDefinition::hash);
        self.modifiers
            .definitions
            .sort_by_key(ActivityModifierDefinition::hash);
        self.milestones.skipped_count = 0;
        self.activities.skipped_count = 0;
        self.modifiers.skipped_count = 0;
        self.objectives
            .definitions
            .sort_by_key(ObjectiveDefinition::hash);
        self.activity_types
            .definitions
            .sort_by_key(ActivityTypeDefinition::hash);
        self.objectives.skipped_count = 0;
        self.activity_types.skipped_count = 0;
        self.stats.definitions.sort_by_key(StatDefinition::hash);
        self.stats.skipped_count = 0;
        self.loadout_names
            .definitions
            .sort_by_key(LoadoutNameDefinition::hash);
        self.loadout_names.skipped_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_provider_text_in_definitions_and_acquisition() {
        let name = "provider-name-sentinel";
        let description = "provider-description-sentinel";
        let milestone = MilestoneDefinition::new(1, name.into(), description.into(), 3).unwrap();
        let activity =
            ActivityDefinition::new(2, name.into(), description.into(), 4, 2000).unwrap();
        let modifier = ActivityModifierDefinition::new(5, name.into(), description.into()).unwrap();
        let definitions = [
            format!("{milestone:?}"),
            format!("{activity:?}"),
            format!("{modifier:?}"),
        ];
        assert!(definitions[0].contains("hash: 1"));
        assert!(definitions[0].contains("milestone_type: 3"));
        assert!(definitions[1].contains("activity_type_hash: 4"));
        assert!(definitions[1].contains("light_level: 2000"));
        let acquisition = WorldDefinitionsAcquisition {
            armor_catalogs: Vec::new(),
            item_descriptions: Vec::new(),
            loadout_names: WorldDefinitionTable {
                definitions: vec![LoadoutNameDefinition::new(9, name.into()).unwrap()],
                skipped_count: 0,
            },
            version: DefinitionVersion::new("v1"),
            stats: WorldDefinitionTable {
                definitions: vec![StatDefinition::new(8, name.into()).unwrap()],
                skipped_count: 0,
            },
            objectives: WorldDefinitionTable {
                definitions: vec![ObjectiveDefinition::new(6, description.into(), 1).unwrap()],
                skipped_count: 0,
            },
            activity_types: WorldDefinitionTable {
                definitions: vec![
                    ActivityTypeDefinition::new(7, name.into(), description.into()).unwrap(),
                ],
                skipped_count: 0,
            },
            milestones: WorldDefinitionTable {
                definitions: vec![milestone],
                skipped_count: 0,
            },
            activities: WorldDefinitionTable {
                definitions: vec![activity],
                skipped_count: 0,
            },
            modifiers: WorldDefinitionTable {
                definitions: vec![modifier],
                skipped_count: 0,
            },
        };
        for debug in definitions.into_iter().chain([format!("{acquisition:?}")]) {
            assert!(!debug.contains(name), "provider name leaked");
            assert!(!debug.contains(description), "provider description leaked");
        }
    }
}
