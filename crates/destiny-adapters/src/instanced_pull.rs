//! Single-attempt, instanced-only PullFromPostmaster adapter.
//!
//! Requires OAuth MoveEquipDestinyItems. Bungie's documented action throttle is
//! 0.1 seconds; pacing across calls belongs to a future orchestrator, not this
//! gateway. There is no sleep, retry, rollback, or global rate guarantee here.
//! Acknowledgement is provider evidence only, not verified recovery. Fresh
//! ownership/location, side-effect eligibility and capacity preflight, followed
//! by post-action observation, remain future service responsibilities. Anonymous
//! stacks and live-provider verification are not supported by this slice.
//!
//! No action-specific numeric refusal guarantees are established. Only a valid
//! standard success envelope on HTTP 200 acknowledges; 401/403 refuse auth.
//! All other received HTTP/provider results (including 429 and 5xx) are uncertain.

use crate::transfer::{HttpTransferTransport, ensure_unexpired};
use destiny_core::{
    AppError, Clock, CredentialSource, ErrorCategory, InstancedPullGateway, InstancedPullOutcome,
    InstancedPullRequest, StaticCredentialSource,
};
use serde::{Deserialize, Deserializer};
use serde_json::json;
use std::{collections::BTreeMap, fmt};

// Reuse only hardened transport, secret redaction and bounded wire primitives;
// the transfer endpoint and its refusal assumptions are intentionally not reused.
pub use crate::transfer::{
    TransferApiKey as InstancedPullApiKey, TransferBearerToken as InstancedPullBearerToken,
    TransferHttpRequest as InstancedPullHttpRequest,
    TransferHttpResponse as InstancedPullHttpResponse,
    TransferTransportError as InstancedPullTransportError,
};

pub const INSTANCED_PULL_ENDPOINT: &str =
    "https://www.bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster/";
/// Shared actual-byte and declared-length cap (8 KiB), before JSON allocation.
pub const INSTANCED_PULL_RESPONSE_LIMIT: usize = crate::transfer::TRANSFER_RESPONSE_LIMIT;

/// Exact bytes only: no alternate origin, explicit port, query or normalization.
pub fn is_allowed_instanced_pull_url(url: &str) -> bool {
    url == INSTANCED_PULL_ENDPOINT
}

pub trait InstancedPullTransport {
    fn post(
        &mut self,
        request: &InstancedPullHttpRequest<'_>,
    ) -> Result<InstancedPullHttpResponse, InstancedPullTransportError>;
}

/// Hardened HTTPS client; action allowlist precedes shared secret/header access.
/// Pull-specific client disables redirects, proxies, decompression and retries,
/// and bounds both connect and total request time.
pub struct HttpInstancedPullTransport(HttpTransferTransport);
impl HttpInstancedPullTransport {
    pub fn new() -> Result<Self, AppError> {
        HttpTransferTransport::new_instanced_pull().map(Self)
    }
}
impl fmt::Debug for HttpInstancedPullTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpInstancedPullTransport")
            .finish_non_exhaustive()
    }
}
impl InstancedPullTransport for HttpInstancedPullTransport {
    fn post(
        &mut self,
        request: &InstancedPullHttpRequest<'_>,
    ) -> Result<InstancedPullHttpResponse, InstancedPullTransportError> {
        if !is_allowed_instanced_pull_url(request.url()) {
            return Err(InstancedPullTransportError::Configuration);
        }
        self.0.post_allowlisted(request)
    }
}

