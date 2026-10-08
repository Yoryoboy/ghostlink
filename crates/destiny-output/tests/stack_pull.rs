use destiny_app::test_support::{leak_sentinels::*, *};
use destiny_app::*;
use destiny_output::stack_pull::StackPullOutput;
use serde_json::{Value, json};

const BUCKET: i64 = 1469714392;
const FRESH: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 60 };
const OWNER: &str = "2";
const VERSION: &str = "v1\n\u{1b}";
fn membership() -> DestinyMembership {
    DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
}
fn input(destination: i64, postmaster: i64) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new("123"),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new(VERSION),
        checked_at: Timestamp::new(100),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new(OWNER),
            name: Some(API_KEY.into()),
            class: None,
            race: None,
            gender: None,
            light: None,
            date_last_played: None,
        }],
        items: vec![],
        stacks: vec![
            StackObservation::new(
                ItemHash::new(7),
                BucketHash::new(BUCKET),
                destination,
                StackContainer::ProfileInventory,
            )
            .unwrap(),
            // Same hash in another profile bucket distinguishes whole-profile
            // totals from destination-bucket totals without overlapping sources.
            StackObservation::new(
                ItemHash::new(7),
                BucketHash::new(3313201758),
                5,
                StackContainer::ProfileInventory,
            )
            .unwrap(),
            StackObservation::new(
                ItemHash::new(7),
                BucketHash::new(215593132),
                postmaster,
                StackContainer::Postmaster(CharacterId::new(OWNER)),
            )
            .unwrap(),
        ],
        plug_set_entries: vec![],
        profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
    }
}
#[derive(Clone)]
struct Repository(std::rc::Rc<std::cell::RefCell<InMemoryInventoryRepository>>);
impl InventoryRepository for Repository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        self.0.borrow().load_current()
    }
    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        self.0.borrow_mut().replace_current(input)
    }
    fn freshness_invalidated(&self) -> Result<bool, AppError> {
        self.0.borrow().freshness_invalidated()
    }
    fn invalidate_freshness(&mut self) -> Result<(), AppError> {
        self.0.borrow_mut().invalidate_freshness()
    }
    fn apply_acknowledged_effects(
        &mut self,
        membership: &DestinyMembership,
        effects: &[InventoryEffect],
    ) -> Result<InventorySnapshot, AppError> {
        self.0
            .borrow_mut()
            .apply_acknowledged_effects(membership, effects)
    }
}

type Service = StackPullService<
    QueuedProfileGateway,
    Repository,
    InMemoryDefinitionRepository,
    QueuedManifestGateway,
    FakeClock,
    DeterministicPlanIds,
