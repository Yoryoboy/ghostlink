use super::*;
use crate::test_support::{
    DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
    QueuedInstancedPullGateway, QueuedManifestGateway, QueuedProfileGateway,
};
use destiny_core::*;
use std::{cell::Cell, rc::Rc};

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
fn queue_preflight(s: &mut Service, input: InventorySnapshotInput, observed: bool) {
    let rows = observed.then(|| {
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
    s.sync
        .gateway_mut()
        .queue_membership(Ok(DestinyMembership::new(
            input.membership_type,
            input.membership_id.clone(),
        )
        .unwrap()));
    s.sync
        .gateway_mut()
        .queue_acquisition(Ok(ProfileAcquisition::new(input, rows).unwrap()));
    s.manifest.queue_capacity_evidence(evidence());
}

// These wrappers are installed only after planning and queueing valid renewed
// evidence. Their counters therefore describe execution, not planning reads.
#[derive(Default)]
struct DispatchReads {
    clocks: Cell<usize>,
    loads: Cell<usize>,
    publications: Cell<usize>,
    lookups: Cell<usize>,
    versions: Cell<usize>,
    now: Cell<u64>,
}

#[derive(Clone, Copy, Debug)]
enum FinalInventoryFault {
    None,
    Error,
    Missing,
    MembershipId,
    MembershipType,
    Revision,
    DefinitionVersion,
}

struct DispatchInventory {
    inner: InMemoryInventoryRepository,
    reads: Rc<DispatchReads>,
    fault: FinalInventoryFault,
    advance_to: Option<u64>,
}
impl InventoryRepository for DispatchInventory {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        let call = self.reads.loads.get() + 1;
        self.reads.loads.set(call);
        let mut snapshot = self.inner.load_current()?;
        if call == 2 {
            // The first load is sync's freshness read; the second must follow
            // successful publication and the renewed definition safety checks.
            assert_eq!(self.reads.publications.get(), 1);
            assert_eq!(self.reads.lookups.get(), 1);
            assert_eq!(self.reads.versions.get(), 1);
            if let Some(now) = self.advance_to {
                self.reads.now.set(now); // Simulated slow acquisition/read path.
            }
            match self.fault {
                FinalInventoryFault::None => {}
                FinalInventoryFault::Error => {
                    return Err(AppError::new(ErrorCategory::LocalStorage));
                }
                FinalInventoryFault::Missing => return Ok(None),
                fault => {
                    let metadata = &mut snapshot.as_mut().unwrap().metadata;
                    match fault {
                        FinalInventoryFault::MembershipId => {
                            metadata.membership_id = MembershipId::new("456");
                        }
                        FinalInventoryFault::MembershipType => {
                            metadata.membership_type = MembershipType::new(2);
                        }
                        FinalInventoryFault::Revision => {
                            metadata.revision = SyncRevision::new(metadata.revision.value() + 1);
                        }
                        FinalInventoryFault::DefinitionVersion => {
                            metadata.definition_version = DefinitionVersion::new("v2");
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
        Ok(snapshot)
    }
    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        let snapshot = self.inner.replace_current(input)?;
        self.reads
            .publications
            .set(self.reads.publications.get() + 1);
        Ok(snapshot)
    }
}

struct DispatchDefinitions {
    inner: InMemoryDefinitionRepository,
    reads: Rc<DispatchReads>,
    final_version: Option<Result<DefinitionVersion, AppError>>,
}
impl DefinitionRepository for DispatchDefinitions {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        let call = self.reads.versions.get() + 1;
        self.reads.versions.set(call);
        if call == 2 {
            assert_eq!(self.reads.loads.get(), 2);
            assert_eq!(self.reads.publications.get(), 1);
            assert_eq!(self.reads.lookups.get(), 1);
            if let Some(result) = &self.final_version {
                return result.clone();
            }
        }
        self.inner.current_definition_version()
    }
    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        self.reads.lookups.set(self.reads.lookups.get() + 1);
        self.inner.lookup_definition(hash)
    }
    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        self.inner.current_metadata()
    }
    fn replace_definitions(
        &mut self,
        input: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.inner.replace_definitions(input)
    }
}

struct DispatchClock {
    reads: Rc<DispatchReads>,
    error_on: Option<usize>,
}
impl Clock for DispatchClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        let call = self.reads.clocks.get() + 1;
        self.reads.clocks.set(call);
        if self.error_on == Some(call) {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(Timestamp::new(self.reads.now.get()))
    }
}

type FaultService = InstancedPullService<
    QueuedProfileGateway,
    DispatchInventory,
    DispatchDefinitions,
    QueuedManifestGateway,
    DispatchClock,
    DeterministicPlanIds,
>;
fn armed_dispatch() -> (FaultService, InstancedPullPlan, Rc<DispatchReads>) {
    let mut s = service(input(), vec![definition()], "v1", true, evidence());
    let plan = plan(&mut s, &request()).unwrap();
    queue_preflight(&mut s, input(), true);
    let reads = Rc::new(DispatchReads::default());
    reads.now.set(100);
    let (gateway, repository) = s.sync.into_parts();
    let wrapped = InstancedPullService::new(
        ProfileSyncService::new(
            gateway,
            DispatchInventory {
                inner: repository,
                reads: reads.clone(),
                fault: FinalInventoryFault::None,
                advance_to: None,
            },
        ),
        DispatchDefinitions {
            inner: s.definitions,
            reads: reads.clone(),
            final_version: None,
        },
        s.manifest,
        s.policy,
        DispatchClock {
            reads: reads.clone(),
            error_on: None,
        },
        s.plan_ids,
    );
    (wrapped, plan, reads)
}

fn assert_fault_dispatch(
    s: &mut FaultService,
    plan: InstancedPullPlan,
    expected: PreDispatchError,
    clocks: usize,
    loads: usize,
    versions: usize,
) {
    let mut gateway = QueuedInstancedPullGateway::new();
    gateway.queue(Ok(InstancedPullOutcome::Acknowledged));
    assert_eq!(
        s.preflight_dispatch(plan, &mut gateway, FreshnessPolicy { ttl_seconds: 60 }),
        Err(expected)
    );
    assert_eq!(gateway.calls(), 0);
    assert!(gateway.requests().is_empty());
    let reads = &s.clock.reads;
    assert_eq!(reads.clocks.get(), clocks);
    assert_eq!(reads.loads.get(), loads);
    assert_eq!(reads.versions.get(), versions);
    let renewed = usize::from(loads != 0);
    assert_eq!(reads.publications.get(), renewed);
    assert_eq!(reads.lookups.get(), renewed);
    assert_eq!(s.manifest.capacity_evidence_calls(), 1 + renewed);
    assert_eq!(s.sync.gateway_mut().membership_calls(), 1 + renewed);
    assert_eq!(
        s.sync.gateway_mut().observed_acquisition_calls(),
        1 + renewed
    );
    assert_eq!(s.sync.gateway_mut().inventory_calls(), 1 + renewed);
    assert_eq!(
        s.sync.gateway_mut().requested_reasons(),
        vec![ProfileSyncReason::Preflight; 1 + renewed].as_slice()
    );
    assert_eq!(s.plan_ids.calls(), 1);
}

#[test]
fn final_clock_rejects_expiry_after_slow_reads_at_and_beyond_boundary() {
    for beyond in [0, 1] {
        let (mut s, plan, _) = armed_dispatch();
        s.sync.repository_mut().advance_to = Some(plan.expires_at().seconds() + beyond);
        assert_fault_dispatch(&mut s, plan, PreDispatchError::Expired, 2, 2, 2);
    }
}

#[test]
fn slow_read_control_dispatches_once_just_before_expiry() {
    let (mut s, plan, reads) = armed_dispatch();
    let dispatch_at = Timestamp::new(plan.expires_at().seconds() - 1);
    s.sync.repository_mut().advance_to = Some(dispatch_at.seconds());
    let mut gateway = QueuedInstancedPullGateway::new();
    gateway.queue(Ok(InstancedPullOutcome::Acknowledged));
    let attempt = s
        .preflight_dispatch(plan, &mut gateway, FreshnessPolicy { ttl_seconds: 60 })
        .unwrap();
    assert_eq!(attempt.preflight_at, Timestamp::new(100));
    assert_eq!(attempt.dispatch_at, dispatch_at);
    assert_eq!(attempt.outcome, Ok(InstancedPullOutcome::Acknowledged));
    assert_eq!(gateway.calls(), 1);
    assert_eq!(gateway.requests().len(), 1);
    assert_eq!(reads.clocks.get(), 2);
    assert_eq!(reads.loads.get(), 2);
    assert_eq!(reads.publications.get(), 1);
    assert_eq!(reads.lookups.get(), 1);
    assert_eq!(reads.versions.get(), 2);
    assert_eq!(s.plan_ids.calls(), 1);
}

#[test]
fn execution_clock_errors_preserve_sanitized_category_at_each_stage() {
    for call in [1, 2] {
        let (mut s, plan, _) = armed_dispatch();
        s.clock.error_on = Some(call);
        let renewed = call == 2;
        assert_fault_dispatch(
            &mut s,
            plan,
            PreDispatchError::Eligibility(InstancedPullPlanningError::Infrastructure(
                AppError::new(ErrorCategory::ProviderValidation),
            )),
            call,
            if renewed { 2 } else { 0 },
            if renewed { 2 } else { 0 },
        );
    }
}

#[test]
fn final_inventory_error_and_missing_snapshot_follow_renewed_eligibility() {
    for fault in [FinalInventoryFault::Error, FinalInventoryFault::Missing] {
        let (mut s, plan, _) = armed_dispatch();
        s.sync.repository_mut().fault = fault;
        assert_fault_dispatch(
            &mut s,
            plan,
            PreDispatchError::Eligibility(InstancedPullPlanningError::Infrastructure(
                AppError::new(ErrorCategory::LocalStorage),
            )),
            1,
            2,
            1,
        );
    }
}

#[test]
fn final_definition_read_error_and_version_drift_follow_renewed_eligibility() {
    for (result, expected) in [
        (
            Err(AppError::new(ErrorCategory::LocalStorage)),
            PreDispatchError::Eligibility(InstancedPullPlanningError::Infrastructure(
                AppError::new(ErrorCategory::LocalStorage),
            )),
        ),
        (
            Ok(DefinitionVersion::new("v2")),
            PreDispatchError::DefinitionVersionDrift,
        ),
    ] {
        let (mut s, plan, _) = armed_dispatch();
        s.definitions.final_version = Some(result);
        assert_fault_dispatch(&mut s, plan, expected, 1, 2, 2);
    }
}

#[test]
fn publication_drift_visible_only_at_final_reload_refuses_dispatch() {
    // This models drift visible BEFORE the last local read, not atomic protection
    // against publication after it. Caller serialization remains required.
    for (fault, expected) in [
        (
            FinalInventoryFault::MembershipId,
            PreDispatchError::IdentityDrift,
        ),
        (
            FinalInventoryFault::MembershipType,
            PreDispatchError::IdentityDrift,
        ),
        (
            FinalInventoryFault::Revision,
            PreDispatchError::RevisionDrift,
        ),
        (
            FinalInventoryFault::DefinitionVersion,
            PreDispatchError::DefinitionVersionDrift,
        ),
    ] {
        let (mut s, plan, _) = armed_dispatch();
        s.sync.repository_mut().fault = fault;
        assert_fault_dispatch(&mut s, plan, expected, 1, 2, 1);
    }
}

#[test]
fn consumed_preflight_dispatches_exactly_once_without_reminting() {
    for outcome in [
        Ok(InstancedPullOutcome::Acknowledged),
        Ok(InstancedPullOutcome::Rejected(
            InstancedPullRejection::InvalidRequest,
        )),
        Ok(InstancedPullOutcome::AuthenticationRefused),
        Ok(InstancedPullOutcome::Transient),
        Ok(InstancedPullOutcome::Uncertain),
        Err(AppError::new(ErrorCategory::ProviderUnavailable)),
    ] {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        queue_preflight(&mut s, input(), true);
        let mut gateway = QueuedInstancedPullGateway::new();
        gateway.queue(outcome.clone());
        gateway.queue(Ok(InstancedPullOutcome::Acknowledged));
        let attempt = s
            .preflight_dispatch(plan, &mut gateway, FreshnessPolicy { ttl_seconds: 60 })
            .unwrap();
        assert_eq!(attempt.outcome, outcome);
        assert_eq!(attempt.context.item, ItemInstanceId::new("1"));
        assert_eq!(gateway.calls(), 1);
        assert_eq!(
            gateway.requests(),
            &[InstancedPullRequest::new(
                membership(),
                ItemInstanceId::new("1"),
                ItemHash::new(7),
                CharacterId::new("2")
            )
            .unwrap()]
        );
        assert_eq!(gateway.requests()[0].quantity(), 1);
        assert_eq!(s.plan_ids.calls(), 1);
        assert_eq!(s.manifest.capacity_evidence_calls(), 2);
        assert_eq!(
            s.sync.gateway_mut().requested_reasons(),
            &[ProfileSyncReason::Preflight, ProfileSyncReason::Preflight]
        );
    }
}

#[test]
fn preflight_expiry_and_backwards_clock_do_not_acquire() {
    for (now, expected) in [
        (99, PreDispatchError::BackwardsClock),
        (1000, PreDispatchError::Expired),
    ] {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        s.clock = FakeClock::fixed(Timestamp::new(now));
        let mut gateway = QueuedInstancedPullGateway::new();
        assert_eq!(
            s.preflight_dispatch(plan, &mut gateway, FreshnessPolicy { ttl_seconds: 60 }),
            Err(expected)
        );
        assert_eq!(gateway.calls(), 0);
        assert_eq!(s.sync.gateway_mut().membership_calls(), 1);
        assert_eq!(s.manifest.capacity_evidence_calls(), 1);
    }
}

fn assert_preflight_refusal(s: &mut Service, plan: InstancedPullPlan, expected: PreDispatchError) {
    let mut gateway = QueuedInstancedPullGateway::new();
    gateway.queue(Ok(InstancedPullOutcome::Acknowledged));
    assert_eq!(
        s.preflight_dispatch(plan, &mut gateway, FreshnessPolicy { ttl_seconds: 60 }),
        Err(expected)
    );
    assert_eq!(gateway.calls(), 0);
    assert!(gateway.requests().is_empty());
    assert_eq!(s.plan_ids.calls(), 1);
    assert!(
        s.sync
            .gateway_mut()
            .requested_reasons()
            .iter()
            .all(|r| *r == ProfileSyncReason::Preflight)
    );
}
fn refused(reason: InstancedPullRefusal) -> PreDispatchError {
    PreDispatchError::Eligibility(InstancedPullPlanningError::Refused(reason))
}

#[test]
fn preflight_identity_and_revision_changes_fail_closed() {
    for case in 0..7 {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        let mut changed = input();
        let expected = match case {
            0 => {
                changed.membership_id = MembershipId::new("456");
                refused(InstancedPullRefusal::MembershipMismatch)
            }
            1 => {
                changed.membership_type = MembershipType::new(2);
                refused(InstancedPullRefusal::MembershipMismatch)
            }
            2 => {
                let mut other = changed.characters[0].clone();
                other.id = CharacterId::new("4");
                changed.characters.push(other);
                changed.items[0].location = ItemLocation::Postmaster(CharacterId::new("4"));
                PreDispatchError::IdentityDrift
            }
            3 => {
                changed.items[0].item_hash = ItemHash::new(8);
                let mut row = definition();
                row.hash = ItemHash::new(8);
                s.definitions =
                    InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), vec![row]);
                PreDispatchError::IdentityDrift
            }
            4 => {
                changed.items[0].location = ItemLocation::Character(CharacterId::new("2"));
                refused(InstancedPullRefusal::NotInPostmaster)
            }
            5 => {
                changed.items[0].locked = true;
                PreDispatchError::RevisionDrift
            }
            _ => {
                changed.items.clear();
                refused(InstancedPullRefusal::TargetMissing)
            }
        };
        queue_preflight(&mut s, changed, true);
        assert_preflight_refusal(&mut s, plan, expected);
    }
}

