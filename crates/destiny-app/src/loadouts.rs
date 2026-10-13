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
    pub element: Option<String>,
    pub missing: bool,
    pub plugs: Vec<LoadoutPlugView>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutPlugView {
    pub socket_index: u8,
    pub plug_hash: ItemHash,
    pub name: Option<String>,
    pub description: Option<String>,
    pub clarity: Option<String>,
    pub clarity_match: Option<String>,
    pub subclass_kind: Option<SubclassPlugKind>,
    pub empty: bool,
}
/// One resolved saved loadout, retaining all enriched item and plug observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadoutDetailResult {
    pub character_id: CharacterId,
    pub class: Option<String>,
    pub loadout: LoadoutView,
}

/// Shared exact normalized name / one-based slot resolution for reads and apply.
pub fn resolve_loadout(
    candidates: &[crate::LoadoutCandidate],
    selector: &crate::LoadoutSelector,
) -> Result<crate::LoadoutCandidate, crate::ApplyLoadoutError> {
    use crate::{ApplyLoadoutError, LoadoutSelector};
    use destiny_core::SearchTerm;
    match selector {
        LoadoutSelector::Slot(slot) => {
            if !(1..=12).contains(slot) {
                return Err(ApplyLoadoutError::InvalidSlot);
            }
            let matches: Vec<_> = candidates
                .iter()
                .filter(|c| c.slot == *slot)
                .cloned()
                .collect();
            match matches.as_slice() {
                [candidate] => Ok(candidate.clone()),
                [] => Err(ApplyLoadoutError::EmptySlot),
                _ => Err(ApplyLoadoutError::AmbiguousName(matches)),
            }
        }
        LoadoutSelector::Name(name) => {
            let normalized = SearchTerm::new(name)?;
            let matches: Vec<_> = candidates
                .iter()
                .filter(|c| {
                    c.name
                        .as_deref()
                        .is_some_and(|name| SearchTerm::new(name).is_ok_and(|n| n == normalized))
                })
                .cloned()
                .collect();
            match matches.as_slice() {
                [candidate] => Ok(candidate.clone()),
                [] => Err(ApplyLoadoutError::UnknownName(candidates.to_vec())),
                _ => Err(ApplyLoadoutError::AmbiguousName(matches)),
            }
        }
    }
}

/// Omitted character may search all characters, but must resolve exactly one loadout.
pub fn select_loadout(
    result: &LoadoutsResult,
    selector: &crate::LoadoutSelector,
) -> Result<LoadoutDetailResult, crate::ApplyLoadoutError> {
    let saved: Vec<_> = result
        .characters
        .iter()
        .flat_map(|character| {
            character
                .loadouts
                .iter()
                .filter(|loadout| !loadout.items.is_empty())
                .map(move |loadout| (character, loadout))
        })
        .collect();
    let candidates: Vec<_> = saved
        .iter()
        .map(|(_, loadout)| crate::LoadoutCandidate {
            slot: loadout.slot,
            name: loadout.name.clone(),
        })
        .collect();
    let selected = resolve_loadout(&candidates, selector)?;
    let (character, loadout) = saved
        .into_iter()
        .find(|(_, loadout)| loadout.slot == selected.slot && loadout.name == selected.name)
        .expect("resolved candidate belongs to the saved loadouts");
    Ok(LoadoutDetailResult {
        character_id: character.character_id.clone(),
        class: character.class.clone(),
        loadout: loadout.clone(),
    })
}