>;
fn armed(
    preflight: InventorySnapshotInput,
    posts: Vec<Result<InventorySnapshotInput, AppError>>,
) -> (Service, StackPullPlan) {
    let mut profile = QueuedProfileGateway::new();
    // Queued later reads must remain unused while the committed state is fresh.
    for next in posts {
        profile.queue_membership(Ok(next.as_ref().map_or_else(
            |_| membership(),
            |i| DestinyMembership::new(i.membership_type, i.membership_id.clone()).unwrap(),
        )));
        profile.queue_inventory(next);
    }
    let mut manifest = QueuedManifestGateway::new();
    for _ in 0..2 {
        manifest.queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
            DefinitionVersion::new(VERSION),
            "artifact",
        )
        .unwrap()));
    }
    let definition = ItemDefinition {
        hash: ItemHash::new(7),
        bucket_hash: Some(BucketHash::new(BUCKET)),
        does_postmaster_pull_have_side_effects: Some(false),
        is_instance_item: Some(false),
        max_stack_size: Some(100),
        stack_unique_label: Some(String::new()),
        name: BEARER_TOKEN.into(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec![RAW_PAYLOAD.into()],
        version: DefinitionVersion::new(VERSION),
    };
    let mut repository = Repository(std::rc::Rc::new(std::cell::RefCell::new(
        InMemoryInventoryRepository::new(),
    )));
    repository.replace_current(input(20, 10)).unwrap();
    let mut service = StackPullService::new(
        ProfileSyncService::new(profile, repository.clone()),
        InMemoryDefinitionRepository::new(DefinitionVersion::new(VERSION), vec![definition]),
        manifest,
        StackBucketCapacityPolicy::new(
            DefinitionVersion::new(VERSION),
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
    let plan = service
        .plan(
            &StackPullPlanRequest {
                membership: membership(),
                owner: CharacterId::new(OWNER),
                item_hash: ItemHash::new(7),
                amount: 3,
            },
            PlanLifetime::MAX,
            FRESH,
        )
        .unwrap();
    // Publish explicitly between plan and execution rather than forcing a read.
    // Unchanged content retains revision; changed source eligibility still refuses.
    repository.replace_current(preflight).unwrap();
    (service, plan)
}
fn execute(
    receipt: StackPullOutcome,
    posts: Vec<Result<InventorySnapshotInput, AppError>>,
) -> StackPullOutput {
    let (mut service, plan) = armed(input(20, 10), posts);
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(receipt));
    let outcome = service.execute(plan, &mut gateway, FRESH).unwrap();
    assert_eq!(gateway.calls(), 1);
    StackPullOutput::from_outcome(&outcome)
}
fn no_secrets(output: &StackPullOutput) {
    for rendered in [
        output.json().to_string(),
        output.render_text(),
        format!("{output:?}"),
    ] {
        for secret in SECRETS {
            assert!(!rendered.contains(secret));
        }
    }
}
#[test]
fn every_received_outcome_has_independent_receipt_and_disposition() {
    for (receipt, expected, disposition) in [
        (
            StackPullOutcome::Acknowledged,
            json!({"kind":"acknowledged"}),
            PullDisposition::Applied,
        ),
        (
            StackPullOutcome::Uncertain,
            json!({"kind":"uncertain"}),
            PullDisposition::Uncertain,
        ),
        (
            StackPullOutcome::Transient,
            json!({"kind":"transient"}),
            PullDisposition::Refused,
        ),
        (
            StackPullOutcome::AuthenticationRefused,
            json!({"kind":"authentication_refused"}),
            PullDisposition::Refused,
        ),
        (
            StackPullOutcome::Rejected(StackPullRejection::InvalidRequest),
            json!({"kind":"rejected","reason":"invalid_request"}),
            PullDisposition::Refused,
        ),
        (
            StackPullOutcome::Rejected(StackPullRejection::DestinationFull),
            json!({"kind":"rejected","reason":"destination_full"}),
            PullDisposition::Refused,
        ),
    ] {
        let output = execute(receipt, vec![Ok(input(23, 7))]);
        assert_eq!(output.disposition(), disposition);
        assert_eq!(output.is_success(), disposition == PullDisposition::Applied);
        let v = output.json();
        assert_eq!(v["kind"], "stack_pull");
        assert_eq!(v["plan_id"], "plan-1");
        assert_eq!(v["receipt"], expected);
        assert!(v["failure"].is_null());
        assert_eq!(v["dispatch"]["item"], json!({"item_hash":7,"amount":3}));
        assert_eq!(
            v["dispatch"]["destination"],
            json!({"character_id":OWNER,"normal_bucket":BUCKET})
        );
        assert_eq!(
            v["dispatch"]["pre_quantities"],
            json!({"destination":20,"profile":25,"postmaster":10})
        );
        assert!(v.get("observations").is_none());
        assert_eq!(v["dispatch"]["definition_version"], "v1??");
        assert_eq!(serde_json::from_str::<Value>(&v.to_string()).unwrap(), v);
        no_secrets(&output);
    }
}
#[test]
fn acknowledged_stale_stack_is_applied_without_invented_quantities() {
    let output = execute(StackPullOutcome::Acknowledged, vec![Ok(input(20, 10)); 3]);
    assert!(output.is_success());
    assert_eq!(output.json()["disposition"], "applied");
    assert!(output.json().get("observations").is_none());
    let text = output.render_text();
    assert!(text.contains("stack_pull: applied"));
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    for forbidden in ["membership_id", "characters", "plugs", "perks", "stats"] {
        assert!(!output.json().to_string().contains(forbidden));
    }
    no_secrets(&output);
}
#[test]
fn pre_dispatch_and_gateway_errors_never_invent_receipts() {
    for gateway_failure in [false, true] {
        let (mut service, plan) = armed(
            if gateway_failure {
                input(20, 10)
            } else {
                input(20, 1)
            },
            vec![],
        );
        let mut gateway = QueuedStackPullGateway::new();
        gateway.queue(Err(AppError::invalid_argument(BEARER_TOKEN, RAW_PAYLOAD)));
        let failure = service.execute(plan, &mut gateway, FRESH).unwrap_err();
        let output = StackPullOutput::execution_failed(&failure);
        let v = output.json();
        assert!(!output.is_success());
        assert_eq!(v["plan_id"], "plan-1");
        assert_eq!(v["disposition"], "refused");
        assert!(v.get("observations").is_none());
        assert!(v["receipt"].is_null());
        assert_eq!(gateway.calls(), usize::from(gateway_failure));
        assert_eq!(v["dispatch"].is_null(), !gateway_failure);
        assert_eq!(
            v["failure"],
            if gateway_failure {
                json!({"stage":"gateway","kind":"infrastructure","category":"provider_validation"})
            } else {
                json!({"stage":"pre_dispatch","kind":"eligibility_refused","reason":"capacity","capacity_reason":{"kind":"exceeds_smallest_postmaster_stack"}})
            }
        );
        no_secrets(&output);
    }
}
#[test]
fn every_planning_and_capacity_refusal_has_a_stable_code() {
    use StackPullPlanningRefusal::*;
    for (reason, code) in [
        (MembershipMismatch, "membership_mismatch"),
        (OwnerNotOwned, "owner_not_owned"),
        (DefinitionMissing, "definition_missing"),
        (DefinitionAmbiguous, "definition_ambiguous"),
        (DefinitionVersionMismatch, "definition_version_mismatch"),
        (SideEffects, "side_effects"),
        (SideEffectsUnknown, "side_effects_unknown"),
        (Instanced, "instanced"),
        (InstanceKindUnknown, "instance_kind_unknown"),
        (MissingBucket, "missing_bucket"),
        (UnsupportedBucket, "unsupported_bucket"),
        (PolicyVersionMismatch, "policy_version_mismatch"),
        (PolicyArtifactMismatch, "policy_artifact_mismatch"),
    ] {
        let output = StackPullOutput::planning_failed(&StackPullPlanningError::Refused(reason));
        assert_eq!(
            output.json(),
            json!({"kind":"stack_pull","disposition":"refused","plan_id":null,"dispatch":null,"receipt":null,"failure":{"stage":"planning","kind":"eligibility_refused","reason":code}})
        );
        assert!(output.render_text().contains(code));
        assert!(!output.is_success());
    }
    use StackPullRefusal::*;
    for (reason, code) in [
        (NoPostmasterStack, "no_postmaster_stack"),
        (
            ExceedsSmallestPostmasterStack,
            "exceeds_smallest_postmaster_stack",
        ),
        (UnknownCapacity, "unknown_capacity"),
        (UnknownOccupancy, "unknown_occupancy"),
        (UnknownExistingQuantity, "unknown_existing_quantity"),
        (UnknownMaxStackSize, "unknown_max_stack_size"),
        (UnknownUniqueness, "unknown_uniqueness"),
        (UnknownAccountWide, "unknown_account_wide"),
        (UnknownSideEffects, "unknown_side_effects"),
        (PullHasSideEffects, "pull_has_side_effects"),
        (InvalidCapacityMetadata, "invalid_capacity_metadata"),
        (ArithmeticOverflow, "arithmetic_overflow"),
        (InsufficientCapacity, "insufficient_capacity"),
    ] {
        let output =
            StackPullOutput::planning_failed(&StackPullPlanningError::Refused(Capacity(reason)));
        assert_eq!(
            output.json()["failure"],
            json!({"stage":"planning","kind":"eligibility_refused","reason":"capacity","capacity_reason":{"kind":code}})
        );
        assert!(output.render_text().contains(code));
        assert!(!output.is_success());
    }
    let output = StackPullOutput::planning_failed(&StackPullPlanningError::Infrastructure(
        AppError::invalid_argument(API_KEY, RAW_PAYLOAD),
    ));
    assert_eq!(
        output.json()["failure"],
        json!({"stage":"planning","kind":"infrastructure","category":"provider_validation"})
    );
    no_secrets(&output);
}