#[test]
fn preflight_rechecks_definition_safety_and_versions() {
    for case in 0..6 {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        let mut row = definition();
        let mut version = "v1";
        let reason = match case {
            0 => {
                row.does_postmaster_pull_have_side_effects = Some(true);
                InstancedPullRefusal::SideEffects
            }
            1 => {
                row.does_postmaster_pull_have_side_effects = None;
                InstancedPullRefusal::SideEffectsUnknown
            }
            2 => {
                row.is_instance_item = Some(false);
                InstancedPullRefusal::NonInstanced
            }
            3 => {
                row.is_instance_item = None;
                InstancedPullRefusal::InstanceKindUnknown
            }
            4 => {
                row.version = DefinitionVersion::new("v2");
                InstancedPullRefusal::DefinitionVersionMismatch
            }
            _ => {
                version = "v2";
                InstancedPullRefusal::DefinitionVersionMismatch
            }
        };
        s.definitions =
            InMemoryDefinitionRepository::new(DefinitionVersion::new(version), vec![row]);
        queue_preflight(&mut s, input(), true);
        assert_preflight_refusal(&mut s, plan, refused(reason));
    }
}

#[test]
fn changed_published_definition_version_refuses_even_when_new_policy_qualifies() {
    let mut s = service(input(), vec![definition()], "v1", true, evidence());
    let plan = plan(&mut s, &request()).unwrap();
    let mut changed = input();
    changed.definition_version = DefinitionVersion::new("v2");
    let mut row = definition();
    row.version = DefinitionVersion::new("v2");
    s.definitions = InMemoryDefinitionRepository::new(DefinitionVersion::new("v2"), vec![row]);
    s.policy = EquipmentCapacityPolicy::new(
        DefinitionVersion::new("v2"),
        "artifact",
        [
            1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954,
            1585787867,
        ]
        .map(|b| (BucketHash::new(b), 10))
        .to_vec(),
    )
    .unwrap();
    s.manifest
        .queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
            DefinitionVersion::new("v2"),
            "artifact",
        )
        .unwrap()));
    queue_preflight(&mut s, changed, true);
    assert_preflight_refusal(&mut s, plan, PreDispatchError::DefinitionVersionDrift);
}

