use destiny_app::{
    CharacterId, CharacterLoadoutsView, ItemHash, ItemInstanceId, LoadoutItemView, LoadoutPlugView,
    LoadoutSelector, LoadoutView, LoadoutsResult,
};
use serde_json::{Value, json};
use std::io::Write;

fn fixture() -> LoadoutsResult {
    let plug = |text: &str| LoadoutPlugView {
        socket_index: 2,
        plug_hash: ItemHash::new(10),
        name: Some("Perk".into()),
        description: Some("Official tooltip".repeat(100)),
        clarity: Some(text.repeat(100)),
        clarity_match: Some("variant".into()),
        subclass_kind: None,
        empty: false,
    };
    let item = |id: &str, text: &str| LoadoutItemView {
        instance_id: ItemInstanceId::new(id),
        item_hash: Some(ItemHash::new(if id == "a" { 1 } else { 2 })),
        name: Some("Weapon".into()),
        item_type: Some("Auto Rifle".into()),
        bucket_hash: Some(1498876634),
        element: None,
        missing: false,
        plugs: vec![plug(text), plug(text)],
    };
    LoadoutsResult {
        characters: vec![CharacterLoadoutsView {
            character_id: CharacterId::new("1"),
            class: Some("Hunter".into()),
            loadouts: (1..=4)
                .map(|slot| LoadoutView {
                    slot,
                    name: Some(format!("Build {slot}")),
                    name_hash: 7,
                    color_hash: 8,
                    icon_hash: 9,
                    items: vec![
                        item("a", "Parent A research"),
                        item("b", "Parent B research"),
                    ],
                })
                .collect(),
        }],
    }
}

#[test]
fn summary_is_small_and_contains_no_plug_text() {
    let result = fixture();
    let summary = destiny_output::loadouts::loadouts_json(&result);
    let old = json!({"kind":"loadouts", "characters": [{"character_id":"1", "class":"Hunter",
        "loadouts": result.characters[0].loadouts.iter().map(|loadout| json!({
            "slot":loadout.slot,"name":loadout.name,"name_hash":loadout.name_hash,
            "color_hash":loadout.color_hash,"icon_hash":loadout.icon_hash,
            "items":loadout.items.iter().map(|item| json!({
                "instance_id":item.instance_id.as_str(),"item_hash":item.item_hash.unwrap().value(),
                "name":item.name,"item_type":item.item_type,"bucket_hash":item.bucket_hash,"missing":item.missing,
                "plugs":item.plugs.iter().map(|plug| json!({"socket_index":plug.socket_index,
                    "plug_hash":plug.plug_hash.value(),"name":plug.name,"subclass_kind":null,"empty":plug.empty,
                    "description":plug.description,"clarity":plug.clarity,"clarity_match":plug.clarity_match})).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>() } ]});
    let before = old.to_string().len();
    let after = summary.to_string().len();
    writeln!(
        std::io::stderr(),
        "loadout fixture bytes: old={before}, summary={after}"
    )
    .unwrap();
    assert!(after < 10_000);
    assert!(after * 10 < before);
    for key in ["plugs", "description", "clarity", "clarity_match"] {
        assert!(!summary.to_string().contains(key));
    }
    assert_eq!(
        destiny_mcp::service::loadouts_output(&result)
            .into_call_tool_result()
            .structured_content
            .unwrap(),
        summary
    );
}

#[test]
fn detail_resolves_slot_name_and_never_guesses() {
    let result = fixture();
    for selector in [
        LoadoutSelector::Slot(2),
        LoadoutSelector::Name("  bUiLd 2 ".into()),
    ] {
        let selected = destiny_app::loadouts::select_loadout(&result, &selector).unwrap();
        assert_eq!(selected.loadout.slot, 2);
    }
    assert!(matches!(
        destiny_app::loadouts::select_loadout(&result, &LoadoutSelector::Slot(0)),
        Err(destiny_app::ApplyLoadoutError::InvalidSlot)
    ));
    assert!(matches!(
        destiny_app::loadouts::select_loadout(&result, &LoadoutSelector::Slot(5)),
        Err(destiny_app::ApplyLoadoutError::EmptySlot)
    ));
    assert!(matches!(
        destiny_app::loadouts::select_loadout(&result, &LoadoutSelector::Name("Build".into())),
        Err(destiny_app::ApplyLoadoutError::UnknownName(_))
    ));
    let mut duplicate = result;
    duplicate.characters[0].loadouts[1].name = Some("Build 1".into());
    assert!(matches!(
        destiny_app::loadouts::select_loadout(&duplicate, &LoadoutSelector::Name("Build 1".into())),
        Err(destiny_app::ApplyLoadoutError::AmbiguousName(_))
    ));
    duplicate.characters.push(duplicate.characters[0].clone());
    assert!(destiny_app::loadouts::select_loadout(&duplicate, &LoadoutSelector::Slot(1)).is_err());
}

#[test]
fn content_dedup_preserves_same_hash_parent_variants_and_cross_hash_duplicates() {
    let mut result = fixture();
    result.characters[0].loadouts[0].items[0].plugs[1].plug_hash = ItemHash::new(11);
    let selected =
        destiny_app::loadouts::select_loadout(&result, &LoadoutSelector::Slot(1)).unwrap();
    let detail = destiny_output::loadouts::loadout_json(&selected);
    let insights = detail["plug_insights"].as_array().unwrap();
    assert_eq!(insights.len(), 2);
    for (item_index, item) in detail["items"].as_array().unwrap().iter().enumerate() {
        for plug in item["plugs"].as_array().unwrap() {
            assert!(plug.get("description").is_none());
            assert!(plug.get("clarity").is_none());
            let index = plug["insight"].as_u64().unwrap() as usize;
            assert_eq!(index, item_index);
            let source = &selected.loadout.items[item_index].plugs[0];
            assert_eq!(insights[index]["description"], json!(source.description));
            assert_eq!(insights[index]["clarity"], json!(source.clarity));
            assert_eq!(insights[index]["clarity_match"], "variant");
        }
    }
    let mcp = destiny_mcp::service::loadout_output(&selected).into_call_tool_result();
    assert_eq!(mcp.structured_content.unwrap(), detail);
    assert_eq!(destiny_output::loadouts::loadout_json(&selected), detail);
    writeln!(
        std::io::stderr(),
        "loadout fixture detail bytes: {}",
        detail.to_string().len()
    )
    .unwrap();
    let mut no_text = selected;
    no_text.loadout.items[0].plugs[0].description = None;
    no_text.loadout.items[0].plugs[0].clarity = None;
    no_text.loadout.items[0].plugs[0].clarity_match = None;
    let no_text: Value = destiny_output::loadouts::loadout_json(&no_text);
    assert!(no_text["items"][0]["plugs"][0].get("insight").is_none());
}
