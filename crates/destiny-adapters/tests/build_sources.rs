use destiny_adapters::{dim_loadout::DimBuildCodec, sqlite::SqliteStore};
use destiny_app::{
    ProfileSyncService,
    builds::{BuildOmission, BuildService},
};
use destiny_core::domain::ItemSlot;
use destiny_core::ports::WorldDefinitionRepository;
use destiny_core::*;

fn definition(hash: i64, bucket: Option<BucketHash>, category: Option<&str>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: bucket,
        name: format!("Item {hash}"),
        item_type: category.map(|_| "Armor Mod".into()),
        tier: None,
        element: None,
        slot: bucket.and_then(ItemSlot::from_equipment_bucket),
        class_type: None,
        plug_category: category.map(str::to_owned),
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: ArmorDefinition {
            plug_category_hash: category.map(|_| 2912171003),
            ..Default::default()
        },
        non_transferrable: None,
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
    }
}
struct Gateway(InventorySnapshotInput, Vec<CharacterLoadouts>, usize);
impl ProfileGateway for Gateway {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        self.2 += 1;
        DestinyMembership::new(self.0.membership_type, self.0.membership_id.clone())
    }
    fn load_inventory(
        &mut self,
        _: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.2 += 1;
        Ok(self.0.clone())
    }
    fn load_inventory_profile_scoped(
        &mut self,
        _: &DestinyMembership,
        _: ProfileSyncReason,
    ) -> Result<ProfileInventoryRead, AppError> {
        self.2 += 1;
        Ok((self.0.clone(), None, Some(self.1.clone())))
    }
}
fn fixture() -> ProfileSyncService<Gateway, SqliteStore> {
    let character = Character {
        id: CharacterId::new("1"),
        class: Some("Hunter".into()),
        name: None,
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    };
    let items = [(1, 1498876634), (2, 3448274439), (3, 3284755031), (4, 999)]
        .into_iter()
        .map(|(id, hash)| ItemInstance {
            instance_id: ItemInstanceId::new(id.to_string()),
            item_hash: ItemHash::new(hash),
            location: ItemLocation::Character(character.id.clone()),
            quantity: 1,
            equipped: true,
            locked: false,
            power: None,
            stats: vec![],
            perks: vec![],
            plugs: vec![PlugSummary {
                plug_hash: ItemHash::new(10),
                enabled: true,
            }],
            energy: None,
            gear_tier: None,
            sockets: Vec::new(),
            transfer_status: None,
            available_plugs: vec![],
        })
        .collect();
    let input = InventorySnapshotInput {
        membership_id: MembershipId::new("99"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters: vec![character],
        items,
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        stacks: vec![],
        plug_set_entries: vec![],
    };
    let saved = vec![CharacterLoadouts {
        character_id: CharacterId::new("1"),
        loadouts: vec![Loadout {
            slot_index: 2,
            name_hash: 7,
            color_hash: 8,
            icon_hash: 9,
            items: ["1", "2", "3", "404"]
                .into_iter()
                .map(|id| LoadoutItem {
                    instance_id: ItemInstanceId::new(id),
                    plugs: vec![LoadoutPlug {
                        socket_index: 7,
                        plug_hash: ItemHash::new(10),
                    }],
                })
                .collect(),
        }],
    }];
    let mut repository = SqliteStore::open_in_memory().unwrap();
    repository
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions: vec![
                    definition(1498876634, Some(BucketHash::new(1498876634)), None),
                    definition(3448274439, Some(SUBCLASS_BUCKET_HASH), None),
                    definition(3284755031, Some(BucketHash::new(3448274439)), None),
                    definition(999, Some(BucketHash::new(999)), None),
                    definition(10, None, Some("enhancements.v2_head")),
                ],
            },
            checked_at: Timestamp::new(100),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    repository
        .replace_current_profile(input.clone(), None, Some(saved.clone()))
        .unwrap();
    ProfileSyncService::new_with_definitions(Gateway(input, saved, 0), repository)
}
#[test]
fn dim_mod_classification_uses_hashes_not_display_names() {
    let mut plug = definition(10, None, Some("enhancements.v2_head"));
    plug.name = "Empty misleading Socket".into();
    assert!(build_capture::insertable_armor_mod(&plug));
    plug.hash = ItemHash::new(807186981);
    plug.name = "Not empty by name".into();
    assert!(!build_capture::insertable_armor_mod(&plug));
    plug.hash = ItemHash::new(10);
    plug.bucket_hash = Some(BucketHash::new(3313201758));
    assert!(!build_capture::insertable_armor_mod(&plug));
    plug.bucket_hash = None;
    plug.armor.plug_category_hash = None;
    assert!(!build_capture::insertable_armor_mod(&plug));
    plug.armor.plug_category_hash = Some(2912171003);
    plug.item_type = None;
    assert!(!build_capture::insertable_armor_mod(&plug));
}

