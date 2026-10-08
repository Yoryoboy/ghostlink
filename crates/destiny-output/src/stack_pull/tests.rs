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
fn local_state_failure_is_sanitized_and_dispositions_are_closed() {
    let value = local_state_failure(&AppError::invalid_argument(API_KEY, RAW_PAYLOAD));
    assert_eq!(
        value["message"],
        "local state could not be updated after the action; do not blindly retry"
    );
    for secret in SECRETS {
        assert!(!value.to_string().contains(secret));
    }
    for (disposition, code) in [
        (PullDisposition::Applied, "applied"),
        (PullDisposition::Uncertain, "uncertain"),
        (PullDisposition::Refused, "refused"),
        (PullDisposition::LocalStateFailed, "local_state_failed"),
    ] {
        let output = StackPullOutput {
            disposition,
            value: json!({"disposition":code}),
        };
        assert_eq!(disposition_kind(disposition), code);
        assert_eq!(output.is_success(), disposition == PullDisposition::Applied);
    }
    assert_eq!(clean("id\n\r\t\0\u{1b}\u{7f}"), "id??????");
}
