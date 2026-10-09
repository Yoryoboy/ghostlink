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
    s.sync.gateway_mut().queue_inventory(Ok(input));
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
        &[ProfileSyncReason::Ordinary]
    );
}
#[test]
fn fresh_projection_dispatches_without_acquisition_or_manifest_fetch() {
    let mut s = service(input(), vec![definition()]);
    s.sync.repository_mut().replace_current(input()).unwrap();
    let p = plan(&mut s, 3).unwrap();
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    let outcome = s.execute(p, &mut gateway, FRESH).unwrap();
    assert_eq!(outcome.disposition(), PullDisposition::Applied);
    assert_eq!(gateway.calls(), 1);
    assert!(s.sync.gateway_mut().requested_reasons().is_empty());
    assert_eq!(s.manifest.capacity_evidence_calls(), 0);
    let current = s.sync.repository().load_current().unwrap().unwrap();
    assert!(
        current
            .stacks
            .contains(&row(7, StackContainer::Postmaster(CharacterId::new("2"))))
    );
    assert!(
        current
            .stacks
            .contains(&row(23, StackContainer::ProfileInventory))
    );
    assert_eq!(current.metadata.revision, SyncRevision::new(2));
    assert!(!s.sync.repository().freshness_invalidated().unwrap());
}
#[test]
fn second_pull_uses_updated_stacks_and_capacity() {
    let mut next = input();
    next.stacks = vec![row(10, StackContainer::Postmaster(CharacterId::new("2")))];
    next.stacks
        .extend((0..49).map(|_| row(100, StackContainer::ProfileInventory)));
    next.stacks.push(row(97, StackContainer::ProfileInventory));
    let mut s = service(next, vec![definition()]);
    let p = plan(&mut s, 2).unwrap();
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    s.execute(p, &mut gateway, FRESH).unwrap();
    let p = plan(&mut s, 1).unwrap();
    assert_eq!(p.context().pre_postmaster_quantity(), 8);
    assert_eq!(p.context().pre_destination_quantity(), 4999);
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    s.execute(p, &mut gateway, FRESH).unwrap();
    assert_eq!(
        plan(&mut s, 1).unwrap_err(),
        StackPullPlanningError::Refused(StackPullPlanningRefusal::Capacity(
            StackPullRefusal::InsufficientCapacity
        ))
    );
    assert_eq!(gateway.calls(), 2);
    assert_eq!(
        s.sync
            .repository()
            .load_current()
            .unwrap()
            .unwrap()
            .metadata
            .revision,
        SyncRevision::new(3)
    );
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Ordinary]
    );
    assert_eq!(s.manifest.capacity_evidence_calls(), 0);
    assert!(!s.sync.repository().freshness_invalidated().unwrap());
}

#[test]
fn time_and_revision_binding_refuse_without_dispatch() {
    for case in 0..3 {
        let (mut s, p) = armed();
        if case == 2 {
            let mut next = input();
            next.stacks[1] = row(21, StackContainer::ProfileInventory);
            s.sync.repository_mut().replace_current(next).unwrap();
        } else {
            s.clock = FakeClock::fixed(if case == 0 {
                p.expires_at()
            } else {
                Timestamp::new(99)
            });
        }
        let mut gateway = QueuedStackPullGateway::new();
        let failure = s.execute(p, &mut gateway, FRESH).unwrap_err();
        assert_eq!(
            failure.reason(),
            &StackPullExecutionFailureReason::PreDispatch(match case {
                0 => StackPullPreDispatchError::Expired,
                1 => StackPullPreDispatchError::BackwardsClock,
                _ => StackPullPreDispatchError::RevisionDrift,
            })
        );
        assert_eq!(gateway.calls(), 0);
        assert!(!s.sync.repository().freshness_invalidated().unwrap());
    }
}