pub struct BungieInstancedPullGateway<T, C, S = StaticCredentialSource> {
    transport: T,
    clock: C,
    source: S,
    api_key: InstancedPullApiKey,
}
impl<T, C, S> BungieInstancedPullGateway<T, C, S> {
    pub fn new(
        transport: T,
        clock: C,
        source: S,
        api_key: InstancedPullApiKey,
    ) -> Result<Self, AppError> {
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
impl<T, C, S> fmt::Debug for BungieInstancedPullGateway<T, C, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BungieInstancedPullGateway")
            .finish_non_exhaustive()
    }
}
impl<T: InstancedPullTransport, C: Clock, S: CredentialSource> InstancedPullGateway
    for BungieInstancedPullGateway<T, C, S>
{
    fn pull(&mut self, request: &InstancedPullRequest) -> Result<InstancedPullOutcome, AppError> {
        let credential = self.source.current()?;
        let bearer = std::str::from_utf8(credential.material.expose_secret())
            .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
            .and_then(|token| InstancedPullBearerToken::new(token.to_owned()))?;
        ensure_unexpired(self.clock.now()?, credential.metadata.expires_at)?;
        let body = build_body(request)?;
        let http = InstancedPullHttpRequest::new(
            INSTANCED_PULL_ENDPOINT,
            body,
            Some(&self.api_key),
            Some(&bearer),
        );
        let mut response = match self.transport.post(&http) {
            Ok(response) => response,
            Err(InstancedPullTransportError::NotDispatched) => {
                return Err(AppError::new(ErrorCategory::ProviderUnavailable));
            }
            Err(InstancedPullTransportError::Configuration) => {
                return Err(AppError::new(ErrorCategory::ConfigurationError));
            }
            Err(InstancedPullTransportError::Indeterminate) => {
                return Ok(InstancedPullOutcome::Uncertain);
            }
        };
        Ok(classify_response(&mut response))
    }
}

fn build_body(request: &InstancedPullRequest) -> Result<String, AppError> {
    let invalid = || AppError::new(ErrorCategory::ProviderValidation);
    let hash = u32::try_from(request.item_hash().value()).map_err(|_| invalid())?;
    let membership =
        i32::try_from(request.membership().membership_type().value()).map_err(|_| invalid())?;
    // The immutable core contract already guarantees canonical positive int64 IDs.
    serde_json::to_string(&json!({
        "itemReferenceHash": hash, "stackSize": request.quantity(),
        "itemId": request.item().as_str(), "characterId": request.character().as_str(),
        "membershipType": membership,
    }))
    .map_err(|_| AppError::new(ErrorCategory::ConfigurationError))
}

// A present optional field must have its documented type, never null. Derived
// struct deserialization also rejects duplicate known fields, unlike Value maps.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Envelope {
    response: i32,
    error_code: i32,
    throttle_seconds: i32,
    #[serde(default, deserialize_with = "present")]
    error_status: Option<String>,
    #[serde(default, deserialize_with = "present")]
    message: Option<String>,
    #[serde(default, deserialize_with = "present")]
    message_data: Option<BTreeMap<String, String>>,
    #[serde(default, deserialize_with = "present")]
    detailed_error_trace: Option<String>,
}

