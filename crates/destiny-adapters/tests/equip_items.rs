use destiny_adapters::equip_items::*;
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    DestinyMembership, EquipItemResult, EquipItemStatus, EquipItemsGateway, EquipItemsOutcome,
    EquipItemsRequest, ErrorCategory, ItemInstanceId, MAX_EQUIP_ITEMS, MembershipId,
    MembershipType, StaticCredentialSource, Timestamp,
};
use serde_json::{Value, json};
use std::io::Cursor;

fn request(ids: &[&str]) -> Result<EquipItemsRequest, AppError> {
    EquipItemsRequest::new(
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap(),
        CharacterId::new("456"),
        ids.iter().map(|id| ItemInstanceId::new(*id)).collect(),
    )
}
#[test]
fn validated_request_and_port() {
    fn accepts_port(_: &mut dyn EquipItemsGateway) {}
    let _ = accepts_port;
    assert_eq!(request(&["789", "790"]).unwrap().items().len(), 2);
    for ids in [
        vec![],
        vec![""],
        vec!["789", "789"],
        vec!["01"],
        vec!["-1"],
        vec!["not-an-id"],
        vec!["9223372036854775808"],
    ] {
        assert!(request(&ids).is_err());
    }
    let ids: Vec<String> = (1..=MAX_EQUIP_ITEMS + 1).map(|id| id.to_string()).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    assert!(request(&refs[..MAX_EQUIP_ITEMS]).is_ok());
    assert!(request(&refs).is_err());
    for (character, membership) in [("", 3), ("01", 3), ("456", u32::MAX)] {
        assert!(
            EquipItemsRequest::new(
                DestinyMembership::new(MembershipType::new(membership), MembershipId::new("123"))
                    .unwrap(),
                CharacterId::new(character),
                vec![ItemInstanceId::new("789")],
            )
            .is_err()
        );
    }
}
struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(1))
    }
}
struct FakeTransport {
    response: Option<Result<EquipItemsHttpResponse, EquipItemsTransportError>>,
    calls: usize,
}
impl EquipItemsTransport for FakeTransport {
    fn post(
        &mut self,
        http: &EquipItemsHttpRequest<'_>,
    ) -> Result<EquipItemsHttpResponse, EquipItemsTransportError> {
        self.calls += 1;
        assert_eq!(http.url(), EQUIP_ITEMS_ENDPOINT);
        assert!(format!("{http:?}").contains("POST"));
        assert_eq!(
            serde_json::from_str::<Value>(http.body()).unwrap(),
            json!({"itemIds": ["6917529027641081856", "790"], "characterId": "456", "membershipType": 3})
        );
        assert!(http.api_key().is_some() && http.bearer().is_some());
        self.response.take().unwrap()
    }
}
fn gateway(
    response: Result<EquipItemsHttpResponse, EquipItemsTransportError>,
    expires: u64,
) -> BungieEquipItemsGateway<FakeTransport, TestClock> {
    BungieEquipItemsGateway::new(
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
        EquipItemsApiKey::new("key-canary").unwrap(),
    )
    .unwrap()
}
fn batch() -> EquipItemsRequest {
    request(&["6917529027641081856", "790"]).unwrap()
}
fn response(status: u16, body: Value) -> EquipItemsHttpResponse {
    let bytes = body.to_string().into_bytes();
    EquipItemsHttpResponse::new(
        status,
        Some(bytes.len() as u64),
        Some("application/json".into()),
        Box::new(Cursor::new(bytes)),
    )
}
fn outcome(status: u16, body: Value) -> EquipItemsOutcome {
    let mut gateway = gateway(Ok(response(status, body)), 100);
    let receipt = gateway.equip_items(&batch()).unwrap();
    assert_eq!(gateway.transport().calls, 1);
    receipt
}
fn envelope(results: Value) -> Value {
    json!({"ErrorCode": 1, "Response": {"equipResults": results}, "Message": "token-canary"})
}
#[test]
fn exact_post_all_success_and_mixed_results_in_request_order() {
    for second in [1, 1641, 0, -1, i32::MAX] {
        let receipt = outcome(
            200,
            envelope(json!([
                {"itemInstanceId": 790, "equipStatus": second},
                {"itemInstanceId": "6917529027641081856", "equipStatus": 1}
            ])),
        );
        assert_eq!(
            receipt,
            EquipItemsOutcome::Acknowledged(vec![
                EquipItemResult {
                    item: ItemInstanceId::new("6917529027641081856"),
                    status: EquipItemStatus::Success,
                },
                EquipItemResult {
                    item: ItemInstanceId::new("790"),
                    status: if second == 1 {
                        EquipItemStatus::Success
                    } else {
                        EquipItemStatus::Failure(second)
                    },
                },
            ])
        );
        assert_eq!(receipt.fully_applied(), second == 1);
        assert!(!format!("{receipt:?}").contains("token-canary"));
    }
    assert!(matches!(
        outcome(
            200,
            envelope(json!([
                {"itemInstanceId": 6917529027641081856_i64, "equipStatus": 1},
                {"itemInstanceId": "790", "equipStatus": 1}
            ]))
        ),
        EquipItemsOutcome::Acknowledged(_)
    ));
}
#[test]
fn missing_results_preserve_reported_statuses() {
    for code in [1, 1641] {
        let receipt = outcome(
            200,
            envelope(json!([{"itemInstanceId": "790", "equipStatus": code}])),
        );
        assert!(!receipt.fully_applied());
        let EquipItemsOutcome::Acknowledged(results) = receipt else {
            panic!("missing IDs should not invalidate the acknowledged envelope");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].item.as_str(), "6917529027641081856");
        assert_eq!(results[0].status, EquipItemStatus::NotReported);
        assert_eq!(results[1].item.as_str(), "790");
        assert_eq!(
            results[1].status,
            if code == 1 {
                EquipItemStatus::Success
            } else {
                EquipItemStatus::Failure(code)
            }
        );
    }
}
#[test]
fn no_reported_items_are_acknowledged_but_not_applied() {
    let receipt = outcome(200, envelope(json!([])));
    assert!(!receipt.fully_applied());
    assert_eq!(
        receipt,
        EquipItemsOutcome::Acknowledged(
            batch()
                .items()
                .iter()
                .map(|item| EquipItemResult {
                    item: item.clone(),
                    status: EquipItemStatus::NotReported,
                })
                .collect()
        )
    );
    assert!(!EquipItemsOutcome::Acknowledged(vec![]).fully_applied());
    for receipt in [
        EquipItemsOutcome::Refused,
        EquipItemsOutcome::AuthenticationRefused,
        EquipItemsOutcome::Transient,
        EquipItemsOutcome::Uncertain,
    ] {
        assert!(!receipt.fully_applied());
    }
}
#[test]
fn extra_duplicate_unknown_and_malformed_results_are_uncertain() {
    let first = json!({"itemInstanceId": "6917529027641081856", "equipStatus": 1});
    let second = json!({"itemInstanceId": "790", "equipStatus": 1});
    for results in [
        json!([first, first]),
        json!([first, second, second]),
        json!([first, {"itemInstanceId": "791", "equipStatus": 1}]),
        json!([first, {"itemInstanceId": "0790", "equipStatus": 1}]),
        json!([first, {"itemInstanceId": 790.0, "equipStatus": 1}]),
        json!([first, {"itemInstanceId": "790", "equipStatus": "1"}]),
        json!([first, {"itemInstanceId": "790", "equipStatus": 2147483648_i64}]),
        json!([first, {"itemInstanceId": "790"}]),
        json!([first, {"equipStatus": 1}]),
        json!(null),
    ] {
        assert_eq!(
            outcome(200, envelope(results)),
            EquipItemsOutcome::Uncertain
        );
    }
    for body in [
        json!({}),
        json!([]),
        json!({"ErrorCode": "1"}),
        json!({"ErrorCode": 1}),
        json!({"ErrorCode": 1, "Response": {}}),
        json!({"ErrorCode": 999}),
    ] {
        assert_eq!(outcome(200, body), EquipItemsOutcome::Uncertain);
    }
}
#[test]
fn envelope_refusal_auth_transient_and_unknown() {
    use EquipItemsOutcome as O;
    for (status, expected) in [
        (401, O::AuthenticationRefused),
        (403, O::AuthenticationRefused),
        (429, O::Transient),
        (400, O::Refused),
        (404, O::Refused),
        (500, O::Uncertain),
        (503, O::Uncertain),
        (302, O::Uncertain),
        (201, O::Uncertain),
    ] {
        assert_eq!(outcome(status, json!({"ErrorCode": 1})), expected);
    }
    for (status, expected) in [
        ("DestinyCharacterNotFound", O::Refused),
        ("DestinyCannotPerformActionAtThisLocation", O::Refused),
        ("DestinyItemUniqueEquipRestricted", O::Refused),
        ("WebAuthRequired", O::AuthenticationRefused),
        ("AccessTokenHasExpired", O::AuthenticationRefused),
        ("AccessTokenInvalid", O::AuthenticationRefused),
        ("DestinyThrottledByGameServer", O::Transient),
        ("ThrottleLimitExceeded", O::Transient),
        ("unknown-canary", O::Uncertain),
    ] {
        assert_eq!(
            outcome(200, json!({"ErrorCode": 999, "ErrorStatus": status})),
            expected
        );
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
            vec![b' '; EQUIP_ITEMS_RESPONSE_LIMIT + 1],
        ),
        (
            Some((EQUIP_ITEMS_RESPONSE_LIMIT + 1) as u64),
            "application/json",
            b"{\"ErrorCode\":1}".to_vec(),
        ),
    ] {
        let mut gateway = gateway(
            Ok(EquipItemsHttpResponse::new(
                200,
                length,
                Some(media.into()),
                Box::new(Cursor::new(bytes)),
            )),
            100,
        );
        assert_eq!(
            gateway.equip_items(&batch()).unwrap(),
            EquipItemsOutcome::Uncertain
        );
        assert_eq!(gateway.transport().calls, 1);
    }
}
#[test]
fn transport_boundary_expiry_and_redaction() {
    for (error, category) in [
        (
            EquipItemsTransportError::NotDispatched,
            Some(ErrorCategory::ProviderUnavailable),
        ),
        (
            EquipItemsTransportError::Configuration,
            Some(ErrorCategory::ConfigurationError),
        ),
        (EquipItemsTransportError::Indeterminate, None),
    ] {
        let mut gateway = gateway(Err(error), 100);
        let result = gateway.equip_items(&batch());
        if let Some(category) = category {
            assert_eq!(result.unwrap_err().category(), category);
        } else {
            assert_eq!(result.unwrap(), EquipItemsOutcome::Uncertain);
        }
        assert_eq!(gateway.transport().calls, 1);
    }
    let mut gateway = gateway(Ok(response(200, json!({}))), 1);
    assert_eq!(
        gateway.equip_items(&batch()).unwrap_err().category(),
        ErrorCategory::ProviderAuthentication
    );
    assert_eq!(gateway.transport().calls, 0);
    let key = EquipItemsApiKey::new("key-canary").unwrap();
    let bearer = EquipItemsBearerToken::new("token-canary").unwrap();
    let http = EquipItemsHttpRequest::new(
        "https://evil.example/?token=URL_CANARY",
        "BODY_CANARY".into(),
        Some(&key),
        Some(&bearer),
    );
    let debug = format!("{:?} {gateway:?} {http:?} {key:?} {bearer:?}", batch());
    for canary in [
        "123",
        "456",
        "6917529027641081856",
        "790",
        "key-canary",
        "token-canary",
        "URL_CANARY",
        "BODY_CANARY",
        "https://",
    ] {
        assert!(!debug.contains(canary));
    }
}
#[test]
fn batch_allowlist_does_not_loosen_single_item_or_loadout() {
    assert!(is_allowed_equip_items_url(EQUIP_ITEMS_ENDPOINT));
    assert!(!destiny_adapters::is_allowed_equip_url(
        EQUIP_ITEMS_ENDPOINT
    ));
    assert!(!destiny_adapters::is_allowed_loadout_action_url(
        EQUIP_ITEMS_ENDPOINT
    ));
    assert!(!destiny_adapters::is_allowed_transfer_url(
        EQUIP_ITEMS_ENDPOINT
    ));
    assert!(!destiny_adapters::is_allowed_lock_url(EQUIP_ITEMS_ENDPOINT));
}
