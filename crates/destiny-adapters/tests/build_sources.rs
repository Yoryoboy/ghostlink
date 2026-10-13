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

#[test]
fn build_apply_plans_vault_staged_moves_only_with_projected_vault_evidence() {
    use destiny_core::armor_catalog::*;
    use destiny_core::build_apply::{ApplyPhase, ApplySkipReason, BuildAction};
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
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
    let definitions = ManifestDefinitions {
        version: DefinitionVersion::new("v1"),
        definitions,
    };
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    // The kinetic item sits on another character: Vault staging is required.
    input.characters.push(Character {
        id: CharacterId::new("2"),
        class: Some("Titan".into()),
        name: None,
        race: None,
        gender: None,
        light: None,
        date_last_played: None,
    });
    input.items[0].location = ItemLocation::Character(CharacterId::new("2"));
    input.items[0].equipped = false;
    sync.repository_mut().replace_current(input).unwrap();
    let request = destiny_app::builds::BuildCheckRequest {
        selector: destiny_app::builds::BuildSelector::Id(source.build.definition.id),
        character: None,
    };
    let policy = destiny_adapters::pinned_equipment_capacity_policy();
    let plan = |sync: &mut ProfileSyncService<_, SqliteStore>| {
        service
            .plan_apply(
                sync,
                &request,
                &policy,
                FreshnessPolicy { ttl_seconds: 300 },
                Timestamp::new(100),
            )
            .unwrap()
    };
    let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
    let publish = |sync: &mut ProfileSyncService<_, SqliteStore>, world| {
        sync.repository_mut()
            .publish_manifest(
                DefinitionReplacementInput {
                    definitions: definitions.clone(),
                    checked_at: Timestamp::new(100),
                    source: SourceMarker::Manifest,
                },
                world,
            )
            .unwrap();
    };
    // Without projected evidence the typed skip remains.
    publish(&mut sync, world.clone());
    let preview = plan(&mut sync);
    assert_eq!(
        preview.plan.skipped[0].reason,
        ApplySkipReason::VaultCapacityUnavailable
    );
    assert!(preview.plan.unknown.contains(&"vault_capacity"));
    // DIM mapping: accepted kinetic bucket -> general Vault bucket with its itemCount.
    world.armor_catalogs = vec![
        ArmorCatalogRecord::InventoryBucket(InventoryBucketDefinition {
            hash: 138197802,
            item_count: 1300,
        }),
        ArmorCatalogRecord::VaultVendor(VaultVendorDefinition {
            hash: VAULT_VENDOR_HASH,
            accepted_items: vec![VaultAcceptedItem {
                accepted_bucket: 1498876634,
                destination_bucket: 138197802,
            }],
        }),
    ];
    publish(&mut sync, world);
    let preview = plan(&mut sync);
    // Only the unrelated subclass bucket remains unsupported.
    assert!(
        preview
            .plan
            .skipped
            .iter()
            .all(|s| s.reason == ApplySkipReason::UnsupportedBucket),
        "{:?}",
        preview.plan.skipped
    );
    assert!(!preview.plan.unknown.contains(&"vault_capacity"));
    let moves: Vec<_> = preview
        .plan
        .steps
        .iter()
        .filter(|s| s.phase == ApplyPhase::Move)
        .map(|s| s.action.clone())
        .collect();
    assert_eq!(
        moves[..2],
        [
            BuildAction::Move {
                item: ItemInstanceId::new("1"),
                from: ItemLocation::Character(CharacterId::new("2")),
                to: ItemLocation::Vault,
            },
            BuildAction::Move {
                item: ItemInstanceId::new("1"),
                from: ItemLocation::Vault,
                to: ItemLocation::Character(CharacterId::new("1")),
            },
        ]
    );
}