fn classify_response(response: &mut InstancedPullHttpResponse) -> InstancedPullOutcome {
    match response.status() {
        200 => {}
        401 | 403 => return InstancedPullOutcome::AuthenticationRefused,
        _ => return InstancedPullOutcome::Uncertain,
    }
    let Some(body) = response.json_body(response.content_length()) else {
        return InstancedPullOutcome::Uncertain;
    };
    let Ok(envelope) = serde_json::from_slice::<Envelope>(&body) else {
        return InstancedPullOutcome::Uncertain;
    };
    // These fields are validated, not surfaced or interpreted as function-result
    // semantics. In particular Response need not be zero to acknowledge.
    let _ = (
        envelope.response,
        envelope.message,
        envelope.message_data,
        envelope.detailed_error_trace,
    );
    // Bungie Platform DestinyNoRoomInDestination = 1642. There is no local
    // PlatformErrorCodes table; never infer this refusal from provider text.
    if envelope.error_code == 1642 {
        return InstancedPullOutcome::Rejected(
            destiny_core::InstancedPullRejection::DestinationFull,
        );
    }
    if envelope.error_code == 1
        && envelope.throttle_seconds >= 0
        && envelope
            .error_status
            .as_deref()
            .is_none_or(|status| status == "Success")
    {
        InstancedPullOutcome::Acknowledged
    } else {
        InstancedPullOutcome::Uncertain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        AccessCredential, AccessCredentialMetadata, CharacterId, CredentialMaterial,
        DestinyMembership, ItemHash, ItemInstanceId, MembershipId, MembershipType, Timestamp,
    };
    use serde_json::{Value, json};
    use std::{
        cell::Cell,
        io::{self, Cursor, Read},
        rc::Rc,
    };

    const ACK: &str = r#"{"Response":0,"ErrorCode":1,"ThrottleSeconds":0,"ErrorStatus":"Success"}"#;
    fn request(hash: i64, kind: u32) -> InstancedPullRequest {
        InstancedPullRequest::new(
            DestinyMembership::new(MembershipType::new(kind), MembershipId::new("00123456789"))
                .unwrap(),
            ItemInstanceId::new("9223372036854775807"),
            ItemHash::new(hash),
            CharacterId::new("9223372036854775807"),
        )
        .unwrap()
    }
    struct TestClock(Rc<Cell<usize>>);
    impl Clock for TestClock {
        fn now(&self) -> Result<Timestamp, AppError> {
            self.0.set(self.0.get() + 1);
            Ok(Timestamp::new(1))
        }
    }
    struct Source {
        calls: Rc<Cell<usize>>,
        material: Vec<u8>,
        expiry: u64,
        missing: bool,
    }
    impl CredentialSource for Source {
        fn current(&self) -> Result<AccessCredential, AppError> {
            self.calls.set(self.calls.get() + 1);
            if self.missing {
                return Err(AppError::new(ErrorCategory::ProviderAuthentication));
            }
            Ok(AccessCredential {
                refresh_grant: None,
                material: CredentialMaterial::from_bytes(self.material.clone()),
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(self.expiry),
                },
            })
        }
    }
    struct FakeTransport {
        result: Option<Result<InstancedPullHttpResponse, InstancedPullTransportError>>,
        calls: usize,
        body: Option<Value>,
    }
    impl InstancedPullTransport for FakeTransport {
        fn post(
            &mut self,
            http: &InstancedPullHttpRequest<'_>,
        ) -> Result<InstancedPullHttpResponse, InstancedPullTransportError> {
            self.calls += 1;
            assert_eq!(http.url(), INSTANCED_PULL_ENDPOINT);
            crate::transfer::record_request_credentials(http);
            self.body = Some(serde_json::from_str(http.body()).unwrap());
            self.result.take().unwrap()
        }
    }
    fn gateway(
        result: Result<InstancedPullHttpResponse, InstancedPullTransportError>,
    ) -> BungieInstancedPullGateway<FakeTransport, TestClock, Source> {
        BungieInstancedPullGateway::new(
            FakeTransport {
                result: Some(result),
                calls: 0,
                body: None,
            },
            TestClock(Rc::new(Cell::new(0))),
            Source {
                calls: Rc::new(Cell::new(0)),
                material: b"token-canary".to_vec(),
                expiry: 100,
                missing: false,
            },
            InstancedPullApiKey::new("key-canary").unwrap(),
        )
        .unwrap()
    }
    fn response(status: u16, text: &str) -> InstancedPullHttpResponse {
        InstancedPullHttpResponse::new(
            status,
            Some(text.len() as u64),
            Some("application/json".into()),
            Box::new(Cursor::new(text.as_bytes().to_vec())),
        )
    }
    fn assert_outcome(response: InstancedPullHttpResponse, expected: InstancedPullOutcome) {
        let mut g = gateway(Ok(response));
        assert_eq!(g.pull(&request(42, 3)).unwrap(), expected);
        assert_eq!(g.transport().calls, 1);
        assert_eq!(g.source.calls.get(), 1);
        assert_eq!(g.clock.0.get(), 1);
    }
    #[test]
    fn instanced_pull_exact_endpoint_body_and_acknowledgement() {
        assert_eq!(
            INSTANCED_PULL_ENDPOINT,
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster/"
        );
        for (hash, kind) in [(0, 3), (u32::MAX as i64, i32::MAX as u32)] {
            let mut g = gateway(Ok(response(200, ACK)));
            assert_eq!(
                g.pull(&request(hash, kind)).unwrap(),
                InstancedPullOutcome::Acknowledged
            );
            assert_eq!(
                g.transport().body,
                Some(
                    json!({"itemReferenceHash":hash,"stackSize":1,"itemId":"9223372036854775807","characterId":"9223372036854775807","membershipType":kind})
                )
            );
            assert_eq!(g.transport().calls, 1);
            assert_eq!(g.source.calls.get(), 1);
            assert_eq!(g.clock.0.get(), 1);
        }
    }
    #[test]
    fn instanced_pull_unknown_http_and_provider_codes_never_borrow_refusals() {
        for status in [
            201, 204, 301, 302, 307, 308, 400, 404, 409, 429, 500, 503, 599,
        ] {
            assert_outcome(response(status, ACK), InstancedPullOutcome::Uncertain);
        }
        for status in [401, 403] {
            assert_outcome(
                response(status, "not JSON"),
                InstancedPullOutcome::AuthenticationRefused,
            );
        }
        for status in [
            "NoRoomInDestination",
            "NotTransferrable",
            "DestinyThrottledByGameServer",
            "Success",
        ] {
            let body =
                json!({"Response":0,"ErrorCode":999,"ThrottleSeconds":0,"ErrorStatus":status});
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Uncertain,
            );
        }
    }
    #[test]
    fn numeric_no_room_refuses_without_borrowing_provider_text() {
        for status in ["DestinyNoRoomInDestination", "Success", "untrusted"] {
            let body =
                json!({"Response":0,"ErrorCode":1642,"ThrottleSeconds":0,"ErrorStatus":status});
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Rejected(
                    destiny_core::InstancedPullRejection::DestinationFull,
                ),
            );
        }
        for code in [1641, 1643, 999] {
            let body = json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0,"ErrorStatus":"DestinyNoRoomInDestination"});
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Uncertain,
            );
        }
    }

    #[test]
    fn instanced_pull_strict_numeric_envelope_and_duplicate_keys() {
        for text in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"ErrorStatus":"Success","Response":0,"ThrottleSeconds":0}"#,
            r#"{"ErrorCode":1}"#,
            r#"{"Response":0,"ErrorCode":1,"ThrottleSeconds":0,"ErrorStatus":null}"#,
        ] {
            assert_outcome(response(200, text), InstancedPullOutcome::Uncertain);
        }
        for field in ["Response", "ErrorCode", "ThrottleSeconds"] {
            for value in [
                json!(null),
                json!("1"),
                json!(1.0),
                json!(2147483648_i64),
                json!(-2147483649_i64),
                json!({}),
                json!([]),
                json!(true),
            ] {
                let mut body: Value = serde_json::from_str(ACK).unwrap();
                body[field] = value;
                assert_outcome(
                    response(200, &body.to_string()),
                    InstancedPullOutcome::Uncertain,
                );
            }
            let duplicate = format!("{{\"{field}\":999,{}", &ACK[1..]);
            assert_outcome(response(200, &duplicate), InstancedPullOutcome::Uncertain);
        }
        for (field, value) in [
            ("ThrottleSeconds", json!(-1)),
            ("ErrorStatus", json!("Failure")),
            ("Message", json!({})),
            ("MessageData", json!("bad")),
            ("DetailedErrorTrace", json!([])),
        ] {
            let mut body: Value = serde_json::from_str(ACK).unwrap();
            body[field] = value;
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Uncertain,
            );
        }
        for code in [0, -1, i32::MAX] {
            let body = json!({"Response":0,"ErrorCode":code,"ThrottleSeconds":0});
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Uncertain,
            );
        }
        for result in [i32::MIN, 42, i32::MAX] {
            let body = json!({"Response":result,"ErrorCode":1,"ThrottleSeconds":i32::MAX});
            assert_outcome(
                response(200, &body.to_string()),
                InstancedPullOutcome::Acknowledged,
            );
        }
        let duplicate = ACK.replace(
            "\"ErrorStatus\":\"Success\"",
            "\"ErrorStatus\":\"Failure\",\"ErrorStatus\":\"Success\"",
        );
        assert_outcome(response(200, &duplicate), InstancedPullOutcome::Uncertain);
    }
    struct BrokenReader;
    impl Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("provider-trace-canary"))
        }
    }
    struct NeverRead;
    impl Read for NeverRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("body must not be read")
        }
    }
    #[test]
    fn instanced_pull_bounded_media_length_and_read_failures() {
        assert_eq!(
            INSTANCED_PULL_RESPONSE_LIMIT,
            crate::transfer::TRANSFER_RESPONSE_LIMIT
        );
        for status in [401, 403, 429, 503, 302] {
            let outcome = if matches!(status, 401 | 403) {
                InstancedPullOutcome::AuthenticationRefused
            } else {
                InstancedPullOutcome::Uncertain
            };
            assert_outcome(
                InstancedPullHttpResponse::new(status, None, None, Box::new(NeverRead)),
                outcome,
            );
        }
        assert_outcome(
            InstancedPullHttpResponse::new(
                200,
                Some(INSTANCED_PULL_RESPONSE_LIMIT as u64 + 1),
                Some("application/json".into()),
                Box::new(NeverRead),
            ),
            InstancedPullOutcome::Uncertain,
        );
        assert_outcome(
            InstancedPullHttpResponse::new(
                200,
                None,
                Some("application/json".into()),
                Box::new(BrokenReader),
            ),
            InstancedPullOutcome::Uncertain,
        );
        for length in [None, Some(1), Some(ACK.len() as u64 + 1)] {
            let text = if length.is_none() {
                "x".repeat(INSTANCED_PULL_RESPONSE_LIMIT + 1)
            } else {
                ACK.into()
            };
            assert_outcome(
                InstancedPullHttpResponse::new(
                    200,
                    length,
                    Some("application/json".into()),
                    Box::new(Cursor::new(text.into_bytes())),
                ),
                InstancedPullOutcome::Uncertain,
            );
        }
        for media in [None, Some("text/html"), Some("application/json\r\nsecret")] {
            assert_outcome(
                InstancedPullHttpResponse::new(
                    200,
                    None,
                    media.map(str::to_owned),
                    Box::new(NeverRead),
                ),
                InstancedPullOutcome::Uncertain,
            );
        }
    }
    #[test]
    fn instanced_pull_transport_failures_are_one_shot_and_sanitized() {
        for (failure, category) in [
            (
                InstancedPullTransportError::NotDispatched,
                Some(ErrorCategory::ProviderUnavailable),
            ),
            (
                InstancedPullTransportError::Configuration,
                Some(ErrorCategory::ConfigurationError),
            ),
            (InstancedPullTransportError::Indeterminate, None),
        ] {
            let mut g = gateway(Err(failure));
            let result = g.pull(&request(42, 3));
            if let Some(category) = category {
                assert_eq!(result.unwrap_err(), AppError::new(category));
            } else {
                assert_eq!(result.unwrap(), InstancedPullOutcome::Uncertain);
            }
            assert_eq!(g.transport().calls, 1);
            assert_eq!(g.source.calls.get(), 1);
            assert_eq!(g.clock.0.get(), 1);
        }
    }
    #[test]
    fn instanced_pull_predispatch_credential_failures() {
        for (material, expiry, missing, category) in [
            (
                b"token-canary".to_vec(),
                1,
                false,
                ErrorCategory::ProviderAuthentication,
            ),
            (vec![], 100, false, ErrorCategory::ConfigurationError),
            (
                b"bad\r\nheader-canary".to_vec(),
                100,
                false,
                ErrorCategory::ConfigurationError,
            ),
            (vec![255], 100, false, ErrorCategory::ConfigurationError),
            (vec![], 100, true, ErrorCategory::ProviderAuthentication),
        ] {
            let mut g = gateway(Ok(response(200, ACK)));
            g.source.material = material;
            g.source.expiry = expiry;
            g.source.missing = missing;
            assert_eq!(
                g.pull(&request(42, 3)).unwrap_err(),
                AppError::new(category)
            );
            assert_eq!(g.transport().calls, 0);
            assert_eq!(g.source.calls.get(), 1);
        }
    }
    #[test]
    fn instanced_pull_allowlist_precedes_secret_access_and_network() {
        let mut transport = HttpInstancedPullTransport::new().unwrap();
        let key = InstancedPullApiKey::new("key-canary").unwrap();
        let bearer = InstancedPullBearerToken::new("token-canary").unwrap();
        assert!(is_allowed_instanced_pull_url(INSTANCED_PULL_ENDPOINT));
        for url in [
            "http://www.bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster/",
            "https://bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster/",
            "https://www.bungie.net:443/Platform/Destiny2/Actions/Items/PullFromPostmaster/",
            "https://user@www.bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster/",
            "https://www.bungie.net.evil/Platform/Destiny2/Actions/Items/PullFromPostmaster/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/PullFromPostmaster",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items%2fPullFromPostmaster/",
            "https://www.bungie.net/Platform/Destiny2/Actions/Items/../Items/PullFromPostmaster/",
            crate::equip::EQUIP_ENDPOINT,
            crate::lock::LOCK_ENDPOINT,
            crate::transfer::TRANSFER_ENDPOINT,
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([
            format!("{INSTANCED_PULL_ENDPOINT}?token=query-canary"),
            format!("{INSTANCED_PULL_ENDPOINT}#fragment"),
        ]) {
            assert!(!is_allowed_instanced_pull_url(&url));
            let http = InstancedPullHttpRequest::new(
                &url,
                "private-body-canary".into(),
                Some(&key),
                Some(&bearer),
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| count.set(0));
            assert_eq!(
                transport.post(&http).unwrap_err(),
                InstancedPullTransportError::Configuration
            );
            crate::transfer::CREDENTIAL_ACCESSES.with(|count| assert_eq!(count.get(), 0));
            let debug = format!("{http:?}");
            for canary in [
                "query-canary",
                "private-body-canary",
                "key-canary",
                "token-canary",
            ] {
                assert!(!debug.contains(canary));
            }
        }
    }
    #[test]
    fn instanced_pull_debug_redacts_all_private_material() {
        let body = r#"{"Response":0,"ErrorCode":1,"ThrottleSeconds":0,"Message":"message-canary","MessageData":{"private":"data-canary"},"DetailedErrorTrace":"trace-canary"}"#;
        let mut g = gateway(Ok(response(200, body)));
        let key = InstancedPullApiKey::new("key-canary").unwrap();
        let token = InstancedPullBearerToken::new("token-canary").unwrap();
        let http = InstancedPullHttpRequest::new(
            INSTANCED_PULL_ENDPOINT,
            "body-canary".into(),
            Some(&key),
            Some(&token),
        );
        let debug = format!(
            "{g:?} {key:?} {token:?} {http:?} {:?}",
            g.transport().result
        );
        for canary in [
            "key-canary",
            "token-canary",
            "body-canary",
            "message-canary",
            "data-canary",
            "trace-canary",
        ] {
            assert!(!debug.contains(canary));
        }
        assert_eq!(
            g.pull(&request(42, 3)).unwrap(),
            InstancedPullOutcome::Acknowledged
        );
        for secret in ["", "header-canary\r\n", "nonascii-é"] {
            let error = InstancedPullApiKey::new(secret).unwrap_err();
            assert!(!format!("{error:?} {error}").contains("canary"));
        }
    }
}
