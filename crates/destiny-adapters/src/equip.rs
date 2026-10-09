//! Credential-safe, single-call Bungie EquipItem adapter.
//!
//! Requires MoveEquipDestinyItems. Bungie requires social space, orbit, or
//! offline state and documents 0.1 seconds between actions per user. This
//! single-call gateway never retries; sequencing and pacing belong to the caller.

use crate::transfer::{HttpTransferTransport, ensure_unexpired, is_canonical_identifier};
use destiny_core::{
    AppError, Clock, CredentialSource, EquipGateway, EquipItemRequest, EquipOutcome,
    EquipRejection, ErrorCategory, StaticCredentialSource,
};
use serde_json::{Value, json};
use std::fmt;

// Share credential validation, redaction, bounded response types and hardened
// HTTP machinery, not the transfer allowlist or its outcome classifications.
pub use crate::transfer::{
    TransferApiKey as EquipApiKey, TransferBearerToken as EquipBearerToken,
    TransferHttpRequest as EquipHttpRequest, TransferHttpResponse as EquipHttpResponse,
    TransferTransportError as EquipTransportError,
};

pub const EQUIP_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/";
pub const EQUIP_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;

pub fn is_allowed_equip_url(url: &str) -> bool {
    url == EQUIP_ENDPOINT
}

pub trait EquipTransport {
    fn post(
        &mut self,
        request: &EquipHttpRequest<'_>,
    ) -> Result<EquipHttpResponse, EquipTransportError>;
}

