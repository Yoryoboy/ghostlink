use super::*;
use crate::test_support::{
    DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
    QueuedManifestGateway, QueuedProfileGateway,
};
use destiny_core::*;

const BUCKET: i64 = 1498876634;
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn definition() -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(7),
        bucket_hash: Some(BucketHash::new(BUCKET)),
        does_postmaster_pull_have_side_effects: Some(false),
        is_instance_item: Some(true),
        max_stack_size: None,
        stack_unique_label: None,
        name: "gear".into(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![],
        version: DefinitionVersion::new("v1"),
    }
}
fn item(id: &str, location: ItemLocation) -> ItemInstance {
    ItemInstance {
        instance_id: ItemInstanceId::new(id),
        item_hash: ItemHash::new(7),
        location,
        quantity: 1,
        equipped: false,
        locked: false,
        power: None,
        stats: vec![],
        plugs: vec![],
        available_plugs: vec![],
        perks: vec![],
    }
}
fn input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: membership().membership_id().clone(),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("v1"),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new("2"),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: vec![item("1", ItemLocation::Postmaster(CharacterId::new("2")))],
        stacks: vec![],
        plug_set_entries: vec![],
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
    }
}
fn policy() -> EquipmentCapacityPolicy {
    EquipmentCapacityPolicy::new(
        DefinitionVersion::new("v1"),
        "artifact",
        [
            1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954,
            1585787867,
        ]
        .map(|b| (BucketHash::new(b), 10))
        .to_vec(),
    )
    .unwrap()
}
type Service = InstancedPullService<
    QueuedProfileGateway,
    InMemoryInventoryRepository,
    InMemoryDefinitionRepository,
    QueuedManifestGateway,
    FakeClock,
    DeterministicPlanIds,
