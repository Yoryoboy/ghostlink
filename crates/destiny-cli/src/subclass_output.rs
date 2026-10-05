//! Snake-case JSON projection of equipped subclasses.
use serde_json::{Value, json};

/// Stable CLI envelope; slots retain socket order.
pub fn subclass_json(result: &destiny_app::SubclassResult) -> Value {
    use destiny_app::SubclassPlugKind::*;
    json!({"kind": "subclass", "characters": result.characters.iter().map(|character| json!({
        "character_id": character.character_id.as_str(), "class": character.class,
        "subclass": character.subclass.as_ref().map(|config| json!({
            "instance_id": config.instance_id.as_str(), "item_hash": config.item_hash.value(),
            "name": config.name, "element": config.element,
            "slots": config.slots.iter().map(|slot| super::with_description(json!({
                "kind": match slot.kind {
                    Super => "super", Aspect => "aspect", Fragment => "fragment", Grenade => "grenade",
                    Melee => "melee", ClassAbility => "class_ability", Movement => "movement", Transcendence => "transcendence",
                },
                "plug_hash": slot.plug_hash.value(), "name": slot.name, "empty": slot.empty,
            }), slot.description.as_deref())).collect::<Vec<_>>()
        }))
    })).collect::<Vec<_>>()})
}
