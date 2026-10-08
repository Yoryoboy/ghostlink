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
#[derive(Default)]
struct Delay;
impl RetryDelay for Delay {
    fn wait(&mut self, _: std::time::Duration) {}
}
type Service = StackPullService<
    QueuedProfileGateway,
    InMemoryInventoryRepository,
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
    for next in [Ok(input(20, 10)), Ok(preflight)].into_iter().chain(posts) {
        // Resolve the candidate's membership coherently so account drift is
        // published and classified by the pull service, not rejected by sync.
        profile.queue_membership(Ok(next.as_ref().map_or_else(
            |_| membership(),
            |i| DestinyMembership::new(i.membership_type, i.membership_id.clone()).unwrap(),
        )));
        profile.queue_acquisition(next.and_then(|i| ProfileAcquisition::new(i, None)));
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
    let mut service = StackPullService::new(
        ProfileSyncService::new(profile, InMemoryInventoryRepository::new()),
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
    (service, plan)
}
fn execute(
    receipt: StackPullOutcome,
    posts: Vec<Result<InventorySnapshotInput, AppError>>,
) -> StackPullOutput {
    let (mut service, plan) = armed(input(20, 10), posts);
    let mut gateway = QueuedStackPullGateway::new();
    gateway.queue(Ok(receipt));
    let outcome = service
        .execute_verified(plan, &mut gateway, &mut Delay, FRESH)
        .unwrap();
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
            PullDisposition::Verified,
        ),
        (
            StackPullOutcome::Uncertain,
            json!({"kind":"uncertain"}),
            PullDisposition::Verified,
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
    ] {
        let output = execute(receipt, vec![Ok(input(23, 7))]);
        assert_eq!(output.disposition(), disposition);
        assert_eq!(
            output.is_success(),
            disposition == PullDisposition::Verified
        );
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
        assert_eq!(
            v["observations"][0]["quantities"],
            json!({"destination":{"pre":20,"post":23},"profile":{"pre":25,"post":28},"postmaster":{"pre":10,"post":7}})
        );
        assert_eq!(v["observations"][0]["classification"], "verified_arrival");
        assert_eq!(
            v["observations"][0]["publication"]["definition_version"],
            "v1??"
        );
        assert_eq!(serde_json::from_str::<Value>(&v.to_string()).unwrap(), v);
        assert_eq!(
            v["observations"][0]["publication"]["dataset"],
            "profile_inventory"
        );
        no_secrets(&output);
    }
}
#[test]
fn equal_timestamp_observations_stay_ordered_in_json_and_text() {
    let output = execute(
        StackPullOutcome::Acknowledged,
        vec![Ok(input(21, 9)), Ok(input(23, 7))],
    );
    let v = output.json();
    assert_eq!(v["observations"].as_array().unwrap().len(), 2);
    assert_eq!(v["observations"][0]["classification"], "quantity_mismatch");
    assert_eq!(v["observations"][1]["classification"], "verified_arrival");
    let text = output.render_text();
    let markers = [
        "stack_pull: verified",
        "plan_id: plan-1",
        "item_hash: 7",
        "amount: 3",
        "owner_character: 2",
        "destination_bucket: 1469714392",
        "receipt: acknowledged",
        "observation 1:",
        "classification: quantity_mismatch",
        "destination_quantity: 20 -> 21",
        "profile_quantity: 25 -> 26",
        "postmaster_quantity: 10 -> 9",
        "observation 2:",
        "classification: verified_arrival",
        "destination_quantity: 20 -> 23",
        "profile_quantity: 25 -> 28",
        "postmaster_quantity: 10 -> 7",
    ];
    let mut start = 0;
    for marker in markers {
        start += text[start..]
            .find(marker)
            .unwrap_or_else(|| panic!("missing ordered {marker}: {text}"))
            + marker.len();
    }
    assert!(!text.chars().any(|c| c.is_control() && c != '\n'));
    no_secrets(&output);
}
#[test]
fn unverified_acknowledgement_and_uncertain_receipt_never_succeed() {
    for receipt in [StackPullOutcome::Acknowledged, StackPullOutcome::Uncertain] {
        let output = execute(receipt, vec![Ok(input(20, 10)); 3]);
        assert_eq!(output.disposition(), PullDisposition::Uncertain);
        assert!(!output.is_success());
        assert_eq!(output.json()["disposition"], "uncertain");
        no_secrets(&output);
    }
}
#[test]
fn refresh_failure_preserves_nulls_and_later_arrival() {
    let output = execute(
        StackPullOutcome::Acknowledged,
        vec![
            Err(AppError::invalid_argument(API_KEY, RAW_PAYLOAD)),
            Ok(input(23, 7)),
        ],
    );
    let v = output.json();
    assert!(output.is_success());
    assert_eq!(
        v["observations"][0]["failure"],
        json!({"stage":"observation","kind":"refresh","category":"provider_validation"})
    );
    assert!(v["observations"][0]["publication"].is_null());
    assert!(v["observations"][0]["classification"].is_null());
    assert_eq!(
        v["observations"][0]["quantities"],
        json!({"destination":{"pre":20,"post":null},"profile":{"pre":25,"post":null},"postmaster":{"pre":10,"post":null}})
    );
    no_secrets(&output);
}
#[test]
fn whole_profile_contradiction_is_visible_even_when_destination_arrives() {
    let mut post = input(23, 7);
    post.stacks[1] = StackObservation::new(
        ItemHash::new(7),
        BucketHash::new(3313201758),
        2,
        StackContainer::ProfileInventory,
    )
    .unwrap();
    let output = execute(StackPullOutcome::Acknowledged, vec![Ok(post); 3]);
    assert!(!output.is_success());
    let observation = &output.json()["observations"][0];
    assert_eq!(observation["classification"], "quantity_mismatch");
    assert_eq!(
        observation["quantities"]["destination"],
        json!({"pre":20,"post":23})
    );
    assert_eq!(
        observation["quantities"]["profile"],
        json!({"pre":25,"post":25})
    );
    assert!(output.render_text().contains("profile_quantity: 25 -> 25"));
}
#[test]
fn unavailable_observations_never_fabricate_zero_quantities() {
    let output = execute(StackPullOutcome::Uncertain, vec![]);
    assert!(!output.is_success());
    let v = output.json();
    assert_eq!(v["observations"].as_array().unwrap().len(), 3);
    for observation in v["observations"].as_array().unwrap() {
        assert!(observation["publication"].is_null());
        assert!(observation["classification"].is_null());
        for scope in ["destination", "profile", "postmaster"] {
            assert!(observation["quantities"][scope]["post"].is_null());
        }
        assert_eq!(
            observation["failure"],
            json!({"stage":"observation","kind":"refresh","category":"provider_unavailable"})
        );
    }
    assert!(
        output
            .render_text()
            .contains("destination_quantity: 20 -> null")
    );
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
        let failure = service
            .execute_verified(plan, &mut gateway, &mut Delay, FRESH)
            .unwrap_err();
        let output = StackPullOutput::execution_failed(&failure);
        let v = output.json();
        assert!(!output.is_success());
        assert_eq!(v["plan_id"], "plan-1");
        assert_eq!(v["disposition"], "refused");
        assert_eq!(v["observations"], json!([]));
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
            json!({"kind":"stack_pull","disposition":"refused","plan_id":null,"dispatch":null,"receipt":null,"failure":{"stage":"planning","kind":"eligibility_refused","reason":code},"observations":[]})
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
#[test]
fn drift_and_backwards_publication_remain_unsuccessful() {
    for (kind, expected) in [
        (0, "membership_mismatch"),
        (1, "definition_version_drift"),
        (2, "verified_arrival"),
    ] {
        let mut post = input(23, 7);
        match kind {
            0 => post.membership_id = MembershipId::new("999"),
            1 => post.definition_version = DefinitionVersion::new("other"),
            _ => post.checked_at = Timestamp::new(99),
        }
        let output = execute(StackPullOutcome::Acknowledged, vec![Ok(post)]);
        assert!(!output.is_success());
        assert_eq!(output.json()["observations"][0]["classification"], expected);
        if kind == 2 {
            assert_eq!(
                output.json()["observations"][0]["failure"],
                json!({"stage":"observation","kind":"backwards_clock"})
            );
        }
    }
}