#[test]
fn build_apply_plans_subclass_socket_overrides_from_projected_catalogs() {
    use destiny_core::armor_catalog::*;
    use destiny_core::build_apply::{ApplyPhase, BuildAction};
    let mut sync = fixture();
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    service
        .execute_local(
            &membership,
            &destiny_app::builds::LocalBuildRequest::Import(
                r#"{"id":"s1","name":"Sockets","classType":1,"equipped":[{"id":"2","hash":3448274439,"socketOverrides":{"0":300}}],"unequipped":[]}"#
                    .into(),
            ),
            Timestamp::new(100),
        )
        .unwrap();
    let mut subclass = definition(3448274439, Some(SUBCLASS_BUCKET_HASH), None);
    subclass.class_type = Some(destiny_core::domain::ClassType::Any);
    subclass.is_instance_item = Some(true);
    subclass.armor.socket_entries = vec![DefinitionSocketEntry {
        socket_type_hash: 20,
        single_initial_item_hash: 301,
        reusable_plug_set_hash: Some(800),
        plug_sources: Some(2),
        ..Default::default()
    }];
    subclass.armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 1,
        socket_indexes: vec![0],
    }];
    let definitions = ManifestDefinitions {
        version: DefinitionVersion::new("v1"),
        definitions: vec![
            subclass,
            definition(300, None, None),
            definition(301, None, None),
        ],
    };
    let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = vec![
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash: 20,
            socket_category_hash: 1,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        }),
        ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
            hash: 1,
            category_style: 0,
            name: "Generic".into(),
        }),
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash: 800,
            reusable_plug_items: [300, 301]
                .map(|plug_item_hash| PlugSetItem {
                    plug_item_hash,
                    currently_can_roll: true,
                })
                .to_vec(),
        }),
    ];
    sync.repository_mut()
        .publish_manifest(
            DefinitionReplacementInput {
                definitions,
                checked_at: Timestamp::new(100),
                source: SourceMarker::Manifest,
            },
            world,
        )
        .unwrap();
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    input.items[1].sockets = vec![IndexedSocket {
        socket_index: 0,
        plug_hash: Some(ItemHash::new(301)),
        enabled: true,
        visible: true,
        reusable_plugs: Some(vec![]),
        plug_objectives: None,
    }];
    sync.repository_mut().replace_current(input).unwrap();
    let preview = service
        .plan_apply(
            &mut sync,
            &destiny_app::builds::BuildCheckRequest {
                selector: destiny_app::builds::BuildSelector::Name("Sockets".into()),
                character: None,
            },
            &destiny_adapters::pinned_equipment_capacity_policy(),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let steps: Vec<_> = preview
        .plan
        .steps
        .iter()
        .map(|s| (s.phase, s.action.clone(), s.needs.clone()))
        .collect();
    assert_eq!(
        steps,
        [(
            ApplyPhase::Sockets,
            BuildAction::InsertPlug {
                item: ItemInstanceId::new("2"),
                character: CharacterId::new("1"),
                socket_index: 0,
                plug: ItemHash::new(300),
            },
            vec![]
        )]
    );
    assert!(preview.plan.socket_skipped.is_empty() && preview.plan.not_applied.is_empty());
    assert_eq!(
        preview.plug_names,
        [(ItemHash::new(300), "Item 300".to_owned())]
    );
}

