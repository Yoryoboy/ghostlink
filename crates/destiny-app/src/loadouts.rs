//! Live saved loadouts joined against freshness-synchronized inventory.
use crate::ProfileSyncService;
use destiny_core::{
    AppError, CharacterId, FreshnessPolicy, ItemHash, ItemInstanceId, SubclassPlugKind, Timestamp,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutsResult {
    pub characters: Vec<CharacterLoadoutsView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterLoadoutsView {
    pub character_id: CharacterId,
    pub class: Option<String>,
    pub loadouts: Vec<LoadoutView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutView {
    /// One-based saved slot, retaining gaps from omitted unused slots.
    pub slot: u8,
    pub name: Option<String>,
    pub name_hash: u32,
    pub color_hash: u32,
    pub icon_hash: u32,
    pub items: Vec<LoadoutItemView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutItemView {
    pub instance_id: ItemInstanceId,
    pub item_hash: Option<ItemHash>,
    pub name: Option<String>,
    pub item_type: Option<String>,
    pub bucket_hash: Option<u32>,
    pub missing: bool,
    pub plugs: Vec<LoadoutPlugView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutPlugView {
    pub socket_index: u8,
    pub plug_hash: ItemHash,
    pub name: Option<String>,
    pub description: Option<String>,
    pub subclass_kind: Option<SubclassPlugKind>,
    pub empty: bool,
}
pub trait LoadoutOperations {
    fn loadouts(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<LoadoutsResult, AppError>;
}
pub struct LoadoutService<G, R> {
    sync: ProfileSyncService<G, R>,
}
impl<G, R> LoadoutService<G, R>
where
    G: destiny_core::ProfileGateway + destiny_core::LoadoutGateway,
    R: destiny_core::InventoryRepository + destiny_core::ports::WorldDefinitionRepository,
{
    pub fn new(sync: ProfileSyncService<G, R>) -> Self {
        Self { sync }
    }

    /// Requires definitions and syncs first; None selects every committed character.
    pub fn loadouts(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<LoadoutsResult, AppError> {
        LoadoutOperations::loadouts(self, selector, policy, now)
    }
}
impl<G, R> LoadoutOperations for LoadoutService<G, R>
where
    G: destiny_core::ProfileGateway + destiny_core::LoadoutGateway,
    R: destiny_core::InventoryRepository + destiny_core::ports::WorldDefinitionRepository,
{
    fn loadouts(
        &mut self,
        selector: Option<&CharacterId>,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<LoadoutsResult, AppError> {
        use destiny_core::{DestinyMembership, ErrorCategory, ProfileSyncReason};
        if self.sync.repository().current_metadata()?.is_none() {
            return Err(AppError::new(ErrorCategory::DefinitionsUnavailable));
        }
        self.sync.sync(ProfileSyncReason::Ordinary, policy, now)?;
        let snapshot = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let characters = match selector {
            Some(selector) => vec![
                crate::weekly_selection::select_weekly_character(
                    &snapshot.characters,
                    Some(selector),
                )
                .map_err(|error| {
                    AppError::new(match error {
                        crate::weekly_selection::WeeklySelectionError::InvalidRequest => {
                            ErrorCategory::ProviderValidation
                        }
                        crate::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                            ErrorCategory::AmbiguousMatch
                        }
                    })
                })?,
            ],
            None => snapshot.characters,
        };
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id,
        )?;
        let live = self.sync.gateway_mut().load_loadouts(&membership)?;
        live.validate()?;
        let repository = self.sync.repository();
        let instances: std::collections::HashMap<_, _> = snapshot
            .items
            .iter()
            .map(|item| (&item.instance_id, item))
            .collect();
        let mut output = Vec::with_capacity(characters.len());
        for character in characters {
            let mut loadouts = Vec::new();
            if let Some(saved) = live
                .characters
                .iter()
                .find(|saved| saved.character_id == character.id)
            {
                for loadout in &saved.loadouts {
                    let mut items = Vec::with_capacity(loadout.items.len());
                    for item in &loadout.items {
                        let instance = instances.get(&item.instance_id);
                        let definition = instance
                            .map(|instance| lookup_definition(repository, instance.item_hash))
                            .transpose()?
                            .flatten();
                        let mut plugs = Vec::new();
                        for plug in &item.plugs {
                            let definition = lookup_definition(repository, plug.plug_hash)?;
                            if definition.as_ref().is_some_and(is_cosmetic_plug) {
                                continue;
                            }
                            plugs.push(LoadoutPlugView {
                                socket_index: plug.socket_index,
                                plug_hash: plug.plug_hash,
                                subclass_kind: definition
                                    .as_ref()
                                    .and_then(|d| d.plug_category.as_deref())
                                    .and_then(destiny_core::classify_subclass_plug),
                                empty: definition.as_ref().is_some_and(|d| {
                                    d.name.starts_with("Empty ")
                                        && (d.name.contains("Socket") || d.name.contains("Mod"))
                                }),
                                description: if definition.is_some() {
                                    repository.lookup_item_description(plug.plug_hash)?
                                } else {
                                    None
                                },
                                name: definition.map(|d| d.name),
                            });
                        }
                        let group = item_group(definition.as_ref());
                        items.push((
                            group,
                            LoadoutItemView {
                                instance_id: item.instance_id.clone(),
                                item_hash: instance.map(|instance| instance.item_hash),
                                name: definition.as_ref().map(|d| d.name.clone()),
                                item_type: definition.as_ref().and_then(|d| d.item_type.clone()),
                                bucket_hash: definition
                                    .as_ref()
                                    .and_then(|d| d.bucket_hash)
                                    .map(|hash| {
                                        u32::try_from(hash.value())
                                            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))
                                    })
                                    .transpose()?,
                                missing: instance.is_none(),
                                plugs,
                            },
                        ));
                    }
                    // Stable sorting preserves provider order within each equipment group.
                    items.sort_by_key(|(group, _)| *group);
                    loadouts.push(LoadoutView {
                        slot: loadout.slot_index + 1,
                        name: repository.lookup_loadout_name(loadout.name_hash)?,
                        name_hash: loadout.name_hash,
                        color_hash: loadout.color_hash,
                        icon_hash: loadout.icon_hash,
                        items: items.into_iter().map(|(_, item)| item).collect(),
                    });
                }
            }
            output.push(CharacterLoadoutsView {
                character_id: character.id,
                class: character.class,
                loadouts,
            });
        }
        Ok(LoadoutsResult { characters: output })
    }
}

fn lookup_definition<R: destiny_core::DefinitionRepository>(
    repository: &R,
    hash: ItemHash,
) -> Result<Option<destiny_core::ItemDefinition>, AppError> {
    match repository.lookup_definition(hash)? {
        destiny_core::Resolution::Unique(definition) => Ok(Some(definition)),
        destiny_core::Resolution::Missing => Ok(None),
        destiny_core::Resolution::Ambiguous(_) => {
            Err(AppError::new(destiny_core::ErrorCategory::LocalStorage))
        }
    }
}

fn item_group(definition: Option<&destiny_core::ItemDefinition>) -> u8 {
    use destiny_core::{ARTIFACT_BUCKET_HASH, SUBCLASS_BUCKET_HASH, domain::ItemSlot};
    let Some(definition) = definition else {
        return 4;
    };
    match definition.bucket_hash {
        Some(SUBCLASS_BUCKET_HASH) => return 2,
        Some(ARTIFACT_BUCKET_HASH) => return 3,
        _ => (),
    }
    match definition.slot {
        Some(ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power) => 0,
        Some(
            ItemSlot::Helmet
            | ItemSlot::Gauntlets
            | ItemSlot::Chest
            | ItemSlot::Legs
            | ItemSlot::ClassItem,
        ) => 1,
        _ => match definition.bucket_hash.map(|hash| hash.value()) {
            Some(1498876634 | 2465295065 | 953998645) => 0,
            Some(3448274439 | 3551918588 | 14239492 | 20886954 | 1585787867) => 1,
            _ => 4,
        },
    }
}

/// Hide only known cosmetic categories (including kill_vfx combat flair) and
/// stat trackers, not unknown plugs.
/// Category identifiers are case-sensitive; the name fallback covers trackers
/// whose Manifest category does not identify them explicitly.
fn is_cosmetic_plug(definition: &destiny_core::ItemDefinition) -> bool {
    definition.plug_category.as_deref().is_some_and(|category| {
        category.starts_with("shader")
            || category.contains("skins")
            || category.contains("ornament")
            || category.contains("kill_vfx")
            || category == "v400.plugs.weapons.masterworks.trackers"
    }) || definition.name.ends_with("Tracker")
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::test_support::{InMemoryDefinitionRepository, InMemoryInventoryRepository};
    use destiny_core::*;
    use destiny_core::{domain::ItemSlot, ports::WorldDefinitionRepository};
    use std::cell::RefCell;
    use std::rc::Rc;
    const NOW: Timestamp = Timestamp::new(1000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    struct Repository {
        description_error: bool,
        inventory: InMemoryInventoryRepository,
        definitions: InMemoryDefinitionRepository,
    }
    impl InventoryRepository for Repository {
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.inventory.load_current()
        }
        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.inventory.replace_current(input)
        }
    }
    impl DefinitionRepository for Repository {
        fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
            self.definitions.current_metadata()
        }
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.definitions.current_definition_version()
        }
        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<ItemDefinition>, AppError> {
            self.definitions.lookup_definition(hash)
        }
        fn replace_definitions(
            &mut self,
            input: DefinitionReplacementInput,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.replace_definitions(input)
        }
    }
    impl WorldDefinitionRepository for Repository {
        fn lookup_item_description(&self, hash: ItemHash) -> Result<Option<String>, AppError> {
            if self.description_error {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            self.definitions.lookup_item_description(hash)
        }
        fn lookup_milestone(&self, hash: u32) -> Result<Option<MilestoneDefinition>, AppError> {
            self.definitions.lookup_milestone(hash)
        }
        fn lookup_activity(&self, hash: u32) -> Result<Option<ActivityDefinition>, AppError> {
            self.definitions.lookup_activity(hash)
        }
        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<ActivityModifierDefinition>, AppError> {
            self.definitions.lookup_activity_modifier(hash)
        }
        fn lookup_objective(&self, hash: u32) -> Result<Option<ObjectiveDefinition>, AppError> {
            self.definitions.lookup_objective(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<ActivityTypeDefinition>, AppError> {
            self.definitions.lookup_activity_type(hash)
        }
        fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
            Ok((hash == 7).then(|| "PvE".into()))
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            self.definitions.world_definition_counts()
        }
        fn publish_manifest(
            &mut self,
            items: DefinitionReplacementInput,
            world: WorldDefinitionsAcquisition,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.publish_manifest(items, world)
        }
    }
    struct Gateway {
        input: InventorySnapshotInput,
        live: LoadoutSnapshot,
        calls: Rc<RefCell<Vec<&'static str>>>,
    }
    impl ProfileGateway for Gateway {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.calls.borrow_mut().push("membership");
            DestinyMembership::new(self.input.membership_type, self.input.membership_id.clone())
        }
        fn load_inventory(
            &mut self,
            _: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.calls.borrow_mut().push("inventory");
            Ok(self.input.clone())
        }
    }
    impl LoadoutGateway for Gateway {
        fn load_loadouts(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<LoadoutSnapshot, AppError> {
            assert_eq!(membership.membership_id(), &self.input.membership_id);
            self.calls.borrow_mut().push("loadouts");
            Ok(self.live.clone())
        }
    }
    fn definition(
        hash: i64,
        name: &str,
        slot: Option<ItemSlot>,
        category: Option<&str>,
    ) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            name: name.into(),
            slot,
            plug_category: category.map(str::to_owned),
            bucket_hash: None,
            item_type: Some("Type".into()),
            tier: None,
            element: None,
            class_type: None,
            tags: vec![],
            version: DefinitionVersion::new("v1"),
        }
    }
    fn fixture(
        stale: bool,
    ) -> (
        LoadoutService<Gateway, Repository>,
        Rc<RefCell<Vec<&'static str>>>,
    ) {
        let definitions = vec![
            definition(1, "Weapon", Some(ItemSlot::Kinetic), None),
            definition(2, "Armor", Some(ItemSlot::Helmet), None),
            definition(10, "Aspect", None, Some("hunter.arc.aspects")),
            definition(11, "Empty Mod Socket", None, Some("armor.mod")),
            definition(12, "Shader", None, Some("shader")),
        ];
        let items = [1, 2]
            .into_iter()
            .map(|hash| ItemInstance {
                instance_id: ItemInstanceId::new(hash.to_string()),
                item_hash: ItemHash::new(hash),
                location: ItemLocation::Vault,
                quantity: 1,
                equipped: false,
                locked: false,
                power: None,
                stats: vec![],
                plugs: vec![],
                perks: vec![],
                available_plugs: vec![],
            })
            .collect();
        let characters = [("1", "Hunter"), ("2", "Titan")]
            .into_iter()
            .map(|(id, class)| Character {
                id: CharacterId::new(id),
                class: Some(class.into()),
                name: None,
                race: None,
                gender: None,
                light: None,
                date_last_played: None,
            })
            .collect();
        let input = InventorySnapshotInput {
            plug_set_entries: Vec::new(),
            membership_type: MembershipType::new(3),
            membership_id: MembershipId::new("999"),
            checked_at: NOW,
            source: SourceMarker::Profile,
            definition_version: DefinitionVersion::new("v1"),
            characters,
            items,
        };
        let mut repository = Repository {
            description_error: false,
            inventory: InMemoryInventoryRepository::new(),
            definitions: InMemoryDefinitionRepository::new(
                DefinitionVersion::new("v1"),
                definitions.clone(),
            ),
        };
        let replacement = DefinitionReplacementInput {
            definitions: destiny_core::ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions,
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        };
        let description = destiny_core::ItemDescription::new(
            ItemHash::new(10),
            "Official aspect tooltip.".into(),
        )
        .unwrap();
        repository
            .definitions
            .replace_definitions_with_descriptions(replacement, vec![description])
            .unwrap();
        if !stale {
            repository.replace_current(input.clone()).unwrap();
        }
        // The gateway contract has already removed unused slots and unset plugs;
        // retained slot/socket gaps must survive the application projection.
        let live = LoadoutSnapshot {
            characters: vec![CharacterLoadouts {
                character_id: CharacterId::new("1"),
                loadouts: vec![Loadout {
                    slot_index: 2,
                    name_hash: 7,
                    color_hash: 8,
                    icon_hash: 9,
                    items: ["2", "1", "404"]
                        .into_iter()
                        .map(|id| LoadoutItem {
                            instance_id: ItemInstanceId::new(id),
                            plugs: [10, 11, 12, 99]
                                .into_iter()
                                .enumerate()
                                .map(|(i, hash)| LoadoutPlug {
                                    socket_index: (i + 1) as u8,
                                    plug_hash: ItemHash::new(hash),
                                })
                                .collect(),
                        })
                        .collect(),
                }],
            }],
        };
        let calls = Rc::new(RefCell::new(vec![]));
        let gateway = Gateway {
            input,
            live,
            calls: calls.clone(),
        };
        (
            LoadoutService::new(ProfileSyncService::new_with_definitions(
                gateway, repository,
            )),
            calls,
        )
    }
    #[test]
    fn joins_names_missing_instances_and_retains_socket_and_slot_gaps() {
        let (mut service, calls) = fixture(true);
        let result = service.loadouts(None, TTL, NOW).unwrap();
        assert_eq!(&*calls.borrow(), &["membership", "inventory", "loadouts"]);
        assert_eq!(result.characters.len(), 2);
        assert!(result.characters[1].loadouts.is_empty());
        let loadout = &result.characters[0].loadouts[0];
        assert_eq!(
            (
                loadout.slot,
                loadout.name.as_deref(),
                loadout.color_hash,
                loadout.icon_hash
            ),
            (3, Some("PvE"), 8, 9)
        );
        assert_eq!(
            loadout
                .items
                .iter()
                .map(|i| i.instance_id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "404"]
        );
        assert_eq!(loadout.items[0].name.as_deref(), Some("Weapon"));
        assert_eq!(loadout.items[0].item_type.as_deref(), Some("Type"));
        assert!(!loadout.items[0].missing);
        let missing = &loadout.items[2];
        assert!(missing.missing);
        assert_eq!((missing.item_hash, missing.name.as_ref()), (None, None));
        let plugs = &missing.plugs;
        assert_eq!(
            plugs.iter().map(|p| p.socket_index).collect::<Vec<_>>(),
            [1, 2, 4]
        );
        assert_eq!(plugs[0].subclass_kind, Some(SubclassPlugKind::Aspect));
        assert_eq!(
            plugs[0].description.as_deref(),
            Some("Official aspect tooltip.")
        );
        assert_eq!(plugs[1].description, None);
        assert_eq!(plugs[2].description, None);
        assert!(plugs[1].empty);
        assert_eq!(plugs[2].name, None);
        assert!(!plugs[2].empty);
    }
    #[test]
    fn description_storage_error_propagates() {
        let (mut service, _) = fixture(false);
        service.sync.repository_mut().description_error = true;
        assert_eq!(
            service.loadouts(None, TTL, NOW).unwrap_err().category(),
            ErrorCategory::LocalStorage
        );
    }

    #[test]
    fn selectors_and_freshness_match_artifact() {
        for selector in ["1", "HuNtEr", "2", "Titan"] {
            let (mut service, calls) = fixture(false);
            let result = service
                .loadouts(Some(&CharacterId::new(selector)), TTL, NOW)
                .unwrap();
            assert_eq!(result.characters.len(), 1);
            assert_eq!(&*calls.borrow(), &["loadouts"]);
        }
        let (mut service, calls) = fixture(false);
        assert_eq!(
            service
                .loadouts(Some(&CharacterId::new("unknown")), TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn definitions_gate_precedes_all_gateway_calls() {
        let (mut service, calls) = fixture(true);
        service.sync.repository_mut().definitions = InMemoryDefinitionRepository::default();
        assert_eq!(
            service.loadouts(None, TTL, NOW).unwrap_err().category(),
            ErrorCategory::DefinitionsUnavailable
        );
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn ordering_is_stable_across_all_equipment_groups() {
        let mut subclass = definition(3, "Subclass", None, None);
        subclass.bucket_hash = Some(SUBCLASS_BUCKET_HASH);
        let mut artifact = definition(4, "Artifact", None, None);
        artifact.bucket_hash = Some(ARTIFACT_BUCKET_HASH);
        let mut weapon = definition(5, "Bucket-only weapon", None, None);
        weapon.bucket_hash = Some(BucketHash::new(1498876634));
        let mut armor = definition(6, "Bucket-only armor", None, None);
        armor.bucket_hash = Some(BucketHash::new(3448274439));
        let mut definitions = [
            artifact,
            definition(7, "Other", None, None),
            armor,
            subclass,
            weapon,
            definition(8, "Second weapon", Some(ItemSlot::Power), None),
        ];
        definitions.sort_by_key(|d| item_group(Some(d)));
        assert_eq!(
            definitions
                .iter()
                .map(|d| d.hash.value())
                .collect::<Vec<_>>(),
            [5, 8, 6, 3, 4, 7]
        );
        assert_eq!(item_group(None), 4);
    }
    #[test]
    fn unknown_item_definitions_names_and_ambiguous_selectors() {
        let (mut service, _) = fixture(false);
        service.sync.repository_mut().definitions = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("v1"),
            vec![definition(10, "Aspect", None, Some("hunter.arc.aspects"))],
        );
        service.sync.gateway_mut().live.characters[0].loadouts[0].name_hash = 100;
        let result = LoadoutOperations::loadouts(&mut service, None, TTL, NOW).unwrap();
        let loadout = &result.characters[0].loadouts[0];
        assert_eq!(loadout.name, None);
        assert_eq!(loadout.items[0].name, None);
        assert_eq!(loadout.items[0].item_hash, Some(ItemHash::new(2)));
        assert!(!loadout.items[0].missing);
        let (mut service, _) = fixture(false);
        let mut input = service.sync.gateway_mut().input.clone();
        input.characters[1].class = Some("Hunter".into());
        service
            .sync
            .repository_mut()
            .replace_current(input)
            .unwrap();
        assert_eq!(
            service
                .loadouts(Some(&CharacterId::new("hunter")), TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::AmbiguousMatch
        );
        assert_eq!(
            service
                .loadouts(Some(&CharacterId::new("1")), TTL, NOW)
                .unwrap()
                .characters
                .len(),
            1
        );
    }
    #[test]
    fn combat_flair_is_omitted_without_hiding_gameplay_plugs() {
        assert!(is_cosmetic_plug(&definition(
            1,
            "Default Combat Flair",
            None,
            Some("weapon_tiering_kill_vfx")
        )));
        assert!(!is_cosmetic_plug(&definition(
            2,
            "Damage perk",
            None,
            Some("weapon_tiering")
        )));
    }
    #[test]
    fn cosmetics_are_narrowly_filtered() {
        for (category, name, omitted) in [
            ("shader", "Color", true),
            ("shader.default", "Color", true),
            ("weapon.skins", "Skin", true),
            ("armor.ornament", "Look", true),
            ("v400.plugs.weapons.masterworks.trackers", "Counter", true),
            ("other", "Kill Tracker", true),
            ("armor.mod", "Mod", false),
            ("hunter.arc.aspects", "Aspect", false),
        ] {
            assert_eq!(
                is_cosmetic_plug(&definition(1, name, None, Some(category))),
                omitted,
                "{category}"
            );
        }
    }
}
