//! Minimal provider-neutral records used by inventory workflows.

use crate::ids::{
    BucketHash, CharacterId, DefinitionVersion, ItemHash, ItemInstanceId, MembershipId,
    MembershipType, PerkHash, SyncRevision,
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

/// One provider-neutral summary of a perk the provider reports for an item instance.
///
/// The record deliberately retains only the perk's provider definition
/// identifier and the two provider-reported state flags. It carries no provider
/// display text, no description, no icon path, no socket or plug structure, and
/// no raw provider payload: the Manifest definition layer owns every display
/// string, and `perk_hash` is an opaque reference that callers must not render.
///
/// Field semantics:
///
/// - `perk_hash` identifies the perk definition. It is a [`PerkHash`] and must
///   not be confused with an item or bucket definition hash.
/// - `active` is true only when the provider reports the summarized perk as
///   active/selected on this instance.
/// - `visible` is true only when the provider reports the summarized perk as
///   visible for this instance. It is independent of `active`, so a perk may be
///   visible but inactive, active and visible, or neither.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PerkSummary {
    pub perk_hash: PerkHash,
    pub active: bool,
    pub visible: bool,
}

/// One socketed inventory-item definition reference, separate from component-302 perks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlugSummary {
    pub plug_hash: ItemHash,
    pub enabled: bool,
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
    /// Ordered perk summaries for this instance.
    ///
    /// The order is semantic and deterministic: it is the order the provider
    /// summarized the perks in, so a reordering is a content change. Duplicate
    /// [`PerkSummary::perk_hash`] values are allowed, because the provider may
    /// summarize the same perk from more than one source. An item with no
    /// summarized perks carries an empty vector, which is distinct from any
    /// non-empty list.
    pub perks: Vec<PerkSummary>,
    /// Enabled, non-empty plugs in socket order; duplicate hashes are semantic.
    /// Acquisition is classification-neutral. Publication keeps weapon perks,
    /// armor mods and unknown definitions using [`classify_plug`].
    pub plugs: Vec<PlugSummary>,
}

impl ItemInstance {
    /// Application publication filter. `None` means the definition is unknown
    /// and is retained; a known definition without a category supplies `Some("")`.
    /// The caller resolves hashes from the available definition catalog.
    /// Selected plug order and duplicates are preserved.
    pub fn retain_searchable_plugs<'a>(
        &mut self,
        mut category_for: impl FnMut(ItemHash) -> Option<&'a str>,
    ) {
        self.plugs.retain(|plug| {
            plug.enabled
                && category_for(plug.plug_hash)
                    .is_none_or(|category| classify_plug(category) != PlugKind::Other)
        });
    }
}

/// Manifest damage type; independent of an item's inventory slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DamageType {
    Kinetic,
    Arc,
    Solar,
    Void,
    Stasis,
    Strand,
}

/// Provider-neutral equipment slot. Unknown provider categories remain explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemSlot {
    Kinetic,
    Energy,
    Power,
    Helmet,
    Gauntlets,
    Chest,
    Legs,
    ClassItem,
    Other,
    Unknown,
}

/// Class restriction, with unrestricted and unknown kept distinct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassType {
    Titan,
    Hunter,
    Warlock,
    Any,
    Unknown,
}

macro_rules! stable_filter_name {
    ($kind:ty, $($variant:ident => $name:literal),+ $(,)?) => {
        impl $kind {
            /// Stable lowercase name used by persisted filter attributes.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name,)+ }
            }
        }
    };
}
stable_filter_name!(DamageType, Kinetic => "kinetic", Arc => "arc", Solar => "solar", Void => "void", Stasis => "stasis", Strand => "strand");
stable_filter_name!(ItemSlot, Kinetic => "kinetic", Energy => "energy", Power => "power", Helmet => "helmet", Gauntlets => "gauntlets", Chest => "chest", Legs => "legs", ClassItem => "classitem", Other => "other", Unknown => "unknown");
stable_filter_name!(ClassType, Titan => "titan", Hunter => "hunter", Warlock => "warlock", Any => "any", Unknown => "unknown");

macro_rules! parse_filter_name {
    ($kind:ty, $($variant:ident),+ $(,)?) => {
        impl std::str::FromStr for $kind {
            type Err = crate::error::AppError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                $(if value.trim().eq_ignore_ascii_case(stringify!($variant)) {
                    return Ok(Self::$variant);
                })+
                Err(crate::error::AppError::new(
                    crate::error::ErrorCategory::ProviderValidation,
                ))
            }
        }
    };
}

parse_filter_name!(DamageType, Kinetic, Arc, Solar, Void, Stasis, Strand);
parse_filter_name!(
    ItemSlot, Kinetic, Energy, Power, Helmet, Gauntlets, Chest, Legs, ClassItem, Other, Unknown
);
parse_filter_name!(ClassType, Titan, Hunter, Warlock, Any, Unknown);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDefinition {
    pub hash: ItemHash,
    pub bucket_hash: Option<BucketHash>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    /// Kept as text for compatibility with existing projections.
    pub element: Option<String>,
    pub slot: Option<ItemSlot>,
    pub class_type: Option<ClassType>,
    /// Stable plug category identifier; absent for non-plug definitions.
    pub plug_category: Option<String>,
    pub tags: Vec<String>,
    pub version: DefinitionVersion,
}

/// Query-time classification of a Manifest plug category; never persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlugKind {
    WeaponPerk,
    ArmorMod,
    Other,
}