#[test]
fn build_apply_plans_armor_mods_from_projected_catalogs_and_unlocks() {
    use destiny_core::armor_catalog::*;
    use destiny_core::build_apply::{ApplyPhase, BuildAction};
    use destiny_core::domain::{ClassType, PlugSetEntry, PlugSetScope, ReusablePlug};
    const BUCKETS: [i64; 5] = [3448274439, 3551918588, 14239492, 20886954, 1585787867];
    const EMPTY: i64 = 235531041;
    let mut sync = fixture();
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    service
        .execute_local(
            &membership,
            &destiny_app::builds::LocalBuildRequest::Import(
                r#"{"id":"m1","name":"Mods","classType":1,"equipped":[],"unequipped":[],"parameters":{"mods":[2001]}}"#
                    .into(),
            ),
            Timestamp::new(100),
        )
        .unwrap();
    let mut definitions = vec![
        definition(EMPTY, None, Some("enhancements.v2_head")),
        definition(2001, None, Some("enhancements.v2_head")),
    ];
    for def in &mut definitions {
        def.armor.energy_cost = Some(PlugEnergyCost {
            energy_cost: 3,
            energy_type: 1,
            energy_type_hash: 1,
        });
    }
    let mut items = vec![];
    for (index, bucket) in BUCKETS.into_iter().enumerate() {
        let mut armor = definition(20 + index as i64, Some(BucketHash::new(bucket)), None);
        armor.tier = Some("Legendary".into());
        armor.class_type = Some(ClassType::Hunter);
        armor.is_instance_item = Some(true);
        armor.armor.socket_entries = vec![DefinitionSocketEntry {
            socket_type_hash: 99,
            reusable_plug_set_hash: Some(800),
            plug_sources: Some(2),
            ..Default::default()
        }];
        armor.armor.socket_categories = vec![DefinitionSocketCategory {
            socket_category_hash: 590099826,
            socket_indexes: vec![0],
        }];
        items.push((index, armor.hash));
        definitions.push(armor);
    }
    let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = vec![
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash: 99,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        }),
        ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
            hash: 100,
            category_style: 0,
            name: "Mods".into(),
        }),
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash: 800,
            reusable_plug_items: [EMPTY as u32, 2001]
                .map(|plug_item_hash| PlugSetItem {
                    plug_item_hash,
                    currently_can_roll: true,
                })
                .to_vec(),
        }),
    ];
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
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    input.items = items
        .into_iter()
        .map(|(index, hash)| ItemInstance {
            instance_id: ItemInstanceId::new((index + 1).to_string()),
            item_hash: hash,
            location: ItemLocation::Character(CharacterId::new("1")),
            quantity: 1,
            equipped: true,
            locked: false,
            power: None,
            stats: vec![],
            perks: vec![],
            plugs: vec![],
            energy: Some(ItemEnergy {
                energy_capacity: 10,
                ..Default::default()
            }),
            gear_tier: None,
            sockets: vec![IndexedSocket {
                socket_index: 0,
                plug_hash: Some(ItemHash::new(EMPTY)),
                enabled: true,
                visible: true,
                reusable_plugs: Some(vec![ReusablePlug {
                    plug_hash: ItemHash::new(EMPTY),
                    enabled: true,
                    can_insert: true,
                }]),
                plug_objectives: None,
            }],
            transfer_status: None,
            available_plugs: vec![],
        })
        .collect();
    input.plug_set_entries = vec![PlugSetEntry {
        scope: PlugSetScope::Profile,
        plug_hash: ItemHash::new(2001),
        can_insert: true,
        enabled: true,
    }];
    sync.repository_mut().replace_current(input).unwrap();
    let preview = service
        .plan_apply(
            &mut sync,
            &destiny_app::builds::BuildCheckRequest {
                selector: destiny_app::builds::BuildSelector::Name("Mods".into()),
                character: None,
            },
            &destiny_adapters::pinned_equipment_capacity_policy(),
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
        [(
            ApplyPhase::Mods,
            BuildAction::InsertPlug {
                item: ItemInstanceId::new("1"),
                character: CharacterId::new("1"),
                socket_index: 0,
                plug: ItemHash::new(2001),
            }
        )]
    );
    assert!(preview.plan.mod_skipped.is_empty() && preview.plan.not_applied.is_empty());
}

