//! Credential-safe, single-call official InsertSocketPlugFree adapter.
//!
//! Requires MoveEquipDestinyItems (no Advanced Write Action token). Bungie only
//! accepts free, reversible plugs here, for a character in orbit, a social space
//! or offline, and asks for 0.5 s between insertions: the gateway enforces that
//! spacing itself and never retries. Sequencing belongs to the caller.

use crate::transfer::{HttpTransferTransport, ensure_unexpired, is_canonical_identifier};
use destiny_core::{
    AppError, Clock, CredentialSource, ErrorCategory, InsertPlugGateway, InsertPlugOutcome,
    InsertPlugRejection, InsertPlugRequest, StaticCredentialSource,
};
use serde_json::{Value, json};
use std::{fmt, time::Duration};

pub use crate::transfer::{
    TransferApiKey as InsertPlugApiKey, TransferBearerToken as InsertPlugBearerToken,
    TransferHttpRequest as InsertPlugHttpRequest, TransferHttpResponse as InsertPlugHttpResponse,
    TransferTransportError as InsertPlugTransportError,
};

pub const INSERT_PLUG_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/InsertSocketPlugFree/";
pub const INSERT_PLUG_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;
/// Bungie: "Wait at least 0.5s between actions" (bungie-api-ts 5.10.0 `insertSocketPlugFree`).
pub const INSERT_PLUG_MIN_INTERVAL: Duration = Duration::from_millis(500);

pub fn is_allowed_insert_plug_url(url: &str) -> bool {
    url == INSERT_PLUG_ENDPOINT
}

pub trait InsertPlugTransport {
    fn post(
        &mut self,
        request: &InsertPlugHttpRequest<'_>,
    ) -> Result<InsertPlugHttpResponse, InsertPlugTransportError>;
}

/// Hardened transport with an InsertSocketPlugFree-only pre-secret allowlist.
pub struct HttpInsertPlugTransport(HttpTransferTransport);
impl HttpInsertPlugTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new().map(Self)
    }
}
impl fmt::Debug for HttpInsertPlugTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpInsertPlugTransport")
            .finish_non_exhaustive()
    }
}
impl InsertPlugTransport for HttpInsertPlugTransport {
    fn post(
        &mut self,
        request: &InsertPlugHttpRequest<'_>,
    ) -> Result<InsertPlugHttpResponse, InsertPlugTransportError> {
        if !is_allowed_insert_plug_url(request.url()) {
            return Err(InsertPlugTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieInsertPlugGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: InsertPlugApiKey,
    min_interval: Duration,
    last: Option<std::time::Instant>,
}
impl<T, C, S> BungieInsertPlugGateway<T, C, S> {
    /// No spacing; production compositions use [`Self::paced`].
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: InsertPlugApiKey,
    ) -> Result<Self, AppError> {
        Ok(Self {
            transport,
            clock,
            source,
            api_key,
            min_interval: Duration::ZERO,
            last: None,
        })
    }
    /// Waits so that consecutive insertions are at least [`INSERT_PLUG_MIN_INTERVAL`] apart.
    pub fn paced(self) -> Self {
        Self {
            min_interval: INSERT_PLUG_MIN_INTERVAL,
            ..self
        }
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }
}
impl<T, C, S> fmt::Debug for BungieInsertPlugGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieInsertPlugGateway")
            .finish_non_exhaustive()
    }
}
impl<T: InsertPlugTransport, C: Clock, S: CredentialSource> InsertPlugGateway
    for BungieInsertPlugGateway<T, C, S>
{
    fn insert_plug(&mut self, request: &InsertPlugRequest) -> Result<InsertPlugOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| InsertPlugBearerToken::new(token.to_owned()))?;
        let body = build_body(request)?;
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        if let Some(wait) = self
            .last
            .and_then(|at| self.min_interval.checked_sub(at.elapsed()))
        {
            std::thread::sleep(wait);
        }
        self.last = Some(std::time::Instant::now());
        let http = InsertPlugHttpRequest::new(
            INSERT_PLUG_ENDPOINT,
            body,
            Some(&self.api_key),
            Some(&bearer),
        );
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(InsertPlugTransportError::Throttled(seconds)) => {
                return Err(AppError::bungie_rate_limit(seconds));
            }
            Err(InsertPlugTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(InsertPlugTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(InsertPlugTransportError::Indeterminate) => {
                return Ok(InsertPlugOutcome::Uncertain);
            }
        };
        classify_response(&mut response)
    }
}

