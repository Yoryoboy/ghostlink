//! Sanitized acknowledgement-based presentation of genuine-instance Postmaster pulls.
use destiny_app::{
    AppError, CapacityRefusal, ErrorCategory, InstancedPullExecutionFailure,
    InstancedPullExecutionFailureReason, InstancedPullExecutionOutcome, InstancedPullOutcome,
    InstancedPullPlanningError, InstancedPullRefusal, InstancedPullRejection, PreDispatchError,
    PullAttempt, PullDisposition,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct InstancedPullOutput {
    disposition: PullDisposition,
    value: Value,
}
impl InstancedPullOutput {
    pub fn from_outcome(outcome: &InstancedPullExecutionOutcome) -> Self {
        let disposition = outcome.disposition();
        Self {
            disposition,
            value: json!({
                "kind":"instanced_pull", "disposition":disposition_kind(disposition),
                "local_state":if disposition == PullDisposition::Applied { "updated" } else { "refresh_pending" },
                "plan_id":clean(outcome.plan_id().as_str()),
                "dispatch":dispatch(outcome.dispatch()), "receipt":receipt(outcome.gateway_outcome()),
                "failure":outcome.local_state_error().map(local_state_failure),
            }),
        }
    }
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
                let reason = match attempt.gateway_result() {
                    Err(error) => infrastructure(error, "gateway", "infrastructure"),
                    Ok(_) => json!({"stage":"gateway", "kind":"evidence_unavailable"}),
                };
                (dispatch(attempt), reason)
            }
        };
        if let Some(error) = failure.local_state_error() {
            return Self {
                disposition: PullDisposition::LocalStateFailed,
                value: json!({"kind":"instanced_pull", "disposition":"local_state_failed", "local_state":"refresh_pending",
                    "plan_id":clean(failure.plan_id().as_str()), "dispatch":dispatch,
                    "receipt":null, "failure":local_state_failure(error)}),
            };
        }
        Self::failed(Some(failure.plan_id().as_str()), dispatch, reason)
    }
    fn failed(plan_id: Option<&str>, dispatch: Value, failure: Value) -> Self {
        Self {
            disposition: PullDisposition::Refused,
            value: json!({"kind":"instanced_pull", "disposition":"refused", "local_state":"unchanged",
                "plan_id":plan_id.map(clean), "dispatch":dispatch, "receipt":null, "failure":failure}),
        }
    }
    pub fn disposition(&self) -> PullDisposition {
        self.disposition
    }
    pub fn is_success(&self) -> bool {
        self.disposition == PullDisposition::Applied
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
        "preflight_at":attempt.preflight_at().seconds(), "dispatch_at":attempt.dispatch_at().seconds(),
        "item":{"instance_id":clean(context.item().as_str()), "item_hash":context.item_hash().value()},
        "destination":{"character_id":clean(context.owner().as_str()), "normal_bucket":context.destination_bucket().value()},
        "definition_version":clean(context.definition_version().as_str()), "revision":context.revision().value(),
    })
}
fn receipt(value: &InstancedPullOutcome) -> Value {
    match value {
        InstancedPullOutcome::Acknowledged => json!({"kind":"acknowledged"}),
        InstancedPullOutcome::Uncertain => json!({"kind":"uncertain"}),
        InstancedPullOutcome::Transient => json!({"kind":"transient"}),
        InstancedPullOutcome::AuthenticationRefused => json!({"kind":"authentication_refused"}),
        InstancedPullOutcome::Rejected(InstancedPullRejection::Provider(reason)) => {
            json!({"kind":"rejected", "reason":reason.reason(), "message":reason.message()})
        }
        InstancedPullOutcome::Rejected(InstancedPullRejection::DestinationFull) => {
            json!({"kind":"rejected", "reason":"destination_full", "message":"destination full; free a slot and retry"})
        }
        InstancedPullOutcome::Rejected(InstancedPullRejection::InvalidRequest) => {
            json!({"kind":"rejected", "reason":"invalid_request"})
        }
    }
}
fn infrastructure(error: &AppError, stage: &str, kind: &str) -> Value {
    json!({"stage":stage, "kind":kind, "category":category_kind(error.category())})
}
fn local_state_failure(error: &AppError) -> Value {
    json!({"stage":"local_state", "kind":"invalidation_failed", "category":category_kind(error.category()),
        "message":"local state could not be updated after the action; do not blindly retry"})
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
macro_rules! kinds {
    ($function:ident, $ty:ident, {$($variant:ident => $text:literal),+ $(,)?}) => {
        fn $function(value: $ty) -> &'static str { match value { $($ty::$variant => $text),+ } }
    };
}
kinds!(disposition_kind, PullDisposition, {
    Applied => "applied", Uncertain => "uncertain", Refused => "refused", LocalStateFailed => "local_state_failed"
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
    use InstancedPullRefusal::*;
    match value {
        MembershipMismatch => "membership_mismatch",
        TargetMissing => "target_missing",
        TargetAmbiguous => "target_ambiguous",
        NotInPostmaster => "not_in_postmaster",
        OwnerNotOwned => "owner_not_owned",
        DefinitionMissing => "definition_missing",
        DefinitionAmbiguous => "definition_ambiguous",
        DefinitionVersionMismatch => "definition_version_mismatch",
        SideEffects => "side_effects",
        SideEffectsUnknown => "side_effects_unknown",
        NonInstanced => "non_instanced",
        InstanceKindUnknown => "instance_kind_unknown",
        MissingBucket => "missing_bucket",
        UnsupportedBucket => "unsupported_bucket",
        PolicyVersionMismatch => "policy_version_mismatch",
        PolicyArtifactMismatch => "policy_artifact_mismatch",
        Capacity(_) => "capacity",
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
    use destiny_app::test_support::*;
    use destiny_app::*;

    fn execute(receipt: InstancedPullOutcome) -> InstancedPullExecutionOutcome {
        let version = DefinitionVersion::new("v1");
        let membership =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap();
        let input = InventorySnapshotInput {
            membership_id: membership.membership_id().clone(),
            membership_type: MembershipType::new(3),
            definition_version: version.clone(),
            checked_at: Timestamp::new(100),
            source: SourceMarker::Profile,
            characters: vec![Character {
                id: CharacterId::new("2"),
                name: Some("NOT_FOR_OUTPUT".into()),
                class: None,
                race: None,
                gender: None,
                light: None,
                date_last_played: None,
            }],
            items: vec![ItemInstance {
                instance_id: ItemInstanceId::new("1"),
                item_hash: ItemHash::new(7),
                location: ItemLocation::Postmaster(CharacterId::new("2")),
                quantity: 1,
                equipped: false,
                locked: false,
                power: None,
                stats: vec![],
                plugs: vec![],
                available_plugs: vec![],
                perks: vec![],
            }],
            stacks: vec![],
            plug_set_entries: vec![],
            profile_currencies_coverage: ProfileCurrenciesCoverage::NotObserved,
        };
        let mut profile = QueuedProfileGateway::new();
        let mut manifest = QueuedManifestGateway::new();
        for _ in 0..2 {
            profile.queue_membership(Ok(membership.clone()));
            profile.queue_acquisition(Ok(ProfileAcquisition::new(
                input.clone(),
                Some(vec![
                    InstancedBucketObservation::new(
                        ItemInstanceId::new("1"),
                        BucketHash::new(215593132),
                        ItemLocation::Postmaster(CharacterId::new("2")),
                    )
                    .unwrap(),
                ]),
            )
            .unwrap()));
            manifest.queue_capacity_evidence(Ok(EquipmentCapacityEvidence::new(
                version.clone(),
                "artifact",
            )
            .unwrap()));
        }
        let definition = ItemDefinition {
            hash: ItemHash::new(7),
            bucket_hash: Some(BucketHash::new(1498876634)),
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
            version: version.clone(),
        };
        let policy = EquipmentCapacityPolicy::new(
            version.clone(),
            "artifact",
            [
                1498876634, 2465295065, 953998645, 3448274439, 3551918588, 14239492, 20886954,
                1585787867,
            ]
            .map(|bucket| (BucketHash::new(bucket), 10))
            .to_vec(),
        )
        .unwrap();
        let mut repository = InMemoryInventoryRepository::new();
        repository.replace_current(input).unwrap();
        let mut service = InstancedPullService::new(
            ProfileSyncService::new(profile, repository),
            InMemoryDefinitionRepository::new(version, vec![definition]),
            manifest,
            policy,
            FakeClock::fixed(Timestamp::new(100)),
            DeterministicPlanIds::starting_at(1),
        );
        let freshness = FreshnessPolicy { ttl_seconds: 60 };
        let plan = service
            .plan(
                &InstancedPullPlanRequest {
                    membership,
                    item: ItemInstanceId::new("1"),
                },
                PlanLifetime::MAX,
                freshness,
            )
            .unwrap();
        let mut gateway = QueuedInstancedPullGateway::new();
        gateway.queue(Ok(receipt));
        let outcome = service.execute(plan, &mut gateway, freshness).unwrap();
        assert_eq!(gateway.calls(), 1);
        outcome
    }

    #[test]
    fn applied_maps_exact_dispatch_without_private_fields() {
        let outcome = execute(InstancedPullOutcome::Acknowledged);
        let output = InstancedPullOutput::from_outcome(&outcome);
        assert!(output.is_success());
        let value = output.json();
        assert_eq!(value["disposition"], "applied");
        assert_eq!(value["local_state"], "updated");
        assert_eq!(value["receipt"], json!({"kind":"acknowledged"}));
        assert_eq!(
            value["dispatch"],
            json!({
                "preflight_at":100, "dispatch_at":100,
                "item":{"instance_id":"1", "item_hash":7},
                "destination":{"character_id":"2", "normal_bucket":1498876634},
                "definition_version":"v1", "revision":outcome.context().revision().value(),
            })
        );
        assert_eq!(value["plan_id"], "plan-1");
        assert!(value["failure"].is_null());
        assert!(value.get("observations").is_none());
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
    fn exact_receipts_are_independent_of_profile_arrival() {
        for (receipt, expected, disposition) in [
            (
                InstancedPullOutcome::Acknowledged,
                json!({"kind":"acknowledged"}),
                PullDisposition::Applied,
            ),
            (
                InstancedPullOutcome::Uncertain,
                json!({"kind":"uncertain"}),
                PullDisposition::Uncertain,
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
            let outcome = execute(receipt);
            let output = InstancedPullOutput::from_outcome(&outcome);
            assert_eq!(output.disposition(), disposition);
            assert_eq!(output.is_success(), disposition == PullDisposition::Applied);
            assert_eq!(output.json()["receipt"], expected);
            assert_eq!(
                output.json()["local_state"],
                if disposition == PullDisposition::Applied {
                    "updated"
                } else {
                    "refresh_pending"
                }
            );
            assert!(output.json().get("observations").is_none());
        }
    }

    #[test]
    fn preflight_refusals_and_capacity_retain_stable_codes() {
        use InstancedPullRefusal::*;
        for (reason, code) in [
            (MembershipMismatch, "membership_mismatch"),
            (TargetMissing, "target_missing"),
            (TargetAmbiguous, "target_ambiguous"),
            (NotInPostmaster, "not_in_postmaster"),
            (OwnerNotOwned, "owner_not_owned"),
            (DefinitionMissing, "definition_missing"),
            (DefinitionAmbiguous, "definition_ambiguous"),
            (DefinitionVersionMismatch, "definition_version_mismatch"),
            (SideEffects, "side_effects"),
            (SideEffectsUnknown, "side_effects_unknown"),
            (NonInstanced, "non_instanced"),
            (InstanceKindUnknown, "instance_kind_unknown"),
            (MissingBucket, "missing_bucket"),
            (UnsupportedBucket, "unsupported_bucket"),
            (PolicyVersionMismatch, "policy_version_mismatch"),
            (PolicyArtifactMismatch, "policy_artifact_mismatch"),
        ] {
            let output =
                InstancedPullOutput::planning_failed(&InstancedPullPlanningError::Refused(reason));
            assert!(!output.is_success());
            assert_eq!(output.json()["failure"]["reason"], code);
            assert_eq!(output.json()["local_state"], "unchanged");
            assert!(output.json()["receipt"].is_null());
        }
        let output = InstancedPullOutput::planning_failed(&InstancedPullPlanningError::Refused(
            Capacity(CapacityRefusal::AtCapacity {
                occupied: 10,
                capacity: 10,
            }),
        ));
        assert_eq!(
            output.json()["failure"]["capacity_reason"],
            json!({"kind":"at_capacity", "occupied":10, "capacity":10})
        );
    }

    #[test]
    fn pre_dispatch_guard_codes_are_closed() {
        use PreDispatchError::*;
        for (reason, code) in [
            (Expired, "expired"),
            (BackwardsClock, "backwards_clock"),
            (IdentityDrift, "identity_drift"),
            (DefinitionVersionDrift, "definition_version_drift"),
            (RevisionDrift, "revision_drift"),
            (FingerprintDrift, "fingerprint_drift"),
        ] {
            assert_eq!(pre_dispatch_kind(&reason), code);
        }
    }

    #[test]
    fn planning_and_local_storage_failures_never_echo_error_details() {
        assert_eq!(clean("v1\n\0"), "v1??");
        assert_eq!(clean("id\n\r\t\0\u{1b}\u{7f}"), "id??????");
        let error = AppError::invalid_argument("secret-key", "raw-payload");
        let value = InstancedPullOutput::planning_failed(
            &InstancedPullPlanningError::Infrastructure(error.clone()),
        )
        .json();
        assert_eq!(value["failure"]["category"], "provider_validation");
        assert!(value["receipt"].is_null());
        assert!(!value.to_string().contains("secret-key"));
        assert!(
            !local_state_failure(&error)
                .to_string()
                .contains("raw-payload")
        );
    }
}
