//! Shared presentation of anonymous-stack Postmaster pulls, not genuine instances.
//! Callers must serialize local inventory publication and definition use through
//! execution. Published quantities are neither causal proof nor an atomic guarantee.
//! This module makes no live or full-stack readiness claim.
//! Quantities are same-hash totals: destination means the normal bucket in
//! ProfileInventory; profile means all ProfileInventory buckets; postmaster
//! means all Postmaster buckets of the owner character.
use destiny_app::{
    AppError, ErrorCategory, PullDisposition, PullObservationFailure, SourceMarker,
    StackPullArrival, StackPullAttempt, StackPullExecutionFailure, StackPullExecutionFailureReason,
    StackPullExecutionOutcome, StackPullOutcome, StackPullPlanningError, StackPullPlanningRefusal,
    StackPullPreDispatchError, StackPullRefusal, StackPullRejection,
};
use serde_json::{Value, json};

/// One transport-neutral projection; no CLI exit-code or MCP policy.
#[derive(Clone, Debug)]
pub struct StackPullOutput {
    disposition: PullDisposition,
    value: Value,
}
impl StackPullOutput {
    /// Copy the domain disposition independently of receipt and quantity evidence.
    pub fn from_outcome(outcome: &StackPullExecutionOutcome) -> Self {
        let context = outcome.context();
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
                "quantities": {
                    "destination": {"pre": context.pre_destination_quantity(), "post": observation.and_then(|pair| pair.destination_quantity())},
                    "profile": {"pre": context.pre_profile_quantity(), "post": observation.and_then(|pair| pair.profile_quantity())},
                    "postmaster": {"pre": context.pre_postmaster_quantity(), "post": observation.and_then(|pair| pair.postmaster_quantity())},
                },
            })
        }).collect();
        let disposition = outcome.disposition();
        Self {
            disposition,
            value: json!({
                "kind": "stack_pull", "disposition": disposition_kind(disposition),
                "plan_id": clean(outcome.plan_id().as_str()),
                "dispatch": dispatch(outcome.dispatch()),
                "receipt": receipt(outcome.gateway_outcome()),
                "failure": null, "observations": observations,
            }),
        }
    }
    /// No action was admitted; absent action evidence is represented by nulls.
    pub fn planning_failed(error: &StackPullPlanningError) -> Self {
        Self::failed(None, Value::Null, planning_failure(error, "planning"))
    }
    pub fn execution_failed(failure: &StackPullExecutionFailure) -> Self {
        let (dispatch, reason) = match failure.reason() {
            StackPullExecutionFailureReason::PreDispatch(error) => {
                (Value::Null, pre_dispatch_failure(error))
            }
            StackPullExecutionFailureReason::Gateway(attempt) => {
                // Err is pre-dispatch-only under the gateway contract. Retain
                // attempt context/time but never invent a received receipt.
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
                "kind":"stack_pull", "disposition":"refused",
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
    /// Fixed field order and original attempt order, including equal timestamps.
    /// Unknown evidence is printed as null, never as a fabricated zero.
    pub fn render_text(&self) -> String {
        let value = &self.value;
        let mut lines = vec![format!(
            "stack_pull: {}",
            disposition_kind(self.disposition)
        )];
        lines.push(format!("plan_id: {}", text(&value["plan_id"])));
        let dispatch = &value["dispatch"];
        if !dispatch.is_null() {
            for (label, field) in [
                ("item_hash", &dispatch["item"]["item_hash"]),
                ("amount", &dispatch["item"]["amount"]),
                ("owner_character", &dispatch["destination"]["character_id"]),
                (
                    "destination_bucket",
                    &dispatch["destination"]["normal_bucket"],
                ),
                ("preflight_at", &dispatch["preflight_at"]),
                ("dispatch_at", &dispatch["dispatch_at"]),
                ("definition_version", &dispatch["definition_version"]),
                ("revision", &dispatch["revision"]),
            ] {
                lines.push(format!("{label}: {}", text(field)));
            }
            for scope in ["destination", "profile", "postmaster"] {
                lines.push(format!(
                    "pre_{scope}_quantity: {}",
                    text(&dispatch["pre_quantities"][scope])
                ));
            }
        }
        let receipt = &value["receipt"];
        lines.push(format!(
            "receipt: {}",
            if receipt.is_null() {
                "null".into()
            } else {
                text(&receipt["kind"])
            }
        ));
        if !receipt["reason"].is_null() {
            lines.push(format!("receipt_reason: {}", text(&receipt["reason"])));
        }
        lines.push(format!("failure: {}", text(&value["failure"])));
        if let Some(observations) = value["observations"].as_array() {
            for (index, observation) in observations.iter().enumerate() {
                lines.push(format!("observation {}:", index + 1));
                lines.push(format!(
                    "  checked_at: {}",
                    text(&observation["checked_at"])
                ));
                lines.push(format!(
                    "  classification: {}",
                    text(&observation["classification"])
                ));
                for scope in ["destination", "profile", "postmaster"] {
                    let quantity = &observation["quantities"][scope];
                    lines.push(format!(
                        "  {scope}_quantity: {} -> {}",
                        text(&quantity["pre"]),
                        text(&quantity["post"])
                    ));
                }
                lines.push(format!(
                    "  publication: {}",
                    text(&observation["publication"])
                ));
                lines.push(format!("  failure: {}", text(&observation["failure"])));
            }
        }
        lines.join("\n")
    }
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
fn text(value: &Value) -> String {
    match value.as_str() {
        Some(value) => clean(value),
        None => value.to_string(),
    }
}
fn dispatch(attempt: &StackPullAttempt) -> Value {
    let context = attempt.context();
    let request = context.request();
    json!({
        "preflight_at": attempt.preflight_at().seconds(),
        "dispatch_at": attempt.dispatch_at().seconds(),
        "item": {"item_hash":request.item_hash().value(), "amount":request.quantity()},
        "destination": {"character_id":clean(request.character().as_str()), "normal_bucket":context.destination_bucket().value()},
        "definition_version":clean(context.definition_version().as_str()),
        "revision":context.revision().value(),
        "pre_quantities": {"destination":context.pre_destination_quantity(), "profile":context.pre_profile_quantity(), "postmaster":context.pre_postmaster_quantity()},
    })
}
fn receipt(value: &StackPullOutcome) -> Value {
    match value {
        StackPullOutcome::Acknowledged => json!({"kind":"acknowledged"}),
        StackPullOutcome::Uncertain => json!({"kind":"uncertain"}),
        StackPullOutcome::Transient => json!({"kind":"transient"}),
        StackPullOutcome::AuthenticationRefused => json!({"kind":"authentication_refused"}),
        StackPullOutcome::Rejected(StackPullRejection::InvalidRequest) => {
            json!({"kind":"rejected", "reason":"invalid_request"})
        }
    }
}
fn infrastructure(error: &AppError, stage: &str, kind: &str) -> Value {
    // Never Display/Debug errors: arbitrary provider details are not evidence.
    json!({"stage":stage, "kind":kind, "category":category_kind(error.category())})
}
fn planning_failure(error: &StackPullPlanningError, stage: &str) -> Value {
    match error {
        StackPullPlanningError::Infrastructure(error) => {
            infrastructure(error, stage, "infrastructure")
        }
        StackPullPlanningError::Refused(StackPullPlanningRefusal::Capacity(reason)) => {
            json!({"stage":stage, "kind":"eligibility_refused", "reason":"capacity", "capacity_reason":{"kind":capacity_kind(*reason)}})
        }
        StackPullPlanningError::Refused(reason) => {
            json!({"stage":stage, "kind":"eligibility_refused", "reason":refusal_kind(*reason)})
        }
    }
}
fn pre_dispatch_failure(error: &StackPullPreDispatchError) -> Value {
    let kind = match error {
        StackPullPreDispatchError::Eligibility(error) => {
            return planning_failure(error, "pre_dispatch");
        }
        StackPullPreDispatchError::Expired => "expired",
        StackPullPreDispatchError::BackwardsClock => "backwards_clock",
        StackPullPreDispatchError::IdentityDrift => "identity_drift",
        StackPullPreDispatchError::DefinitionVersionDrift => "definition_version_drift",
        StackPullPreDispatchError::RevisionDrift => "revision_drift",
        StackPullPreDispatchError::FingerprintDrift => "fingerprint_drift",
    };
    json!({"stage":"pre_dispatch", "kind":kind})
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
// Exhaustive closed mappings intentionally reject new variants at compile time,
// rather than serializing debug names or untrusted provider strings.
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
kinds!(arrival_kind, StackPullArrival, {
    VerifiedArrival => "verified_arrival", QuantityMismatch => "quantity_mismatch",
    MembershipMismatch => "membership_mismatch", DefinitionVersionDrift => "definition_version_drift",
    ArithmeticOverflow => "arithmetic_overflow"
});
kinds!(capacity_kind, StackPullRefusal, {
    NoPostmasterStack => "no_postmaster_stack", ExceedsSmallestPostmasterStack => "exceeds_smallest_postmaster_stack",
    UnknownCapacity => "unknown_capacity", UnknownOccupancy => "unknown_occupancy",
    UnknownExistingQuantity => "unknown_existing_quantity", UnknownMaxStackSize => "unknown_max_stack_size",
    UnknownUniqueness => "unknown_uniqueness", UnknownAccountWide => "unknown_account_wide",
    UnknownSideEffects => "unknown_side_effects", PullHasSideEffects => "pull_has_side_effects",
    InvalidCapacityMetadata => "invalid_capacity_metadata", ArithmeticOverflow => "arithmetic_overflow",
    InsufficientCapacity => "insufficient_capacity"
});
fn refusal_kind(value: StackPullPlanningRefusal) -> &'static str {
    match value {
        StackPullPlanningRefusal::MembershipMismatch => "membership_mismatch",
        StackPullPlanningRefusal::OwnerNotOwned => "owner_not_owned",
        StackPullPlanningRefusal::DefinitionMissing => "definition_missing",
        StackPullPlanningRefusal::DefinitionAmbiguous => "definition_ambiguous",
        StackPullPlanningRefusal::DefinitionVersionMismatch => "definition_version_mismatch",
        StackPullPlanningRefusal::SideEffects => "side_effects",
        StackPullPlanningRefusal::SideEffectsUnknown => "side_effects_unknown",
        StackPullPlanningRefusal::Instanced => "instanced",
        StackPullPlanningRefusal::InstanceKindUnknown => "instance_kind_unknown",
        StackPullPlanningRefusal::MissingBucket => "missing_bucket",
        StackPullPlanningRefusal::UnsupportedBucket => "unsupported_bucket",
        StackPullPlanningRefusal::PolicyVersionMismatch => "policy_version_mismatch",
        StackPullPlanningRefusal::PolicyArtifactMismatch => "policy_artifact_mismatch",
        StackPullPlanningRefusal::Capacity(_) => "capacity",
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
mod tests;
