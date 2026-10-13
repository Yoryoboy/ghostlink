//! Credential-safe, single-call Bungie SetLockState adapter.
//!
//! Requires MoveEquipDestinyItems. Bungie documents 0.1 seconds between actions
//! per user. This gateway never retries; sequencing and pacing belong to callers.

use crate::transfer::{HttpTransferTransport, ensure_unexpired, is_canonical_identifier};
use destiny_core::{
    AppError, Clock, CredentialSource, ErrorCategory, LockGateway, LockOutcome, LockRejection,
    SetLockStateRequest, StaticCredentialSource,
};
use serde_json::{Value, json};
use std::fmt;

// Share redacted credentials, bounded responses and hardened HTTP machinery,
// not the transfer allowlist or equip eligibility policy.
pub use crate::transfer::{
    TransferApiKey as LockApiKey, TransferBearerToken as LockBearerToken,
    TransferHttpRequest as LockHttpRequest, TransferHttpResponse as LockHttpResponse,
    TransferTransportError as LockTransportError,
};

pub const LOCK_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/";
pub const LOCK_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;

pub fn is_allowed_lock_url(url: &str) -> bool {
    url == LOCK_ENDPOINT
}

pub trait LockTransport {
    fn post(
        &mut self,
        request: &LockHttpRequest<'_>,
    ) -> Result<LockHttpResponse, LockTransportError>;
}

/// Hardened transport with a SetLockState-only pre-secret allowlist.
pub struct HttpLockTransport(HttpTransferTransport);
impl HttpLockTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new().map(Self)
    }
}
impl fmt::Debug for HttpLockTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpLockTransport").finish_non_exhaustive()
    }
}
impl LockTransport for HttpLockTransport {
    fn post(
        &mut self,
        request: &LockHttpRequest<'_>,
    ) -> Result<LockHttpResponse, LockTransportError> {
        if !is_allowed_lock_url(request.url()) {
            return Err(LockTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieLockGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: LockApiKey,
}
impl<T, C, S> BungieLockGateway<T, C, S> {
    pub fn new(transport: T, clock: C, source: S, api_key: LockApiKey) -> Result<Self, AppError> {
        Ok(Self {
            transport,
            clock,
            source,
            api_key,
        })
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }
    pub fn into_transport(self) -> T {
        self.transport
    }
}
impl<T, C, S> fmt::Debug for BungieLockGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieLockGateway").finish_non_exhaustive()
    }
}
impl<T: LockTransport, C: Clock, S: CredentialSource> LockGateway for BungieLockGateway<T, C, S> {
    fn set_lock_state(&mut self, request: &SetLockStateRequest) -> Result<LockOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| LockBearerToken::new(token.to_owned()))?;
        let body = build_body(request)?;
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let http = LockHttpRequest::new(LOCK_ENDPOINT, body, Some(&self.api_key), Some(&bearer));
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(LockTransportError::Throttled(seconds)) => {
                return Err(AppError::bungie_rate_limit(seconds));
            }
            Err(LockTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(LockTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(LockTransportError::Indeterminate) => return Ok(LockOutcome::Uncertain),
        };
        classify_response(&mut response)
    }
}

fn build_body(request: &SetLockStateRequest) -> Result<String, AppError> {
    let invalid = || AppError::new(ErrorCategory::ProviderValidation);
    let membership =
        i32::try_from(request.membership().membership_type().value()).map_err(|_| invalid())?;
    if !is_canonical_identifier(request.item().as_str())
        || !is_canonical_identifier(request.character().as_str())
    {
        return Err(invalid());
    }
    Ok(json!({"state": request.locked(), "itemId": request.item().as_str(), "characterId": request.character().as_str(), "membershipType": membership}).to_string())
}

fn lock_rejection(reason: destiny_core::ProviderRejection) -> LockOutcome {
    LockOutcome::Rejected(if reason == destiny_core::ProviderRejection::ItemNotFound {
        LockRejection::ItemNotFound
    } else {
        LockRejection::Provider(reason)
    })
}