/// Hardened transport with an EquipItem-only pre-secret allowlist.
pub struct HttpEquipTransport(HttpTransferTransport);
impl HttpEquipTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new().map(Self)
    }
}
impl fmt::Debug for HttpEquipTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpEquipTransport").finish_non_exhaustive()
    }
}
impl EquipTransport for HttpEquipTransport {
    fn post(
        &mut self,
        request: &EquipHttpRequest<'_>,
    ) -> Result<EquipHttpResponse, EquipTransportError> {
        if !is_allowed_equip_url(request.url()) {
            return Err(EquipTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieEquipGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: EquipApiKey,
}
impl<T, C, S> BungieEquipGateway<T, C, S> {
    pub fn new(transport: T, clock: C, source: S, api_key: EquipApiKey) -> Result<Self, AppError> {
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
impl<T, C, S> fmt::Debug for BungieEquipGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieEquipGateway").finish_non_exhaustive()
    }
}
impl<T: EquipTransport, C: Clock, S: CredentialSource> EquipGateway
    for BungieEquipGateway<T, C, S>
{
    fn equip(&mut self, request: &EquipItemRequest) -> Result<EquipOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| EquipBearerToken::new(token.to_owned()))?;
        let body = build_body(request)?;
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let http = EquipHttpRequest::new(EQUIP_ENDPOINT, body, Some(&self.api_key), Some(&bearer));
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(EquipTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(EquipTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(EquipTransportError::Indeterminate) => return Ok(EquipOutcome::Uncertain),
        };
        classify_response(&mut response)
    }
}

fn build_body(request: &EquipItemRequest) -> Result<String, AppError> {
    let invalid = || AppError::new(ErrorCategory::ProviderValidation);
    let membership =
        i32::try_from(request.membership().membership_type().value()).map_err(|_| invalid())?;
    if !is_canonical_identifier(request.item().as_str())
        || !is_canonical_identifier(request.character().as_str())
    {
        return Err(invalid());
    }
    Ok(json!({"itemId": request.item().as_str(), "characterId": request.character().as_str(), "membershipType": membership}).to_string())
}

fn classify_response(response: &mut EquipHttpResponse) -> Result<EquipOutcome, AppError> {
    match response.status() {
        200 => {}
        401 | 403 => return Ok(EquipOutcome::AuthenticationRefused),
        429 => return Ok(EquipOutcome::Transient),
        500..=599 => return Ok(EquipOutcome::Uncertain),
        400..=499 => return Ok(EquipOutcome::Rejected(EquipRejection::InvalidRequest)),
        _ => return Ok(EquipOutcome::Uncertain),
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return Ok(EquipOutcome::Uncertain);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return Ok(EquipOutcome::Uncertain);
    };
    match value.get("ErrorCode").and_then(Value::as_u64) {
        Some(1) => Ok(EquipOutcome::Equipped),
        Some(code) => Ok(crate::bungie_errors::rejection(code)
            .map_or(EquipOutcome::Uncertain, |reason| {
                EquipOutcome::Rejected(EquipRejection::Provider(reason))
            })),
        None => Ok(EquipOutcome::Uncertain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, CharacterId, CredentialMaterial,
        DestinyMembership, ItemInstanceId, MembershipId, MembershipType, Timestamp,
    };
    use serde_json::json;
    use std::io::Cursor;

    fn request() -> EquipItemRequest {
        EquipItemRequest::new(
            DestinyMembership::new(
                MembershipType::new(3),
                MembershipId::new("4611686018484639825"),
            )
            .unwrap(),
            ItemInstanceId::new("6917529027641081856"),
            CharacterId::new("2305843009260670444"),
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
        response: Option<Result<EquipHttpResponse, EquipTransportError>>,
        calls: usize,
    }
    impl EquipTransport for FakeTransport {
        fn post(
            &mut self,
            http: &EquipHttpRequest<'_>,
        ) -> Result<EquipHttpResponse, EquipTransportError> {
            self.calls += 1;
            assert_eq!(http.url(), EQUIP_ENDPOINT);
            assert_eq!(
                serde_json::from_str::<Value>(http.body()).unwrap(),
                json!({
                    "itemId": "6917529027641081856", "characterId": "2305843009260670444", "membershipType": 3
                })
            );
            assert!(http.api_key().is_some());
            assert!(http.bearer().is_some());
            self.response.take().unwrap()
        }
    }
    fn gateway(
        response: Result<EquipHttpResponse, EquipTransportError>,
    ) -> BungieEquipGateway<FakeTransport, TestClock> {
        BungieEquipGateway::new(
            FakeTransport {
                response: Some(response),
                calls: 0,
            },
            TestClock,
            StaticCredentialSource::new(AccessCredential {
                refresh_grant: None,
                material: CredentialMaterial::from_bytes(b"token-canary".to_vec()),
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(100),
                },
            }),
            EquipApiKey::new("key-canary").unwrap(),
        )
        .unwrap()
    }
    fn response(status: u16, body: Value) -> EquipHttpResponse {
        let bytes = body.to_string().into_bytes();
        EquipHttpResponse::new(
            status,
            Some(bytes.len() as u64),
            Some("application/json".into()),
            Box::new(Cursor::new(bytes)),
        )
    }
    #[test]
    fn numeric_action_rejections() {
        crate::bungie_errors::assert_action_codes(|value| {
            match classify_response(&mut response(200, value)).unwrap() {
                EquipOutcome::Rejected(EquipRejection::Provider(reason)) => Some(reason),
                EquipOutcome::Uncertain => None,
                other => panic!("unexpected {other:?}"),
            }
        });
    }
    #[test]
    fn exact_url_body_and_success_without_retry() {
        let mut gateway = gateway(Ok(response(200, json!({"ErrorCode":1}))));
        assert_eq!(gateway.equip(&request()).unwrap(), EquipOutcome::Equipped);
        assert_eq!(gateway.transport().calls, 1);
    }
    #[test]
    fn allowlist_refuses_other_actions_before_credentials() {
        assert!(is_allowed_equip_url(EQUIP_ENDPOINT));
        let mut transport = HttpEquipTransport::new().unwrap();
        let key = EquipApiKey::new("key-canary").unwrap();
        let token = EquipBearerToken::new("token-canary").unwrap();
        // Confirm the recorder observes real credential exposure, not just references.
        crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
        crate::transfer::record_request_credentials(&EquipHttpRequest::new(
            EQUIP_ENDPOINT,
            "{}".into(),
            Some(&key),
            Some(&token),
        ));
        crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 2));
        for url in [
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItems/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/TransferItem/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/?x=1",
            "https://evil.example/Platform/Destiny2/Actions/Items/EquipItem/",
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/#x",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/../Items/EquipItem/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/equipitem/",
        ] {
            assert!(!is_allowed_equip_url(url));
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
            assert_eq!(
                transport
                    .post(&EquipHttpRequest::new(
                        url,
                        "{}".into(),
                        Some(&key),
                        Some(&token)
                    ))
                    .unwrap_err(),
                EquipTransportError::Configuration
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 0));
        }
        assert!(!crate::transfer::is_allowed_transfer_url(EQUIP_ENDPOINT));
    }
    #[test]
    fn equip_request_debug_never_prints_untrusted_url_or_body() {
        let key = EquipApiKey::new("KEY_CANARY").unwrap();
        let token = EquipBearerToken::new("TOKEN_CANARY").unwrap();
        for (url, label) in [
            (EQUIP_ENDPOINT, "EquipItem"),
            (crate::lock::LOCK_ENDPOINT, "SetLockState"),
            (
                "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/?access_token=URL_CANARY",
                "<unallowlisted>",
            ),
        ] {
            let request = EquipHttpRequest::new(
                url,
                r#"{"FIELD_CANARY":"VALUE_CANARY"}"#.into(),
                Some(&key),
                Some(&token),
            );
            let rendered = format!("{request:?}");
            for canary in [
                "URL_CANARY",
                "FIELD_CANARY",
                "VALUE_CANARY",
                "KEY_CANARY",
                "TOKEN_CANARY",
            ] {
                assert!(!rendered.contains(canary), "Debug leaked {canary}");
            }
            assert!(!rendered.contains("https://"));
            assert!(rendered.contains(label));
        }
    }

