//! Shared presentation of genuine-instance equipment pulls, not anonymous stacks.
//! Callers must serialize local inventory publication and definition use through
//! execution. Published arrival is neither causal proof nor an atomic guarantee.
//! This module makes no live or full-stack readiness claim.
use destiny_app::{
    AppError, CapacityRefusal, ErrorCategory, InstancedPullExecutionFailure,
    InstancedPullExecutionFailureReason, InstancedPullExecutionOutcome, InstancedPullOutcome,
    InstancedPullPlanningError, InstancedPullRefusal, InstancedPullRejection, ItemLocation,
    PreDispatchError, PullArrival, PullAttempt, PullDisposition, PullObservationFailure,
    SourceMarker,
};
use serde_json::{Value, json};

/// A single transport-neutral structured projection; no CLI exit or MCP policy.
#[derive(Clone, Debug)]
pub struct InstancedPullOutput {
    disposition: PullDisposition,
    value: Value,
}
impl InstancedPullOutput {
    /// Copy the domain disposition verbatim, independently of receipt and arrival.
    pub fn from_outcome(outcome: &InstancedPullExecutionOutcome) -> Self {
        let disposition = outcome.disposition();
        let observations: Vec<_> = outcome.observations().iter().map(|attempt| {
            let observation = attempt.observation();
            json!({
                "checked_at": attempt.checked_at().map(|time| time.seconds()),
                "classification": observation.map(|pair| arrival_kind(pair.classification())),
                "failure": attempt.failure().map(observation_failure),
                "publication": observation.map(|pair| {
                    let metadata = pair.metadata();
                    json!({
                        "checked_at": metadata.checked_at.seconds(),
                        "revision": metadata.revision.value(),
                        "definition_version": clean(metadata.definition_version.as_str()),
                        "dataset": "profile_inventory",
                        "source": match metadata.source {
                            SourceMarker::Profile => "profile",
                            SourceMarker::Manifest => "manifest",
                        },
                    })
                }),
                "actual_buckets_available": observation.map(|pair| pair.actual_buckets_available()),
                "target": observation.and_then(|pair| pair.target_id().map(|id| json!({
                    "instance_id": clean(id.as_str()),
                    "item_hash": pair.target_hash().map(|hash| hash.value()),
                    "location": pair.target_location().map(location),
                    "runtime_bucket": pair.target_actual_bucket().map(|bucket| bucket.value()),
                }))),
            })
        }).collect();
        Self {
            disposition,
            value: json!({
                "kind": "instanced_pull",
                "disposition": disposition_kind(disposition),
                "plan_id": clean(outcome.plan_id().as_str()),
                "dispatch": dispatch(outcome.dispatch()),
                "receipt": receipt(outcome.gateway_outcome()),
                "failure": null,
                "observations": observations,
            }),
        }
    }
    /// No action was admitted; absence of action evidence is represented by nulls.
    pub fn planning_failed(error: &InstancedPullPlanningError) -> Self {
        Self::failed(None, Value::Null, planning_failure(error, "planning"))
    }
    pub fn execution_failed(failure: &InstancedPullExecutionFailure) -> Self {
        let (dispatch, reason) = match failure.reason() {
            InstancedPullExecutionFailureReason::PreDispatch(error) => {
                let reason = match error {
                    PreDispatchError::Eligibility(error) => planning_failure(error, "pre_dispatch"),
                    other => json!({"stage":"pre_dispatch", "kind":pre_dispatch_kind(other)}),
                };
                (Value::Null, reason)
            }
            InstancedPullExecutionFailureReason::Gateway(attempt) => {
                // The gateway contract makes Err pre-dispatch-only. Retain the
                // attempt context/time, but never invent a received receipt.
                let reason = match attempt.gateway_result() {
                    Err(error) => infrastructure(error, "gateway", "infrastructure"),
                    Ok(_) => json!({"stage":"gateway", "kind":"evidence_unavailable"}),
                };
                (dispatch(attempt), reason)
            }
        };
        Self::failed(Some(failure.plan_id().as_str()), dispatch, reason)
    }
    fn failed(plan_id: Option<&str>, dispatch: Value, failure: Value) -> Self {
        Self {
            disposition: PullDisposition::Refused,
            value: json!({
                "kind":"instanced_pull", "disposition":"refused",
                "plan_id":plan_id.map(clean), "dispatch":dispatch,
                "receipt":null, "failure":failure, "observations":[],
            }),
        }
    }
    pub fn disposition(&self) -> PullDisposition {
        self.disposition
    }
    pub fn is_success(&self) -> bool {
        self.disposition == PullDisposition::Verified
    }
    pub fn json(&self) -> Value {
        self.value.clone()
    }
}

fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
fn dispatch(attempt: &PullAttempt) -> Value {
    let context = attempt.context();
    json!({
        "preflight_at":attempt.preflight_at().seconds(),
        "dispatch_at":attempt.dispatch_at().seconds(),
        "item":{"instance_id":clean(context.item().as_str()), "item_hash":context.item_hash().value()},
        "destination":{"character_id":clean(context.owner().as_str()), "normal_bucket":context.destination_bucket().value()},
        "definition_version":clean(context.definition_version().as_str()),
        "revision":context.revision().value(),
    })
}
fn location(value: &ItemLocation) -> Value {
    match value {
        ItemLocation::Character(owner) => {
            json!({"kind":"character", "character_id":clean(owner.as_str())})
        }
        ItemLocation::Postmaster(owner) => {
            json!({"kind":"postmaster", "character_id":clean(owner.as_str())})
        }
        ItemLocation::Vault => json!({"kind":"vault"}),
        // Other's free text is not public location evidence; never echo it.
        ItemLocation::Other(_) => json!({"kind":"other"}),
    }
}
fn receipt(value: &InstancedPullOutcome) -> Value {
    match value {
        InstancedPullOutcome::Acknowledged => json!({"kind":"acknowledged"}),
        InstancedPullOutcome::Uncertain => json!({"kind":"uncertain"}),
        InstancedPullOutcome::Transient => json!({"kind":"transient"}),
        InstancedPullOutcome::AuthenticationRefused => json!({"kind":"authentication_refused"}),
        InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest) => {
            json!({"kind":"rejected", "reason":"invalid_request"})
        }
    }
}
fn infrastructure(error: &AppError, stage: &str, kind: &str) -> Value {
    // Do not use Display: even a value-free error type can carry argument details.
    json!({"stage":stage, "kind":kind, "category":category_kind(error.category())})
}
fn planning_failure(error: &InstancedPullPlanningError, stage: &str) -> Value {
    match error {
        InstancedPullPlanningError::Infrastructure(error) => {
            infrastructure(error, stage, "infrastructure")
        }
        InstancedPullPlanningError::Refused(InstancedPullRefusal::Capacity(reason)) => {
            json!({"stage":stage, "kind":"eligibility_refused", "reason":"capacity", "capacity_reason":capacity_reason(*reason)})
        }
        InstancedPullPlanningError::Refused(reason) => {
            json!({"stage":stage, "kind":"eligibility_refused", "reason":refusal_kind(*reason)})
        }
    }
}
fn observation_failure(error: &PullObservationFailure) -> Value {
    match error {
        PullObservationFailure::Clock(error) => infrastructure(error, "observation", "clock"),
        PullObservationFailure::Refresh(error) => infrastructure(error, "observation", "refresh"),
        PullObservationFailure::BackwardsClock => {
            json!({"stage":"observation", "kind":"backwards_clock"})
        }
        PullObservationFailure::SkippedFresh => {
            json!({"stage":"observation", "kind":"skipped_fresh"})
        }
    }
}
// Exhaustive closed mappings intentionally reject newly added domain variants at
// compile time instead of serializing debug names or arbitrary provider strings.
macro_rules! kinds {
    ($function:ident, $ty:ident, {$($variant:ident => $text:literal),+ $(,)?}) => {
        fn $function(value: $ty) -> &'static str {
            match value { $($ty::$variant => $text),+ }
        }
    };
}
kinds!(disposition_kind, PullDisposition, {
    Verified => "verified", NotYetObserved => "not_yet_observed",
    Uncertain => "uncertain", Refused => "refused"
});
kinds!(arrival_kind, PullArrival, {
    VerifiedArrival => "verified_arrival", MembershipMismatch => "membership_mismatch",
    DefinitionVersionDrift => "definition_version_drift", TargetMissing => "target_missing",
    HashDrift => "hash_drift", ObservationsUnavailable => "observations_unavailable",
    StillInPostmaster => "still_in_postmaster", WrongCharacterOwner => "wrong_character_owner",
    WrongActualBucket => "wrong_actual_bucket", Elsewhere => "elsewhere", UnknownLocation => "unknown_location"
});
fn capacity_reason(value: CapacityRefusal) -> Value {
    let kind = match value {
        CapacityRefusal::MissingDestinationBucket => "missing_destination_bucket",
        CapacityRefusal::UnsupportedDestinationBucket => "unsupported_destination_bucket",
        CapacityRefusal::ObservationsUnavailable => "observations_unavailable",
        CapacityRefusal::DefinitionVersionMismatch => "definition_version_mismatch",
        CapacityRefusal::DestinationStackObserved => "destination_stack_observed",
        CapacityRefusal::ArithmeticOverflow => "arithmetic_overflow",
        CapacityRefusal::AtCapacity { occupied, capacity } => {
            return json!({"kind":"at_capacity", "occupied":occupied, "capacity":capacity});
        }
    };
    json!({"kind":kind})
}
fn refusal_kind(value: InstancedPullRefusal) -> &'static str {
    match value {
        InstancedPullRefusal::MembershipMismatch => "membership_mismatch",
        InstancedPullRefusal::TargetMissing => "target_missing",
        InstancedPullRefusal::TargetAmbiguous => "target_ambiguous",
        InstancedPullRefusal::NotInPostmaster => "not_in_postmaster",
        InstancedPullRefusal::OwnerNotOwned => "owner_not_owned",
        InstancedPullRefusal::DefinitionMissing => "definition_missing",
        InstancedPullRefusal::DefinitionAmbiguous => "definition_ambiguous",
        InstancedPullRefusal::DefinitionVersionMismatch => "definition_version_mismatch",
        InstancedPullRefusal::SideEffects => "side_effects",
        InstancedPullRefusal::SideEffectsUnknown => "side_effects_unknown",
        InstancedPullRefusal::NonInstanced => "non_instanced",
        InstancedPullRefusal::InstanceKindUnknown => "instance_kind_unknown",
        InstancedPullRefusal::MissingBucket => "missing_bucket",
        InstancedPullRefusal::UnsupportedBucket => "unsupported_bucket",
        InstancedPullRefusal::PolicyVersionMismatch => "policy_version_mismatch",
        InstancedPullRefusal::PolicyArtifactMismatch => "policy_artifact_mismatch",
        InstancedPullRefusal::Capacity(_) => "capacity",
    }
}
fn pre_dispatch_kind(value: &PreDispatchError) -> &'static str {
    match value {
        PreDispatchError::Eligibility(_) => "eligibility",
        PreDispatchError::Expired => "expired",
        PreDispatchError::BackwardsClock => "backwards_clock",
        PreDispatchError::IdentityDrift => "identity_drift",
        PreDispatchError::DefinitionVersionDrift => "definition_version_drift",
        PreDispatchError::RevisionDrift => "revision_drift",
        PreDispatchError::FingerprintDrift => "fingerprint_drift",
    }
}
kinds!(category_kind, ErrorCategory, {
    AuthorizationRequired => "authorization_required", ReauthorizationRequired => "reauthorization_required",
    AlreadyAuthorizing => "already_authorizing", AlreadyAuthorized => "already_authorized",
    AuthorizationDenied => "authorization_denied", CsrfStateMismatch => "csrf_state_mismatch",
    CredentialStoreUnavailable => "credential_store_unavailable", ProviderAuthentication => "provider_authentication",
    ProviderRateLimit => "provider_rate_limit", ProviderUnavailable => "provider_unavailable",
    ProviderValidation => "provider_validation", DefinitionsUnavailable => "definitions_unavailable",
    UnknownStatName => "unknown_stat_name", UnknownSearchName => "unknown_search_name",
    AmbiguousSearchName => "ambiguous_search_name", StaleState => "stale_state",
    AmbiguousMatch => "ambiguous_match", UnsupportedOperation => "unsupported_operation",
    PartialFailure => "partial_failure", VerificationFailed => "verification_failed",
    LocalStorage => "local_storage", ConfigurationError => "configuration_error",
    CallbackTimeout => "callback_timeout", ClockOverflow => "clock_overflow", PlanIdExhausted => "plan_id_exhausted"
});

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_app::test_support::{
        DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
        QueuedInstancedPullGateway, QueuedManifestGateway, QueuedProfileGateway,
    };
    use destiny_app::*;

    const BUCKET: i64 = 1498876634;
    const FRESHNESS: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 60 };
    use std::{cell::RefCell, rc::Rc};

    // Test-only shared access observes the existing fake after service ownership
    // transfer; it introduces no production store or synchronization mechanism.
    struct Profile(Rc<RefCell<QueuedProfileGateway>>);
    impl ProfileGateway for Profile {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.0.borrow_mut().resolve_membership()
        }
        fn load_inventory(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.borrow_mut().load_inventory(membership)
        }
        fn load_inventory_with_observations_scoped(
            &mut self,
            membership: &DestinyMembership,
            reason: ProfileSyncReason,
        ) -> Result<ProfileAcquisition, AppError> {
            self.0
                .borrow_mut()
                .load_inventory_with_observations_scoped(membership, reason)
        }
    }
    type Service = InstancedPullService<
        Profile,
        InMemoryInventoryRepository,
        InMemoryDefinitionRepository,
        QueuedManifestGateway,
        FakeClock,
        DeterministicPlanIds,
    >;
    #[derive(Default)]
    struct Delay(usize);
    impl RetryDelay for Delay {
        fn wait(&mut self, _: std::time::Duration) {
            self.0 += 1;
        }
    }
    fn membership() -> DestinyMembership {
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap()
    }
    fn input(location: Option<ItemLocation>, checked_at: u64) -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("123"),
            membership_type: MembershipType::new(3),
            definition_version: DefinitionVersion::new("v1"),
            checked_at: Timestamp::new(checked_at),
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
            items: location
                .into_iter()
                .map(|location| ItemInstance {
                    instance_id: ItemInstanceId::new("1"),
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
                })
                .collect(),
            stacks: vec![],
            plug_set_entries: vec![],
            profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        }
    }
    fn postmaster() -> ItemLocation {
        ItemLocation::Postmaster(CharacterId::new("2"))
    }
    fn arrival() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("2"))
    }
    fn queue(gateway: &mut QueuedProfileGateway, input: InventorySnapshotInput, available: bool) {
        let rows = available.then(|| {
            input
                .items
                .iter()
                .map(|item| {
                    InstancedBucketObservation::new(
                        item.instance_id.clone(),
                        BucketHash::new(if matches!(item.location, ItemLocation::Postmaster(_)) {
                            215593132
                        } else {
                            BUCKET
                        }),
                        item.location.clone(),
                    )
                    .unwrap()
                })
                .collect()
        });
        gateway.queue_membership(Ok(membership()));
        gateway.queue_acquisition(Ok(ProfileAcquisition::new(input, rows).unwrap()));
    }
    fn armed(
        posts: Vec<(InventorySnapshotInput, bool)>,
        preflight: InventorySnapshotInput,
    ) -> (
        Service,
        InstancedPullPlan,
        Rc<RefCell<QueuedProfileGateway>>,
    ) {
        let mut gateway = QueuedProfileGateway::new();
        queue(&mut gateway, input(Some(postmaster()), 100), true);
        queue(&mut gateway, preflight, true);
        for (input, available) in posts {
            queue(&mut gateway, input, available);
        }
        let policy = EquipmentCapacityPolicy::new(
            DefinitionVersion::new("v1"),
            "artifact",
            [
                1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954,
                1585787867,
            ]
            .map(|b| (BucketHash::new(b), 10))
            .to_vec(),
        )
        .unwrap();
        let mut manifest = QueuedManifestGateway::new();
        for _ in 0..2 {
            manifest.queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                DefinitionVersion::new("v1"),
                "artifact",
            )
            .unwrap()));
        }
        let definition = ItemDefinition {
            hash: ItemHash::new(7),
            bucket_hash: Some(BucketHash::new(BUCKET)),
            does_postmaster_pull_have_side_effects: Some(false),
            is_instance_item: Some(true),
            max_stack_size: None,
            stack_unique_label: None,
            name: "NOT_FOR_OUTPUT".into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: vec![],
            version: DefinitionVersion::new("v1"),
        };
        let audit = Rc::new(RefCell::new(gateway));
        let mut service = InstancedPullService::new(
            ProfileSyncService::new(Profile(audit.clone()), InMemoryInventoryRepository::new()),
            InMemoryDefinitionRepository::new(DefinitionVersion::new("v1"), vec![definition]),
            manifest,
            policy,
            FakeClock::fixed(Timestamp::new(100)),
            DeterministicPlanIds::starting_at(1),
        );
        let plan = service
            .plan(
                &InstancedPullPlanRequest {
                    membership: membership(),
                    item: ItemInstanceId::new("1"),
                },
                PlanLifetime::new(60).unwrap(),
                FRESHNESS,
            )
            .unwrap();
        (service, plan, audit)
    }
    fn execute(
        posts: Vec<(InventorySnapshotInput, bool)>,
        receipt: InstancedPullOutcome,
    ) -> InstancedPullExecutionOutcome {
        let (mut service, plan, audit) = armed(posts, input(Some(postmaster()), 100));
        let mut gateway = QueuedInstancedPullGateway::new();
        gateway.queue(Ok(receipt));
        let outcome = service
            .execute_verified(plan, &mut gateway, &mut Delay::default(), FRESHNESS)
            .unwrap();
        assert_eq!(gateway.calls(), 1);
        assert_eq!(
            audit
                .borrow()
                .requested_reasons()
                .iter()
                .filter(|reason| **reason == ProfileSyncReason::PostAction)
                .count(),
            // Exhausted membership queues fail before inventory acquisition.
            outcome
                .observations()
                .iter()
                .filter(|attempt| attempt.observation().is_some())
                .count()
        );
        outcome
    }
    #[test]
    fn instanced_pull_verified_maps_public_service_evidence() {
        let outcome = execute(
            vec![(input(Some(arrival()), 100), true)],
            InstancedPullOutcome::Acknowledged,
        );
        assert_eq!(outcome.disposition(), PullDisposition::Verified);
        let output = InstancedPullOutput::from_outcome(&outcome);
        assert!(output.is_success());
        assert_eq!(output.json()["disposition"], "verified");
        assert_eq!(output.json()["receipt"], json!({"kind":"acknowledged"}));
        assert_eq!(
            output.json()["observations"][0]["classification"],
            "verified_arrival"
        );
        let value = output.json();
        assert_eq!(
            value["dispatch"],
            json!({
                "preflight_at":100, "dispatch_at":100,
                "item":{"instance_id":"1", "item_hash":7},
                "destination":{"character_id":"2", "normal_bucket":BUCKET},
                "definition_version":"v1", "revision":outcome.context().revision().value(),
            })
        );
        assert_eq!(
            value["observations"][0]["target"],
            json!({
                "instance_id":"1", "item_hash":7,
                "location":{"kind":"character", "character_id":"2"}, "runtime_bucket":BUCKET,
            })
        );
        assert_eq!(
            value["observations"][0]["publication"],
            json!({
                "checked_at":100, "definition_version":"v1", "source":"profile",
                "dataset":"profile_inventory",
                "revision":outcome.observations()[0].observation().unwrap().metadata().revision.value(),
            })
        );
        assert_eq!(value["plan_id"], "plan-1");
        assert!(value["failure"].is_null());
        assert!(value["observations"][0]["failure"].is_null());
        assert_eq!(
            serde_json::from_str::<Value>(&value.to_string()).unwrap(),
            value
        );
        for forbidden in [
            "NOT_FOR_OUTPUT",
            "membership_id",
            "characters",
            "plugs",
            "perks",
            "stats",
        ] {
            assert!(!value.to_string().contains(forbidden));
        }
    }
    #[test]
    fn instanced_pull_ack_pending_and_uncertain_are_not_success() {
        for (receipt, expected) in [
            (
                InstancedPullOutcome::Acknowledged,
                PullDisposition::NotYetObserved,
            ),
            (InstancedPullOutcome::Uncertain, PullDisposition::Uncertain),
        ] {
            let outcome = execute(
                (0..3)
                    .map(|_| (input(Some(postmaster()), 100), true))
                    .collect(),
                receipt,
            );
            let output = InstancedPullOutput::from_outcome(&outcome);
            assert_eq!(output.disposition(), expected);
            assert!(!output.is_success());
            let value = output.json();
            assert_eq!(value["disposition"], disposition_kind(expected));
            assert_eq!(value["observations"].as_array().unwrap().len(), 3);
            for attempt in value["observations"].as_array().unwrap() {
                assert_eq!(attempt["classification"], "still_in_postmaster");
                assert_eq!(attempt["target"]["location"]["kind"], "postmaster");
                assert_eq!(attempt["target"]["runtime_bucket"], 215593132);
            }
        }
    }
    #[test]
    fn instanced_pull_exact_receipts_are_independent_of_arrival() {
        for (receipt, expected_receipt, expected_disposition) in [
            (
                InstancedPullOutcome::Acknowledged,
                json!({"kind":"acknowledged"}),
                PullDisposition::Verified,
            ),
            (
                InstancedPullOutcome::Uncertain,
                json!({"kind":"uncertain"}),
                PullDisposition::Verified,
            ),
            (
                InstancedPullOutcome::Transient,
                json!({"kind":"transient"}),
                PullDisposition::Refused,
            ),
            (
                InstancedPullOutcome::AuthenticationRefused,
                json!({"kind":"authentication_refused"}),
                PullDisposition::Refused,
            ),
            (
                InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest),
                json!({"kind":"rejected", "reason":"invalid_request"}),
                PullDisposition::Refused,
            ),
        ] {
            let outcome = execute(vec![(input(Some(arrival()), 100), true)], receipt);
            let output = InstancedPullOutput::from_outcome(&outcome);
            assert_eq!(output.disposition(), expected_disposition);
            assert_eq!(
                output.is_success(),
                expected_disposition == PullDisposition::Verified
            );
            assert_eq!(output.json()["receipt"], expected_receipt);
            assert_eq!(
                output.json()["observations"][0]["classification"],
                "verified_arrival"
            );
        }
    }
    #[test]
    fn instanced_pull_timestamp_failure_coexists_with_arrival() {
        let outcome = execute(
            vec![(input(Some(arrival()), 99), true)],
            InstancedPullOutcome::Acknowledged,
        );
        let output = InstancedPullOutput::from_outcome(&outcome);
        assert_eq!(output.disposition(), PullDisposition::Uncertain);
        assert!(!output.is_success());
        let value = output.json();
        let last = value.get("observations").unwrap().get(0).unwrap();
        assert_eq!(last["classification"], "verified_arrival");
        assert_eq!(
            last["failure"],
            json!({"stage":"observation", "kind":"backwards_clock"})
        );
        assert_eq!(last["publication"]["checked_at"], 99);
        assert_eq!(last["checked_at"], 100);
        assert_eq!(last["target"]["runtime_bucket"], BUCKET);
    }
    #[test]
    fn instanced_pull_preserves_order_and_optional_bucket_evidence() {
        let outcome = execute(
            vec![
                (input(Some(arrival()), 100), false),
                (input(Some(postmaster()), 100), true),
                (input(Some(arrival()), 100), true),
            ],
            InstancedPullOutcome::Acknowledged,
        );
        // Equal timestamps deliberately prove attempts are not collapsed or sorted.
        let output = InstancedPullOutput::from_outcome(&outcome);
        let value = output.json();
        assert_eq!(
            value["observations"][0]["classification"],
            "observations_unavailable"
        );
        assert_eq!(value["observations"][0]["actual_buckets_available"], false);
        assert!(value["observations"][0]["target"]["runtime_bucket"].is_null());
        assert_eq!(
            value["observations"][1]["classification"],
            "still_in_postmaster"
        );
        assert_eq!(
            value["observations"][2]["classification"],
            "verified_arrival"
        );
        assert_eq!(value["observations"].as_array().unwrap().len(), 3);
        assert!(output.is_success());
    }
    #[test]
    fn instanced_pull_earlier_refresh_failure_is_not_hidden_by_later_arrival() {
        let (mut service, plan, audit) = armed(vec![], input(Some(postmaster()), 100));
        {
            let mut profile = audit.borrow_mut();
            profile.queue_membership(Ok(membership()));
            profile.queue_acquisition(Err(AppError::invalid_argument(
                "test",
                "PRIVATE_PROVIDER_BODY",
            )));
            queue(&mut profile, input(Some(arrival()), 100), true);
        }
        let mut gateway = QueuedInstancedPullGateway::new();
        gateway.queue(Ok(InstancedPullOutcome::Acknowledged));
        let mut delay = Delay::default();
        let outcome = service
            .execute_verified(plan, &mut gateway, &mut delay, FRESHNESS)
            .unwrap();
        let output = InstancedPullOutput::from_outcome(&outcome);
        assert!(output.is_success());
        assert_eq!(gateway.calls(), 1);
        assert_eq!(delay.0, 1);
        let value = output.json();
        assert_eq!(value["observations"].as_array().unwrap().len(), 2);
        assert!(value["observations"][0]["classification"].is_null());
        assert_eq!(
            value["observations"][0]["failure"],
            json!({
                "stage":"observation", "kind":"refresh", "category":"provider_validation",
            })
        );
        assert!(value["observations"][0]["publication"].is_null());
        assert_eq!(
            value["observations"][1]["classification"],
            "verified_arrival"
        );
        assert!(value["observations"][1]["failure"].is_null());
        assert!(!value.to_string().contains("PRIVATE_PROVIDER_BODY"));
    }
    #[test]
    fn instanced_pull_missing_target_and_failed_refresh_have_honest_nulls() {
        let missing = execute(
            (0..3).map(|_| (input(None, 100), true)).collect(),
            InstancedPullOutcome::Acknowledged,
        );
        let value = InstancedPullOutput::from_outcome(&missing).json();
        assert_eq!(value["disposition"], "uncertain");
        assert_eq!(value["observations"][0]["classification"], "target_missing");
        assert!(value["observations"][0]["target"].is_null());
        assert!(value["observations"][0]["failure"].is_null());
        let failed = execute(vec![], InstancedPullOutcome::Uncertain);
        let value = InstancedPullOutput::from_outcome(&failed).json();
        assert_eq!(value["observations"].as_array().unwrap().len(), 3);
        for attempt in value["observations"].as_array().unwrap() {
            assert!(attempt["classification"].is_null());
            assert!(attempt["publication"].is_null());
            assert!(attempt["target"].is_null());
            assert!(attempt["actual_buckets_available"].is_null());
            assert_eq!(
                attempt["failure"],
                json!({"stage":"observation", "kind":"refresh", "category":"provider_unavailable"})
            );
        }
    }
    #[test]
    fn instanced_pull_predispatch_and_gateway_failure_have_no_receipt_or_observations() {
        for gateway_failure in [false, true] {
            let preflight = input(
                if gateway_failure {
                    Some(postmaster())
                } else {
                    None
                },
                100,
            );
            let (mut service, plan, audit) = armed(vec![], preflight);
            let mut gateway = QueuedInstancedPullGateway::new();
            gateway.queue(Err(AppError::invalid_argument(
                "test",
                "PRIVATE_PROVIDER_BODY",
            )));
            let mut delay = Delay::default();
            let failure = service
                .execute_verified(plan, &mut gateway, &mut delay, FRESHNESS)
                .unwrap_err();
            let output = InstancedPullOutput::execution_failed(&failure);
            assert!(!output.is_success());
            let value = output.json();
            assert!(value["receipt"].is_null());
            assert_eq!(value["observations"], json!([]));
            assert_eq!(value["plan_id"], "plan-1");
            assert_eq!(delay.0, 0);
            assert!(
                !audit
                    .borrow()
                    .requested_reasons()
                    .contains(&ProfileSyncReason::PostAction)
            );
            assert_eq!(gateway.calls(), usize::from(gateway_failure));
            assert_eq!(value["dispatch"].is_null(), !gateway_failure);
            assert_eq!(
                value["failure"],
                if gateway_failure {
                    json!({"stage":"gateway", "kind":"infrastructure", "category":"provider_validation"})
                } else {
                    json!({"stage":"pre_dispatch", "kind":"eligibility_refused", "reason":"target_missing"})
                }
            );
            assert!(!value.to_string().contains("PRIVATE_PROVIDER_BODY"));
        }
    }
    #[test]
    fn instanced_pull_public_planning_failure_and_sanitized_categories() {
        let (mut service, _, audit) = armed(vec![], input(Some(arrival()), 100));
        let error = service
            .plan(
                &InstancedPullPlanRequest {
                    membership: membership(),
                    item: ItemInstanceId::new("1"),
                },
                PlanLifetime::new(60).unwrap(),
                FRESHNESS,
            )
            .unwrap_err();
        assert_eq!(
            error,
            InstancedPullPlanningError::Refused(InstancedPullRefusal::NotInPostmaster)
        );
        assert!(
            !audit
                .borrow()
                .requested_reasons()
                .contains(&ProfileSyncReason::PostAction)
        );
        let output = InstancedPullOutput::planning_failed(&error);
        assert!(!output.is_success());
        assert_eq!(
            output.json(),
            json!({
                "kind":"instanced_pull", "disposition":"refused", "plan_id":null, "dispatch":null,
                "receipt":null, "observations":[],
                "failure":{"stage":"planning", "kind":"eligibility_refused", "reason":"not_in_postmaster"},
            })
        );
        let detailed = InstancedPullPlanningError::Infrastructure(AppError::invalid_argument(
            "secret",
            "PRIVATE_PROVIDER_BODY",
        ));
        assert_eq!(
            InstancedPullOutput::planning_failed(&detailed).json()["failure"],
            json!({"stage":"planning", "kind":"infrastructure", "category":"provider_validation"})
        );
        let capacity = InstancedPullPlanningError::Refused(InstancedPullRefusal::Capacity(
            CapacityRefusal::AtCapacity {
                occupied: 10,
                capacity: 10,
            },
        ));
        assert_eq!(
            InstancedPullOutput::planning_failed(&capacity).json()["failure"]["capacity_reason"],
            json!({"kind":"at_capacity", "occupied":10, "capacity":10})
        );
        assert_eq!(
            location(&ItemLocation::Other("PRIVATE_PROVIDER_BODY".into())),
            json!({"kind":"other"})
        );
        assert_eq!(clean("v1\n\0"), "v1??");
    }
}