fn classify_response(response: &mut LockHttpResponse) -> Result<LockOutcome, AppError> {
    if let Some(reason) = response.status_error_rejection() {
        return Ok(lock_rejection(reason));
    }
    match response.status() {
        200 => {}
        401 | 403 => return Ok(LockOutcome::AuthenticationRefused),
        429 => return Ok(LockOutcome::Transient),
        500..=599 => return Ok(LockOutcome::Uncertain),
        400..=499 => return Ok(LockOutcome::Rejected(LockRejection::InvalidRequest)),
        _ => return Ok(LockOutcome::Uncertain),
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return Ok(LockOutcome::Uncertain);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return Ok(LockOutcome::Uncertain);
    };
    match value.get("ErrorCode").and_then(Value::as_u64) {
        Some(1) => Ok(LockOutcome::Applied),
        Some(code) => Ok(
            crate::bungie_errors::rejection(code).map_or(LockOutcome::Uncertain, lock_rejection)
        ),
        None => Ok(LockOutcome::Uncertain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, CharacterId, CredentialMaterial,
        DestinyMembership, ItemInstanceId, MembershipId, MembershipType, Timestamp,
    };
    use std::io::Cursor;

    fn request(locked: bool) -> SetLockStateRequest {
        SetLockStateRequest::new(
            DestinyMembership::new(
                MembershipType::new(3),
                MembershipId::new("4611686018484639825"),
            )
            .unwrap(),
            ItemInstanceId::new("6917529027641081856"),
            CharacterId::new("2305843009260670444"),
            locked,
        )
        .unwrap()
    }
    struct TestClock;
    impl Clock for TestClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            Ok(Timestamp::new(1))
        }
    }
    struct FakeTransport {
        response: Option<Result<LockHttpResponse, LockTransportError>>,
        calls: usize,
        locked: bool,
    }
    impl LockTransport for FakeTransport {
        fn post(
            &mut self,
            http: &LockHttpRequest<'_>,
        ) -> Result<LockHttpResponse, LockTransportError> {
            self.calls += 1;
            assert_eq!(
                http.url(),
                "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/"
            );
            assert!(format!("{http:?}").contains("POST"));
            assert!(format!("{http:?}").contains("SetLockState"));
            assert_eq!(
                serde_json::from_str::<Value>(http.body()).unwrap(),
                json!({
                    "state": self.locked, "itemId": "6917529027641081856",
                    "characterId": "2305843009260670444", "membershipType": 3
                })
            );
            assert!(http.api_key().is_some() && http.bearer().is_some());
            self.response.take().unwrap()
        }
    }
    fn credentials(expires: u64) -> StaticCredentialSource {
        StaticCredentialSource::new(AccessCredential {
            refresh_grant: None,
            material: CredentialMaterial::from_bytes(b"token-canary".to_vec()),
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(expires),
            },
        })
    }
    fn gateway(
        reply: Result<LockHttpResponse, LockTransportError>,
        locked: bool,
    ) -> BungieLockGateway<FakeTransport, TestClock> {
        BungieLockGateway::new(
            FakeTransport {
                response: Some(reply),
                calls: 0,
                locked,
            },
            TestClock,
            credentials(100),
            LockApiKey::new("key-canary").unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn pre_dispatch_throttle_is_definitive_and_not_retried() {
        let mut gateway = gateway(Err(LockTransportError::Throttled(45)), true);
        let error = gateway.set_lock_state(&request(true)).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
        assert!(error.to_string().contains("retry in about 45 seconds"));
        assert_eq!(gateway.transport().calls, 1);
    }
    #[test]
    fn numeric_action_rejections() {
        // Bungie also reports game refusals with HTTP 500; the envelope decides.
        for status in [200, 500, 503] {
            crate::bungie_errors::assert_action_codes(|value| {
                match classify_response(&mut response(status, value)).unwrap() {
                    LockOutcome::Rejected(LockRejection::Provider(reason)) => Some(reason),
                    LockOutcome::Rejected(LockRejection::ItemNotFound) => {
                        Some(destiny_core::ProviderRejection::ItemNotFound)
                    }
                    LockOutcome::Uncertain => None,
                    other => panic!("unexpected {other:?}"),
                }
            });
        }
    }
    #[test]
    fn status_keeps_its_meaning_without_a_recognized_refusal() {
        use crate::bungie_errors::StatusProbe;
        crate::bungie_errors::assert_status_precedence(
            StatusProbe::Transient,
            |status, media, value| {
                let bytes = value.to_string().into_bytes();
                let mut response = LockHttpResponse::new(
                    status,
                    Some(bytes.len() as u64),
                    Some(media.into()),
                    Box::new(std::io::Cursor::new(bytes)),
                );
                match classify_response(&mut response).unwrap() {
                    LockOutcome::Rejected(LockRejection::Provider(reason)) => {
                        StatusProbe::Refused(reason)
                    }
                    LockOutcome::Rejected(LockRejection::ItemNotFound) => {
                        StatusProbe::Refused(destiny_core::ProviderRejection::ItemNotFound)
                    }
                    LockOutcome::AuthenticationRefused => StatusProbe::AuthenticationRefused,
                    LockOutcome::Transient => StatusProbe::Transient,
                    LockOutcome::Uncertain => StatusProbe::Uncertain,
                    other => panic!("unexpected {other:?}"),
                }
            },
        );
    }
    fn response(status: u16, body: Value) -> LockHttpResponse {
        let bytes = body.to_string().into_bytes();
        LockHttpResponse::new(
            status,
            Some(bytes.len() as u64),
            Some("application/json".into()),
            Box::new(Cursor::new(bytes)),
        )
    }
    #[test]
    fn exact_post_body_for_lock_and_unlock_without_retry() {
        for locked in [true, false] {
            let mut gateway = gateway(
                Ok(response(200, json!({"ErrorCode":1,"Response":0}))),
                locked,
            );
            assert_eq!(
                gateway.set_lock_state(&request(locked)).unwrap(),
                LockOutcome::Applied
            );
            assert_eq!(gateway.transport().calls, 1);
        }
    }
    #[test]
    fn allowlist_refuses_other_endpoints_before_secret_exposure() {
        assert!(is_allowed_lock_url(LOCK_ENDPOINT));
        assert!(!crate::transfer::is_allowed_transfer_url(LOCK_ENDPOINT));
        assert!(!crate::equip::is_allowed_equip_url(LOCK_ENDPOINT));
        let mut transport = HttpLockTransport::new().unwrap();
        let key = LockApiKey::new("key-canary").unwrap();
        let bearer = LockBearerToken::new("token-canary").unwrap();
        crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
        crate::transfer::record_request_credentials(&LockHttpRequest::new(
            LOCK_ENDPOINT,
            "{}".into(),
            Some(&key),
            Some(&bearer),
        ));
        crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 2));
        for url in [
            crate::equip::EQUIP_ENDPOINT,
            crate::transfer::TRANSFER_ENDPOINT,
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/?x=1",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/#x",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/../Items/SetLockState/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/setlockstate/",
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/SetLockState/",
            "https://evil.example/Platform/Destiny2/Actions/Items/SetLockState/",
        ] {
            assert!(!is_allowed_lock_url(url));
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
            assert_eq!(
                transport
                    .post(&LockHttpRequest::new(
                        url,
                        "{}".into(),
                        Some(&key),
                        Some(&bearer)
                    ))
                    .unwrap_err(),
                LockTransportError::Configuration
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 0));
        }
    }
    #[test]
    fn sanitized_envelope_mapping() {
        for status in [
            "DestinyCharacterNotInTower",
            "DestinyCannotPerformActionAtThisLocation",
            "DestinyCanOnlyEquipInGame",
            "DestinyAccountMustBeOffline",
            "DestinyItemNotFound",
            "DestinyCharacterNotFound",
            "DestinyThrottledByGameServer",
            "unknown-canary",
        ] {
            let mut gateway = gateway(
                Ok(response(
                    200,
                    json!({"ErrorCode":999,"ErrorStatus":status,"Message":"token-canary"}),
                )),
                true,
            );
            assert_eq!(
                gateway.set_lock_state(&request(true)).unwrap(),
                LockOutcome::Uncertain
            );
            assert_eq!(gateway.transport().calls, 1);
        }
        for body in [
            json!({}),
            json!([]),
            json!({"ErrorCode":"1"}),
            json!({"ErrorCode":999}),
        ] {
            assert_eq!(
                gateway(Ok(response(200, body)), true)
                    .set_lock_state(&request(true))
                    .unwrap(),
                LockOutcome::Uncertain
            );
        }
    }
    #[test]
    fn http_refusals_and_transport_dispatch_boundary() {
        for (status, outcome) in [
            (401, LockOutcome::AuthenticationRefused),
            (403, LockOutcome::AuthenticationRefused),
            (429, LockOutcome::Transient),
            (400, LockOutcome::Rejected(LockRejection::InvalidRequest)),
            (404, LockOutcome::Rejected(LockRejection::InvalidRequest)),
            (500, LockOutcome::Uncertain),
            (503, LockOutcome::Uncertain),
            (302, LockOutcome::Uncertain),
        ] {
            assert_eq!(
                gateway(Ok(response(status, json!({"ErrorCode":1}))), true)
                    .set_lock_state(&request(true))
                    .unwrap(),
                outcome
            );
        }
        for (error, category) in [
            (
                LockTransportError::NotDispatched,
                Some(ErrorCategory::ProviderUnavailable),
            ),
            (
                LockTransportError::Configuration,
                Some(ErrorCategory::ConfigurationError),
            ),
            (LockTransportError::Indeterminate, None),
        ] {
            let mut gateway = gateway(Err(error), true);
            let result = gateway.set_lock_state(&request(true));
            if let Some(category) = category {
                assert_eq!(result.unwrap_err().category(), category);
            } else {
                assert_eq!(result.unwrap(), LockOutcome::Uncertain);
            }
            assert_eq!(gateway.transport().calls, 1);
        }
    }
    #[test]
    fn invalid_identifiers_and_expired_auth_never_dispatch() {
        let valid = request(true);
        for (item, character) in [("", "123"), ("123", "")] {
            assert_eq!(
                SetLockStateRequest::new(
                    valid.membership().clone(),
                    ItemInstanceId::new(item),
                    CharacterId::new(character),
                    true
                )
                .unwrap_err()
                .category(),
                ErrorCategory::ProviderValidation
            );
        }
        for (item, character) in [("01", "123"), ("123", "-1"), ("not-an-id", "123")] {
            let invalid = SetLockStateRequest::new(
                valid.membership().clone(),
                ItemInstanceId::new(item),
                CharacterId::new(character),
                true,
            )
            .unwrap();
            let mut gateway = gateway(Ok(response(200, json!({"ErrorCode":1}))), true);
            assert_eq!(
                gateway.set_lock_state(&invalid).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(gateway.transport().calls, 0);
        }
        let mut gateway = gateway(Ok(response(200, json!({"ErrorCode":1}))), true);
        gateway.source = credentials(1);
        assert_eq!(
            gateway.set_lock_state(&valid).unwrap_err().category(),
            ErrorCategory::ProviderAuthentication
        );
        assert_eq!(gateway.transport().calls, 0);
    }
    #[test]
    fn untrustworthy_bodies_are_uncertain() {
        for (length, media, bytes) in [
            (Some(2), "application/json", b"broken".to_vec()),
            (Some(1), "application/json", b"{}".to_vec()),
            (None, "text/html", b"{\"ErrorCode\":1}".to_vec()),
            (
                None,
                "application/json",
                vec![b' '; LOCK_RESPONSE_LIMIT + 1],
            ),
        ] {
            let mut gateway = gateway(
                Ok(LockHttpResponse::new(
                    200,
                    length,
                    Some(media.into()),
                    Box::new(Cursor::new(bytes)),
                )),
                true,
            );
            assert_eq!(
                gateway.set_lock_state(&request(true)).unwrap(),
                LockOutcome::Uncertain
            );
            assert_eq!(gateway.transport().calls, 1);
        }
    }
    #[test]
    fn debug_and_outcomes_are_redacted() {
        let request = request(true);
        let key = LockApiKey::new("key-canary").unwrap();
        let bearer = LockBearerToken::new("token-canary").unwrap();
        for url in [LOCK_ENDPOINT, "https://evil.example/?token=URL_CANARY"] {
            let http = LockHttpRequest::new(
                url,
                build_body(&request).unwrap(),
                Some(&key),
                Some(&bearer),
            );
            let text = format!(
                "{request:?} {http:?} {key:?} {bearer:?} {:?} {:?}",
                LockOutcome::Applied,
                gateway(Ok(response(200, json!({"ErrorCode":1}))), true)
            );
            for canary in [
                "4611686018484639825",
                "6917529027641081856",
                "2305843009260670444",
                "key-canary",
                "token-canary",
                "URL_CANARY",
                "https://",
            ] {
                assert!(!text.contains(canary));
            }
        }
    }
}
