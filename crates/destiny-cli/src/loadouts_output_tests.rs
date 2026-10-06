use super::*;
use destiny_app::{
    CharacterId, CharacterLoadoutsView, ItemHash, ItemInstanceId, LoadoutItemView, LoadoutPlugView,
    LoadoutView, LoadoutsResult, SubclassPlugKind,
};

#[test]
fn loadout_descriptions_are_optional_and_follow_detail_visibility() {
    let mut result = fixture();
    let items = &mut result.characters[0].loadouts[0].items;
    items[0].plugs[0].description = Some("Official weapon tooltip.".into());
    items[2].plugs[0].description = Some("Official fragment tooltip.".into());
    let value = loadouts_json(&result);
    let items = &value["characters"][0]["loadouts"][0]["items"];
    assert_eq!(
        items[0]["plugs"][0]["description"],
        "Official weapon tooltip."
    );
    assert!(items[0]["plugs"][1].get("description").is_none());
    let summary = render_loadouts(OutputMode::Human, &result, false);
    assert!(summary.contains("        F1:\n          Official fragment tooltip."));
    assert!(!summary.contains("Official weapon tooltip."));
    let detail = render_loadouts(OutputMode::Human, &result, true);
    assert!(detail.contains("        Countermass:\n          Official weapon tooltip."));
    assert_eq!(detail.matches("Official fragment tooltip.").count(), 1);
    let artifact = &mut result.characters[0].loadouts[0].items[3];
    artifact.plugs[0].description = Some("Artifact effect".into());
    for detail in [false, true] {
        let text = render_loadouts(OutputMode::Human, &result, detail);
        assert!(text.contains("\n        Artifact effect"));
        assert!(!text.contains("        perk1:"));
    }
}

#[test]
fn loadout_clarity_follows_detail_visibility() {
    let mut result = fixture();
    let items = &mut result.characters[0].loadouts[0].items;
    items[0].plugs[0].clarity = Some("Weapon research".into());
    items[0].plugs[0].clarity_match = Some("variant".into());
    items[2].plugs[0].clarity = Some("Fragment research".into());
    let summary = render_loadouts(OutputMode::Human, &result, false);
    assert!(!summary.contains("Weapon research"));
    assert!(summary.contains("        F1:\n          Clarity:\n            Fragment research"));
    let detail = render_loadouts(OutputMode::Human, &result, true);
    assert!(detail.contains(
        "        Countermass:\n          Clarity (variant):\n            Weapon research"
    ));
    let json = loadouts_json(&result);
    assert_eq!(
        json["characters"][0]["loadouts"][0]["items"][0]["plugs"][0]["clarity"],
        "Weapon research"
    );
    assert!(
        json["characters"][0]["loadouts"][0]["items"][0]["plugs"][1]
            .get("clarity")
            .is_none()
    );
}