>;
fn service(
    input: InventorySnapshotInput,
    definitions: Vec<ItemDefinition>,
    version: &str,
    observed: bool,
    evidence: Result<EquipmentCapacityEvidence, AppError>,
) -> Service {
    let observations = observed.then(|| {
        input
            .items
            .iter()
            .map(|i| {
                InstancedBucketObservation::new(
                    i.instance_id.clone(),
                    BucketHash::new(if matches!(i.location, ItemLocation::Postmaster(_)) {
                        215593132
                    } else {
                        BUCKET
                    }),
                    i.location.clone(),
                )
                .unwrap()
            })
            .collect()
    });
    let mut gateway = QueuedProfileGateway::new();
    gateway.queue_membership(Ok(membership()));
    gateway.queue_acquisition(Ok(
        ProfileAcquisition::new(input.clone(), observations).unwrap()
    ));
    let mut repository = InMemoryInventoryRepository::new();
    repository.replace_current(input).unwrap(); // Already fresh: planning must still reacquire.
    let mut manifest = QueuedManifestGateway::new();
    manifest.queue_capacity_evidence(evidence);
    InstancedPullService::new(
        crate::ProfileSyncService::new(gateway, repository),
        InMemoryDefinitionRepository::new(DefinitionVersion::new(version), definitions),
        manifest,
        policy(),
        FakeClock::fixed(Timestamp::new(100)),
        DeterministicPlanIds::starting_at(1),
    )
}
fn evidence() -> Result<EquipmentCapacityEvidence, AppError> {
    Ok(EquipmentCapacityEvidence::new(DefinitionVersion::new("v1"), "artifact").unwrap())
}
fn request() -> InstancedPullPlanRequest {
    InstancedPullPlanRequest {
        membership: membership(),
        item: ItemInstanceId::new("1"),
    }
}
fn plan(
    service: &mut Service,
    request: &InstancedPullPlanRequest,
) -> Result<InstancedPullPlan, InstancedPullPlanningError> {
    let result = service.plan(
        request,
        PlanLifetime::MAX,
        FreshnessPolicy { ttl_seconds: 60 },
    );
    assert_eq!(service.sync.gateway_mut().observed_acquisition_calls(), 1);
    assert_eq!(
        service.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Preflight]
    );
    assert_eq!(service.sync.gateway_mut().membership_calls(), 1);
    assert_eq!(service.sync.gateway_mut().inventory_calls(), 1);
    assert_eq!(service.manifest.capacity_evidence_calls(), 1);
    assert_eq!(service.plan_ids.calls(), u64::from(result.is_ok()));
    result
}
#[test]
fn valid_control_is_fresh_opaque_and_binds_occupancy() {
    let mut input = input();
    input
        .items
        .push(item("9", ItemLocation::Character(CharacterId::new("2"))));
    input
        .items
        .push(item("3", ItemLocation::Character(CharacterId::new("2"))));
    input.items[1].equipped = true;
    let mut service = service(input, vec![definition()], "v1", true, evidence());
    let plan = plan(&mut service, &request()).unwrap();
    assert_eq!(plan.item(), &ItemInstanceId::new("1"));
    assert_eq!(plan.owner(), &CharacterId::new("2"));
    assert_eq!(plan.destination_bucket(), BucketHash::new(BUCKET));
    assert_eq!(plan.id().as_str(), "plan-1");
    assert_eq!(plan.created_at(), Timestamp::new(100));
    assert!(plan.expires_at() > plan.created_at());
    assert_eq!(plan.fingerprint.occupied, 2);
    assert_eq!(
        plan.fingerprint.occupants,
        vec![ItemInstanceId::new("3"), ItemInstanceId::new("9")]
    );
}
#[test]
fn owner_is_derived_and_other_containers_do_not_occupy_its_bucket() {
    let mut input = input();
    let mut other = input.characters[0].clone();
    other.id = CharacterId::new("4");
    input.characters.push(other);
    input.items[0].location = ItemLocation::Postmaster(CharacterId::new("4"));
    for id in 10..20 {
        input.items.push(item(
            &id.to_string(),
            ItemLocation::Character(CharacterId::new("2")),
        ));
    }
    input.items.push(item("30", ItemLocation::Vault));
    let mut s = service(input, vec![definition()], "v1", true, evidence());
    let plan = plan(&mut s, &request()).unwrap();
    assert_eq!(plan.owner(), &CharacterId::new("4"));
    assert_eq!(plan.fingerprint.occupied, 0);
    assert!(plan.fingerprint.occupants.is_empty());
}

#[test]
fn definition_row_version_and_qualified_policy_version_bind_publication() {
    let changed = ItemDefinition {
        version: DefinitionVersion::new("v2"),
        ..definition()
    };
    let mut s = service(input(), vec![changed], "v1", true, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::DefinitionVersionMismatch
        ))
    );
    let mut input = input();
    input.definition_version = DefinitionVersion::new("v2");
    let changed = ItemDefinition {
        version: DefinitionVersion::new("v2"),
        ..definition()
    };
    let mut s = service(input, vec![changed], "v2", true, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::Capacity(CapacityRefusal::DefinitionVersionMismatch)
        ))
    );
}

