use destiny_adapters::weekly_profile::{decode_character_weekly, weekly_profile_endpoint};
use destiny_adapters::{is_allowed_profile_url, profile_endpoint};
use destiny_core::{CharacterId, DestinyMembership, ErrorCategory, MembershipId, MembershipType};
use serde_json::{Value, json};

const FIXTURE: &[u8] = include_bytes!("fixtures/character-weekly.json");
fn character() -> CharacterId {
    CharacterId::new("123456789")
}
fn decode(value: &Value) -> Result<destiny_core::CharacterWeeklySnapshot, destiny_core::AppError> {
    decode_character_weekly(&serde_json::to_vec(value).unwrap(), &character())
}
#[test]
fn weekly_fixture_projects_raid_dungeon_focus_and_milestone_progress() {
    let snapshot = decode_character_weekly(FIXTURE, &character()).unwrap();
    assert_eq!(snapshot.character_id, character());
    assert_eq!(snapshot.available_activities.len(), 3);
    let raid = &snapshot.available_activities[0];
    assert!(raid.completed_ever);
    assert!(!raid.challenges[0].complete);
    assert_eq!(raid.recommended_light, Some(2000));
    assert_eq!(raid.difficulty_tier, Some(2));
    assert_eq!(raid.modifier_hashes, [401]);
    assert!(snapshot.available_activities[1].challenges[0].complete);
    assert!(snapshot.available_activities[2].focused);
    assert_eq!(
        snapshot.available_activities[2].visible_reward_item_hashes,
        [301, 302]
    );
    let milestone = &snapshot.milestones[0];
    assert!(milestone.start.unwrap() < milestone.end.unwrap());
    assert_eq!(milestone.activities[0].challenges[0].progress, 0);
    assert!(milestone.activities[0].phases[0].complete);
    assert!(milestone.rewards[0].entries[0].earned);
    assert!(!milestone.rewards[0].entries[0].redeemed);
    assert!(!format!("{snapshot:?}").contains(character().as_str()));
}
#[test]
fn weekly_missing_components_are_sanitized() {
    for key in ["activities", "progressions"] {
        let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
        value["Response"].as_object_mut().unwrap().remove(key);
        value["Message"] = json!("private-payload-sentinel");
        let error = decode(&value).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert!(!format!("{error:?} {error}").contains("private-payload-sentinel"));
    }
}
#[test]
fn weekly_bounds_reject_oversize_lists_and_body() {
    let base: Value = serde_json::from_slice(FIXTURE).unwrap();
    for (pointer, count) in [
        ("/Response/activities/data/availableActivities", 513),
        (
            "/Response/activities/data/availableActivities/0/challenges",
            65,
        ),
        (
            "/Response/activities/data/availableActivities/0/modifierHashes",
            65,
        ),
        (
            "/Response/activities/data/availableActivities/2/visibleRewards",
            65,
        ),
        (
            "/Response/activities/data/availableActivities/2/visibleRewards/0/rewardItems",
            65,
        ),
        ("/Response/progressions/data/milestones/900/activities", 65),
        (
            "/Response/progressions/data/milestones/900/activities/0/challenges",
            65,
        ),
        (
            "/Response/progressions/data/milestones/900/activities/0/phases",
            65,
        ),
        ("/Response/progressions/data/milestones/900/rewards", 65),
        (
            "/Response/progressions/data/milestones/900/rewards/0/entries",
            65,
        ),
    ] {
        let mut value = base.clone();
        let list = value.pointer_mut(pointer).unwrap();
        *list = Value::Array(vec![list[0].clone(); count]);
        assert_eq!(
            decode(&value).unwrap_err().category(),
            ErrorCategory::ProviderValidation,
            "{pointer}"
        );
    }
    assert!(decode_character_weekly(&vec![b' '; 4 * 1024 * 1024 + 1], &character()).is_err());
}
#[test]
fn weekly_inclusive_bounds_and_aggregate_rewards() {
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    let raid = value["Response"]["activities"]["data"]["availableActivities"][0].clone();
    value["Response"]["activities"]["data"]["availableActivities"] = json!(vec![raid; 512]);
    let activity = &mut value["Response"]["activities"]["data"]["availableActivities"][0];
    activity["modifierHashes"] = json!(vec![401; 64]);
    let challenge = activity["challenges"][0].clone();
    activity["challenges"] = json!(vec![challenge; 64]);
    assert!(decode(&value).is_ok());
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    let rewards =
        &mut value["Response"]["activities"]["data"]["availableActivities"][2]["visibleRewards"];
    let reward = rewards[0].clone();
    *rewards = json!(vec![reward.clone(); 32]);
    assert_eq!(
        decode(&value).unwrap().available_activities[2]
            .visible_reward_item_hashes
            .len(),
        64
    );
    value["Response"]["activities"]["data"]["availableActivities"][2]["visibleRewards"] =
        json!(vec![reward; 33]);
    assert!(decode(&value).is_err());
}

#[test]
fn weekly_exact_allowlist_does_not_change_inventory_query() {
    let membership =
        DestinyMembership::new(MembershipType::new(3), MembershipId::new("456")).unwrap();
    let url = weekly_profile_endpoint(&membership, &character()).unwrap();
    assert_eq!(
        url,
        "https://www.bungie.net/Platform/Destiny2/3/Profile/456/Character/123456789/?components=202,204"
    );
    assert!(is_allowed_profile_url(&url));
    for bad in [
        url.replace("202,204", "204,202"),
        format!("{url}&extra=1"),
        url.replace("123456789", "0123"),
        url.replace("/Character/", "/Characters/"),
    ] {
        assert!(!is_allowed_profile_url(&bad));
    }
    assert!(weekly_profile_endpoint(&membership, &CharacterId::new("../secret")).is_err());
    assert_eq!(
        profile_endpoint(&membership),
        "https://www.bungie.net/Platform/Destiny2/3/Profile/456/?components=102,200,201,205,300,302,305"
    );
}

#[test]
fn weekly_milestone_keys_windows_and_numeric_types_fail_closed() {
    let base: Value = serde_json::from_slice(FIXTURE).unwrap();
    for (pointer, bad) in [
        ("/ErrorCode", json!(2)),
        (
            "/Response/progressions/data/milestones/900/milestoneHash",
            json!(901),
        ),
        (
            "/Response/progressions/data/milestones/900/startDate",
            json!("invalid"),
        ),
        (
            "/Response/progressions/data/milestones/900/endDate",
            json!("2020-01-01T00:00:00Z"),
        ),
        (
            "/Response/activities/data/availableActivities/0/activityHash",
            json!(-1),
        ),
        (
            "/Response/activities/data/availableActivities/0/challenges/0/objective/progress",
            json!(0.5),
        ),
    ] {
        let mut value = base.clone();
        *value.pointer_mut(pointer).unwrap() = bad;
        assert!(decode(&value).is_err());
    }
    let mut value = base.clone();
    let milestones = value["Response"]["progressions"]["data"]["milestones"]
        .as_object_mut()
        .unwrap();
    let record = milestones.remove("900").unwrap();
    for hash in 0..65 {
        let mut record = record.clone();
        record["milestoneHash"] = json!(hash);
        milestones.insert(hash.to_string(), record);
    }
    assert!(decode(&value).is_err());
    let mut value = base;
    value["Response"]["activities"]["data"]["availableActivities"] = json!([]);
    value["Response"]["progressions"]["data"]["milestones"] = json!({});
    assert!(decode(&value).unwrap().available_activities.is_empty());
}