#[test]
fn incomplete_invalidation_failure_retains_receipt() {
    let (mut s, p) = armed();
    let repository = s.sync.repository_mut();
    *repository = std::mem::take(repository).with_invalidation_failure();
    let mut gateway = QueuedStackPullGateway::new();
    let receipt = StackPullOutcome::Rejected(StackPullRejection::DestinationFull);
    gateway.queue(Ok(receipt));
    let outcome = s.execute(p, &mut gateway, FRESH).unwrap();
    assert_eq!(outcome.gateway_outcome(), &receipt);
    assert_eq!(outcome.disposition(), PullDisposition::LocalStateFailed);
    assert_eq!(gateway.calls(), 1);
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
    for case in 0..11 {
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
            9 => DefinitionMissing,
            _ => DefinitionAmbiguous,
        };
        let definitions = match case {
            9 => vec![],
            10 => vec![d.clone(), d],
            _ => vec![d],
        };
        refused(&mut service(input(), definitions), 1, reason);
    }
}
#[test]
fn membership_owner_source_refusals_and_policy_drift_tolerance() {
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
    for (version, artifact) in [("old", "artifact"), ("v1", "changed"), ("old", "changed")] {
        let mut s = service(input(), vec![definition()]);
        s.manifest = QueuedManifestGateway::new();
        s.manifest
            .queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                DefinitionVersion::new(version),
                artifact,
            )
            .unwrap()));
        assert!(plan(&mut s, 1).is_ok());
        assert_eq!(s.manifest.capacity_evidence_calls(), 0);
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
#[test]
fn absent_unique_label_means_not_unique() {
    // Most live Manifest stacks (e.g. Enhancement Core) carry no
    // `stackUniqueLabel`; like DIM, an absent label means "not unique".
    let mut d = definition();
    d.stack_unique_label = None;
    assert!(plan(&mut service(input(), vec![d]), 3).is_ok());
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
    let outcome = s.execute(p, &mut gateway, FRESH).unwrap();
    assert_eq!(outcome.disposition(), PullDisposition::Applied);
    assert_eq!(gateway.calls(), 1);
    assert_eq!(gateway.requests()[0].quantity(), 3);
    assert_eq!(gateway.requests()[0].item_id(), "0");
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        &[ProfileSyncReason::Ordinary]
    );
    assert!(!s.sync.repository().freshness_invalidated().unwrap());
    let current = s.sync.repository().load_current().unwrap().unwrap();
    assert_eq!(current.metadata.revision, SyncRevision::new(2));
    assert!(
        current
            .stacks
            .contains(&row(7, StackContainer::Postmaster(CharacterId::new("2"))))
    );
    assert!(
        current
            .stacks
            .contains(&row(23, StackContainer::ProfileInventory))
    );
}

#[test]
fn persistence_storage_failure_retains_receipt_and_fails_closed() {
    let (mut s, _) = armed();
    let repository = s.sync.repository_mut();
    *repository =
        InMemoryInventoryRepository::with_revision_for_test(input(), SyncRevision::new(u64::MAX));
    // Replan at the overflow revision: persistence fails only after acknowledgement.
    let p = plan(&mut s, 3).unwrap();
    let before = s.sync.repository().load_current().unwrap();
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(StackPullOutcome::Acknowledged));
    let outcome = s.execute(p, &mut gateway, FRESH).unwrap();
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
        &[ProfileSyncReason::Ordinary]
    );
    assert!(s.sync.repository().freshness_invalidated().unwrap());
}

#[test]
fn uncertain_and_refused_receipts_fail_without_post_action_requests() {
    for (receipt, disposition) in [
        (StackPullOutcome::Uncertain, PullDisposition::Uncertain),
        (
            StackPullOutcome::Rejected(StackPullRejection::DestinationFull),
            PullDisposition::Refused,
        ),
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
        let outcome = s.execute(p, &mut gateway, FRESH).unwrap();
        assert_eq!(outcome.disposition(), disposition);
        assert_eq!(outcome.gateway_outcome(), &receipt);
        assert_eq!(gateway.calls(), 1);
        assert!(s.sync.repository().freshness_invalidated().unwrap());
        assert_eq!(
            s.sync.gateway_mut().requested_reasons(),
            &[ProfileSyncReason::Ordinary]
        );
    }
}
#[test]
fn changed_source_consumes_plan_without_dispatch_or_postaction() {
    let mut s = service(input(), vec![definition()]);
    let p = plan(&mut s, 3).unwrap();
    let mut next = input();
    next.stacks
        .push(row(2, StackContainer::Postmaster(CharacterId::new("2"))));
    s.sync.repository_mut().replace_current(next).unwrap();
    let mut gateway = QueuedStackPullGateway::new();
    assert!(s.execute(p, &mut gateway, FRESH).is_err());
    assert_eq!(gateway.calls(), 0);
    assert!(!s.sync.repository().freshness_invalidated().unwrap());
}
