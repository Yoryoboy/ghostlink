//! Shared saved-loadout JSON contracts for CLI and MCP.
use destiny_app::loadouts::LoadoutDetailResult;
use destiny_app::{BucketHash, ItemSlot, LoadoutsResult, SubclassPlugKind};
use serde_json::{Value, json};

fn slot_name(bucket: Option<u32>) -> Option<&'static str> {
    match ItemSlot::from_equipment_bucket(BucketHash::new(i64::from(bucket?)))? {
        ItemSlot::Kinetic => Some("kinetic"),
        ItemSlot::Energy => Some("energy"),
        ItemSlot::Power => Some("power"),
        ItemSlot::Helmet => Some("helmet"),
        ItemSlot::Gauntlets => Some("gauntlets"),
        ItemSlot::Chest => Some("chest"),
        ItemSlot::Legs => Some("legs"),
        ItemSlot::ClassItem => Some("class_item"),
        _ => None,
    }
}

/// Compact discovery: never includes sockets, plug metadata, or insight text.
pub fn loadouts_json(result: &LoadoutsResult) -> Value {
    json!({"kind":"loadouts", "characters":result.characters.iter().map(|character| json!({
        "character_id":character.character_id.as_str(), "class":character.class,
        "loadouts":character.loadouts.iter().map(|loadout| {
            let subclass = loadout.items.iter().find(|item| item.bucket_hash == Some(3284755031));
            json!({"slot":loadout.slot,"name":loadout.name,
                "subclass":subclass.map(|item| json!({"name":item.name,"element":item.element})),
                "items":loadout.items.iter().filter(|item| !matches!(item.bucket_hash, Some(3284755031 | 1506418338)))
                    .map(|item| json!({"slot":slot_name(item.bucket_hash),"name":item.name})).collect::<Vec<_>>()})
        }).collect::<Vec<_>>()
    })).collect::<Vec<_>>()})
}

fn kind_json(kind: SubclassPlugKind) -> &'static str {
    use SubclassPlugKind::*;
    match kind {
        Super => "super",
        ClassAbility => "class_ability",
        Movement => "movement",
        Melee => "melee",
        Grenade => "grenade",
        Aspect => "aspect",
        Fragment => "fragment",
        Transcendence => "transcendence",
    }
}

/// Full detail with verbatim insight content interned in first-occurrence order.
/// Equality includes field presence as well as content, preserving parent-specific variants.
pub fn loadout_json(result: &LoadoutDetailResult) -> Value {
    let loadout = &result.loadout;
    let mut insights = Vec::<Value>::new();
    let items: Vec<_> = loadout.items.iter().map(|item| {
        let plugs: Vec<_> = item.plugs.iter().map(|plug| {
            let mut occurrence = json!({"socket_index":plug.socket_index,"plug_hash":plug.plug_hash.value(),
                "name":plug.name,"subclass_kind":plug.subclass_kind.map(kind_json),"empty":plug.empty});
            let mut insight = serde_json::Map::new();
            for (key, value) in [("description", &plug.description), ("clarity", &plug.clarity), ("clarity_match", &plug.clarity_match)] {
                if let Some(value) = value {
                    insight.insert(key.into(), json!(value));
                }
            }
            if !insight.is_empty() {
                let insight = Value::Object(insight);
                let index = insights.iter().position(|existing| existing == &insight).unwrap_or_else(|| {
                    let index = insights.len();
                    insights.push(insight);
                    index
                });
                occurrence["insight"] = json!(index);
            }
            occurrence
        }).collect();
        json!({"instance_id":item.instance_id.as_str(),"item_hash":item.item_hash.map(|hash| hash.value()),
            "name":item.name,"item_type":item.item_type,"bucket_hash":item.bucket_hash,"element":item.element,
            "missing":item.missing,"plugs":plugs})
    }).collect();
    json!({"kind":"loadout","character_id":result.character_id.as_str(),"class":result.class,
        "slot":loadout.slot,"name":loadout.name,"name_hash":loadout.name_hash,
        "color_hash":loadout.color_hash,"icon_hash":loadout.icon_hash,"items":items,"plug_insights":insights})
}

/// Reuse the existing typed and sanitized apply-loadout selector error vocabulary.
pub fn selector_error(
    error: destiny_app::ApplyLoadoutError,
) -> crate::apply_loadout::ApplyLoadoutOutput {
    let mut output = crate::apply_loadout::ApplyLoadoutOutput::from_result(Err(error));
    output.value["kind"] = json!("loadout");
    output
}