    #[test]
    fn mapped_statuses_and_unknowns() {
        for (code, status) in [
            (1655, "DestinyCanOnlyEquipInGame"),
            (1654, "DestinyAccountMustBeOffline"),
            (1638, "DestinyItemFailedLevelCheck"),
            (1639, "DestinyItemFailedUnlockCheck"),
            (1620, "DestinyCharacterNotFound"),
            (9999, "DestinyItemNotFound"),
        ] {
            let mut gateway = gateway(Ok(response(
                200,
                json!({"ErrorCode":code,"ErrorStatus":status}),
            )));
            assert_eq!(
                gateway.equip(&request()).unwrap(),
                EquipOutcome::Uncertain,
                "{status}"
            );
            assert_eq!(gateway.transport().calls, 1);
        }
    }
    #[test]
    fn http_status_policy() {
        for status in [401, 403, 429, 500, 503, 400, 404, 302] {
            let mut gateway = gateway(Ok(response(status, json!({"ErrorCode":1}))));
            let result = gateway.equip(&request());
            match status {
                // Received refusals preserve dispatch; expired credentials
                // remain ProviderAuthentication errors before dispatch.
                401 | 403 => assert_eq!(result.unwrap(), EquipOutcome::AuthenticationRefused),
                429 => assert_eq!(result.unwrap(), EquipOutcome::Transient),
                400 | 404 => assert_eq!(
                    result.unwrap(),
                    EquipOutcome::Rejected(EquipRejection::InvalidRequest)
                ),
                _ => assert_eq!(result.unwrap(), EquipOutcome::Uncertain),
            }
        }
    }
    #[test]
    fn malformed_envelopes_and_transport_uncertainty() {
        for body in [
            json!({}),
            json!({"ErrorCode":"1"}),
            json!({"ErrorCode":999}),
            json!([]),
        ] {
            assert_eq!(
                gateway(Ok(response(200, body))).equip(&request()).unwrap(),
                EquipOutcome::Uncertain
            );
        }
        for (error, category) in [
            (
                EquipTransportError::NotDispatched,
                Some(ErrorCategory::ProviderUnavailable),
            ),
            (
                EquipTransportError::Configuration,
                Some(ErrorCategory::ConfigurationError),
            ),
            (EquipTransportError::Indeterminate, None),
        ] {
            let result = gateway(Err(error)).equip(&request());
            if let Some(category) = category {
                assert_eq!(result.unwrap_err().category(), category);
            } else {
                assert_eq!(result.unwrap(), EquipOutcome::Uncertain);
            }
        }
    }
    #[test]
    fn invalid_requests_and_expired_credentials_never_dispatch() {
        let valid = request();
        for (item, character) in [("", "123"), ("123", "")] {
            assert_eq!(
                EquipItemRequest::new(
                    valid.membership().clone(),
                    ItemInstanceId::new(item),
                    CharacterId::new(character),
                )
                .unwrap_err()
                .category(),
                ErrorCategory::ProviderValidation
            );
        }
        for (item, character) in [("01", "123"), ("123", "-1"), ("not-an-id", "123")] {
            let invalid = EquipItemRequest::new(
                valid.membership().clone(),
                ItemInstanceId::new(item),
                CharacterId::new(character),
            )
            .unwrap();
            let mut gateway = gateway(Ok(response(200, json!({"ErrorCode":1}))));
            assert_eq!(
                gateway.equip(&invalid).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
            assert_eq!(gateway.transport().calls, 0);
        }
        let mut gateway = gateway(Ok(response(200, json!({"ErrorCode":1}))));
        gateway.source = StaticCredentialSource::new(AccessCredential {
            material: CredentialMaterial::from_bytes(b"token-canary".to_vec()),
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1),
            },
            refresh_grant: None,
        });
        assert_eq!(
            gateway.equip(&valid).unwrap_err().category(),
            ErrorCategory::ProviderAuthentication
        );
        assert_eq!(gateway.transport().calls, 0);
    }

    #[test]
    fn untrustworthy_body_is_uncertain() {
        for (length, media, bytes) in [
            (Some(2), "application/json", b"broken".to_vec()),
            (Some(1), "application/json", b"{}".to_vec()),
            (None, "text/html", b"{\"ErrorCode\":1}".to_vec()),
            (
                None,
                "application/json",
                vec![b' '; EQUIP_RESPONSE_LIMIT + 1],
            ),
        ] {
            let response = EquipHttpResponse::new(
                200,
                length,
                Some(media.into()),
                Box::new(Cursor::new(bytes)),
            );
            assert_eq!(
                gateway(Ok(response)).equip(&request()).unwrap(),
                EquipOutcome::Uncertain
            );
        }
    }

    #[test]
    fn debug_is_redacted() {
        let request = request();
        let key = EquipApiKey::new("key-canary").unwrap();
        let token = EquipBearerToken::new("token-canary").unwrap();
        let http = EquipHttpRequest::new(
            EQUIP_ENDPOINT,
            build_body(&request).unwrap(),
            Some(&key),
            Some(&token),
        );
        let text = format!(
            "{request:?} {http:?} {key:?} {token:?} {:?} {:?}",
            EquipOutcome::Equipped,
            gateway(Ok(response(200, json!({"ErrorCode":1}))))
        );
        for canary in [
            "4611686018484639825",
            "6917529027641081856",
            "2305843009260670444",
            "key-canary",
            "token-canary",
        ] {
            assert!(!text.contains(canary));
        }
    }
}
