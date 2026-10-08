use super::*;
use crate::test_support::*;
const BUCKET: i64 = 1469714392;
const FRESH: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 60 };
type Service = StackPullService<
    QueuedProfileGateway,
    InMemoryInventoryRepository,
    InMemoryDefinitionRepository,
    QueuedManifestGateway,
    FakeClock,
    DeterministicPlanIds,
>;
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn request(amount: i64) -> StackPullPlanRequest {
    StackPullPlanRequest {
        membership: membership(),
        owner: CharacterId::new("2"),
        item_hash: ItemHash::new(7),
        amount,
    }
}
fn row(quantity: i64, container: StackContainer) -> StackObservation {
    StackObservation::new(
        ItemHash::new(7),
        BucketHash::new(if matches!(container, StackContainer::Postmaster(_)) {
            215593132
        } else {
            BUCKET
        }),
        quantity,
        container,
    )
    .unwrap()
}
fn input() -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
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
        items: vec![],
        stacks: vec![
            row(10, StackContainer::Postmaster(CharacterId::new("2"))),
            row(20, StackContainer::ProfileInventory),
        ],
        plug_set_entries: vec![],
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
    }
}
fn definition() -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(7),
        bucket_hash: Some(BucketHash::new(BUCKET)),
        does_postmaster_pull_have_side_effects: Some(false),
        is_instance_item: Some(false),
        max_stack_size: Some(100),
        stack_unique_label: Some(String::new()),
        name: "stack".into(),
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
fn evidence() -> EquipmentCapacityEvidence {
    EquipmentCapacityEvidence::new(DefinitionVersion::new("v1"), "artifact").unwrap()
}
fn queue(s: &mut Service, input: InventorySnapshotInput) {
    s.sync.gateway_mut().queue_membership(Ok(membership()));
    s.sync
        .gateway_mut()
        .queue_acquisition(Ok(ProfileAcquisition::new(input, None).unwrap()));
}
fn service(input: InventorySnapshotInput, definitions: Vec<ItemDefinition>) -> Service {
    let mut s = StackPullService::new(
        ProfileSyncService::new(
            QueuedProfileGateway::new(),
            InMemoryInventoryRepository::new(),
        ),
        InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), definitions),
        QueuedManifestGateway::new(),
        StackBucketCapacityPolicy::new(
            DefinitionVersion::new("v1"),
            "artifact",
            vec![
                (BucketHash::new(BUCKET), 50, true),
                (BucketHash::new(3313201758), 50, true),
            ],
        )
        .unwrap(),
        FakeClock::fixed(Timestamp::new(100)),
        DeterministicPlanIds::starting_at(1),
    );
    queue(&mut s, input);
    s.manifest.queue_capacity_evidence(Ok(evidence()));
    s
}
fn plan(s: &mut Service, amount: i64) -> Result<StackPullPlan, StackPullPlanningError> {
    s.plan(&request(amount), PlanLifetime::MAX, FRESH)
}
fn refused(s: &mut Service, amount: i64, reason: StackPullPlanningRefusal) {
    assert_eq!(
        plan(s, amount).unwrap_err(),
        StackPullPlanningError::Refused(reason)
    );
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Preflight]
    );
}
#[test]
fn duplicate_selection_uses_smallest_not_sum() {
    let mut next = input();
    next.stacks
        .push(row(3, StackContainer::Postmaster(CharacterId::new("2"))));
    let mut s = service(next.clone(), vec![definition()]);
    refused(
        &mut s,
        4,
        StackPullPlanningRefusal::Capacity(StackPullRefusal::ExceedsSmallestPostmasterStack),
    );
    assert!(plan(&mut service(next, vec![definition()]), 3).is_ok());
}
#[test]
fn planning_refuses_each_definition_gate() {
    use StackPullPlanningRefusal::*;
    for case in 0..12 {
        let mut d = definition();
        let reason = match case {
            0 => {
                d.version = DefinitionVersion::new("old");
                DefinitionVersionMismatch
            }
            1 => {
                d.does_postmaster_pull_have_side_effects = Some(true);
                SideEffects
            }
            2 => {
                d.does_postmaster_pull_have_side_effects = None;
                SideEffectsUnknown
            }
            3 => {
                d.is_instance_item = Some(true);
                Instanced
            }
            4 => {
                d.is_instance_item = None;
                InstanceKindUnknown
            }
            5 => {
                d.bucket_hash = None;
                MissingBucket
            }
            6 => {
                d.bucket_hash = Some(BucketHash::new(3865314626));
                UnsupportedBucket
            }
            7 => {
                d.max_stack_size = None;
                Capacity(StackPullRefusal::UnknownMaxStackSize)
            }
            8 => {
                d.max_stack_size = Some(0);
                Capacity(StackPullRefusal::InvalidCapacityMetadata)
            }
            9 => {
                d.stack_unique_label = None;
                Capacity(StackPullRefusal::UnknownUniqueness)
            }
            10 => DefinitionMissing,
            _ => DefinitionAmbiguous,
        };
        let definitions = match case {
            10 => vec![],
            11 => vec![d.clone(), d],
            _ => vec![d],
        };
        refused(&mut service(input(), definitions), 1, reason);
    }
}
#[test]
fn membership_owner_source_and_policy_refusals() {
    use StackPullPlanningRefusal::*;
    let mut s = service(input(), vec![definition()]);
    let mut r = request(1);
    r.membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("999")).unwrap();
    assert_eq!(
        s.plan(&r, PlanLifetime::MAX, FRESH).unwrap_err(),
        StackPullPlanningError::Refused(MembershipMismatch)
    );
    let mut next = input();
    next.characters.clear();
    next.stacks.clear();
    refused(&mut service(next, vec![definition()]), 1, OwnerNotOwned);
    let mut next = input();
    next.stacks
        .retain(|r| r.container() == &StackContainer::ProfileInventory);
    refused(
        &mut service(next, vec![definition()]),
        1,
        Capacity(StackPullRefusal::NoPostmasterStack),
    );
    for (version, artifact, reason) in [
        ("old", "artifact", PolicyVersionMismatch),
        ("v1", "changed", PolicyArtifactMismatch),
        ("old", "changed", PolicyVersionMismatch),
    ] {
        let mut s = service(input(), vec![definition()]);
        s.manifest = QueuedManifestGateway::new();
        s.manifest
            .queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                DefinitionVersion::new(version),
                artifact,
            )
            .unwrap()));
        refused(&mut s, 1, reason);
    }
}
#[test]
fn capacity_counts_profile_rows_and_excludes_overlapping_sources() {
    let mut next = input();
    next.stacks = vec![row(10, StackContainer::Postmaster(CharacterId::new("2")))];
    next.stacks
        .extend((0..50).map(|_| row(100, StackContainer::ProfileInventory)));
    refused(
        &mut service(next.clone(), vec![definition()]),
        1,
        StackPullPlanningRefusal::Capacity(StackPullRefusal::InsufficientCapacity),
    );
    next.stacks.pop();
    next.stacks.push(row(90, StackContainer::ProfileInventory));
    next.profile_currencies_coverage = ProfileCurrenciesCoverage::Available;
    next.stacks
        .push(row(100, StackContainer::ProfileCurrencies));
    next.stacks.push(row(
        100,
        StackContainer::CharacterInventory(CharacterId::new("2")),
    ));
    assert!(plan(&mut service(next, vec![definition()]), 10).is_ok());
    let mut d = definition();
    d.stack_unique_label = Some("unique".into());
    let mut next = input();
    next.stacks[1] = row(100, StackContainer::ProfileInventory);
    refused(
        &mut service(next, vec![d]),
        1,
        StackPullPlanningRefusal::Capacity(StackPullRefusal::InsufficientCapacity),
    );
}
fn armed() -> (Service, StackPullPlan) {
    let mut s = service(input(), vec![definition()]);
    let p = plan(&mut s, 3).unwrap();
    queue(&mut s, input());
    s.manifest.queue_capacity_evidence(Ok(evidence()));
    (s, p)
}
#[test]
fn acknowledged_stale_profile_is_applied_without_post_action_read() {
    let (mut s, p) = armed();
    for _ in 0..3 {
        queue(&mut s, input());
    }
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    let outcome = s.execute_verified(p, &mut gateway, FRESH).unwrap();
    assert_eq!(outcome.disposition(), PullDisposition::Applied);
    assert_eq!(gateway.calls(), 1);
    assert_eq!(gateway.requests()[0].quantity(), 3);
    assert_eq!(gateway.requests()[0].item_id(), "0");
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Preflight, ProfileSyncReason::Preflight]
    );
    assert!(s.sync.repository().freshness_invalidated().unwrap());
}

