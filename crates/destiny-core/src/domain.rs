//! Minimal provider-neutral records used by inventory workflows.

use crate::ids::{
    BucketHash, CharacterId, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId,
    MembershipType, SyncRevision,
};
use crate::state::{Dataset, SourceMarker, Timestamp};

/// A location understood by the domain, or a preserved provider value with no inferred meaning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemLocation {
    Vault,
    Character(CharacterId),
    Other(String),
}

impl ItemLocation {
    pub fn other(value: impl Into<String>) -> Self {
        Self::Other(value.into())
    }

    pub fn unknown_value(&self) -> Option<&str> {
        match self {
            Self::Other(value) => Some(value),
            Self::Vault | Self::Character(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Character {
    pub id: CharacterId,
    pub name: Option<String>,
    pub class: Option<String>,
    pub race: Option<String>,
    pub gender: Option<String>,
    pub light: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInstance {
    pub instance_id: ItemInstanceId,
    pub item_hash: ItemHash,
    pub location: ItemLocation,
    pub quantity: u32,
    pub equipped: bool,
    pub locked: bool,
    pub power: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDefinition {
    pub hash: ItemHash,
    pub bucket_hash: Option<BucketHash>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    pub element: Option<String>,
    pub tags: Vec<String>,
    pub version: DefinitionVersion,
}

/// Snapshot identity, provenance markers, and committed freshness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotMetadata {
    pub membership_id: MembershipId,
    pub membership_type: MembershipType,
    pub revision: SyncRevision,
    pub definition_version: DefinitionVersion,
    pub checked_at: Timestamp,
    pub source: SourceMarker,
}

/// The committed identity, freshness, and provenance of one dataset projection.
///
/// The record is provider-neutral: it carries no SQL statement, no schema or
/// migration version, and no raw payload. The database migration version stays
/// adapter-owned and is deliberately absent here. The dataset and its
/// [`SourceMarker`] must agree; [`SyncMetadata::source_is_valid`] enforces that
/// pairing without consulting a clock or a provider.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncMetadata {
    pub dataset: Dataset,
    pub checked_at: Timestamp,
    pub source: SourceMarker,
    pub revision: SyncRevision,
    pub definition_version: DefinitionVersion,
}

impl SyncMetadata {
    /// True when the source marker is the one this dataset requires.
    pub fn source_is_valid(&self) -> bool {
        self.source == self.dataset.expected_source()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySnapshot {
    pub metadata: SnapshotMetadata,
    pub characters: Vec<Character>,
    pub items: Vec<ItemInstance>,
}

#[cfg(test)]
mod tests {
    use super::{Character, InventorySnapshot, ItemLocation, SnapshotMetadata, SyncMetadata};
    use crate::ids::{CharacterId, DefinitionVersion, MembershipId, MembershipType, SyncRevision};
    use crate::state::{Dataset, SourceMarker, Timestamp};

    #[test]
    fn locations_preserve_unknown_provider_values() {
        let unknown = ItemLocation::other("future-location-17");
        assert_eq!(unknown.unknown_value(), Some("future-location-17"));
        assert_eq!(ItemLocation::Vault.unknown_value(), None);
        assert_eq!(
            ItemLocation::Character(CharacterId::new("warlock")).unknown_value(),
            None
        );
    }

    #[test]
    fn snapshot_records_are_normalized_and_provider_neutral() {
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: MembershipId::new("membership"),
                membership_type: MembershipType::new(3),
                revision: SyncRevision::new(1),
                definition_version: DefinitionVersion::new("manifest-v1"),
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
            },
            characters: vec![Character {
                id: CharacterId::new("warlock"),
                name: Some("Nova".into()),
                class: Some("Warlock".into()),
                race: None,
                gender: None,
                light: Some(1810),
            }],
            items: Vec::new(),
        };

        assert_eq!(snapshot.characters[0].id.as_str(), "warlock");
        assert_eq!(snapshot.metadata.revision.value(), 1);
    }

    #[test]
    fn sync_metadata_pairs_each_dataset_with_its_single_source() {
        let manifest = SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: Timestamp::new(40),
            source: SourceMarker::Manifest,
            revision: SyncRevision::new(2),
            definition_version: DefinitionVersion::new("manifest-v1"),
        };
        assert!(manifest.source_is_valid());

        let profile = SyncMetadata {
            dataset: Dataset::ProfileInventory,
            source: SourceMarker::Profile,
            ..manifest.clone()
        };
        assert!(profile.source_is_valid());

        let mismatched = SyncMetadata {
            source: SourceMarker::Profile,
            ..manifest.clone()
        };
        assert!(!mismatched.source_is_valid());

        assert_eq!(manifest.checked_at, Timestamp::new(40));
        assert_eq!(manifest.revision, SyncRevision::new(2));
    }
}