/// Parse the positional CLI / MCP slot-or-name selector without guessing names.
pub fn parse_loadout_selector(
    value: &str,
) -> Result<crate::LoadoutSelector, crate::ApplyLoadoutError> {
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
        let slot: u8 = value
            .parse()
            .map_err(|_| crate::ApplyLoadoutError::InvalidSlot)?;
        if !(1..=12).contains(&slot) {
            return Err(crate::ApplyLoadoutError::InvalidSlot);
        }
        Ok(crate::LoadoutSelector::Slot(slot))
    } else {
        destiny_core::SearchTerm::new(value)?;
        Ok(crate::LoadoutSelector::Name(value.into()))
    }
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
        let live = match self.sync.repository().saved_loadouts()? {
            Some(characters) => destiny_core::LoadoutSnapshot { characters },
            None => self.sync.gateway_mut().load_loadouts(&membership)?,
        };
        live.validate()?;
        let repository = self.sync.repository();
        let instances: std::collections::HashMap<_, _> = snapshot
            .items
            .iter()
            .map(|item| (&item.instance_id, item))
            .collect();
        let mut clarity_cache = crate::clarity::ClarityCache::default();
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
                            let (clarity, clarity_match) = match &definition {
                                Some(plug) => clarity_cache.resolve(
                                    repository,
                                    plug,
                                    instance.map(|instance| instance.item_hash),
                                )?,
                                None => (None, None),
                            };
                            plugs.push(LoadoutPlugView {
                                clarity,
                                clarity_match,
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
                                element: definition.as_ref().and_then(|d| d.element.clone()),
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
    match definition
        .slot
        .filter(|slot| !matches!(slot, ItemSlot::Other | ItemSlot::Unknown))
        .or_else(|| {
            definition
                .bucket_hash
                .and_then(ItemSlot::from_equipment_bucket)
        }) {
        Some(ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power) => 0,
        Some(
            ItemSlot::Helmet
            | ItemSlot::Gauntlets
            | ItemSlot::Chest
            | ItemSlot::Legs
            | ItemSlot::ClassItem,
        ) => 1,
        _ => 4,
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
        effects_failure: bool,
        duplicate_neighbor_observation: bool,
        inventory: InMemoryInventoryRepository,
        definitions: InMemoryDefinitionRepository,
    }
    impl InventoryRepository for Repository {
        fn apply_acknowledged_effects(
            &mut self,
            membership: &DestinyMembership,
            effects: &[InventoryEffect],
        ) -> Result<InventorySnapshot, AppError> {
            if self.effects_failure {
                return Err(AppError::new(ErrorCategory::LocalStorage));
            }
            self.inventory
                .apply_acknowledged_effects(membership, effects)
        }
        fn saved_loadouts(&self) -> Result<Option<Vec<CharacterLoadouts>>, AppError> {
            self.inventory.saved_loadouts()
        }
        fn replace_current_profile(
            &mut self,
            input: InventorySnapshotInput,
            minted: Option<ProfileMintedAt>,
            loadouts: Option<Vec<CharacterLoadouts>>,
        ) -> Result<InventorySnapshot, AppError> {
            self.inventory
                .replace_current_profile(input, minted, loadouts)
        }
        fn invalidate_freshness(&mut self) -> Result<(), AppError> {
            self.inventory.invalidate_freshness()
        }
        fn freshness_invalidated(&self) -> Result<bool, AppError> {
            self.inventory.freshness_invalidated()
        }
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            let mut snapshot = self.inventory.load_current()?;
            // Fault injection: the reducer must reject duplicate referenced subjects.
            if self.duplicate_neighbor_observation {
                let snapshot = snapshot.as_mut().unwrap();
                let neighbor = snapshot
                    .items
                    .iter()
                    .find(|item| item.instance_id.as_str() == "91")
                    .unwrap()
                    .clone();
                snapshot.items.push(neighbor);
            }
            Ok(snapshot)
        }
        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.inventory.replace_current(input)
        }
    }
    impl SearchRepository for Repository {
        fn search_definitions(
            &self,
            query: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.search_projection()?.search_definitions(query)
        }
        fn search_instances(
            &self,
            query: &InstanceSearchQuery,
        ) -> Result<Vec<ItemInstance>, AppError> {
            self.search_projection()?.search_instances(query)
        }
        fn search_instance_query_page(
            &self,
            query: &InstanceSearchQuery,
            execution: Option<&destiny_core::search_query::ResolvedQuery>,
        ) -> Result<(Vec<ItemInstance>, bool), AppError> {
            if execution.is_none() {
                return self.search_instance_page(query);
            }
            self.search_projection()?
                .search_instance_query_page(query, execution)
        }
        fn lookup_instance(
            &self,
            id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.search_projection()?.lookup_instance(id)
        }
    }
    impl Repository {
        fn search_projection(
            &self,
        ) -> Result<crate::test_support::InMemorySearchRepository, AppError> {
            let definitions = [1, 2]
                .into_iter()
                .filter_map(
                    |hash| match self.definitions.lookup_definition(ItemHash::new(hash)) {
                        Ok(Resolution::Unique(definition)) => Some(definition),
                        _ => None,
                    },
                )
                .collect();
            Ok(
                crate::test_support::InMemorySearchRepository::with_projection(
                    definitions,
                    &self.inventory.load_current()?.unwrap(),
                ),
            )
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
        fn lookup_clarity_by_hash(&self, hash: u32) -> Result<Option<ClarityInsight>, AppError> {
            Ok((hash == 10).then(|| ClarityInsight {
                hash,
                name: "Aspect".into(),
                kind: "Subclass Aspect".into(),
                item_hash: None,
                item_name: None,
                text: "  Community research.\nSecond line.\n".into(),
            }))
        }
        fn lookup_clarity_by_name(&self, name: &str) -> Result<Vec<ClarityInsight>, AppError> {
            if name != "Parent-specific Aspect" {
                return Ok(vec![]);
            }
            Ok([(1, "Parent A research"), (2, "Parent B research")]
                .into_iter()
                .map(|(parent, text)| ClarityInsight {
                    hash: 100 + parent,
                    name: name.into(),
                    kind: "Subclass Aspect".into(),
                    item_hash: Some(parent),
                    item_name: None,
                    text: text.into(),
                })
                .collect())
        }
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
        cache: bool,
        input: InventorySnapshotInput,
        live: LoadoutSnapshot,
        calls: Rc<RefCell<Vec<&'static str>>>,
    }
    impl ProfileGateway for Gateway {
        fn load_inventory_profile_scoped(
            &mut self,
            membership: &DestinyMembership,
            reason: ProfileSyncReason,
        ) -> Result<ProfileInventoryRead, AppError> {
            let pair = self.load_inventory_with_observations_scoped(membership, reason)?;
            let rows = pair.saved_loadouts().map(<[_]>::to_vec);
            Ok((pair.into_parts().0, None, rows))
        }
        fn load_inventory_with_observations_scoped(
            &mut self,
            membership: &DestinyMembership,
            _: ProfileSyncReason,
        ) -> Result<ProfileAcquisition, AppError> {
            let input = self.load_inventory(membership)?;
            Ok(ProfileAcquisition::new(input, None)?
                .with_saved_loadouts(self.cache.then(|| self.live.characters.clone())))
        }
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
            does_postmaster_pull_have_side_effects: None,
            is_instance_item: None,
            max_stack_size: None,
            stack_unique_label: None,
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
            profile_currencies_coverage: destiny_core::ProfileCurrenciesCoverage::NotObserved,
            stacks: Vec::new(),
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
            effects_failure: false,
            duplicate_neighbor_observation: false,
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
            cache: false,
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
    fn apply_request(loadout: crate::LoadoutSelector) -> crate::ApplyLoadoutRequest {
        crate::ApplyLoadoutRequest {
            character: CharacterId::new("HuNtEr"),
            loadout,
            intent: MoveIntentAuthorization::Explicit,
        }
    }

    #[test]
    fn fresh_cached_loadouts_are_gateway_free_and_legacy_falls_back_once() {
        let (mut service, calls) = fixture(false);
        service.loadouts(None, TTL, NOW).unwrap();
        assert_eq!(&*calls.borrow(), &["loadouts"]);
        calls.borrow_mut().clear();
        let rows = service.sync.gateway_mut().live.characters.clone();
        let input = service.sync.gateway_mut().input.clone();
        service
            .sync
            .repository_mut()
            .replace_current_profile(input, None, Some(rows))
            .unwrap();
        service.loadouts(None, TTL, NOW).unwrap();
        assert!(calls.borrow().is_empty());
    }

    fn cached_apply_fixture() -> (
        LoadoutService<Gateway, Repository>,
        Rc<RefCell<Vec<&'static str>>>,
    ) {
        let (mut read, calls) = fixture(false);
        let mut definitions = vec![
            definition(1, "Weapon", Some(ItemSlot::Kinetic), None),
            definition(2, "Armor", Some(ItemSlot::Helmet), None),
        ];
        for definition in &mut definitions {
            definition.tier = Some("Legendary".into());
            definition.class_type = Some(domain::ClassType::Any);
        }
        read.sync.repository_mut().definitions =
            InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), definitions);
        let mut input = read.sync.gateway_mut().input.clone();
        input.items[0].plugs = vec![domain::PlugSummary {
            plug_hash: ItemHash::new(99),
            enabled: true,
        }];
        // One Vault item and one unequipped item already on the target character,
        // plus equipped replacements. Bungie's EquipLoadout handles both sources.
        input.items[1].location = ItemLocation::Character(CharacterId::new("1"));
        for item in input.items.clone() {
            let mut previous = item;
            previous.instance_id =
                ItemInstanceId::new(format!("9{}", previous.instance_id.as_str()));
            previous.location = ItemLocation::Character(CharacterId::new("1"));
            previous.equipped = true;
            input.items.push(previous);
        }
        let mut rows = read.sync.gateway_mut().live.characters.clone();
        rows[0].loadouts[0]
            .items
            .retain(|item| item.instance_id.as_str() != "404");
        read.sync
            .repository_mut()
            .replace_current_profile(input, None, Some(rows))
            .unwrap();
        (read, calls)
    }

    #[test]
    fn acknowledged_cached_apply_updates_equipment_revision_and_preserves_plugs() {
        let (read, calls) = cached_apply_fixture();
        let before = read.sync.repository().load_current().unwrap().unwrap();
        let mut service = crate::ApplyLoadoutService::new(read.sync);
        let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
        action.queue(Ok(LoadoutActionOutcome::Applied));
        let result = service
            .apply(
                &mut action,
                &apply_request(crate::LoadoutSelector::Slot(3)),
                TTL,
                NOW,
            )
            .unwrap();
        assert!(result.local_state_updated);
        assert!(calls.borrow().is_empty());
        assert_eq!(action.calls(), 1);
        let sync = service.into_sync();
        assert!(!sync.repository().freshness_invalidated().unwrap());
        let after = sync.repository().load_current().unwrap().unwrap();
        assert_eq!(
            after.metadata.revision.value(),
            before.metadata.revision.value() + 1
        );
        assert_eq!(after.metadata.checked_at, before.metadata.checked_at);
        for item in &after.items {
            assert_eq!(
                item.location,
                ItemLocation::Character(CharacterId::new("1"))
            );
            assert_eq!(item.equipped, !item.instance_id.as_str().starts_with('9'));
            let prior = before
                .items
                .iter()
                .find(|prior| prior.instance_id == item.instance_id)
                .unwrap();
            assert_eq!(item.plugs, prior.plugs);
            assert_eq!(item.available_plugs, prior.available_plugs);
        }
        let mut mutation = crate::MutationService::new(
            sync,
            crate::test_support::DeterministicPlanIds::starting_at(1),
        );
        let move_request = MoveRequest {
            intent: MoveIntentAuthorization::Explicit,
            membership: after.metadata.membership_id.clone(),
            item: ItemSelector::Instance(ItemInstanceId::new("91")),
            destination: DestinationSelector::Vault,
        };
        let lifetime = PlanLifetime::new(60).unwrap();
        let plan = mutation
            .plan_move(&move_request, lifetime, TTL, NOW)
            .unwrap();
        assert_eq!(plan.source, ItemLocation::Character(CharacterId::new("1")));
        assert_eq!(plan.revision, after.metadata.revision);
        let equip = EquipRequest {
            intent: MoveIntentAuthorization::Explicit,
            membership: after.metadata.membership_id,
            item: ItemSelector::Instance(ItemInstanceId::new("1")),
            destination: DestinationSelector::Character(CharacterId::new("1")),
        };
        let plan = mutation.plan_equip(&equip, lifetime, TTL, NOW).unwrap();
        assert!(plan.is_already_satisfied());
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn cached_apply_reducer_rejection_applies_remaining_effects_and_invalidates() {
        let (mut read, calls) = cached_apply_fixture();
        read.sync.repository_mut().duplicate_neighbor_observation = true;
        let mut service = crate::ApplyLoadoutService::new(read.sync);
        let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
        action.queue(Ok(LoadoutActionOutcome::Applied));
        let result = service
            .apply(
                &mut action,
                &apply_request(crate::LoadoutSelector::Slot(3)),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(result.receipt, LoadoutActionOutcome::Applied);
        assert!(!result.local_state_updated);
        assert_eq!(action.calls(), 1);
        assert!(calls.borrow().is_empty());
        let sync = service.into_sync();
        assert!(sync.repository().freshness_invalidated().unwrap());
        let snapshot = sync.repository().inventory.load_current().unwrap().unwrap();
        let item = |id| {
            snapshot
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == id)
                .unwrap()
        };
        assert_eq!(item("1").location, ItemLocation::Vault);
        assert!(!item("1").equipped);
        assert!(item("91").equipped);
        assert_eq!(
            item("2").location,
            ItemLocation::Character(CharacterId::new("1"))
        );
        assert!(item("2").equipped);
        assert!(!item("92").equipped);
    }

    #[test]
    fn cached_apply_switches_subclass_without_reads_or_invalidation() {
        let (mut read, calls) = cached_apply_fixture();
        let mut input = InventorySnapshotInput::with_metadata(
            &read.sync.repository().load_current().unwrap().unwrap(),
        );
        let mut subclass = input.items[0].clone();
        subclass.instance_id = ItemInstanceId::new("3");
        subclass.item_hash = ItemHash::new(3);
        subclass.location = ItemLocation::Character(CharacterId::new("1"));
        let mut old = subclass.clone();
        old.instance_id = ItemInstanceId::new("93");
        old.item_hash = ItemHash::new(4);
        old.equipped = true;
        input.items.extend([subclass, old]);
        let mut rows = read.sync.repository().saved_loadouts().unwrap().unwrap();
        rows[0].loadouts[0].items.push(LoadoutItem {
            instance_id: ItemInstanceId::new("3"),
            plugs: vec![LoadoutPlug {
                socket_index: 1,
                plug_hash: ItemHash::new(10),
            }],
        });
        let mut definitions = [1, 2]
            .into_iter()
            .map(|hash| {
                let Resolution::Unique(definition) = read
                    .sync
                    .repository()
                    .lookup_definition(ItemHash::new(hash))
                    .unwrap()
                else {
                    panic!("fixture definition missing")
                };
                definition
            })
            .collect::<Vec<_>>();
        for hash in [3, 4] {
            let mut definition = definition(hash, "Subclass", None, None);
            definition.bucket_hash = Some(SUBCLASS_BUCKET_HASH);
            definitions.push(definition);
        }
        read.sync.repository_mut().definitions =
            InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), definitions);
        let before = read
            .sync
            .repository_mut()
            .replace_current_profile(input, None, Some(rows))
            .unwrap();
        // Single-item equip continues to reject subclass items.
        assert_eq!(
            crate::mutation::equip_effect(
                read.sync.repository(),
                before
                    .items
                    .iter()
                    .find(|item| item.instance_id.as_str() == "3")
                    .unwrap(),
                &CharacterId::new("1"),
                &before
            ),
            Err(crate::MutationError::NotEquippable)
        );
        let mut service = crate::ApplyLoadoutService::new(read.sync);
        let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
        action.queue(Ok(LoadoutActionOutcome::Applied));
        let result = service
            .apply(
                &mut action,
                &apply_request(crate::LoadoutSelector::Slot(3)),
                TTL,
                NOW,
            )
            .unwrap();
        assert!(result.local_state_updated);
        assert_eq!(action.calls(), 1);
        assert!(calls.borrow().is_empty());
        let sync = service.into_sync();
        assert!(!sync.repository().freshness_invalidated().unwrap());
        let after = sync.repository().load_current().unwrap().unwrap();
        assert_eq!(
            after.metadata.revision.value(),
            before.metadata.revision.value() + 1
        );
        for id in ["3", "93"] {
            let item = after
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == id)
                .unwrap();
            let prior = before
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == id)
                .unwrap();
            assert_eq!(item.equipped, id == "3");
            assert_eq!(item.location, prior.location);
            assert_eq!(item.plugs, prior.plugs);
            assert_eq!(item.available_plugs, prior.available_plugs);
        }
        let mut mutation = crate::MutationService::new(
            sync,
            crate::test_support::DeterministicPlanIds::starting_at(1),
        );
        let request = MoveRequest {
            intent: MoveIntentAuthorization::Explicit,
            membership: after.metadata.membership_id,
            item: ItemSelector::Instance(ItemInstanceId::new("91")),
            destination: DestinationSelector::Vault,
        };
        let plan = mutation
            .plan_move(&request, PlanLifetime::new(60).unwrap(), TTL, NOW)
            .unwrap();
        assert_eq!(plan.revision, after.metadata.revision);
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn cached_apply_subclass_not_on_target_applies_other_items_and_invalidates() {
        for location in [
            Some(ItemLocation::Vault),
            Some(ItemLocation::Character(CharacterId::new("2"))),
            None,
        ] {
            let (mut read, calls) = cached_apply_fixture();
            let mut input = InventorySnapshotInput::with_metadata(
                &read.sync.repository().load_current().unwrap().unwrap(),
            );
            let mut subclass = input.items[0].clone();
            subclass.instance_id = ItemInstanceId::new("3");
            subclass.item_hash = ItemHash::new(3);
            if let Some(location) = &location {
                subclass.location = location.clone();
                input.items.push(subclass);
            }
            let mut rows = read.sync.repository().saved_loadouts().unwrap().unwrap();
            rows[0].loadouts[0].items.push(LoadoutItem {
                instance_id: ItemInstanceId::new("3"),
                plugs: vec![],
            });
            let mut subclass = definition(3, "Subclass", None, None);
            subclass.bucket_hash = Some(SUBCLASS_BUCKET_HASH);
            read.sync.repository_mut().definitions = InMemoryDefinitionRepository::new(
                DefinitionVersion::new("v1"),
                vec![
                    definition(1, "Weapon", Some(ItemSlot::Kinetic), None),
                    definition(2, "Armor", Some(ItemSlot::Helmet), None),
                    subclass,
                ],
            );
            read.sync
                .repository_mut()
                .replace_current_profile(input, None, Some(rows))
                .unwrap();
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            action.queue(Ok(LoadoutActionOutcome::Applied));
            let result = service
                .apply(
                    &mut action,
                    &apply_request(crate::LoadoutSelector::Slot(3)),
                    TTL,
                    NOW,
                )
                .unwrap();
            assert!(!result.local_state_updated);
            assert!(calls.borrow().is_empty());
            let sync = service.into_sync();
            assert!(sync.repository().freshness_invalidated().unwrap());
            let snapshot = sync.repository().load_current().unwrap().unwrap();
            let subclass = snapshot
                .items
                .iter()
                .find(|item| item.instance_id.as_str() == "3");
            match &location {
                Some(location) => {
                    let subclass = subclass.unwrap();
                    assert!(!subclass.equipped);
                    assert_eq!(&subclass.location, location);
                }
                None => assert!(subclass.is_none()),
            }
            assert!(
                snapshot
                    .items
                    .iter()
                    .filter(|item| ["1", "2"].contains(&item.instance_id.as_str()))
                    .all(|item| item.equipped)
            );
        }
    }

    /// Bungie's EquipLoadout neither pulls an item held by another character nor
    /// one in the Postmaster, so such an item is reported, never projected.
    #[test]
    fn cached_apply_reports_items_outside_reach_without_projecting_them() {
        for (location, equipped, reason) in [
            (
                ItemLocation::Character(CharacterId::new("2")),
                true,
                crate::LoadoutLocalReason::OnOtherCharacter,
            ),
            (
                ItemLocation::Character(CharacterId::new("2")),
                false,
                crate::LoadoutLocalReason::OnOtherCharacter,
            ),
            (
                ItemLocation::Postmaster(CharacterId::new("1")),
                false,
                crate::LoadoutLocalReason::InPostmaster,
            ),
        ] {
            let (mut read, calls) = cached_apply_fixture();
            let mut input = InventorySnapshotInput::with_metadata(
                &read.sync.repository().load_current().unwrap().unwrap(),
            );
            let item = input
                .items
                .iter_mut()
                .find(|item| item.instance_id.as_str() == "2")
                .unwrap();
            item.location = location.clone();
            item.equipped = equipped;
            let rows = read.sync.repository().saved_loadouts().unwrap().unwrap();
            read.sync
                .repository_mut()
                .replace_current_profile(input, None, Some(rows))
                .unwrap();
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            action.queue(Ok(LoadoutActionOutcome::Applied));
            let result = service
                .apply(
                    &mut action,
                    &apply_request(crate::LoadoutSelector::Slot(3)),
                    TTL,
                    NOW,
                )
                .unwrap();
            assert_eq!(result.receipt, LoadoutActionOutcome::Applied);
            assert!(!result.local_state_updated);
            assert_eq!(
                result.local_state.status,
                crate::LoadoutLocalStatus::Partial
            );
            assert!(result.local_state.persisted);
            assert!(result.local_state.skipped.is_empty());
            assert_eq!(result.local_state.unresolved_items.len(), 1);
            let issue = &result.local_state.unresolved_items[0];
            assert_eq!(issue.reason, reason);
            assert_eq!(issue.item.instance_id.as_str(), "2");
            assert_eq!(issue.item.name.as_deref(), Some("Armor"));
            assert_eq!(issue.item.bucket.as_deref(), Some("helmet"));
            assert_eq!(action.calls(), 1);
            assert!(calls.borrow().is_empty());
            let sync = service.into_sync();
            assert!(sync.repository().freshness_invalidated().unwrap());
            let snapshot = sync.repository().load_current().unwrap().unwrap();
            let item = |id| {
                snapshot
                    .items
                    .iter()
                    .find(|item| item.instance_id.as_str() == id)
                    .unwrap()
            };
            // The unreachable item keeps its place; the target keeps its helmet.
            assert_eq!(item("2").location, location);
            assert_eq!(item("2").equipped, equipped);
            assert!(item("92").equipped);
            // The reachable Vault item is still projected onto the target.
            assert_eq!(
                item("1").location,
                ItemLocation::Character(CharacterId::new("1"))
            );
            assert!(item("1").equipped);
            assert!(!item("91").equipped);
        }
    }

    /// The seasonal artifact is carried by every saved loadout and has no
    /// equipment projection, so its skip is reported without making the
    /// projection partial. Any other unsupported bucket stays partial.
    #[test]
    fn cached_apply_artifact_skip_is_informational_but_other_buckets_stay_partial() {
        for (bucket, complete) in [
            (Some(destiny_core::ARTIFACT_BUCKET_HASH), true),
            (Some(BucketHash::new(77)), false),
        ] {
            let (mut read, calls) = cached_apply_fixture();
            let mut input = InventorySnapshotInput::with_metadata(
                &read.sync.repository().load_current().unwrap().unwrap(),
            );
            let mut extra = input.items[0].clone();
            extra.instance_id = ItemInstanceId::new("5");
            extra.item_hash = ItemHash::new(5);
            extra.location = ItemLocation::Character(CharacterId::new("1"));
            extra.equipped = true;
            input.items.push(extra);
            let mut rows = read.sync.repository().saved_loadouts().unwrap().unwrap();
            rows[0].loadouts[0].items.push(LoadoutItem {
                instance_id: ItemInstanceId::new("5"),
                plugs: vec![],
            });
            let mut extra = definition(5, "Tablet of Ruin", None, None);
            extra.bucket_hash = bucket;
            let mut definitions = vec![
                definition(1, "Weapon", Some(ItemSlot::Kinetic), None),
                definition(2, "Armor", Some(ItemSlot::Helmet), None),
            ];
            for definition in &mut definitions {
                definition.tier = Some("Legendary".into());
                definition.class_type = Some(domain::ClassType::Any);
            }
            definitions.push(extra);
            read.sync.repository_mut().definitions =
                InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), definitions);
            read.sync
                .repository_mut()
                .replace_current_profile(input, None, Some(rows))
                .unwrap();
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            action.queue(Ok(LoadoutActionOutcome::Applied));
            let result = service
                .apply(
                    &mut action,
                    &apply_request(crate::LoadoutSelector::Slot(3)),
                    TTL,
                    NOW,
                )
                .unwrap();
            assert_eq!(result.local_state_updated, complete);
            assert_eq!(
                result.local_state.status,
                if complete {
                    crate::LoadoutLocalStatus::Updated
                } else {
                    crate::LoadoutLocalStatus::Partial
                }
            );
            assert!(result.local_state.unresolved_items.is_empty());
            assert_eq!(result.local_state.skipped.len(), 1);
            assert_eq!(
                result.local_state.skipped[0].reason,
                crate::LoadoutLocalReason::UnsupportedBucket
            );
            assert_eq!(
                result.local_state.skipped[0].item.name.as_deref(),
                Some("Tablet of Ruin")
            );
            assert!(calls.borrow().is_empty());
            let sync = service.into_sync();
            assert_eq!(
                sync.repository().freshness_invalidated().unwrap(),
                !complete
            );
            let snapshot = sync.repository().load_current().unwrap().unwrap();
            assert!(
                snapshot
                    .items
                    .iter()
                    .filter(|item| ["1", "2"].contains(&item.instance_id.as_str()))
                    .all(|item| item.equipped)
            );
        }
    }

    #[test]
    fn cached_apply_persistence_failure_keeps_receipt_invalidates_and_never_retries() {
        let (mut read, calls) = cached_apply_fixture();
        read.sync.repository_mut().effects_failure = true;
        let before = read.sync.repository().load_current().unwrap();
        let mut service = crate::ApplyLoadoutService::new(read.sync);
        let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
        action.queue(Ok(LoadoutActionOutcome::Applied));
        let result = service
            .apply(
                &mut action,
                &apply_request(crate::LoadoutSelector::Slot(3)),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(result.receipt, LoadoutActionOutcome::Applied);
        assert!(!result.local_state_updated);
        assert_eq!(action.calls(), 1);
        assert!(calls.borrow().is_empty());
        let sync = service.into_sync();
        assert!(sync.repository().freshness_invalidated().unwrap());
        assert_eq!(sync.repository().load_current().unwrap(), before);
    }

    #[test]
    fn apply_resolves_cached_slot_and_name_without_live_read() {
        for selector in [
            crate::LoadoutSelector::Slot(3),
            crate::LoadoutSelector::Name("PvE".into()),
        ] {
            let (mut read, calls) = fixture(false);
            let rows = read.sync.gateway_mut().live.characters.clone();
            let input = read.sync.gateway_mut().input.clone();
            read.sync
                .repository_mut()
                .replace_current_profile(input, None, Some(rows))
                .unwrap();
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            action.queue(Ok(LoadoutActionOutcome::Applied));
            let result = service
                .apply(&mut action, &apply_request(selector), TTL, NOW)
                .unwrap();
            assert_eq!(result.slot, 3);
            assert!(calls.borrow().is_empty());
            assert_eq!(action.calls(), 1);
        }
    }

    #[test]
    fn apply_saved_loadout_acknowledges_once_without_post_action_read() {
        for selector in [
            crate::LoadoutSelector::Slot(3),
            crate::LoadoutSelector::Name("  pVe ".into()),
        ] {
            let (read, calls) = fixture(false);
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            action.queue(Ok(LoadoutActionOutcome::Applied));
            let result = service
                .apply(&mut action, &apply_request(selector), TTL, NOW)
                .unwrap();
            assert_eq!(result.receipt, LoadoutActionOutcome::Applied);
            assert!(!result.local_state_updated); // Missing saved item: known effects still apply.
            assert_eq!(result.character, CharacterId::new("1"));
            assert_eq!((result.slot, result.name.as_deref()), (3, Some("PvE")));
            assert_eq!(action.calls(), 1);
            assert_eq!(action.requests()[0].loadout_index(), 2);
            assert_eq!(&*calls.borrow(), &["loadouts"]);
            let sync = service.into_sync();
            assert!(
                sync.repository()
                    .load_current()
                    .unwrap()
                    .unwrap()
                    .items
                    .iter()
                    .all(|item| item.equipped
                        && item.location == ItemLocation::Character(CharacterId::new("1")))
            );
            assert!(sync.repository().freshness_invalidated().unwrap());
        }
    }

    #[test]
    fn apply_saved_loadout_preserves_all_receipts_and_fails_closed_on_storage() {
        for receipt in [
            LoadoutActionOutcome::Applied,
            LoadoutActionOutcome::Rejected(LoadoutActionRejection::RestrictedLocation),
            LoadoutActionOutcome::AuthenticationRefused,
            LoadoutActionOutcome::Transient,
            LoadoutActionOutcome::Uncertain,
        ] {
            for storage_failure in [false, true] {
                let (mut read, calls) = fixture(false);
                if storage_failure {
                    let inventory = std::mem::take(&mut read.sync.repository_mut().inventory);
                    read.sync.repository_mut().inventory = inventory.with_invalidation_failure();
                }
                let mut service = crate::ApplyLoadoutService::new(read.sync);
                let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
                action.queue(Ok(receipt));
                let result = service
                    .apply(
                        &mut action,
                        &apply_request(crate::LoadoutSelector::Slot(3)),
                        TTL,
                        NOW,
                    )
                    .unwrap();
                assert_eq!(result.receipt, receipt);
                assert!(!result.local_state_updated);
                assert_eq!(action.calls(), 1);
                assert_eq!(&*calls.borrow(), &["loadouts"]);
                assert_eq!(
                    service
                        .into_sync()
                        .repository()
                        .freshness_invalidated()
                        .unwrap(),
                    !storage_failure
                );
            }
        }
    }

    #[test]
    fn apply_saved_loadout_name_resolution_never_guesses() {
        for ambiguous in [false, true] {
            let (mut read, _) = fixture(false);
            if ambiguous {
                let mut duplicate = read.sync.gateway_mut().live.characters[0].loadouts[0].clone();
                duplicate.slot_index = 4;
                read.sync.gateway_mut().live.characters[0]
                    .loadouts
                    .push(duplicate);
            }
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            let name = if ambiguous { "PvE" } else { "Pv" };
            let error = service
                .apply(
                    &mut action,
                    &apply_request(crate::LoadoutSelector::Name(name.into())),
                    TTL,
                    NOW,
                )
                .unwrap_err();
            match error {
                crate::ApplyLoadoutError::AmbiguousName(candidates) if ambiguous => {
                    assert_eq!(candidates.len(), 2)
                }
                crate::ApplyLoadoutError::UnknownName(candidates) if !ambiguous => {
                    assert_eq!(candidates[0].slot, 3)
                }
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(action.calls(), 0);
        }
    }

    #[test]
    fn apply_saved_loadout_invalid_empty_and_unauthorized_never_dispatch() {
        for slot in [0, 1, 13, 255] {
            let (read, calls) = fixture(false);
            let mut service = crate::ApplyLoadoutService::new(read.sync);
            let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
            let error = service
                .apply(
                    &mut action,
                    &apply_request(crate::LoadoutSelector::Slot(slot)),
                    TTL,
                    NOW,
                )
                .unwrap_err();
            assert_eq!(
                error,
                if slot == 1 {
                    crate::ApplyLoadoutError::EmptySlot
                } else {
                    crate::ApplyLoadoutError::InvalidSlot
                }
            );
            assert_eq!(action.calls(), 0);
            assert_eq!(calls.borrow().len(), if slot == 1 { 1 } else { 0 });
        }
        let (read, calls) = fixture(false);
        let mut service = crate::ApplyLoadoutService::new(read.sync);
        let mut action = crate::test_support::QueuedLoadoutActionGateway::new();
        let mut request = apply_request(crate::LoadoutSelector::Slot(3));
        request.intent = MoveIntentAuthorization::Absent;
        assert_eq!(
            service.apply(&mut action, &request, TTL, NOW),
            Err(crate::ApplyLoadoutError::MissingIntent)
        );
        assert!(calls.borrow().is_empty());
        assert_eq!(action.calls(), 0);
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
        assert_eq!(
            plugs[0].clarity.as_deref(),
            Some("  Community research.\nSecond line.\n")
        );
        assert_eq!(plugs[0].clarity_match, None);
        assert_eq!(plugs[1].clarity, None);
        assert_eq!(plugs[2].clarity, None);
        assert_eq!(plugs[1].description, None);
        assert_eq!(plugs[2].description, None);
        assert!(plugs[1].empty);
        assert_eq!(plugs[2].name, None);
        assert!(!plugs[2].empty);
    }
    #[test]
    fn same_plug_hash_retains_parent_specific_clarity_after_selection() {
        let (mut service, _) = fixture(false);
        service.sync.repository_mut().definitions = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("v1"),
            vec![
                definition(1, "Weapon", Some(ItemSlot::Kinetic), None),
                definition(2, "Armor", Some(ItemSlot::Helmet), None),
                definition(
                    13,
                    "Parent-specific Aspect",
                    None,
                    Some("hunter.arc.aspects"),
                ),
            ],
        );
        for item in &mut service.sync.gateway_mut().live.characters[0].loadouts[0].items {
            item.plugs = vec![LoadoutPlug {
                socket_index: 2,
                plug_hash: ItemHash::new(13),
            }];
        }
        let result = service
            .loadouts(Some(&CharacterId::new("hunter")), TTL, NOW)
            .unwrap();
        let selected = select_loadout(&result, &crate::LoadoutSelector::Slot(3)).unwrap();
        assert_eq!(
            selected.loadout.items[0].plugs[0].clarity.as_deref(),
            Some("Parent A research")
        );
        assert_eq!(
            selected.loadout.items[1].plugs[0].clarity.as_deref(),
            Some("Parent B research")
        );
        assert_eq!(
            selected.loadout.items[0].plugs[0].plug_hash,
            selected.loadout.items[1].plugs[0].plug_hash
        );
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