#[test]
fn invalidation_storage_failure_retains_receipt_and_fails_closed() {
    let (mut s, p) = armed();
    let repository = s.sync.repository_mut();
    *repository = std::mem::take(repository).with_invalidation_failure();
    let before = s.sync.repository().load_current().unwrap();
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    let outcome = s.execute_verified(p, &mut gateway, FRESH).unwrap();
    assert_eq!(outcome.disposition(), PullDisposition::LocalStateFailed);
    assert_eq!(outcome.gateway_outcome(), &StackPullOutcome::Acknowledged);
    assert_eq!(
        outcome.local_state_error().unwrap().category(),
        ErrorCategory::LocalStorage
    );
    assert_eq!(gateway.calls(), 1);
    assert_eq!(s.sync.repository().load_current().unwrap(), before);
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Preflight; 2]
    );
}

#[test]
fn uncertain_and_refused_receipts_fail_without_post_action_requests() {
    for (receipt, disposition) in [
        (StackPullOutcome::Uncertain, PullDisposition::Uncertain),
        (
            StackPullOutcome::Rejected(StackPullRejection::InvalidRequest),
            PullDisposition::Refused,
        ),
        (StackPullOutcome::Transient, PullDisposition::Refused),
        (
            StackPullOutcome::AuthenticationRefused,
            PullDisposition::Refused,
        ),
    ] {
        let (mut s, p) = armed();
        let mut gateway = QueuedStackPullGateway::new();
        gateway.queue(Ok(receipt));
        let outcome = s.execute_verified(p, &mut gateway, FRESH).unwrap();
        assert_eq!(outcome.disposition(), disposition);
        assert_eq!(outcome.gateway_outcome(), &receipt);
        assert_eq!(gateway.calls(), 1);
        assert!(s.sync.repository().freshness_invalidated().unwrap());
        assert_eq!(
            s.sync.gateway_mut().requested_reasons(),
            &[ProfileSyncReason::Preflight; 2]
        );
    }
}
#[test]
fn drift_consumes_plan_without_dispatch_or_postaction() {
    let mut s = service(input(), vec![definition()]);
    let p = plan(&mut s, 3).unwrap();
    let mut next = input();
    next.stacks
        .push(row(2, StackContainer::Postmaster(CharacterId::new("2"))));
    queue(&mut s, next);
    s.manifest.queue_capacity_evidence(Ok(evidence()));
    let mut gateway = QueuedStackPullGateway::new();
    assert!(s.execute_verified(p, &mut gateway, FRESH).is_err());
    assert_eq!(gateway.calls(), 0);
    assert!(!s.sync.repository().freshness_invalidated().unwrap());
}