#[test]
fn target_and_membership_refusals_are_isolated() {
    for location in [
        ItemLocation::Character(CharacterId::new("2")),
        ItemLocation::Vault,
    ] {
        let mut input = input();
        input.items[0].location = location;
        let mut s = service(input, vec![definition()], "v1", true, evidence());
        assert_eq!(
            plan(&mut s, &request()),
            Err(InstancedPullPlanningError::Refused(
                InstancedPullRefusal::NotInPostmaster
            ))
        );
    }
    let mut missing = input();
    missing.items.clear();
    let mut s = service(missing, vec![definition()], "v1", true, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::TargetMissing
        ))
    );
    for member in [
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("456")).unwrap(),
        DestinyMembership::new(MembershipType::new(2), MembershipId::new("123")).unwrap(),
    ] {
        let mut req = request();
        req.membership = member;
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        assert_eq!(
            plan(&mut s, &req),
            Err(InstancedPullPlanningError::Refused(
                InstancedPullRefusal::MembershipMismatch
            ))
        );
    }
}
#[test]
fn definition_gates_are_isolated() {
    let cases: Vec<(Vec<ItemDefinition>, &str, InstancedPullRefusal)> = vec![
        (vec![], "v1", InstancedPullRefusal::DefinitionMissing),
        (
            vec![definition(), definition()],
            "v1",
            InstancedPullRefusal::DefinitionAmbiguous,
        ),
        (
            vec![definition()],
            "v2",
            InstancedPullRefusal::DefinitionVersionMismatch,
        ),
        (
            vec![ItemDefinition {
                does_postmaster_pull_have_side_effects: Some(true),
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::SideEffects,
        ),
        (
            vec![ItemDefinition {
                does_postmaster_pull_have_side_effects: None,
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::SideEffectsUnknown,
        ),
        (
            vec![ItemDefinition {
                is_instance_item: Some(false),
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::NonInstanced,
        ),
        (
            vec![ItemDefinition {
                is_instance_item: None,
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::InstanceKindUnknown,
        ),
        (
            vec![ItemDefinition {
                bucket_hash: None,
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::MissingBucket,
        ),
        (
            vec![ItemDefinition {
                bucket_hash: Some(BucketHash::new(215593132)),
                ..definition()
            }],
            "v1",
            InstancedPullRefusal::UnsupportedBucket,
        ),
    ];
    for (definitions, version, refusal) in cases {
        let mut s = service(input(), definitions, version, true, evidence());
        assert_eq!(
            plan(&mut s, &request()),
            Err(InstancedPullPlanningError::Refused(refusal))
        );
    }
}
#[test]
fn evidence_fetch_and_qualification_fail_closed() {
    let mut s = service(
        input(),
        vec![definition()],
        "v1",
        true,
        Err(AppError::new(ErrorCategory::ProviderUnavailable)),
    );
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Infrastructure(AppError::new(
            ErrorCategory::ProviderUnavailable
        )))
    );
    for (version, artifact, refusal) in [
        (
            "v2",
            "artifact",
            InstancedPullRefusal::PolicyVersionMismatch,
        ),
        (
            "v1",
            "different",
            InstancedPullRefusal::PolicyArtifactMismatch,
        ),
    ] {
        let mut s = service(
            input(),
            vec![definition()],
            "v1",
            true,
            Ok(EquipmentCapacityEvidence::new(DefinitionVersion::new(version), artifact).unwrap()),
        );
        assert_eq!(
            plan(&mut s, &request()),
            Err(InstancedPullPlanningError::Refused(refusal))
        );
    }
}
#[test]
fn capacity_refusals_are_typed() {
    let mut s = service(input(), vec![definition()], "v1", false, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::Capacity(CapacityRefusal::ObservationsUnavailable)
        ))
    );
    let mut full = input();
    for id in 10..20 {
        full.items.push(item(
            &id.to_string(),
            ItemLocation::Character(CharacterId::new("2")),
        ));
    }
    let mut s = service(full, vec![definition()], "v1", true, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::Capacity(CapacityRefusal::AtCapacity {
                occupied: 10,
                capacity: 10
            })
        ))
    );
    let mut stacked = input();
    stacked.stacks.push(
        StackObservation::new(
            ItemHash::new(8),
            BucketHash::new(BUCKET),
            1,
            StackContainer::CharacterInventory(CharacterId::new("2")),
        )
        .unwrap(),
    );
    let mut s = service(stacked, vec![definition()], "v1", true, evidence());
    assert_eq!(
        plan(&mut s, &request()),
        Err(InstancedPullPlanningError::Refused(
            InstancedPullRefusal::Capacity(CapacityRefusal::DestinationStackObserved)
        ))
    );
}
