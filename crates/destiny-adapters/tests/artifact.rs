use destiny_adapters::artifact::{
    ARTIFACT_RESPONSE_LIMIT, artifact_endpoint, decode_artifact, is_allowed_artifact_url,
};
use destiny_adapters::is_allowed_profile_url;
use destiny_core::*;
use serde_json::{Value, json};

fn fixture() -> Value {
    let tiers: Vec<_> = (0..5)
        .map(|i| {
            json!({
                "tierHash": 100 + i, "isUnlocked": i < 3, "pointsToUnlock": i,
                "items": [
                    {"itemHash": 200 + i, "isActive": true, "isVisible": true},
                    {"itemHash": 300 + i, "isActive": false, "isVisible": true},
                    {"itemHash": 400 + i, "isActive": true, "isVisible": false}
                ]
            })
        })
        .collect();
    let character = json!({"seasonalArtifact": {
        "artifactHash": 1234, "pointsUsed": 12, "resetCount": 2, "tiers": tiers
    }});
    json!({"ErrorCode": 1, "Response": {
        "profileProgression": {"data": {"seasonalArtifact": {
            "artifactHash": 1234, "pointsAcquired": 12, "powerBonus": 9,
            "pointProgression": {"progressionHash": 99}, "powerBonusProgression": {}
        }}},
        "characterProgressions": {"data": {"123456789": character, "987654321": character}}
    }})
}
fn decode(value: &Value) -> Result<ArtifactSnapshot, AppError> {
    decode_artifact(&serde_json::to_vec(value).unwrap())
}
#[test]
fn artifact_url_is_exact_and_canonical() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("456")).unwrap();
    let url = artifact_endpoint(&membership).unwrap();
    assert_eq!(
        url,
        "https://www.bungie.net/Platform/Destiny2/3/Profile/456/?components=104,202"
    );
    assert!(is_allowed_artifact_url(&url));
    assert!(is_allowed_profile_url(&url));
    for bad in [
        url.replace("104,202", "202,104"),
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
fn artifact_fixture_projects_profile_characters_and_visible_perks() {
    let snapshot = decode(&fixture()).unwrap();
    assert_eq!(
        snapshot.profile,
        Some(ArtifactProfileState {
            artifact_hash: ItemHash::new(1234),
            points_acquired: 12,
            power_bonus: 9
        })
    );
    assert_eq!(snapshot.characters.len(), 2);
    let c = &snapshot.characters[0];
    assert_eq!(c.character_id, CharacterId::new("123456789"));
    assert_eq!(c.artifact_hash, ItemHash::new(1234));
    assert_eq!((c.points_used, c.reset_count), (12, 2));
    assert_eq!(c.tiers.len(), 5);
    assert_eq!(c.tiers[0].tier_hash, 100);
    assert!(c.tiers[0].is_unlocked);
    assert!(!c.tiers[4].is_unlocked);
    assert_eq!(c.tiers[4].points_to_unlock, 4);
    assert_eq!(
        c.tiers[0].perks,
        vec![
            ArtifactPerk {
                item_hash: ItemHash::new(200),
                active: true
            },
            ArtifactPerk {
                item_hash: ItemHash::new(300),
                active: false
            }
        ]
    );
    assert!(!format!("{snapshot:?}").contains("123456789"));
}
#[test]
fn artifact_absence_is_not_an_error() {
    for response in [
        json!({}),
        json!({"profileProgression": {}, "characterProgressions": {}}),
        json!({"profileProgression": {"data": null}, "characterProgressions": {"data": null}}),
        json!({"profileProgression": {"data": {}}, "characterProgressions": {"data": {"123": {}}}}),
    ] {
        assert_eq!(
            decode(&json!({"ErrorCode": 1, "Response": response})).unwrap(),
            ArtifactSnapshot {
                profile: None,
                characters: vec![]
            }
        );
    }
    let mut value = fixture();
    value["Response"]["characterProgressions"]["data"]["123456789"] = json!({});
    assert_eq!(decode(&value).unwrap().characters.len(), 1);
}
#[test]
fn artifact_malformed_and_bounded_payloads_fail_closed() {
    let base = fixture();
    for (pointer, bad) in [
        ("/ErrorCode", json!(2)),
        ("/Response", json!(null)),
        (
            "/Response/profileProgression/data/seasonalArtifact/pointsAcquired",
            json!(-1),
        ),
        (
            "/Response/profileProgression/data/seasonalArtifact/artifactHash",
            json!(4294967296_u64),
        ),
        (
            "/Response/characterProgressions/data/123456789/seasonalArtifact/resetCount",
            json!(0.5),
        ),
        (
            "/Response/characterProgressions/data/123456789/seasonalArtifact/tiers/0/isUnlocked",
            json!("true"),
        ),
        (
            "/Response/characterProgressions/data/123456789/seasonalArtifact/tiers/0/items/0/isVisible",
            json!(null),
        ),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert_eq!(
            decode(&value).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
    }
    for (pointer, count) in [
        (
            "/Response/characterProgressions/data/123456789/seasonalArtifact/tiers",
            17,
        ),
        (
            "/Response/characterProgressions/data/123456789/seasonalArtifact/tiers/0/items",
            33,
        ),
    ] {
        let mut value = base.clone();
        let list = value.pointer_mut(pointer).unwrap();
        *list = json!(vec![list[0].clone(); count]);
        assert!(decode(&value).is_err());
    }
    for key in ["0123", "abc", "", "123/456"] {
        let mut value = base.clone();
        value["Response"]["characterProgressions"]["data"]
            .as_object_mut()
            .unwrap()
            .insert(key.into(), json!({}));
        assert!(decode(&value).is_err());
    }
    let mut value = base.clone();
    let data = value["Response"]["characterProgressions"]["data"]
        .as_object_mut()
        .unwrap();
    data.clear();
    for i in 0..9 {
        data.insert(i.to_string(), json!({}));
    }
    assert!(decode(&value).is_err());
    value["Response"]["characterProgressions"]["data"]
        .as_object_mut()
        .unwrap()
        .remove("8");
    assert!(decode(&value).is_ok());
    let mut value = base.clone();
    let tiers = &mut value["Response"]["characterProgressions"]["data"]["123456789"]["seasonalArtifact"]
        ["tiers"];
    tiers[0]["items"] = json!(vec![tiers[0]["items"][0].clone(); 32]);
    *tiers = json!(vec![tiers[0].clone(); 16]);
    assert!(decode(&value).is_ok());
    value["Response"]["characterProgressions"]["data"]
        .as_object_mut()
        .unwrap()
        .insert("1".repeat(129), json!({}));
    assert!(decode(&value).is_err());
    assert!(decode_artifact(&vec![b' '; ARTIFACT_RESPONSE_LIMIT + 1]).is_err());
    assert!(decode_artifact(b"not json").is_err());
    assert!(
        decode_artifact(
            br#"{"ErrorCode":1,"Response":{"characterProgressions":{"data":{"123":{},"123":{}}}}}"#
        )
        .is_err()
    );
    let mut value = base;
    value["Response"]["profileProgression"]["data"]["seasonalArtifact"]
        .as_object_mut()
        .unwrap()
        .remove("powerBonus");
    assert!(decode(&value).is_err());
}
#[test]
fn artifact_constructed_snapshot_validates_inclusive_bounds() {
    let mut snapshot = decode(&fixture()).unwrap();
    let mut c = snapshot.characters[0].clone();
    c.tiers[0].perks = vec![c.tiers[0].perks[0].clone(); 32];
    c.tiers = vec![c.tiers[0].clone(); 16];
    snapshot.characters = (0..8)
        .map(|i| {
            let mut c = c.clone();
            c.character_id = CharacterId::new(i.to_string());
            c
        })
        .collect();
    assert!(snapshot.validate().is_ok());
    let base = snapshot.clone();
    snapshot.characters.push(c.clone());
    assert!(snapshot.validate().is_err());
    snapshot = base.clone();
    snapshot.characters[1].character_id = snapshot.characters[0].character_id.clone();
    assert!(snapshot.validate().is_err());
    snapshot = base.clone();
    snapshot.characters[0].tiers.push(c.tiers[0].clone());
    assert!(snapshot.validate().is_err());
    snapshot = base;
    snapshot.characters[0].tiers[0].perks.push(ArtifactPerk {
        item_hash: ItemHash::new(1),
        active: false,
    });
    assert!(snapshot.validate().is_err());
}