fn fixture() -> LoadoutsResult {
    let item = |id: &str, hash, name: &str, bucket, item_type: &str, plugs| LoadoutItemView {
        instance_id: ItemInstanceId::new(id),
        item_hash: Some(ItemHash::new(hash)),
        name: Some(name.into()),
        item_type: Some(item_type.into()),
        bucket_hash: Some(bucket),
        missing: false,
        plugs,
    };
    let plug = |index, hash, name: Option<&str>, kind, empty| LoadoutPlugView {
        socket_index: index,
        plug_hash: ItemHash::new(hash),
        name: name.map(str::to_owned),
        description: None,
        clarity: None,
        clarity_match: None,
        subclass_kind: kind,
        empty,
    };
    LoadoutsResult {
        characters: vec![
            CharacterLoadoutsView {
                character_id: CharacterId::new("10"),
                class: Some("Hunter".into()),
                loadouts: vec![LoadoutView {
                    slot: 3,
                    name: Some("Stasis".into()),
                    name_hash: 7,
                    color_hash: 8,
                    icon_hash: 9,
                    items: vec![
                        item(
                            "w",
                            1,
                            "Alethonym",
                            1498876634,
                            "Grenade Launcher",
                            vec![
                                plug(2, 11, Some("Countermass"), None, false),
                                plug(4, 12, Some("Empty Mod Socket"), None, true),
                                plug(5, 13, None, None, false),
                            ],
                        ),
                        item("a", 2, "Mask of Fealty", 3448274439, "Helmet", vec![]),
                        item(
                            "s",
                            3,
                            "Revenant",
                            3284755031,
                            "Subclass",
                            vec![
                                plug(0, 20, Some("F1"), Some(SubclassPlugKind::Fragment), false),
                                plug(1, 21, Some("X"), Some(SubclassPlugKind::Super), false),
                                plug(2, 22, Some("A"), Some(SubclassPlugKind::Aspect), false),
                                plug(3, 23, Some("F2"), Some(SubclassPlugKind::Fragment), false),
                            ],
                        ),
                        item(
                            "r",
                            4,
                            "Satchel",
                            1506418338,
                            "Artifact",
                            vec![plug(0, 30, Some("perk1"), None, false)],
                        ),
                        LoadoutItemView {
                            instance_id: ItemInstanceId::new("404"),
                            item_hash: None,
                            name: None,
                            item_type: None,
                            bucket_hash: None,
                            missing: true,
                            plugs: vec![],
                        },
                    ],
                }],
            },
            CharacterLoadoutsView {
                character_id: CharacterId::new("20"),
                class: Some("Warlock".into()),
                loadouts: vec![],
            },
        ],
    }
}
#[test]
fn loadouts_compact_groups_items_and_subclass_plugs() {
    assert_eq!(
        render_loadouts(OutputMode::Human, &fixture(), false),
        "Hunter\n  [3] Stasis\n      Weapons: Alethonym\n      Armor: Mask of Fealty\n      Subclass: Revenant — Super: X; Aspects: A; Fragments: F1, F2\n      Artifact: Satchel — perk1\n      (missing #404)\nWarlock\n  no saved loadouts"
    );
}
#[test]
fn loadouts_detail_lists_every_item_and_nonempty_plugs() {
    let text = render_loadouts(OutputMode::Human, &fixture(), true);
    assert!(text.contains("      Alethonym (Grenade Launcher): Countermass, #13"));
    assert!(text.contains("      Mask of Fealty (Helmet)\n"));
    assert!(text.contains("      Revenant (Subclass): F1, X, A, F2"));
    assert!(!text.contains("Empty Mod Socket"));
}
#[test]
fn loadouts_unknown_names_missing_instances_and_empty_results() {
    let mut result = fixture();
    result.characters[0].class = None;
    let saved = &mut result.characters[0].loadouts[0];
    saved.name = None;
    saved.items[0].name = None;
    let text = render_loadouts(OutputMode::Human, &result, false);
    assert!(text.starts_with("Unknown\n  [3] #7\n      Weapons: #1"));
    assert!(text.contains("(missing #404)"));
    result.characters[0].loadouts[0].name = Some("Stasis\nInjected".into());
    assert!(render_loadouts(OutputMode::Human, &result, false).contains("Stasis?Injected"));
    result.characters.clear();
    assert_eq!(
        render_loadouts(OutputMode::Human, &result, false),
        "no characters"
    );
}
#[test]
fn loadouts_json_is_full_snake_case_and_ignores_detail() {
    let result = fixture();
    let value = loadouts_json(&result);
    assert_eq!(value["kind"], "loadouts");
    let character = &value["characters"][0];
    assert_eq!(character["character_id"], "10");
    assert_eq!(character["class"], "Hunter");
    let saved = &character["loadouts"][0];
    assert_eq!(
        (
            saved["slot"].as_u64(),
            saved["name_hash"].as_u64(),
            saved["color_hash"].as_u64(),
            saved["icon_hash"].as_u64()
        ),
        (Some(3), Some(7), Some(8), Some(9))
    );
    assert_eq!(saved["name"], "Stasis");
    assert_eq!(
        saved["items"][0],
        json!({"instance_id":"w","item_hash":1,"name":"Alethonym","item_type":"Grenade Launcher","bucket_hash":1498876634_u32,"missing":false,"plugs":[
            {"socket_index":2,"plug_hash":11,"name":"Countermass","subclass_kind":null,"empty":false},
            {"socket_index":4,"plug_hash":12,"name":"Empty Mod Socket","subclass_kind":null,"empty":true},
            {"socket_index":5,"plug_hash":13,"name":null,"subclass_kind":null,"empty":false}
        ]})
    );
    assert_eq!(saved["items"][2]["plugs"][0]["subclass_kind"], "fragment");
    assert_eq!(
        saved["items"][4],
        json!({"instance_id":"404","item_hash":null,"name":null,"item_type":null,"bucket_hash":null,"missing":true,"plugs":[]})
    );
    assert_eq!(value["characters"][1]["loadouts"], json!([]));
    for detail in [false, true] {
        assert_eq!(
            serde_json::from_str::<Value>(&render_loadouts(OutputMode::Json, &result, detail))
                .unwrap(),
            value
        );
    }
}
