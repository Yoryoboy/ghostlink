//! Exact numeric PlatformErrorCodes only; never interpret provider text.
use destiny_core::ProviderRejection as R;
use destiny_core::ports::{ReadFailure, RetryAfterHint};
use destiny_core::{AppError, ErrorCategory};
use serde_json::{Map, Value};
use std::time::Duration;

pub(crate) const fn is_throttle(code: u64) -> bool {
    matches!(code, 35 | 36 | 37 | 54 | 55 | 56 | 57 | 1672)
}

pub(crate) fn throttle_seconds(root: &Map<String, Value>) -> Option<u64> {
    root.get("ThrottleSeconds")
        .and_then(Value::as_u64)
        .filter(|n| *n > 0)
}

/// Read-only classification: action rejection semantics below remain unchanged.
pub(crate) fn read_error(root: &Map<String, Value>) -> Result<(), AppError> {
    let code = root.get("ErrorCode").and_then(Value::as_u64);
    let error = match code {
        Some(1) => return Ok(()),
        Some(code) if is_throttle(code) => {
            AppError::bungie_rate_limit(throttle_seconds(root).unwrap_or(1))
        }
        Some(5) => AppError::provider_message(
            ErrorCategory::ProviderUnavailable,
            "Bungie API is down for maintenance",
        ),
        Some(99 | 22 | 2111 | 2123 | 2124) => AppError::provider_message(
            ErrorCategory::ReauthorizationRequired,
            "Bungie authorization has expired or was revoked; reauthorization is required",
        ),
        Some(2101 | 2102) => AppError::provider_message(
            ErrorCategory::ConfigurationError,
            "Bungie API key is missing or invalid",
        ),
        _ => AppError::new(ErrorCategory::ProviderValidation),
    };
    Err(error)
}

pub(crate) fn check_read_body(bytes: &[u8]) -> Result<(), ReadFailure> {
    // Structural validation remains the decoder's responsibility, including
    // truncated streams and duplicate fields. Only inspect complete envelopes.
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(());
    };
    let Some(root) = value
        .as_object()
        .filter(|root| root.contains_key("ErrorCode"))
    else {
        return Ok(());
    };
    read_error(root).map_err(|error| {
        let hint = if error.category().is_rate_limit() {
            throttle_seconds(root).map_or(RetryAfterHint::Absent, |n| {
                RetryAfterHint::Wait(Duration::from_secs(n))
            })
        } else {
            RetryAfterHint::Absent
        };
        ReadFailure::with_retry_after(error, hint)
    })
}
pub(crate) const fn rejection(code: u64) -> Option<R> {
    Some(match code {
        1623 => R::ItemNotFound,
        1632 => R::AlreadyInInventory,
        1634 | 1671 => R::NotInSocialSpace,
        1640 => R::Unequippable,
        1641 => R::ExoticLimit,
        1642 => R::DestinationFull,
        1648 => R::UniquenessViolation,
        1656 => R::Equipped,
        1660 => R::NotTransferable,
        1663 => R::ActionForbidden,
        1672 => R::Throttled,
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) fn assert_action_codes(mut classify: impl FnMut(serde_json::Value) -> Option<R>) {
    use serde_json::json;
    for (code, reason) in [
        (1623, R::ItemNotFound),
        (1632, R::AlreadyInInventory),
        (1634, R::NotInSocialSpace),
        (1640, R::Unequippable),
        (1641, R::ExoticLimit),
        (1642, R::DestinationFull),
        (1648, R::UniquenessViolation),
        (1656, R::Equipped),
        (1660, R::NotTransferable),
        (1663, R::ActionForbidden),
        (1671, R::NotInSocialSpace),
        (1672, R::Throttled),
    ] {
        for status in ["Success", "DestinyItemNotFound", "untrusted"] {
            assert_eq!(
                classify(
                    json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0,"ErrorStatus":status})
                ),
                Some(reason),
                "code {code}"
            );
        }
    }
    for value in [
        json!({"Response":0,"ErrorCode":999,"ThrottleSeconds":0,"ErrorStatus":"DestinyItemNotFound"}),
        json!({"Response":0,"ThrottleSeconds":0,"ErrorStatus":"DestinyThrottledByGameServer"}),
    ] {
        assert_eq!(classify(value), None);
    }
}

/// What an adapter made of an off-`200` response, for [`assert_status_precedence`].
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StatusProbe {
    Refused(R),
    AuthenticationRefused,
    Transient,
    Uncertain,
}

/// A recognized refusal code in a JSON envelope decides any status except
/// success, authentication and throttle; without one, or behind a non-JSON
/// media type, the status keeps its meaning. `classify` receives the status,
/// the media type and the envelope; `throttle` is the adapter's `429` outcome.
#[cfg(test)]
pub(crate) fn assert_status_precedence(
    throttle: StatusProbe,
    mut classify: impl FnMut(u16, &str, serde_json::Value) -> StatusProbe,
) {
    use serde_json::json;
    const JSON: &str = "application/json";
    let envelope =
        |code: u64| json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0,"ErrorStatus":"x"});
    for status in [500, 503] {
        assert_eq!(
            classify(status, JSON, envelope(1)),
            StatusProbe::Uncertain,
            "status {status} with the success code"
        );
    }
    for status in [400, 404] {
        assert_eq!(
            classify(status, JSON, envelope(1623)),
            StatusProbe::Refused(R::ItemNotFound),
            "status {status}"
        );
    }
    for status in [401, 403] {
        assert_eq!(
            classify(status, JSON, envelope(1623)),
            StatusProbe::AuthenticationRefused,
            "status {status}"
        );
    }
    assert_eq!(classify(429, JSON, envelope(1623)), throttle, "status 429");
    assert_eq!(
        classify(500, "text/html", envelope(1623)),
        StatusProbe::Uncertain,
        "non-JSON media type"
    );
}