#[test]
fn build_apply_plans_fashion_from_projected_classification_and_exotic_unlocks() {
    use destiny_core::armor_catalog::*;
    use destiny_core::build_apply::{ApplyPhase, BuildAction};
    use destiny_core::domain::{ClassType, PlugSetEntry, PlugSetScope, ReusablePlug};
    const HELMET: i64 = 3448274439;
    const DEFAULT_SHADER: i64 = 4248210736;
    const DEFAULT_ORNAMENT: i64 = 702981643;
    const SHADER: i64 = 3002;
    const EXOTIC_ORNAMENT: i64 = 3101;
    let mut sync = fixture();
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    service
        .execute_local(
            &membership,
            &destiny_app::builds::LocalBuildRequest::Import(
                format!(
                    r#"{{"id":"f1","name":"Fashion","classType":1,"equipped":[],"unequipped":[],"parameters":{{"modsByBucket":{{"{HELMET}":[{SHADER},{EXOTIC_ORNAMENT}]}}}}}}"#
                )
            ),
            Timestamp::new(100),
        )
        .unwrap();
    let plug = |hash: i64, sub_type: u8, categories: &[u32]| {
        let mut d = definition(hash, None, None);
        d.armor.plug_category_hash = Some(7);
        d.armor.item_sub_type_code = Some(sub_type);
        d.armor.item_category_hashes = categories.to_vec();
        d
    };
    let mut helmet = definition(20, Some(BucketHash::new(HELMET)), None);
    helmet.tier = Some("Exotic".into());
    helmet.class_type = Some(ClassType::Hunter);
    helmet.is_instance_item = Some(true);
    helmet.armor.socket_entries = ["810", "811"]
        .iter()
        .enumerate()
        .map(|(index, set)| DefinitionSocketEntry {
            socket_type_hash: 98 - index as u32,
            reusable_plug_set_hash: Some(set.parse().unwrap()),
            ..Default::default()
        })
        .collect();
    helmet.armor.socket_categories = vec![DefinitionSocketCategory {
        socket_category_hash: 1926152773,
        socket_indexes: vec![0, 1],
    }];
    let definitions = vec![
        helmet,
        plug(DEFAULT_SHADER, 20, &[41]),
        plug(SHADER, 20, &[41]),
        plug(DEFAULT_ORNAMENT, 21, &[41]),
        plug(EXOTIC_ORNAMENT, 21, &[]),
    ];
    let set = |hash, members: &[i64]| {
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash,
            reusable_plug_items: members
                .iter()
                .map(|m| PlugSetItem {
                    plug_item_hash: *m as u32,
                    currently_can_roll: true,
                })
                .collect(),
        })
    };
    let kind = |hash| {
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        })
    };
    let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = vec![
        kind(98),
        kind(97),
        set(810, &[DEFAULT_SHADER, SHADER]),
        set(811, &[DEFAULT_ORNAMENT, EXOTIC_ORNAMENT]),
    ];
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
    let socket = |index, plugged: i64, others: &[i64]| IndexedSocket {
        socket_index: index,
        plug_hash: Some(ItemHash::new(plugged)),
        enabled: true,
        visible: true,
        reusable_plugs: Some(
            others
                .iter()
                .chain(&[plugged])
                .map(|hash| ReusablePlug {
                    plug_hash: ItemHash::new(*hash),
                    enabled: true,
                    can_insert: true,
                })
                .collect(),
        ),
        plug_objectives: None,
    };
    let mut input =
        InventorySnapshotInput::with_metadata(&sync.repository().load_current().unwrap().unwrap());
    input.items = vec![ItemInstance {
        instance_id: ItemInstanceId::new("1"),
        item_hash: ItemHash::new(20),
        location: ItemLocation::Character(CharacterId::new("1")),
        quantity: 1,
        equipped: true,
        locked: false,
        power: None,
        stats: vec![],
        perks: vec![],
        plugs: vec![],
        energy: None,
        gear_tier: None,
        // The exotic's cosmetic socket reports the ornament as insertable, which is the
        // only ownership signal for exotic ornaments.
        sockets: vec![
            socket(0, DEFAULT_SHADER, &[]),
            socket(1, DEFAULT_ORNAMENT, &[EXOTIC_ORNAMENT]),
        ],
        transfer_status: None,
        available_plugs: vec![],
    }];
    input.plug_set_entries = vec![PlugSetEntry {
        scope: PlugSetScope::Profile,
        plug_hash: ItemHash::new(SHADER),
        can_insert: true,
        enabled: true,
    }];
    sync.repository_mut().replace_current(input).unwrap();
    let preview = service
        .plan_apply(
            &mut sync,
            &destiny_app::builds::BuildCheckRequest {
                selector: destiny_app::builds::BuildSelector::Name("Fashion".into()),
                character: None,
            },
            &destiny_adapters::pinned_equipment_capacity_policy(),
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
    let insert = |socket_index, plug| {
        (
            ApplyPhase::Mods,
            BuildAction::InsertPlug {
                item: ItemInstanceId::new("1"),
                character: CharacterId::new("1"),
                socket_index,
                plug: ItemHash::new(plug),
            },
        )
    };
    assert_eq!(steps, [insert(0, SHADER), insert(1, EXOTIC_ORNAMENT)]);
    assert!(preview.plan.mod_skipped.is_empty() && preview.plan.not_applied.is_empty());
}

