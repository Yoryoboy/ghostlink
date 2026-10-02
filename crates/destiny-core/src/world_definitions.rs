//! Bounded provider-neutral world definitions, without provider payloads.
use crate::{AppError, DefinitionVersion, ErrorCategory};

fn validate_text(name: &str, description: &str) -> Result<(), AppError> {
    if name.trim().is_empty()
        || [(name, 256), (description, 2048)]
            .into_iter()
            .any(|(text, cap)| {
                text.chars().count() > cap
                    || text.chars().any(|c| c.is_control() && !c.is_whitespace())
            })
    {
        return Err(AppError::new(ErrorCategory::ProviderValidation));
    }
    Ok(())
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

/// One component's accepted definitions and omitted blank/redacted records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldDefinitionTable<T> {
    pub definitions: Vec<T>,
    pub skipped_count: usize,
}

/// The three world components acquired against one exact index version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldDefinitionsAcquisition {
    pub version: DefinitionVersion,
    pub milestones: WorldDefinitionTable<MilestoneDefinition>,
    pub activities: WorldDefinitionTable<ActivityDefinition>,
    pub modifiers: WorldDefinitionTable<ActivityModifierDefinition>,
}

impl WorldDefinitionsAcquisition {
    /// Validate a complete, bounded, duplicate-free publication against the item version.
    pub fn validate_for(&self, version: &DefinitionVersion) -> Result<(), AppError> {
        fn valid(hashes: impl Iterator<Item = u32>, count: usize, cap: usize) -> bool {
            count > 0
                && count <= cap
                && hashes.collect::<std::collections::HashSet<_>>().len() == count
        }
        if &self.version != version
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
        {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }

    pub fn counts(&self) -> [usize; 3] {
        [
            self.milestones.definitions.len(),
            self.activities.definitions.len(),
            self.modifiers.definitions.len(),
        ]
    }

    pub fn skipped_counts(&self) -> [usize; 3] {
        [
            self.milestones.skipped_count,
            self.activities.skipped_count,
            self.modifiers.skipped_count,
        ]
    }

    /// Canonicalize set order and discard acquisition-only skip counts.
    pub fn canonicalize(&mut self) {
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
            version: DefinitionVersion::new("v1"),
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
