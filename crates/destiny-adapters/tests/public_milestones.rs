use destiny_adapters::manifest::{
    BungieManifestGateway, HttpManifestTransport, MILESTONES_ENDPOINT, ManifestApiKey,
    ManifestHttpRequest, ManifestHttpResponse, ManifestTransport, ManifestTransportError,
    decode_public_milestones,
};
use destiny_core::{Clock, PublicMilestonesGateway, Timestamp};
use serde_json::{Value, json};

const FIXTURE: &[u8] = include_bytes!("fixtures/public-milestones.json");
fn decode(value: Value) -> Result<destiny_core::PublicMilestonesSnapshot, destiny_core::AppError> {
    decode_public_milestones(&serde_json::to_vec(&value).unwrap())
}
#[test]
fn fixture_projects_only_supported_fields() {
    let snapshot = decode_public_milestones(FIXTURE).unwrap();
    assert_eq!(snapshot.milestones.len(), 3);
    let milestone = &snapshot.milestones[0];
    assert_eq!(milestone.milestone_hash, 100);
    assert_eq!(milestone.order, 1);
    assert_eq!(milestone.start.unwrap().seconds(), 1790701200);
    assert_eq!(milestone.end.unwrap().seconds(), 1791306000);
    assert_eq!(milestone.activities[0].activity_hash, 200);
    assert_eq!(milestone.activities[0].modifier_hashes, [300, 301]);
    assert!(snapshot.milestones[1].activities.is_empty());
    assert!(snapshot.milestones[2].activities.is_empty());
}
#[test]
fn rejects_mismatch_duplicates_and_invalid_timestamps() {
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    value["Response"]["100"]["milestoneHash"] = json!(999);
    assert!(decode(value).is_err());
    assert!(decode_public_milestones(br#"{"ErrorCode":1,"ErrorStatus":"Success","Response":{"1":{"milestoneHash":1,"order":1},"1":{"milestoneHash":1,"order":2}}}"#).is_err());
    for start in [
        "2026-10-07T17:00:00Z",
        "bad-provider-date",
        "1960-01-01T00:00:00Z",
    ] {
        let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
        value["Response"]["100"]["startDate"] = json!(start);
        assert!(decode(value).is_err());
    }
}
#[test]
fn caps_all_three_collections_and_body() {
    let mut response = serde_json::Map::new();
    for hash in 0..65 {
        response.insert(hash.to_string(), json!({"milestoneHash":hash,"order":1}));
    }
    assert!(decode(json!({"ErrorCode":1,"ErrorStatus":"Success","Response":response})).is_err());
    for field in ["activities", "modifierHashes"] {
        let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
        if field == "activities" {
            value["Response"]["100"][field] = json!(vec![json!({"activityHash":200}); 65]);
        } else {
            value["Response"]["100"]["activities"][0][field] = json!(vec![300; 65]);
        }
        assert!(decode(value).is_err());
    }
    let mut oversized = FIXTURE.to_vec();
    oversized.resize(1024 * 1024 + 1, b' ');
    assert!(decode_public_milestones(&oversized).is_err());
}
#[test]
fn accepts_inclusive_caps_and_normalizes_offsets() {
    let mut response = serde_json::Map::new();
    for hash in 0..64 {
        response.insert(
            hash.to_string(),
            json!({
                "milestoneHash": hash, "order": 1,
                "startDate": "2026-09-29T19:00:00+02:00",
                "endDate": "2026-09-29T17:00:00Z",
                "activities": vec![json!({"activityHash":200,"modifierHashes":vec![0;64]});64]
            }),
        );
    }
    let snapshot =
        decode(json!({"ErrorCode":1,"ErrorStatus":"Success","Response":response})).unwrap();
    assert_eq!(snapshot.milestones.len(), 64);
    assert_eq!(snapshot.milestones[0].start, snapshot.milestones[0].end);
    assert!(
        decode(json!({"ErrorCode":1,"ErrorStatus":"Success","Response":{}}))
            .unwrap()
            .milestones
            .is_empty()
    );
    assert!(decode_public_milestones(br#"{"ErrorCode":1,"ErrorStatus":"Success","Response":{"01":{"milestoneHash":1,"order":1}}}"#).is_err());
}

#[test]
fn provider_failure_is_sanitized() {
    for (code, status) in [(5, "Success"), (1, "ProviderSecret")] {
        let error = decode(
            json!({"ErrorCode":code,"ErrorStatus":status,"Message":"ProviderSecret","Response":{}}),
        )
        .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("ProviderSecret"));
    }
}
struct TestClock;
impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, destiny_core::AppError> {
        Ok(Timestamp::new(0))
    }
}
struct Script {
    calls: usize,
}
impl ManifestTransport for Script {
    fn get(
        &mut self,
        request: &ManifestHttpRequest<'_>,
    ) -> Result<ManifestHttpResponse, ManifestTransportError> {
        assert_eq!(request.url(), MILESTONES_ENDPOINT);
        assert!(request.api_key().is_some(), "x-api-key material missing");
        assert!(!format!("{request:?}").contains("test-secret"));
        self.calls += 1;
        Ok(ManifestHttpResponse::new(
            200,
            None,
            Some("application/json".into()),
            Box::new(std::io::Cursor::new(FIXTURE)),
        ))
    }
}
#[test]
fn gateway_uses_fixed_endpoint_and_key() {
    let mut gateway = BungieManifestGateway::new(
        ManifestApiKey::new("test-secret").unwrap(),
        Script { calls: 0 },
        TestClock,
    );
    assert_eq!(
        gateway.load_public_milestones().unwrap().milestones.len(),
        3
    );
    assert_eq!(gateway.transport().calls, 1);
}
#[test]
fn transport_rejects_endpoint_variants_without_network() {
    let mut transport = HttpManifestTransport::new().unwrap();
    for url in [
        "https://evil.invalid/",
        "https://www.bungie.net/Platform/Destiny2/Milestones/?x=1",
        "https://www.bungie.net/Platform/Destiny2/Milestones",
    ] {
        assert_eq!(
            transport
                .get(&ManifestHttpRequest::new(url, None))
                .unwrap_err(),
            ManifestTransportError::Configuration
        );
    }
}