#[test]
fn bucket_only_identity_drift_is_detected_with_unchanged_revision() {
    let mut input = input();
    input
        .items
        .push(item("3", ItemLocation::Character(CharacterId::new("2"))));
    input
        .items
        .push(item("9", ItemLocation::Character(CharacterId::new("2"))));
    let mut s = service(input.clone(), vec![definition()], "v1", true, evidence());
    // Initial evidence: only instance 3 occupies the destination bucket.
    let mut g = QueuedProfileGateway::new();
    g.queue_membership(Ok(membership()));
    let acquisition = |occupant: &str| {
        ProfileAcquisition::new(
            input.clone(),
            Some(
                input
                    .items
                    .iter()
                    .map(|i| {
                        InstancedBucketObservation::new(
                            i.instance_id.clone(),
                            BucketHash::new(if i.instance_id.as_str() == occupant {
                                BUCKET
                            } else {
                                215593132
                            }),
                            i.location.clone(),
                        )
                        .unwrap()
                    })
                    .collect(),
            ),
        )
        .unwrap()
    };
    g.queue_acquisition(Ok(acquisition("3")));
    let (_, repository) = s.sync.into_parts();
    s.sync = ProfileSyncService::new(g, repository);
    let plan = plan(&mut s, &request()).unwrap();
    let revision = plan.revision;
    s.sync.gateway_mut().queue_membership(Ok(membership()));
    s.sync.gateway_mut().queue_acquisition(Ok(acquisition("9")));
    s.manifest.queue_capacity_evidence(evidence());
    assert_preflight_refusal(&mut s, plan, PreDispatchError::FingerprintDrift);
    assert_eq!(
        s.sync
            .repository()
            .load_current()
            .unwrap()
            .unwrap()
            .metadata
            .revision,
        revision
    );
}

