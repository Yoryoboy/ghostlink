use destiny_adapters::artifact::{
    ARTIFACT_RESPONSE_LIMIT, artifact_endpoint, decode_artifact, is_allowed_artifact_url,
};
use destiny_adapters::is_allowed_profile_url;
use destiny_core::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    json!({"ErrorCode": 1, "Response": {
        "profileProgression": {"data": {"seasonalArtifact": {
            "artifactHash": 1234, "pointsAcquired": 0, "powerBonus": 0,
            "pointProgression": {"progressionHash": 99, "level": 12, "levelCap": 12,
                "currentProgress": 1000, "progressToNextLevel": 34, "nextLevelAt": 100}
        }}},
        "characterProgressions": {"data": {"stale": {"seasonalArtifact": "ignored"}}}
    }})
}
fn decode(value: &Value) -> Result<ArtifactSnapshot, AppError> {
    decode_artifact(&serde_json::to_vec(value).unwrap())
}
#[test]
fn artifact_point_progression_is_source_of_truth() {
    assert_eq!(
        decode(&fixture()).unwrap().profile,
        Some(ArtifactProfileState {
            points: 12,
            points_cap: 12,
            progress_to_next_level: 34,
            next_level_at: 100,
        })
    );
    let mut value = fixture();
    value["Response"]["profileProgression"]["data"]["seasonalArtifact"]["pointProgression"]["level"] =
        json!(5);
    assert_eq!(decode(&value).unwrap().profile.unwrap().points, 5);
    value["Response"]["profileProgression"]["data"]["seasonalArtifact"]
        .as_object_mut()
        .unwrap()
        .remove("artifactHash");
    assert!(decode(&value).is_ok());
}
#[test]
fn artifact_url_is_exact_and_canonical() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("456")).unwrap();
    let url = artifact_endpoint(&membership).unwrap();
    assert_eq!(
        url,
        "https://www.bungie.net/Platform/Destiny2/3/Profile/456/?components=104"
    );
    assert!(is_allowed_artifact_url(&url));
    assert!(is_allowed_profile_url(&url));
    for bad in [
        url.replace("104", "104,202"),
        url.replace("104", "202,104"),
        url.replace("104", "202"),
        format!("{url}&extra=1"),
        url.replace("456", "0456"),
        url.replace("456", "abc"),
        url.replace("/3/", "/03/"),
        url.replace("/3/", "/254/"),
        url.replace("/3/", "/0/"),
        url.replace("https://", "http://"),
        format!("{url}#fragment"),
        url.replace("/?", "?"),
    ] {
        assert!(!is_allowed_artifact_url(&bad), "{bad}");
        assert!(!is_allowed_profile_url(&bad), "{bad}");
    }
    let bad = DestinyMembership::new(MembershipType::new(3), MembershipId::new("0456")).unwrap();
    assert!(artifact_endpoint(&bad).is_err());
}
#[test]
fn artifact_absence_is_not_an_error() {
    for response in [
        json!({}),
        json!({"profileProgression": {}}),
        json!({"profileProgression": {"data": null}}),
        json!({"profileProgression": {"data": {}}}),
        json!({"profileProgression": {"data": {"seasonalArtifact": null}}}),
        json!({"profileProgression": {"data": {"seasonalArtifact": {}}}}),
        json!({"profileProgression": {"data": {"seasonalArtifact": {"pointProgression": null}}}}),
    ] {
        assert_eq!(
            decode(&json!({"ErrorCode": 1, "Response": response})).unwrap(),
            ArtifactSnapshot { profile: None }
        );
    }
}
#[test]
fn artifact_malformed_and_bounded_payloads_fail_closed() {
    let base = fixture();
    for field in ["level", "levelCap", "progressToNextLevel", "nextLevelAt"] {
        for bad in [
            json!(-1),
            json!(4294967296_u64),
            json!(0.5),
            json!("12"),
            json!(null),
        ] {
            let mut value = base.clone();
            value["Response"]["profileProgression"]["data"]["seasonalArtifact"]["pointProgression"]
                [field] = bad;
            assert_eq!(
                decode(&value).unwrap_err().category(),
                ErrorCategory::ProviderValidation
            );
        }
        let mut value = base.clone();
        value["Response"]["profileProgression"]["data"]["seasonalArtifact"]["pointProgression"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(decode(&value).is_err());
        value["Response"]["profileProgression"]["data"]["seasonalArtifact"]["pointProgression"]
            [field] = json!(u32::MAX);
        assert!(decode(&value).is_ok());
    }
    for (pointer, bad) in [
        ("/ErrorCode", json!(2)),
        ("/Response", json!(null)),
        (
            "/Response/profileProgression/data/seasonalArtifact/pointProgression",
            json!([]),
        ),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(decode(&value).is_err());
    }
    assert!(decode_artifact(&vec![b' '; ARTIFACT_RESPONSE_LIMIT + 1]).is_err());
    assert!(decode_artifact(b"not json").is_err());
}