#[test]
fn build_substitutes_validate_the_limit_and_never_change_check_or_the_apply_plan() {
    use destiny_app::builds::{BuildCheckRequest, BuildSelector};
    let mut sync = fixture();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let source = service
        .from_equipped(
            &mut sync,
            &CharacterId::new("1"),
            "Substitutes",
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap();
    let request = BuildCheckRequest {
        selector: BuildSelector::Id(source.build.definition.id),
        character: None,
    };
    let policy = FreshnessPolicy { ttl_seconds: 300 };
    let capacity = destiny_adapters::pinned_equipment_capacity_policy();
    let check = |service: &BuildService<_, _>, sync: &mut _| {
        service
            .check(sync, &request.selector, None, policy, Timestamp::new(100))
            .unwrap()
    };
    let before = (
        check(&service, &mut sync),
        service
            .plan_apply(&mut sync, &request, &capacity, policy, Timestamp::new(100))
            .unwrap(),
    );
    // Limits outside 1..=200 are invalid requests, whatever the build is.
    for limit in [0, 201] {
        let error = service
            .substitutes(
                &mut sync,
                &request,
                limit,
                destiny_core::optimizer::AnalyzerBudget::substitutes(),
                policy,
                Timestamp::new(100),
            )
            .unwrap_err();
        assert_eq!(
            error,
            destiny_app::builds::BuildError::App(AppError::new(ErrorCategory::ProviderValidation))
        );
    }
    // The fixture has no full armor set: the answer is a typed unknown, never a guess.
    let result = service
        .substitutes(
            &mut sync,
            &request,
            200,
            destiny_core::optimizer::AnalyzerBudget::substitutes(),
            policy,
            Timestamp::new(100),
        )
        .unwrap();
    assert_eq!(result.report.evaluation, "not_evaluated");
    assert!(result.report.reason.is_some());
    assert_eq!(result.report.limit, 200);
    assert!(result.report.sets.is_empty());
    assert_eq!(result.target_character, CharacterId::new("1"));
    // D3: a substitutes run leaves the check and the apply plan exactly as they were.
    let after = (
        check(&service, &mut sync),
        service
            .plan_apply(&mut sync, &request, &capacity, policy, Timestamp::new(100))
            .unwrap(),
    );
    assert_eq!(before, after);
}

/// BE-010b2: Bungie's component 310 lists only sockets with reusable plugs, so
/// armor mod sockets are absent from a fetched 310. DIM reads an absent socket as
/// no raw plugs (sockets.ts:77-79), so the check must evaluate mod fit.
fn check_mod_fit_from_decoded_profile(
    stat_plug: Option<u32>,
    list_mod_sockets_in_310: bool,
) -> destiny_core::mod_assignment::ModFitAnalysis {
    use destiny_adapters::decode_profile;
    use destiny_core::armor_catalog::*;
    use destiny_core::domain::ClassType;
    const BUCKETS: [i64; 5] = [3448274439, 3551918588, 14239492, 20886954, 1585787867];
    const EMPTY: i64 = 235531041;
    let mut sync = fixture();
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("99")).unwrap();
    let mut service = BuildService::new(
        destiny_app::test_support::InMemoryBuildRepository::default(),
        DimBuildCodec,
    );
    let equipped: Vec<_> = (0..5)
        .map(|i| format!(r#"{{"id":"80000000000000000{}","hash":{}}}"#, i + 1, 20 + i))
        .collect();
    service
        .execute_local(
            &membership,
            &destiny_app::builds::LocalBuildRequest::Import(format!(
                r#"{{"id":"m1","name":"Mods","classType":1,"equipped":[{}],"unequipped":[],"parameters":{{"mods":[2001,3001]}}}}"#,
                equipped.join(",")
            )),
            Timestamp::new(100),
        )
        .unwrap();
    let mut definitions = vec![
        definition(EMPTY, None, Some("enhancements.v2_head")),
        definition(2001, None, Some("enhancements.v2_head")),
        definition(3001, None, Some("enhancements.artifice")),
    ];
    definitions[2].armor.plug_category_hash = Some(3773173029);
    for def in &mut definitions {
        def.armor.energy_cost = Some(PlugEnergyCost {
            energy_cost: 3,
            energy_type: 1,
            energy_type_hash: 1,
        });
    }
    for (index, bucket) in BUCKETS.into_iter().enumerate() {
        let mut armor = definition(20 + index as i64, Some(BucketHash::new(bucket)), None);
        armor.tier = Some("Legendary".into());
        armor.class_type = Some(ClassType::Hunter);
        armor.is_instance_item = Some(true);
        armor.armor.socket_entries = vec![DefinitionSocketEntry {
            socket_type_hash: 99,
            reusable_plug_set_hash: Some(800),
            plug_sources: Some(2),
            ..Default::default()
        }];
        armor.armor.socket_categories = vec![DefinitionSocketCategory {
            socket_category_hash: 590099826,
            socket_indexes: vec![0],
        }];
        definitions.push(armor);
    }
    let mut world = destiny_app::test_support::synthetic_world(DefinitionVersion::new("v1"));
    world.armor_catalogs = vec![
        ArmorCatalogRecord::SocketType(SocketTypeDefinition {
            hash: 99,
            socket_category_hash: 100,
            always_randomize_sockets: false,
            plug_whitelist: vec![],
        }),
        ArmorCatalogRecord::SocketCategory(SocketCategoryDefinition {
            hash: 100,
            category_style: 0,
            name: "Mods".into(),
        }),
        ArmorCatalogRecord::PlugSet(PlugSetDefinition {
            hash: 800,
            reusable_plug_items: [EMPTY as u32, 2001, 3001]
                .map(|plug_item_hash| PlugSetItem {
                    plug_item_hash,
                    currently_can_roll: true,
                })
                .to_vec(),
        }),
    ];
    // The Armor 3.0 stat-roll plug is nameless: it lives only in the stat-plug store.
    world.armor_catalogs.extend(stat_plug.map(|hash| {
        ArmorCatalogRecord::StatPlug(StatPlugDefinition {
            hash,
            plug_category_hash: 748854354,
            investment_stats: vec![InvestmentStat {
                stat_type_hash: 2996146975,
                value: 10,
                is_conditionally_active: false,
            }],
        })
    }));
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
    // A real Bungie response: component 310 is present but, as live, lists no
    // armor mod socket (only an unrelated socket of the first piece).
    let mut profile: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/profile-complete.json")).unwrap();
    let components = &mut profile["Response"]["itemComponents"];
    let mut gear = Vec::new();
    for i in 0..5 {
        let id = format!("80000000000000000{}", i + 1);
        gear.push(serde_json::json!({"itemHash": 20 + i, "itemInstanceId": id, "quantity": 1, "location": 1, "state": 0}));
        components["instances"]["data"][&id] = serde_json::json!({"isEquipped": true, "energy": {"energyCapacity": 10, "energyUsed": 0, "energyUnused": 10, "energyType": 1}});
        let mut sockets =
            vec![serde_json::json!({"plugHash": EMPTY, "isEnabled": true, "isVisible": true})];
        if let (0, Some(hash)) = (i, stat_plug) {
            sockets.extend(
                (1..7).map(|_| serde_json::json!({"isEnabled": false, "isVisible": false})),
            );
            sockets
                .push(serde_json::json!({"plugHash": hash, "isEnabled": true, "isVisible": false}));
        }
        components["sockets"]["data"][&id] = serde_json::json!({ "sockets": sockets });
    }
    components["reusablePlugs"]["data"] = serde_json::json!({"800000000000000001": {"plugs": {"5": [{"plugItemHash": 1, "canInsert": true, "enabled": true}]}}});
    if list_mod_sockets_in_310 {
        for i in 1..=5 {
            components["reusablePlugs"]["data"][format!("80000000000000000{i}")] = serde_json::json!({"plugs": {"0": [{"plugItemHash": EMPTY, "canInsert": true, "enabled": true}]}});
        }
    }
    profile["Response"]["characterEquipment"]["data"]["900000000000000002"]["items"] =
        serde_json::Value::Array(gear);
    let mut input = decode_profile(
        &serde_json::to_vec(&profile).unwrap(),
        &DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("4611686018484639825"),
        )
        .unwrap(),
        DefinitionVersion::new("v1"),
        Timestamp::new(100),
    )
    .unwrap();
    input.membership_id = MembershipId::new("99");
    sync.repository_mut().replace_current(input).unwrap();
    service
        .check(
            &mut sync,
            &destiny_app::builds::BuildSelector::Name("Mods".into()),
            Some(&CharacterId::new("900000000000000002")),
            FreshnessPolicy { ttl_seconds: 300 },
            Timestamp::new(100),
        )
        .unwrap()
        .analysis
        .mod_fit
}

/// BE-010b2: Bungie's component 310 lists only sockets with reusable plugs, so
/// armor mod sockets are absent from a fetched 310. DIM reads an absent socket as
/// no raw plugs (sockets.ts:77-79), so the check must evaluate mod fit.
#[test]
fn build_check_evaluates_mod_fit_when_mod_sockets_are_absent_from_fetched_310() {
    let fit = check_mod_fit_from_decoded_profile(None, false);
    assert_eq!(fit.reason, None);
    assert_eq!(fit.evaluation, "evaluated");
    assert_eq!(fit.invalid_mods, Some(vec![]));
}

/// BE-010b1 open item: nameless `armor_stats` plugs live in invisible sockets and
/// are not mods; DIM only assigns mods to ArmorMods sockets (mod-assignment-utils.ts).
#[test]
fn build_check_ignores_known_nameless_stat_plugs_in_mod_checks() {
    let fit = check_mod_fit_from_decoded_profile(Some(481842213), true);
    assert_eq!(fit.reason, None);
    assert_eq!(fit.evaluation, "evaluated");
}