#[test]
fn preflight_capacity_and_evidence_are_reproved() {
    for case in 0..5 {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        let mut changed = input();
        let reason = match case {
            0 => InstancedPullRefusal::Capacity(CapacityRefusal::ObservationsUnavailable),
            1 => {
                for id in 10..20 {
                    changed.items.push(item(
                        &id.to_string(),
                        ItemLocation::Character(CharacterId::new("2")),
                    ));
                }
                InstancedPullRefusal::Capacity(CapacityRefusal::AtCapacity {
                    occupied: 10,
                    capacity: 10,
                })
            }
            2 => {
                changed.stacks.push(
                    StackObservation::new(
                        ItemHash::new(8),
                        BucketHash::new(BUCKET),
                        1,
                        StackContainer::CharacterInventory(CharacterId::new("2")),
                    )
                    .unwrap(),
                );
                InstancedPullRefusal::Capacity(CapacityRefusal::DestinationStackObserved)
            }
            3 => InstancedPullRefusal::PolicyArtifactMismatch,
            _ => InstancedPullRefusal::PolicyVersionMismatch,
        };
        if case >= 3 {
            s.manifest
                .queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                    DefinitionVersion::new(if case == 4 { "v2" } else { "v1" }),
                    if case == 3 { "changed" } else { "artifact" },
                )
                .unwrap()));
        }
        queue_preflight(&mut s, changed, case != 0);
        assert_preflight_refusal(&mut s, plan, refused(reason));
    }
}

#[test]
fn preflight_infrastructure_failures_do_not_dispatch() {
    for case in 0..3 {
        let mut s = service(input(), vec![definition()], "v1", true, evidence());
        let plan = plan(&mut s, &request()).unwrap();
        let error = AppError::new(ErrorCategory::ProviderUnavailable);
        match case {
            0 => s.sync.gateway_mut().queue_membership(Err(error.clone())),
            1 => {
                s.sync.gateway_mut().queue_membership(Ok(membership()));
                s.sync.gateway_mut().queue_acquisition(Err(error.clone()));
            }
            _ => {
                s.manifest.queue_capacity_evidence(Err(error.clone()));
                queue_preflight(&mut s, input(), true);
            }
        }
        assert_preflight_refusal(
            &mut s,
            plan,
            PreDispatchError::Eligibility(InstancedPullPlanningError::Infrastructure(error)),
        );
    }
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