/// Stable, case-sensitive categories, independent of localized display names.
pub fn classify_plug(category: &str) -> PlugKind {
    if category.contains("masterworks") {
        PlugKind::Other
    } else if is_weapon_perk_plug(category) {
        PlugKind::WeaponPerk
    } else if category == "enhancements.universal"
        || category.starts_with("enhancements.v2_")
        || category.starts_with("enhancements.season_")
        || category.starts_with("enhancements.raid_")
    {
        PlugKind::ArmorMod
    } else {
        PlugKind::Other
    }
}

/// Classify weapon perk plugs by an exact, case-sensitive category allowlist.
/// Frames include trait columns as well as weapon frames. Cosmetic, catalyst,
/// masterwork, tracker, enhancement and mod categories (and unknown identifiers)
/// are deliberately excluded; localized names never participate in this rule.
pub fn is_weapon_perk_plug(category: &str) -> bool {
    matches!(
        category,
        "intrinsics"
            | "frames"
            | "barrels"
            | "magazines"
            | "magazines_gl"
            | "arrows"
            | "blades"
            | "guards"
            | "batteries"
            | "grips"
            | "scopes"
            | "stocks"
            | "tubes"
            | "hafts"
            | "bowstrings"
            | "origins"
    )
}

#[cfg(test)]
mod plug_tests {
    #[test]
    fn plug_kind_classification_table() {
        use super::{PlugKind, classify_plug};
        for (category, expected) in [
            ("intrinsics", PlugKind::WeaponPerk),
            ("frames", PlugKind::WeaponPerk),
            ("enhancements.v2_head", PlugKind::ArmorMod),
            ("enhancements.v2_arms", PlugKind::ArmorMod),
            ("enhancements.universal", PlugKind::ArmorMod),
            ("enhancements.season_artifact", PlugKind::ArmorMod),
            ("enhancements.raid_leviathan", PlugKind::ArmorMod),
            ("enhancements.ghosts_activity", PlugKind::Other),
            ("enhancements.v2_head.masterworks", PlugKind::Other),
            (
                "core.gear_systems.armor_tiering.plugs.tuning.mods",
                PlugKind::Other,
            ),
            ("shader", PlugKind::Other),
            ("enhancements.universal_extra", PlugKind::Other),
            ("mods", PlugKind::Other),
            ("", PlugKind::Other),
        ] {
            assert_eq!(classify_plug(category), expected, "{category}");
        }
    }

    #[test]
    fn publication_seam_retains_enabled_weapon_and_unknown_categories_in_order() {
        use super::{ItemInstance, ItemLocation, PlugSummary};
        use crate::{ItemHash, ItemInstanceId};
        let mut item = ItemInstance {
            instance_id: ItemInstanceId::new("1"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            perks: vec![],
            plugs: [1, 2, 3, 4, 1, 5, 6]
                .into_iter()
                .map(|hash| PlugSummary {
                    plug_hash: ItemHash::new(hash),
                    enabled: hash != 5,
                })
                .collect(),
        };
        item.retain_searchable_plugs(|hash| match hash.value() {
            1 | 5 => Some("frames"),
            2 => Some("shader"),
            3 => Some("mods"),
            6 => Some("enhancements.universal"),
            _ => None,
        });
        assert_eq!(
            item.plugs,
            vec![
                PlugSummary {
                    plug_hash: ItemHash::new(1),
                    enabled: true
                },
                PlugSummary {
                    plug_hash: ItemHash::new(4),
                    enabled: true
                },
                PlugSummary {
                    plug_hash: ItemHash::new(1),
                    enabled: true
                },
                PlugSummary {
                    plug_hash: ItemHash::new(6),
                    enabled: true
                },
            ]
        );
    }

    #[test]
    fn weapon_perk_categories_are_an_explicit_allowlist() {
        for category in [
            "intrinsics",
            "frames",
            "barrels",
            "magazines",
            "magazines_gl",
            "arrows",
            "blades",
            "guards",
            "batteries",
            "grips",
            "scopes",
            "stocks",
            "tubes",
            "hafts",
            "bowstrings",
            "origins",
        ] {
            assert!(super::is_weapon_perk_plug(category), "{category}");
        }
        for category in [
            "",
            "unknown",
            "catalysts",
            "masterworks",
            "tracker",
            "shader",
            "skins",
            "ornament",
            "memento",
            "mods",
            "v400.weapon.mod_damage",
            "enhancements",
            "armor",
            "Frames",
            "frames.shader",
        ] {
            assert!(!super::is_weapon_perk_plug(category), "{category}");
        }
    }
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
    fn filter_names_parse_case_insensitively_and_reject_unknown_names() {
        use super::{ClassType, DamageType, ItemSlot};
        for name in ["Kinetic", "Arc", "Solar", "Void", "Stasis", "Strand"] {
            assert_eq!(name.parse::<DamageType>(), name.to_lowercase().parse());
            assert!(name.parse::<DamageType>().is_ok());
        }
        for name in [
            "Kinetic",
            "Energy",
            "Power",
            "Helmet",
            "Gauntlets",
            "Chest",
            "Legs",
            "ClassItem",
            "Other",
            "Unknown",
        ] {
            assert_eq!(name.parse::<ItemSlot>(), name.to_lowercase().parse());
            assert!(name.parse::<ItemSlot>().is_ok());
        }
        for name in ["Titan", "Hunter", "Warlock", "Any", "Unknown"] {
            assert_eq!(name.parse::<ClassType>(), name.to_lowercase().parse());
            assert!(name.parse::<ClassType>().is_ok());
        }
        assert!("invalid".parse::<DamageType>().is_err());
        assert!("invalid".parse::<ItemSlot>().is_err());
        assert!("invalid".parse::<ClassType>().is_err());
    }

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
