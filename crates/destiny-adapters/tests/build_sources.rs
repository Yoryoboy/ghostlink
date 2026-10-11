use destiny_adapters::{dim_loadout::DimBuildCodec, sqlite::SqliteStore};
use destiny_app::{
    ProfileSyncService,
    builds::{BuildOmission, BuildService},
};
use destiny_core::domain::ItemSlot;
use destiny_core::*;

fn definition(hash: i64, bucket: Option<BucketHash>, category: Option<&str>) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: bucket,
        name: format!("Item {hash}"),
        item_type: None,
        tier: None,
        element: None,
        slot: bucket.and_then(ItemSlot::from_equipment_bucket),
        class_type: None,
        plug_category: category.map(str::to_owned),
        tags: vec![],
        version: DefinitionVersion::new("v1"),
        armor: Default::default(),
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
        result
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
    assert_eq!(parameters.mods, Some(vec![ItemHash::new(10)]));
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