#[test]
fn equipped_capture_retains_disabled_inserted_mods_like_dim() {
    let mut sync = fixture();
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    input.items[2].plugs.clear();
    input.items[2].sockets = vec![IndexedSocket {
        socket_index: 0,
        plug_hash: Some(ItemHash::new(10)),
        enabled: false,
        visible: false,
        reusable_plugs: None,
        plug_objectives: None,
    }];
    sync.repository_mut().replace_current(input).unwrap();
    let mut service = BuildService::new(SqliteStore::open_in_memory().unwrap(), DimBuildCodec);
    let capture = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Disabled",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(
        capture.build.definition.parameters.unwrap().mods,
        Some(vec![ItemHash::new(10)])
    );
}

fn table<T>(definitions: Vec<T>) -> WorldDefinitionTable<T> {
    WorldDefinitionTable {
        definitions,
        skipped_count: 0,
    }
}

#[test]
fn equipped_and_in_game_capture_crafted_date_and_resolve_changed_identity() {
    let mut sync = fixture();
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    let mut weapon = definition(1498876634, Some(BucketHash::new(1498876634)), None);
    weapon.is_instance_item = Some(true);
    weapon.armor.socket_entries = vec![DefinitionSocketEntry::default()];
    weapon.armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 3583996951,
        socket_indexes: vec![0],
    }];
    input.items[0].sockets = vec![IndexedSocket {
        socket_index: 0,
        plug_hash: Some(ItemHash::new(99)),
        enabled: false,
        visible: false,
        reusable_plugs: None,
        plug_objectives: Some(vec![(1, Some(5)), (2, Some(1)), (3, Some(1700000000))]),
    }];
    let mut definitions = vec![weapon];
    for hash in [3448274439, 3284755031, 999, 10] {
        if let Resolution::Unique(d) = sync
            .repository()
            .lookup_definition(ItemHash::new(hash))
            .unwrap()
        {
            definitions.push(d);
        }
    }
    for index in [1, 3] {
        input.items[index].sockets = (0..7)
            .map(|socket_index| IndexedSocket {
                socket_index,
                plug_hash: None,
                enabled: false,
                visible: false,
                reusable_plugs: None,
                plug_objectives: None,
            })
            .collect();
        input.items[index].sockets.push(IndexedSocket {
            socket_index: 7,
            plug_hash: Some(ItemHash::new(10)),
            enabled: false,
            visible: false,
            reusable_plugs: Some(vec![destiny_core::domain::ReusablePlug {
                plug_hash: ItemHash::new(10),
                can_insert: false,
                enabled: false,
            }]),
            plug_objectives: None,
        });
        let d = &mut definitions[index];
        d.armor.socket_entries = vec![
            DefinitionSocketEntry {
                socket_type_hash: 123,
                single_initial_item_hash: 10,
                plug_sources: Some(2),
                ..Default::default()
            };
            8
        ];
        d.armor.socket_categories = vec![DefinitionSocketCategory {
            socket_category_hash: 457473665,
            socket_indexes: vec![7],
        }];
    }
    definitions[3].bucket_hash = Some(ARTIFACT_BUCKET_HASH);
    definitions[2].armor.equipable_item_set_hash = Some(42);
    definitions[2].armor.socket_entries = vec![DefinitionSocketEntry::default(); 8];
    definitions[2].armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 590099826,
        socket_indexes: vec![7],
    }];
    let world = WorldDefinitionsAcquisition {
        version: DefinitionVersion::new("v1"),
        armor_catalogs: vec![
            armor_catalog::ArmorCatalogRecord::EquipableSet(
                armor_catalog::EquipableSetDefinition {
                    hash: 42,
                    name: "Fixture set".into(),
                    set_items: vec![],
                    set_perks: vec![armor_catalog::SetPerk {
                        name: Some("Active perk".into()),
                        icon: None,
                        required_set_count: 1,
                        sandbox_perk_hash: 100,
                    }],
                },
            ),
            destiny_core::armor_catalog::ArmorCatalogRecord::SocketType(
                destiny_core::armor_catalog::SocketTypeDefinition {
                    hash: 123,
                    socket_category_hash: 457473665,
                    always_randomize_sockets: false,
                    plug_whitelist: vec![],
                },
            ),
            destiny_core::armor_catalog::ArmorCatalogRecord::SocketCategory(
                destiny_core::armor_catalog::SocketCategoryDefinition {
                    hash: 457473665,
                    category_style: 7,
                    name: "Super".into(),
                },
            ),
        ],
        item_descriptions: vec![],
        milestones: table(vec![
            MilestoneDefinition::new(1, "Milestone".into(), "".into(), 1).unwrap(),
        ]),
        activities: table(vec![
            ActivityDefinition::new(1, "Activity".into(), "".into(), 1, 1).unwrap(),
        ]),
        modifiers: table(vec![
            ActivityModifierDefinition::new(1, "Modifier".into(), "".into()).unwrap(),
        ]),
        activity_types: table(vec![
            ActivityTypeDefinition::new(1, "Type".into(), "".into()).unwrap(),
        ]),
        stats: table(vec![StatDefinition::new(1, "Stat".into()).unwrap()]),
        loadout_names: table(vec![]),
        objectives: table(
            (1..=3)
                .map(|hash| {
                    ObjectiveDefinition::new(hash, "".into(), 0)
                        .unwrap()
                        .with_ui_style(Some(hash + 1))
                })
                .collect(),
        ),
    };
    sync.repository_mut()
        .publish_manifest(
            DefinitionReplacementInput {
                definitions: ManifestDefinitions {
                    version: DefinitionVersion::new("v1"),
                    definitions,
                },
                checked_at: Timestamp::new(100),
                source: SourceMarker::Manifest,
            },
            world,
        )
        .unwrap();
    let saved = sync.repository().saved_loadouts().unwrap();
    sync.repository_mut()
        .replace_current_profile(input.clone(), None, saved.clone())
        .unwrap();
    let builds = SqliteStore::open_in_memory().unwrap();
    let mut service = BuildService::new(builds, DimBuildCodec);
    let captured = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Crafted",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(
        captured.build.definition.equipped[0].crafted_date,
        Some(1700000000)
    );
    assert!(
        !captured
            .omissions
            .contains(&BuildOmission::CraftedDatesUnavailable)
    );
    for index in [1, 3] {
        assert_eq!(
            captured.build.definition.equipped[index].socket_overrides,
            vec![(7, ItemHash::new(10))]
        );
    }
    assert!(
        !captured
            .omissions
            .contains(&BuildOmission::ArtifactUnlocksUnavailable)
    );
    assert!(
        captured
            .build
            .definition
            .parameters
            .as_ref()
            .and_then(|p| p.artifact_unlocks.as_ref())
            .is_none()
    );
    let reloaded = service
        .show(
            &DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap(),
            &destiny_app::builds::BuildSelector::Id(captured.build.definition.id.clone()),
        )
        .unwrap();
    assert_eq!(reloaded.definition, captured.build.definition);
    let converted = service
        .from_loadout(
            &mut sync,
            &CharacterId::new("1"),
            &destiny_app::LoadoutSelector::Slot(3),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(
        converted.build.definition.equipped[0].crafted_date,
        Some(1700000000)
    );
    assert_eq!(
        converted.build.definition.equipped[1].socket_overrides,
        vec![(7, ItemHash::new(10))]
    );
    assert_eq!(
        converted.build.definition.parameters.as_ref().unwrap().mods,
        Some(vec![ItemHash::new(10)])
    );
    input.items[0].instance_id = ItemInstanceId::new("555");
    sync.repository_mut()
        .replace_current_profile(input, None, saved)
        .unwrap();
    let checked = service
        .check(
            &mut sync,
            &destiny_app::builds::BuildSelector::Id(captured.build.definition.id),
            None,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let bonuses = &checked.analysis.equipped_set_bonuses;
    assert_eq!(bonuses.evaluation, "evaluated");
    assert_eq!(bonuses.sets[0].hash, 42);
    assert_eq!(bonuses.sets[0].equipped_count, 1);
    assert_eq!(
        bonuses.sets[0].perks.as_ref().unwrap()[0].active,
        Some(true)
    );
    assert_eq!(
        bonuses.sets[0].perks.as_ref().unwrap()[0]
            .perk
            .name
            .as_deref(),
        Some("Active perk")
    );
    assert_eq!(
        checked.analysis.items[0]
            .inventory
            .as_ref()
            .unwrap()
            .instance_id
            .as_str(),
        "555"
    );
}

#[test]
fn build_check_uses_current_projection_and_unique_class_target() {
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let result = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Check",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let checked = service
        .check(
            &mut sync,
            &destiny_app::builds::BuildSelector::Id(result.build.definition.id),
            None,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(checked.analysis.target_character, CharacterId::new("1"));
    assert_eq!(
        checked.analysis.mod_fit.reason,
        Some("armor_mod_socket_categories_unavailable")
    );
    assert_eq!(checked.analysis.specified, 3);
    assert_eq!(checked.analysis.resolved, 3);
    assert_eq!(checked.analysis.missing, 0);
    assert_eq!(sync.into_parts().0.2, 0); // fresh reads do not acquire
}

#[test]
fn build_check_stale_refresh_and_character_ambiguity_do_not_guess() {
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let source = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Check",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let selector = destiny_app::builds::BuildSelector::Id(source.build.definition.id);
    let (mut gateway, repository) = sync.into_parts();
    gateway.0.items.retain(|i| i.instance_id.as_str() != "1");
    let mut second = gateway.0.characters[0].clone();
    second.id = CharacterId::new("2");
    gateway.0.characters.push(second);
    gateway.0.checked_at = Timestamp::new(1000);
    let mut sync = ProfileSyncService::new_with_definitions(gateway, repository);
    let error = service
        .check(
            &mut sync,
            &selector,
            None,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1000),
        )
        .unwrap_err();
    assert_eq!(
        error,
        destiny_app::builds::BuildError::App(AppError::new(ErrorCategory::AmbiguousMatch))
    );
    let result = service
        .check(
            &mut sync,
            &selector,
            Some(&CharacterId::new("1")),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(1000),
        )
        .unwrap();
    assert_eq!(result.analysis.missing, 1);
    assert_eq!(result.analysis.resolved, 2);
    assert_eq!(result.metadata.sync.checked_at, Timestamp::new(1000));
    assert_eq!(sync.into_parts().0.2, 2);
}

#[test]
fn equipped_snapshot_is_faithful_and_reports_unavailable_indices() {
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let result = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Equipped",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(result.build.definition.class_type, 1);
    assert_eq!(result.build.definition.equipped.len(), 3); // no unknown bucket
    assert!(
        result
            .build
            .definition
            .equipped
            .iter()
            .all(|i| i.socket_overrides.is_empty())
    );
    assert!(
        result
            .omissions
            .contains(&BuildOmission::InventorySocketIndicesUnavailable)
    );
    assert!(
        !result
            .omissions
            .contains(&BuildOmission::ArtifactUnlocksUnavailable)
    );
    assert_eq!(
        result.build.definition.parameters.unwrap().mods,
        Some(vec![ItemHash::new(10)])
    );
    assert_eq!(sync.into_parts().0.2, 0);
}
#[test]
fn saved_loadout_skips_missing_items_and_retains_subclass_socket_gaps() {
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let result = service
        .from_loadout(
            &mut sync,
            &CharacterId::new("1"),
            &destiny_app::LoadoutSelector::Slot(3),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(result.build.definition.equipped.len(), 3);
    assert_eq!(result.unresolved_items, vec![ItemInstanceId::new("404")]);
    assert!(
        result.build.definition.equipped[0]
            .socket_overrides
            .is_empty()
    );
    assert_eq!(
        result.build.definition.equipped[1].socket_overrides,
        vec![(7, ItemHash::new(10))]
    );
    let parameters = result.build.definition.parameters.unwrap();
    assert_eq!(parameters.in_game_identifiers.unwrap().name_hash, 7);
    // Missing socket-category projection is not replaced by plug-name guessing.
    assert_eq!(parameters.mods, Some(vec![]));
    assert!(
        result
            .omissions
            .contains(&BuildOmission::SocketCategoriesUnavailable)
    );
    assert_eq!(sync.into_parts().0.2, 0);
}
#[test]
fn stale_source_syncs_and_invalid_slot_does_not_save() {
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    assert!(
        service
            .from_loadout(
                &mut sync,
                &CharacterId::new("1"),
                &destiny_app::LoadoutSelector::Slot(13),
                FreshnessPolicy { ttl_seconds: 1 },
                Timestamp::new(200)
            )
            .is_err()
    );
    assert!(
        service
            .list(&DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap())
            .unwrap()
            .is_empty()
    );
    assert_eq!(sync.into_parts().0.2, 2);
}

#[test]
fn build_apply_plan_reads_only_and_marks_missing_vault_evidence() {
    use destiny_core::build_apply::{ApplyPhase, BuildAction};
    let mut sync = fixture();
    let service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let mut service = service;
    let source = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Apply",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let mut definitions = vec![
        definition(1498876634, Some(BucketHash::new(1498876634)), None),
        definition(3448274439, Some(SUBCLASS_BUCKET_HASH), None),
        definition(3284755031, Some(BucketHash::new(3448274439)), None),
        definition(999, Some(BucketHash::new(999)), None),
    ];
    for d in &mut definitions {
        d.class_type = Some(destiny_core::domain::ClassType::Any);
    }
    sync.repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new("v1"),
                definitions,
            },
            checked_at: Timestamp::new(100),
            source: SourceMarker::Manifest,
        })
        .unwrap();
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    input.items[0].location = ItemLocation::Vault;
    input.items[0].equipped = false;
    sync.repository_mut().replace_current(input).unwrap();
    let policy = destiny_adapters::pinned_equipment_capacity_policy();
    let preview = service
        .plan_apply(
            &mut sync,
            &destiny_app::builds::BuildCheckRequest {
                selector: destiny_app::builds::BuildSelector::Id(source.build.definition.id),
                character: None,
            },
            &policy,
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let steps: Vec<_> = preview
        .plan
        .steps
        .iter()
        .map(|s| (s.phase, s.action.clone()))
        .collect();
    assert_eq!(
        steps,
        [
            (
                ApplyPhase::Move,
                BuildAction::Move {
                    item: ItemInstanceId::new("1"),
                    from: ItemLocation::Vault,
                    to: ItemLocation::Character(CharacterId::new("1")),
                }
            ),
            (
                ApplyPhase::Move,
                BuildAction::Equip {
                    items: vec![ItemInstanceId::new("1")],
                    character: CharacterId::new("1"),
                }
            ),
        ]
    );
    // No Vault bucket projection or saved-loadout ranking evidence is invented.
    assert!(preview.plan.unknown.contains(&"vault_capacity"));
    assert_eq!(preview.names[0].1, "Item 1498876634");
    assert_eq!(
        sync.into_parts().0.2,
        0,
        "fresh state needs no provider call"
    );
}
