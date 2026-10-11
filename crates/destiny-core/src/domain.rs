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
    /// Owned Postmaster holdings, never carried or equipped inventory.
    Postmaster(CharacterId),
    Other(String),
}

impl ItemLocation {
    pub fn other(value: impl Into<String>) -> Self {
        Self::Other(value.into())
    }

    pub fn unknown_value(&self) -> Option<&str> {
        match self {
            Self::Other(value) => Some(value),
            Self::Vault | Self::Character(_) | Self::Postmaster(_) => None,
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
    /// Component-200 `dateLastPlayed`, normalized to Unix seconds; absent in legacy snapshots.
    pub date_last_played: Option<Timestamp>,
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

/// One instance stat, keyed by its unsigned Manifest definition hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatSummary {
    pub stat_hash: u32,
    pub value: i32,
}

/// One selectable plug, in socket-index then provider plug order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailablePlug {
    pub socket_index: u8,
    pub plug_hash: ItemHash,
}

/// Exact component-300 energy values, without inferred armor semantics.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ItemEnergy {
    pub energy_capacity: u32,
    pub energy_used: u32,
    pub energy_unused: u32,
    pub energy_type: Option<u32>,
    pub energy_type_hash: Option<u32>,
}

pub const MAX_ITEM_SOCKETS: usize = 32;

/// Component-305 socket state. Index is the real provider array position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexedSocket {
    pub socket_index: u8,
    pub plug_hash: Option<ItemHash>,
    pub enabled: bool,
    pub visible: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInstance {
    pub energy: Option<ItemEnergy>,
    /// Omission stays unknown; consumers may use `unwrap_or(0)` like DIM.
    pub gear_tier: Option<u32>,
    /// Complete raw socket projection; publication filtering only affects `plugs`.
    pub sockets: Vec<IndexedSocket>,
    /// Exact provider transferStatus; absent in legacy projections.
    pub transfer_status: Option<u32>,
    /// Selectable weapon perks, armor mods and unknown plugs, stored separately
    /// from selected sockets. Socket order and per-socket plug order are content.
    pub available_plugs: Vec<AvailablePlug>,
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
    /// armor mods, armor archetypes, subclass plugs, artifact perks and unknown definitions using
    /// [`classify_plug`].
    pub plugs: Vec<PlugSummary>,
    /// Unique stats in ascending hash order; order is not semantic.
    pub stats: Vec<StatSummary>,
}

impl ItemInstance {
    /// DIM Boolean(nonTransferrable || transferStatus === NotTransferrable).
    pub fn notransfer(&self, definition: &ItemDefinition) -> bool {
        definition.non_transferrable == Some(true) || self.transfer_status == Some(2)
    }

