use destiny_core::{
    CharacterId, DestinyMembership, EquipLoadoutRequest, LoadoutActionGateway,
    LoadoutActionOutcome, MembershipId, MembershipType,
};

fn request(
    index: i32,
    character: &str,
    membership_type: u32,
) -> Result<EquipLoadoutRequest, destiny_core::AppError> {
    EquipLoadoutRequest::new(
        DestinyMembership::new(
            MembershipType::new(membership_type),
            MembershipId::new("123"),
        )
        .unwrap(),
        CharacterId::new(character),
        index,
    )
}

#[test]
fn validated_request_and_narrow_port() {
    fn accepts_port(_: &mut dyn LoadoutActionGateway) {}
    let _ = accepts_port;
    for index in [0, 11] {
        let request = request(index, "456", 3).unwrap();
        assert_eq!(request.loadout_index(), index);
        assert_eq!(request.character().as_str(), "456");
    }
    for index in [-1, 12, i32::MAX] {
        assert!(request(index, "456", 3).is_err());
    }
    for character in ["", "01", "-1", "not-an-id"] {
        assert!(request(0, character, 3).is_err());
    }
    assert!(request(0, &"1".repeat(129), 3).is_err());
    assert!(request(0, "456", u32::MAX).is_err());
    assert!(LoadoutActionOutcome::Applied.applied());
}

use destiny_adapters::loadout_action::*;
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial, ErrorCategory,
    LoadoutActionRejection, StaticCredentialSource, Timestamp,
};
use serde_json::{Value, json};
use std::io::Cursor;

struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(1))
    }
}
struct FakeTransport {
    response: Option<Result<LoadoutActionHttpResponse, LoadoutActionTransportError>>,
    calls: usize,
}
impl LoadoutActionTransport for FakeTransport {
    fn post(
        &mut self,
        http: &LoadoutActionHttpRequest<'_>,
    ) -> Result<LoadoutActionHttpResponse, LoadoutActionTransportError> {
        self.calls += 1;
        assert_eq!(http.url(), LOADOUT_ACTION_ENDPOINT);
        assert!(format!("{http:?}").contains("POST"));
        assert_eq!(
            serde_json::from_str::<Value>(http.body()).unwrap(),
            json!({"loadoutIndex": 0, "characterId": "456", "membershipType": 3})
        );
        assert!(http.api_key().is_some() && http.bearer().is_some());
        self.response.take().unwrap()
    }
}
fn gateway(
    response: Result<LoadoutActionHttpResponse, LoadoutActionTransportError>,
    expires: u64,
) -> BungieLoadoutActionGateway<FakeTransport, TestClock> {
    BungieLoadoutActionGateway::new(
        FakeTransport {
            response: Some(response),
            calls: 0,
        },
        TestClock,
        StaticCredentialSource::new(AccessCredential {
            refresh_grant: None,
            material: CredentialMaterial::from_bytes(b"token-canary".to_vec()),
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(expires),
            },
        }),
        LoadoutActionApiKey::new("key-canary").unwrap(),
    )
    .unwrap()
}
fn response(status: u16, body: Value) -> LoadoutActionHttpResponse {
    let bytes = body.to_string().into_bytes();
    LoadoutActionHttpResponse::new(
        status,
        Some(bytes.len() as u64),
        Some("application/json".into()),
        Box::new(Cursor::new(bytes)),
    )
}
fn outcome(status: u16, body: Value) -> LoadoutActionOutcome {
    let mut gateway = gateway(Ok(response(status, body)), 100);
    let receipt = gateway
        .equip_loadout(&request(0, "456", 3).unwrap())
        .unwrap();
    assert_eq!(gateway.transport().calls, 1);
    receipt
}

#[test]
fn exact_single_post_ack_and_http_receipts() {
    use LoadoutActionOutcome as O;
    for (status, expected) in [
        (200, O::Applied),
        (401, O::AuthenticationRefused),
        (403, O::AuthenticationRefused),
        (429, O::Transient),
        (400, O::Rejected(LoadoutActionRejection::InvalidRequest)),
        (404, O::Rejected(LoadoutActionRejection::InvalidRequest)),
        (500, O::Uncertain),
        (503, O::Uncertain),
        (302, O::Uncertain),
        (201, O::Uncertain),
    ] {
        assert_eq!(outcome(status, json!({"ErrorCode": 1})), expected);
    }
}

#[test]
fn bounded_provider_refusals_and_throttle_no_raw_messages() {
    use LoadoutActionOutcome as O;
    for (status, expected) in [
        (
            "DestinyCharacterNotInTower",
            O::Rejected(LoadoutActionRejection::RestrictedLocation),
        ),
        (
            "DestinyCannotPerformActionAtThisLocation",
            O::Rejected(LoadoutActionRejection::RestrictedLocation),
        ),
        (
            "DestinyCanOnlyEquipInGame",
            O::Rejected(LoadoutActionRejection::RestrictedLocation),
        ),
        (
            "DestinyAccountMustBeOffline",
            O::Rejected(LoadoutActionRejection::RestrictedLocation),
        ),
        (
            "DestinyCharacterNotFound",
            O::Rejected(LoadoutActionRejection::CharacterNotFound),
        ),
        ("DestinyThrottledByGameServer", O::Transient),
        ("ThrottleLimitExceeded", O::Transient),
        ("unknown-canary", O::Uncertain),
    ] {
        assert_eq!(
            outcome(
                200,
                json!({"ErrorCode": 999, "ErrorStatus": status, "Message": "token-canary"})
            ),
            expected
        );
    }
    for body in [
        json!({}),
        json!([]),
        json!({"ErrorCode": "1"}),
        json!({"ErrorCode": 999}),
    ] {
        assert_eq!(outcome(200, body), O::Uncertain);
    }
}

