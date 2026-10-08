use super::*;
use destiny_app::test_support::leak_sentinels::{API_KEY, RAW_PAYLOAD, SECRETS};

#[test]
fn every_pre_dispatch_guard_has_a_stable_code() {
    use StackPullPreDispatchError::*;
    for (error, kind) in [
        (Expired, "expired"),
        (BackwardsClock, "backwards_clock"),
        (IdentityDrift, "identity_drift"),
        (DefinitionVersionDrift, "definition_version_drift"),
        (RevisionDrift, "revision_drift"),
        (FingerprintDrift, "fingerprint_drift"),
    ] {
        assert_eq!(
            pre_dispatch_failure(&error),
            json!({"stage":"pre_dispatch", "kind":kind})
        );
    }
    let error = Eligibility(StackPullPlanningError::Infrastructure(
        AppError::invalid_argument(API_KEY, RAW_PAYLOAD),
    ));
    let value = pre_dispatch_failure(&error);
    assert_eq!(
        value,
        json!({"stage":"pre_dispatch", "kind":"infrastructure", "category":"provider_validation"})
    );
    for secret in SECRETS {
        assert!(!value.to_string().contains(secret));
    }
}
#[test]
fn every_observation_failure_keeps_its_category_without_error_details() {
    let detailed = || AppError::invalid_argument(API_KEY, RAW_PAYLOAD);
    for (failure, expected) in [
        (
            PullObservationFailure::Clock(detailed()),
            json!({"stage":"observation", "kind":"clock", "category":"provider_validation"}),
        ),
        (
            PullObservationFailure::Refresh(detailed()),
            json!({"stage":"observation", "kind":"refresh", "category":"provider_validation"}),
        ),
        (
            PullObservationFailure::BackwardsClock,
            json!({"stage":"observation", "kind":"backwards_clock"}),
        ),
        (
            PullObservationFailure::SkippedFresh,
            json!({"stage":"observation", "kind":"skipped_fresh"}),
        ),
    ] {
        let value = observation_failure(&failure);
        assert_eq!(value, expected);
        for secret in SECRETS {
            assert!(!value.to_string().contains(secret));
        }
    }
}
#[test]
fn all_classifications_and_dispositions_are_closed_mappings() {
    for (arrival, code) in [
        (StackPullArrival::VerifiedArrival, "verified_arrival"),
        (StackPullArrival::QuantityMismatch, "quantity_mismatch"),
        (StackPullArrival::MembershipMismatch, "membership_mismatch"),
        (
            StackPullArrival::DefinitionVersionDrift,
            "definition_version_drift",
        ),
        (StackPullArrival::ArithmeticOverflow, "arithmetic_overflow"),
    ] {
        assert_eq!(arrival_kind(arrival), code);
    }
    for (disposition, code) in [
        (PullDisposition::Verified, "verified"),
        (PullDisposition::NotYetObserved, "not_yet_observed"),
        (PullDisposition::Uncertain, "uncertain"),
        (PullDisposition::Refused, "refused"),
    ] {
        let output = StackPullOutput {
            disposition,
            value: json!({"disposition":code, "observations":[]}),
        };
        assert_eq!(disposition_kind(disposition), code);
        assert_eq!(
            output.is_success(),
            disposition == PullDisposition::Verified
        );
    }
    assert_eq!(clean("id\n\r\t\0\u{1b}\u{7f}"), "id??????");
}