    /// Application publication filter. `None` means the definition is unknown
    /// and is retained; a known definition without a category supplies `Some("")`.
    /// The caller resolves hashes from the available definition catalog.
    /// Selected plug order and duplicates are preserved.
    pub fn retain_searchable_plugs<'a>(
        &mut self,
        mut category_for: impl FnMut(ItemHash) -> Option<&'a str>,
    ) {
        self.available_plugs.retain(|plug| {
            category_for(plug.plug_hash).is_none_or(|category| {
                matches!(
                    classify_plug(category),
                    PlugKind::WeaponPerk | PlugKind::ArmorMod
                )
            })
        });
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

impl ItemSlot {
    /// Maps the eight weapon/armor inventory buckets to their equipment slots.
    /// Other buckets are outside the equipment contract and return `None`.
    pub fn from_equipment_bucket(bucket: BucketHash) -> Option<Self> {
        match bucket.value() {
            1498876634 => Some(Self::Kinetic),
            2465295065 => Some(Self::Energy),
            953998645 => Some(Self::Power),
            3448274439 => Some(Self::Helmet),
            3551918588 => Some(Self::Gauntlets),
            14239492 => Some(Self::Chest),
            20886954 => Some(Self::Legs),
            1585787867 => Some(Self::ClassItem),
            _ => None,
        }
    }
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
    /// Manifest-only investment and plug metadata, never runtime totals.
    pub armor: crate::ArmorDefinition,
    /// Official top-level nonTransferrable; absence is retained.
    pub non_transferrable: Option<bool>,
    pub hash: ItemHash,
    /// Normal Manifest definition bucket, not an observed runtime container.
    pub bucket_hash: Option<BucketHash>,
    /// Official DestinyInventoryItemDefinition.doesPostmasterPullHaveSideEffects.
    /// `Some(true)` warns of potential destructive redemption, not certainty;
    /// unknown (`None`) is not explicit false and cannot prove safe eligibility.
    pub does_postmaster_pull_have_side_effects: Option<bool>,
    /// Official DestinyItemInventoryBlockDefinition.isInstanceItem; unknown is
    /// not false and does not identify any particular runtime observation.
    pub is_instance_item: Option<bool>,
    /// Official inventory.maxStackSize, preserved as supplied signed int32.
    /// Unknown or nonpositive values cannot establish usable capacity.
    pub max_stack_size: Option<i32>,
    /// Official inventory.stackUniqueLabel, preserved exactly without trimming.
    /// A populated label forbids multiple stacks with that label; empty is known
    /// empty, while `None` is unknown. Neither proves destination coverage.
    pub stack_unique_label: Option<String>,
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

/// Supported subclass elements, distinct from weapon damage types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubclassElement {
    Arc,
    Solar,
    Void,
    Stasis,
    Strand,
    Prismatic,
}

stable_filter_name!(SubclassElement, Arc => "arc", Solar => "solar", Void => "void", Stasis => "stasis", Strand => "strand", Prismatic => "prismatic");

impl std::str::FromStr for SubclassElement {
    type Err = crate::AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "arc" => Ok(Self::Arc),
            "solar" => Ok(Self::Solar),
            "void" => Ok(Self::Void),
            "stasis" => Ok(Self::Stasis),
            "strand" => Ok(Self::Strand),
            "prism" | "prismatic" => Ok(Self::Prismatic),
            _ => Err(crate::AppError::new(
                crate::ErrorCategory::ProviderValidation,
            )),
        }
    }
}

/// Parse stable Manifest categories, including element-independent shared fragments.
/// Unknown elements cannot be safely assigned to a group and are omitted.
pub fn parse_subclass_category(
    category: &str,
) -> Option<(&str, Option<SubclassElement>, SubclassPlugKind)> {
    let kind = classify_subclass_plug(category)?;
    if category == "shared.fragments" {
        return Some(("shared", None, kind));
    }
    let mut parts = category.split('.');
    Some((parts.next()?, Some(parts.next()?.parse().ok()?), kind))
}

/// Equipped subclass socket kind, identified only by stable Manifest categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubclassPlugKind {
    Super,
    Aspect,
    Fragment,
    Grenade,
    Melee,
    ClassAbility,
    Movement,
    Transcendence,
}

/// Stable, case-sensitive subclass categories; localized names never participate.
pub fn classify_subclass_plug(category: &str) -> Option<SubclassPlugKind> {
    if category == "shared.fragments" {
        return Some(SubclassPlugKind::Fragment);
    }
    let mut parts = category.split('.');
    let prefix = parts.next()?;
    let element = parts.next()?;
    let kind = parts.next()?;
    if !matches!(prefix, "hunter" | "titan" | "warlock" | "shared")
        || element.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    match kind {
        "supers" => Some(SubclassPlugKind::Super),
        "aspects" | "totems" => Some(SubclassPlugKind::Aspect),
        "fragments" | "trinkets" => Some(SubclassPlugKind::Fragment),
        "grenades" | "prism_grenade" => Some(SubclassPlugKind::Grenade),
        "melee" => Some(SubclassPlugKind::Melee),
        "class_abilities" => Some(SubclassPlugKind::ClassAbility),
        "movement" => Some(SubclassPlugKind::Movement),
        "transcendence" => Some(SubclassPlugKind::Transcendence),
        _ => None,
    }
}

/// Query-time classification of a Manifest plug category; never persisted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlugKind {
    WeaponPerk,
    ArmorMod,
    ArmorArchetype,
    Subclass(SubclassPlugKind),
    ArtifactPerk,
    Other,
}