#[test]
fn malformed_oversized_wrong_media_and_length_are_uncertain() {
    for (length, media, bytes) in [
        (Some(2), "application/json", b"broken".to_vec()),
        (Some(1), "application/json", b"{}".to_vec()),
        (None, "text/html", b"{\"ErrorCode\":1}".to_vec()),
        (
            None,
            "application/json",
            vec![b' '; LOADOUT_ACTION_RESPONSE_LIMIT + 1],
        ),
        (
            Some((LOADOUT_ACTION_RESPONSE_LIMIT + 1) as u64),
            "application/json",
            b"{\"ErrorCode\":1}".to_vec(),
        ),
    ] {
        let mut gateway = gateway(
            Ok(LoadoutActionHttpResponse::new(
                200,
                length,
                Some(media.into()),
                Box::new(Cursor::new(bytes)),
            )),
            100,
        );
        assert_eq!(
            gateway
                .equip_loadout(&request(0, "456", 3).unwrap())
                .unwrap(),
            LoadoutActionOutcome::Uncertain
        );
        assert_eq!(gateway.transport().calls, 1);
    }
}

#[test]
fn transport_boundary_and_expired_credentials_never_retry() {
    for (error, category) in [
        (
            LoadoutActionTransportError::NotDispatched,
            Some(ErrorCategory::ProviderUnavailable),
        ),
        (
            LoadoutActionTransportError::Configuration,
            Some(ErrorCategory::ConfigurationError),
        ),
        (LoadoutActionTransportError::Indeterminate, None),
    ] {
        let mut gateway = gateway(Err(error), 100);
        let result = gateway.equip_loadout(&request(0, "456", 3).unwrap());
        if let Some(category) = category {
            assert_eq!(result.unwrap_err().category(), category);
        } else {
            assert_eq!(result.unwrap(), LoadoutActionOutcome::Uncertain);
        }
        assert_eq!(gateway.transport().calls, 1);
    }
    let mut gateway = gateway(Ok(response(200, json!({"ErrorCode": 1}))), 1);
    assert_eq!(
        gateway
            .equip_loadout(&request(0, "456", 3).unwrap())
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderAuthentication
    );
    assert_eq!(gateway.transport().calls, 0);
}

#[test]
fn exact_allowlist_preserves_other_action_boundaries() {
    assert!(is_allowed_loadout_action_url(LOADOUT_ACTION_ENDPOINT));
    assert!(!destiny_adapters::is_allowed_equip_url(
        LOADOUT_ACTION_ENDPOINT
    ));
    assert!(!destiny_adapters::is_allowed_lock_url(
        LOADOUT_ACTION_ENDPOINT
    ));
    assert!(!destiny_adapters::is_allowed_transfer_url(
        LOADOUT_ACTION_ENDPOINT
    ));
    let batch = "https://www.bungie.net/Platform/Destiny2/Actions/Items/EquipItems/";
    assert!(!destiny_adapters::is_allowed_equip_url(batch));
    let mut transport = HttpLoadoutActionTransport::new().unwrap();
    for url in [
        batch.to_owned(),
        destiny_adapters::EQUIP_ENDPOINT.to_owned(),
        format!("{LOADOUT_ACTION_ENDPOINT}?x=1"),
        format!("{LOADOUT_ACTION_ENDPOINT}#x"),
        LOADOUT_ACTION_ENDPOINT.trim_end_matches('/').to_owned(),
        LOADOUT_ACTION_ENDPOINT.replace("https:", "http:"),
        LOADOUT_ACTION_ENDPOINT.replace("www.bungie.net", "evil.example"),
        LOADOUT_ACTION_ENDPOINT.replace("EquipLoadout", "equiploadout"),
        LOADOUT_ACTION_ENDPOINT.replace("Loadouts/", "Items/../Loadouts/"),
    ] {
        assert!(!is_allowed_loadout_action_url(&url));
        assert_eq!(
            transport
                .post(&LoadoutActionHttpRequest::new(
                    &url,
                    "{}".into(),
                    None,
                    None
                ))
                .unwrap_err(),
            LoadoutActionTransportError::Configuration
        );
    }
}

#[test]
fn request_gateway_http_and_credentials_debug_are_redacted() {
    let request = request(0, "456", 3).unwrap();
    let key = LoadoutActionApiKey::new("key-canary").unwrap();
    let bearer = LoadoutActionBearerToken::new("token-canary").unwrap();
    let http = LoadoutActionHttpRequest::new(
        "https://evil.example/?token=URL_CANARY",
        "BODY_CANARY".into(),
        Some(&key),
        Some(&bearer),
    );
    let gateway = gateway(Ok(response(200, json!({"ErrorCode": 1}))), 100);
    let debug = format!("{request:?} {gateway:?} {http:?} {key:?} {bearer:?}");
    for canary in [
        "123",
        "456",
        "key-canary",
        "token-canary",
        "URL_CANARY",
        "BODY_CANARY",
        "https://",
    ] {
        assert!(!debug.contains(canary));
    }
}
