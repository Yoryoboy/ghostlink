use destiny_adapters::is_allowed_profile_url;
use destiny_adapters::loadout::{
    LOADOUT_RESPONSE_LIMIT, decode_loadouts, is_allowed_loadout_url, loadout_endpoint,
};
use destiny_core::*;
use serde_json::{Value, json};

fn item(id: &str) -> Value {
    json!({"itemInstanceId":id,"plugItemHashes":[10,2166136261u32,20]})
}
fn slot(items: Vec<Value>) -> Value {
    json!({"nameHash":1,"colorHash":2,"iconHash":3,"items":items})
}
fn envelope(data: Value) -> Value {
    json!({"ErrorCode":1,"Response":{"characterLoadouts":{"data":data}}})
}
fn decode(value: Value) -> Result<LoadoutSnapshot, AppError> {
    decode_loadouts(&serde_json::to_vec(&value).unwrap())
}
#[test]
fn loadout_url_is_exact_and_credential_allowlisted() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("123")).unwrap();
    let url = loadout_endpoint(&membership).unwrap();
    assert_eq!(
        url,
        "https://www.bungie.net/Platform/Destiny2/3/Profile/123/?components=206"
    );
    assert!(is_allowed_loadout_url(&url));
    assert!(is_allowed_profile_url(&url));
    for bad in [
        url.replace("https:", "http:"),
        url.replace("www.bungie.net", "evil.test"),
        url.replace("/3/", "/03/"),
        url.replace("/3/", "/254/"),
        url.replace("/3/", "/0/"),
        url.replace("/123/", "/0123/"),
        url.replace("/123/", "/%31/"),
        format!("{url}&x=1"),
        format!("{url}#x"),
        url.replace("206", "104"),
        url.replace("206", "206,200"),
    ] {
        assert!(!is_allowed_loadout_url(&bad), "{bad}");
    }
    assert!(
        loadout_endpoint(
            &DestinyMembership::new(MembershipType::new(3), MembershipId::new("0123")).unwrap()
        )
        .is_err()
    );
}
#[test]
fn loadout_realistic_slots_preserve_positions() {
    let used = slot(vec![item("123"), item("0")]);
    let unused = slot(vec![item("0"); 10]);
    let snapshot = decode(envelope(json!({"1":{"loadouts":[used,unused,slot(vec![item("456")]),unused,unused,unused]},"2":{"loadouts":[unused,unused,unused,unused,unused,unused]},"3":{"loadouts":[unused,unused,unused,unused,unused,unused]}}))).unwrap();
    assert_eq!(snapshot.characters.len(), 3);
    let c = snapshot
        .characters
        .iter()
        .find(|c| c.character_id.as_str() == "1")
        .unwrap();
    assert_eq!(c.loadouts.len(), 2);
    assert_eq!(c.loadouts[0].slot_index, 0);
    assert_eq!(c.loadouts[1].slot_index, 2);
    let l = &c.loadouts[0];
    assert_eq!((l.name_hash, l.color_hash, l.icon_hash), (1, 2, 3));
    assert_eq!(l.items.len(), 1);
    assert_eq!(l.items[0].instance_id.as_str(), "123");
    assert_eq!(
        l.items[0].plugs,
        vec![
            LoadoutPlug {
                socket_index: 0,
                plug_hash: ItemHash::new(10)
            },
            LoadoutPlug {
                socket_index: 2,
                plug_hash: ItemHash::new(20)
            }
        ]
    );
    assert!(
        snapshot
            .characters
            .iter()
            .filter(|c| c.character_id.as_str() != "1")
            .all(|c| c.loadouts.is_empty())
    );
    snapshot.validate().unwrap();
}
#[test]
fn loadout_absent_private_and_malformed() {
    for response in [
        json!({}),
        json!({"characterLoadouts":{ "privacy":2 }}),
        json!({"characterLoadouts":{"data":null}}),
    ] {
        assert!(
            decode(json!({"ErrorCode":1,"Response":response}))
                .unwrap()
                .characters
                .is_empty()
        );
    }
    for bad in [
        json!({}),
        json!({"ErrorCode":2,"Response":{}}),
        envelope(json!({"01":{"loadouts":[]}})),
        envelope(json!({"x":{"loadouts":[]}})),
        envelope(json!({"1":{"loadouts":[slot(vec![item("01")])]}})),
        envelope(json!({"1":{"loadouts":[{"items":[]}]}})),
        envelope(
            json!({"1":{"loadouts":[slot(vec![json!({"itemInstanceId":"1","plugItemHashes":[-1]})])]}}),
        ),
    ] {
        assert!(decode(bad).is_err());
    }
    assert!(decode_loadouts(b"not json").is_err());
    assert!(decode_loadouts(br#"{"ErrorCode":1,"Response":{"characterLoadouts":{"data":{"1":{"loadouts":[]},"1":{"loadouts":[]}}}}}"#).is_err());
}
#[test]
fn loadout_bounds_checked_before_filtering() {
    let data: serde_json::Map<String, Value> = (1..=8).map(|n|(n.to_string(),json!({"loadouts":vec![slot(vec![json!({"itemInstanceId":"1","plugItemHashes":vec![1;32]});16]);12]}))).collect();
    let snapshot = decode(envelope(Value::Object(data.clone()))).unwrap();
    snapshot.validate().unwrap();
    let mut too_many = data;
    too_many.insert("9".into(), json!({"loadouts":[]}));
    assert!(decode(envelope(Value::Object(too_many))).is_err());
    for slots in [
        vec![slot(vec![item("0")]); 13],
        vec![slot(vec![item("0"); 17])],
        vec![slot(vec![
            json!({"itemInstanceId":"0","plugItemHashes":vec![2166136261u32;33]}),
        ])],
    ] {
        assert!(decode(envelope(json!({"1":{"loadouts":slots}}))).is_err());
    }
    assert!(decode_loadouts(&vec![b' '; LOADOUT_RESPONSE_LIMIT + 1]).is_err());
    let mut duplicate = LoadoutSnapshot {
        characters: vec![snapshot.characters[0].clone()],
    };
    duplicate.characters.push(snapshot.characters[0].clone());
    assert!(duplicate.validate().is_err());
    let mut bad = snapshot.clone();
    bad.characters[0].character_id = CharacterId::new("01");
    assert!(bad.validate().is_err());
    let mut bad = snapshot;
    bad.characters[0].loadouts[0].items[0].plugs[0].socket_index = 32;
    assert!(bad.validate().is_err());
}

#[test]
fn loadout_constructed_snapshot_validation() {
    let valid = decode(envelope(
        json!({"1":{"loadouts":[slot(vec![item("123")])]}}),
    ))
    .unwrap();
    let mut cases = Vec::new();
    let mut bad = valid.clone();
    bad.characters = vec![valid.characters[0].clone(); 9];
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0].loadouts = vec![valid.characters[0].loadouts[0].clone(); 13];
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0].loadouts[0].slot_index = 12;
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0]
        .loadouts
        .push(valid.characters[0].loadouts[0].clone());
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0].loadouts[0].items.clear();
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0].loadouts[0].items =
        vec![valid.characters[0].loadouts[0].items[0].clone(); 17];
    cases.push(bad);
    for id in ["0", "01", "", "x"] {
        let mut bad = valid.clone();
        bad.characters[0].loadouts[0].items[0].instance_id = ItemInstanceId::new(id);
        cases.push(bad);
    }
    let mut bad = valid.clone();
    bad.characters[0].loadouts[0].items[0].plugs =
        vec![valid.characters[0].loadouts[0].items[0].plugs[0].clone(); 33];
    cases.push(bad);
    let mut bad = valid.clone();
    bad.characters[0].loadouts[0].items[0]
        .plugs
        .push(valid.characters[0].loadouts[0].items[0].plugs[0].clone());
    cases.push(bad);
    for hash in [-1, i64::from(u32::MAX) + 1, i64::from(UNSET_LOADOUT_PLUG)] {
        let mut bad = valid.clone();
        bad.characters[0].loadouts[0].items[0].plugs[0].plug_hash = ItemHash::new(hash);
        cases.push(bad);
    }
    for bad in cases {
        assert_eq!(
            bad.validate().unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
    }
    let mut exact_limit = serde_json::to_vec(&json!({"ErrorCode":1,"Response":{}})).unwrap();
    exact_limit.resize(LOADOUT_RESPONSE_LIMIT, b' ');
    assert!(decode_loadouts(&exact_limit).unwrap().characters.is_empty());
}