/// Stable, case-sensitive categories, independent of localized display names.
pub fn classify_plug(category: &str) -> PlugKind {
    if let Some(kind) = classify_subclass_plug(category) {
        PlugKind::Subclass(kind)
    } else if category == "artifact_perks" {
        PlugKind::ArtifactPerk
    } else if category == "armor_archetypes" {
        PlugKind::ArmorArchetype
    } else if category.contains("masterworks") {
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
    fn artifact_perks_are_retained_without_becoming_searchable_mods() {
        use super::{
            AvailablePlug, ItemInstance, ItemLocation, PlugKind, PlugSummary, classify_plug,
        };
        use crate::{ItemHash, ItemInstanceId};
        assert_ne!(classify_plug("artifact_perks"), PlugKind::Other);
        assert_ne!(classify_plug("artifact_perks"), PlugKind::ArmorMod);
        assert_ne!(classify_plug("artifact_perks"), PlugKind::WeaponPerk);
        for category in [
            "Artifact_perks",
            "artifact_perks.extra",
            "artifact_perks_extra",
        ] {
            assert_eq!(classify_plug(category), PlugKind::Other);
        }
        assert_eq!(
            classify_plug("enhancements.season_artifact"),
            PlugKind::ArmorMod
        );
        let mut item = ItemInstance {
            instance_id: ItemInstanceId::new("artifact"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: true,
            locked: false,
            power: None,
            perks: vec![],
            stats: vec![],
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: vec![AvailablePlug {
                socket_index: 0,
                plug_hash: ItemHash::new(1),
            }],
            plugs: [1, 2, 1, 3]
                .into_iter()
                .map(|hash| PlugSummary {
                    plug_hash: ItemHash::new(hash),
                    enabled: hash != 3,
                })
                .collect(),
        };
        item.retain_searchable_plugs(|hash| {
            Some(if hash.value() == 2 {
                "shader"
            } else {
                "artifact_perks"
            })
        });
        assert_eq!(
            item.plugs
                .iter()
                .map(|plug| plug.plug_hash.value())
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
        assert!(item.available_plugs.is_empty());
    }

    #[test]
    fn subclass_category_classification_table() {
        use super::{PlugKind, SubclassPlugKind, classify_plug, classify_subclass_plug};
        use SubclassPlugKind::*;
        for (category, expected) in [
            ("hunter.arc.supers", Super),
            ("titan.solar.aspects", Aspect),
            ("warlock.stasis.totems", Aspect),
            ("shared.void.fragments", Fragment),
            ("hunter.stasis.trinkets", Fragment),
            ("shared.fragments", Fragment),
            ("shared.arc.grenades", Grenade),
            ("hunter.prism.prism_grenade", Grenade),
            ("titan.strand.melee", Melee),
            ("warlock.solar.class_abilities", ClassAbility),
            ("hunter.void.movement", Movement),
            ("shared.prism.transcendence", Transcendence),
        ] {
            assert_eq!(
                classify_subclass_plug(category),
                Some(expected),
                "{category}"
            );
            assert_eq!(
                classify_plug(category),
                PlugKind::Subclass(expected),
                "{category}"
            );
        }
        for category in [
            "exotic_new_grenade_launcher0_skins",
            "enhancements.x",
            "hunter.arc",
            "weapon.arc.supers",
            "Hunter.arc.supers",
            "hunter.fragments",
            "hunter..supers",
            "hunter.arc.supers.extra",
            "shared.fragments.extra",
            "hunter.arc.unknown",
            "",
        ] {
            assert_eq!(classify_subclass_plug(category), None, "{category}");
            assert_eq!(classify_plug(category), PlugKind::Other, "{category}");
        }
    }

    #[test]
    fn publication_seam_retains_subclass_plugs_but_not_other_plugs() {
        use super::{AvailablePlug, ItemInstance, ItemLocation, PlugSummary};
        use crate::{ItemHash, ItemInstanceId};
        let mut item = ItemInstance {
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: (1..=4)
                .map(|hash| AvailablePlug {
                    socket_index: 0,
                    plug_hash: ItemHash::new(hash),
                })
                .collect(),
            instance_id: ItemInstanceId::new("1"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: true,
            locked: false,
            power: None,
            perks: vec![],
            stats: vec![],
            plugs: [1, 2, 3, 1, 4]
                .into_iter()
                .map(|hash| PlugSummary {
                    plug_hash: ItemHash::new(hash),
                    enabled: hash != 4,
                })
                .collect(),
        };
        item.retain_searchable_plugs(|hash| match hash.value() {
            1 => Some("hunter.arc.supers"),
            2 => Some("shader"),
            3 => Some("shared.fragments"),
            4 => Some("hunter.stasis.totems"),
            _ => None,
        });
        assert_eq!(
            item.plugs
                .iter()
                .map(|plug| plug.plug_hash.value())
                .collect::<Vec<_>>(),
            vec![1, 3, 1],
        );
        // Selectable weapon/mod searches remain unchanged: subclass options are excluded.
        assert!(item.available_plugs.is_empty());
    }

    #[test]
    fn archetype_category_is_searchable_and_exact() {
        assert_ne!(
            super::classify_plug("armor_archetypes"),
            super::PlugKind::Other
        );
        for category in ["shader", "Armor_archetypes", "armor_archetypes.extra"] {
            assert_eq!(super::classify_plug(category), super::PlugKind::Other);
        }
    }

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
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: Vec::new(),
            instance_id: ItemInstanceId::new("1"),
            item_hash: ItemHash::new(1),
            location: ItemLocation::Vault,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            perks: vec![],
            stats: Vec::new(),
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

/// Scope of a provider plug-set option; flags are independent per scope.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PlugSetScope {
    Profile,
    Character(CharacterId),
}

/// One normalized option, deduplicated across plug sets within its scope.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlugSetEntry {
    pub scope: PlugSetScope,
    pub plug_hash: ItemHash,
    pub can_insert: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventorySnapshot {
    /// Explicit profile currency qualification, independent of holdings.
    pub profile_currencies_coverage: crate::ProfileCurrenciesCoverage,
    pub stacks: Vec<crate::StackObservation>,
    pub plug_set_entries: Vec<PlugSetEntry>,
    pub metadata: SnapshotMetadata,
    pub characters: Vec<Character>,
    pub items: Vec<ItemInstance>,
}

#[cfg(test)]
mod tests {
    #[test]
    fn equipment_bucket_mapping_covers_all_slots_and_excludes_subclass() {
        use super::{BucketHash, ItemSlot};
        for (hash, slot) in [
            (1498876634, ItemSlot::Kinetic),
            (2465295065, ItemSlot::Energy),
            (953998645, ItemSlot::Power),
            (3448274439, ItemSlot::Helmet),
            (3551918588, ItemSlot::Gauntlets),
            (14239492, ItemSlot::Chest),
            (20886954, ItemSlot::Legs),
            (1585787867, ItemSlot::ClassItem),
        ] {
            assert_eq!(
                ItemSlot::from_equipment_bucket(BucketHash::new(hash)),
                Some(slot)
            );
        }
        assert_eq!(
            ItemSlot::from_equipment_bucket(crate::SUBCLASS_BUCKET_HASH),
            None
        );
    }

    #[test]
    fn subclass_element_parser_and_categories() {
        use super::{SubclassElement as E, SubclassPlugKind as K, parse_subclass_category};
        for element in [
            E::Arc,
            E::Solar,
            E::Void,
            E::Stasis,
            E::Strand,
            E::Prismatic,
        ] {
            assert_eq!(element.as_str().parse::<E>().unwrap(), element);
        }
        assert_eq!(" PRISM ".parse::<E>().unwrap(), E::Prismatic);
        assert!("kinetic".parse::<E>().is_err());
        assert_eq!(
            parse_subclass_category("hunter.prism.supers"),
            Some(("hunter", Some(E::Prismatic), K::Super))
        );
        assert_eq!(
            parse_subclass_category("shared.fragments"),
            Some(("shared", None, K::Fragment))
        );
        for invalid in [
            "hunter.unknown.supers",
            "other.arc.supers",
            "hunter.arc.supers.extra",
            "shared.arc.shader",
        ] {
            assert!(parse_subclass_category(invalid).is_none());
        }
    }

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
            profile_currencies_coverage: crate::ProfileCurrenciesCoverage::NotObserved,
            stacks: Vec::new(),
            plug_set_entries: Vec::new(),
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
                date_last_played: None,
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