/// bungie-api-ts `DestinyInsertPlugsFreeActionRequest`; `socketArrayType` 0 is `Default`
/// (DIM `advanced-write-actions.ts:196-208`).
fn build_body(request: &InsertPlugRequest) -> Result<String, AppError> {
    let invalid = || AppError::new(ErrorCategory::ProviderValidation);
    let membership =
        i32::try_from(request.membership().membership_type().value()).map_err(|_| invalid())?;
    if !is_canonical_identifier(request.item().as_str())
        || !is_canonical_identifier(request.character().as_str())
    {
        return Err(invalid());
    }
    Ok(json!({
        "plug": {
            "socketIndex": request.socket_index(),
            "socketArrayType": 0,
            "plugItemHash": request.plug().value(),
        },
        "itemId": request.item().as_str(),
        "characterId": request.character().as_str(),
        "membershipType": membership,
    })
    .to_string())
}

fn classify_response(response: &mut InsertPlugHttpResponse) -> Result<InsertPlugOutcome, AppError> {
    match response.status() {
        200 => {}
        401 | 403 => return Ok(InsertPlugOutcome::AuthenticationRefused),
        429 => return Ok(InsertPlugOutcome::Transient),
        500..=599 => return Ok(InsertPlugOutcome::Uncertain),
        400..=499 => {
            return Ok(InsertPlugOutcome::Rejected(
                InsertPlugRejection::InvalidRequest,
            ));
        }
        _ => return Ok(InsertPlugOutcome::Uncertain),
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return Ok(InsertPlugOutcome::Uncertain);
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return Ok(InsertPlugOutcome::Uncertain);
    };
    match value.get("ErrorCode").and_then(Value::as_u64) {
        Some(1) => Ok(InsertPlugOutcome::Inserted),
        Some(code) => Ok(crate::bungie_errors::rejection(code)
            .map_or(InsertPlugOutcome::Uncertain, |reason| {
                InsertPlugOutcome::Rejected(InsertPlugRejection::Provider(reason))
            })),
        None => Ok(InsertPlugOutcome::Uncertain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, CharacterId, CredentialMaterial,
        DestinyMembership, ItemHash, ItemInstanceId, MembershipId, MembershipType, Timestamp,
    };
    use std::io::Cursor;

    fn request() -> InsertPlugRequest {
        InsertPlugRequest::new(
            DestinyMembership::new(
                MembershipType::new(3),
                MembershipId::new("4611686018484639825"),
            )
            .unwrap(),
            ItemInstanceId::new("6917529027641081856"),
            CharacterId::new("2305843009260670444"),
            7,
            ItemHash::new(2_272_984_671),
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
        responses: Vec<Result<InsertPlugHttpResponse, InsertPlugTransportError>>,
        calls: usize,
    }
    impl InsertPlugTransport for FakeTransport {
        fn post(
            &mut self,
            http: &InsertPlugHttpRequest<'_>,
        ) -> Result<InsertPlugHttpResponse, InsertPlugTransportError> {
            self.calls += 1;
            assert_eq!(http.url(), INSERT_PLUG_ENDPOINT);
            assert_eq!(
                serde_json::from_str::<Value>(http.body()).unwrap(),
                json!({
                    "plug": {"socketIndex": 7, "socketArrayType": 0, "plugItemHash": 2272984671_i64},
                    "itemId": "6917529027641081856",
                    "characterId": "2305843009260670444",
                    "membershipType": 3
                })
            );
            assert!(http.api_key().is_some());
            assert!(http.bearer().is_some());
            self.responses.remove(0)
        }
    }
    fn gateway(
        responses: Vec<Result<InsertPlugHttpResponse, InsertPlugTransportError>>,
    ) -> BungieInsertPlugGateway<FakeTransport, TestClock> {
        BungieInsertPlugGateway::new(
            FakeTransport {
                responses,
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
            InsertPlugApiKey::new("key-canary").unwrap(),
        )
        .unwrap()
    }
    fn response(status: u16, body: Value) -> InsertPlugHttpResponse {
        let bytes = body.to_string().into_bytes();
        InsertPlugHttpResponse::new(
            status,
            Some(bytes.len() as u64),
            Some("application/json".into()),
            Box::new(Cursor::new(bytes)),
        )
    }
    #[test]
    fn exact_url_body_and_success_without_retry() {
        let mut gateway = gateway(vec![Ok(response(200, json!({"ErrorCode":1})))]);
        assert_eq!(
            gateway.insert_plug(&request()).unwrap(),
            InsertPlugOutcome::Inserted
        );
        assert_eq!(gateway.transport().calls, 1);
    }
    #[test]
    fn numeric_action_rejections() {
        crate::bungie_errors::assert_action_codes(|value| {
            match classify_response(&mut response(200, value)).unwrap() {
                InsertPlugOutcome::Rejected(InsertPlugRejection::Provider(reason)) => Some(reason),
                InsertPlugOutcome::Uncertain => None,
                other => panic!("unexpected {other:?}"),
            }
        });
    }
    #[test]
    fn http_status_policy() {
        for status in [401, 403, 429, 500, 503, 400, 404, 302] {
            let result = gateway(vec![Ok(response(status, json!({"ErrorCode":1})))])
                .insert_plug(&request())
                .unwrap();
            assert_eq!(
                result,
                match status {
                    401 | 403 => InsertPlugOutcome::AuthenticationRefused,
                    429 => InsertPlugOutcome::Transient,
                    400 | 404 => InsertPlugOutcome::Rejected(InsertPlugRejection::InvalidRequest),
                    _ => InsertPlugOutcome::Uncertain,
                },
                "{status}"
            );
        }
    }
    #[test]
    fn malformed_envelopes_and_transport_failures() {
        for body in [
            json!({}),
            json!({"ErrorCode":"1"}),
            json!({"ErrorCode":999}),
            json!([]),
        ] {
            assert_eq!(
                gateway(vec![Ok(response(200, body))])
                    .insert_plug(&request())
                    .unwrap(),
                InsertPlugOutcome::Uncertain
            );
        }
        for (error, category) in [
            (
                InsertPlugTransportError::NotDispatched,
                Some(ErrorCategory::ProviderUnavailable),
            ),
            (
                InsertPlugTransportError::Configuration,
                Some(ErrorCategory::ConfigurationError),
            ),
            (InsertPlugTransportError::Indeterminate, None),
        ] {
            let result = gateway(vec![Err(error)]).insert_plug(&request());
            match category {
                Some(category) => assert_eq!(result.unwrap_err().category(), category),
                None => assert_eq!(result.unwrap(), InsertPlugOutcome::Uncertain),
            }
        }
    }
    #[test]
    fn allowlist_refuses_other_actions_before_credentials() {
        assert!(is_allowed_insert_plug_url(INSERT_PLUG_ENDPOINT));
        let mut transport = HttpInsertPlugTransport::new().unwrap();
        let key = InsertPlugApiKey::new("key-canary").unwrap();
        let token = InsertPlugBearerToken::new("token-canary").unwrap();
        for url in [
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/InsertSocketPlug/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItem/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/InsertSocketPlugFree/?x=1",
            "https://evil.example/Platform/Destiny2/Actions/Items/InsertSocketPlugFree/",
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/InsertSocketPlugFree/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/InsertSocketPlugFree",
        ] {
            assert!(!is_allowed_insert_plug_url(url));
            assert_eq!(
                transport
                    .post(&InsertPlugHttpRequest::new(
                        url,
                        "{}".into(),
                        Some(&key),
                        Some(&token)
                    ))
                    .unwrap_err(),
                InsertPlugTransportError::Configuration
            );
        }
        assert!(!crate::transfer::is_allowed_transfer_url(
            INSERT_PLUG_ENDPOINT
        ));
    }
    #[test]
    fn invalid_requests_and_expired_credentials_never_dispatch() {
        let valid = request();
        for (item, character, plug) in [("", "123", 1), ("123", "", 1), ("123", "123", 0)] {
            assert_eq!(
                InsertPlugRequest::new(
                    valid.membership().clone(),
                    ItemInstanceId::new(item),
                    CharacterId::new(character),
                    0,
                    ItemHash::new(plug),
                )
                .unwrap_err()
                .category(),
                ErrorCategory::ProviderValidation
            );
        }
        let invalid = InsertPlugRequest::new(
            valid.membership().clone(),
            ItemInstanceId::new("01"),
            CharacterId::new("123"),
            0,
            ItemHash::new(1),
        )
        .unwrap();
        let mut gateway = gateway(vec![Ok(response(200, json!({"ErrorCode":1})))]);
        assert_eq!(
            gateway.insert_plug(&invalid).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        gateway.source = StaticCredentialSource::new(AccessCredential {
            material: CredentialMaterial::from_bytes(b"token-canary".to_vec()),
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1),
            },
            refresh_grant: None,
        });
        assert_eq!(
            gateway.insert_plug(&valid).unwrap_err().category(),
            ErrorCategory::ProviderAuthentication
        );
        assert_eq!(gateway.transport().calls, 0);
    }
    #[test]
    fn consecutive_insertions_are_spaced() {
        let mut gateway = gateway(vec![
            Ok(response(200, json!({"ErrorCode":1}))),
            Ok(response(200, json!({"ErrorCode":1}))),
        ]);
        gateway.min_interval = Duration::from_millis(60);
        let start = std::time::Instant::now();
        gateway.insert_plug(&request()).unwrap();
        gateway.insert_plug(&request()).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(60));
        assert_eq!(INSERT_PLUG_MIN_INTERVAL, Duration::from_millis(500));
    }
    #[test]
    fn debug_is_redacted() {
        let text = format!("{:?} {:?}", request(), gateway(vec![]));
        for canary in [
            "4611686018484639825",
            "6917529027641081856",
            "2305843009260670444",
        ] {
            assert!(!text.contains(canary));
        }
    }
}
