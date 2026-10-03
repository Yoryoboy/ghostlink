//! Shared snake-case seasonal artifact projection for CLI and MCP.
use destiny_app::ArtifactResult;
use serde_json::{Value, json};

/// Stable full result envelope, retaining equipped socket order.
pub fn artifact_json(result: &ArtifactResult) -> Value {
    json!({
        "kind": "artifact",
        "points": result.points.as_ref().map(|points| json!({
            "points": points.points, "points_cap": points.points_cap,
            "progress_to_next_level": points.progress_to_next_level, "next_level_at": points.next_level_at,
        })),
        "characters": result.characters.iter().map(|character| json!({
            "character_id": character.character_id.as_str(), "class": character.class,
            "equipped": character.equipped.as_ref().map(|equipped| json!({
                "instance_id": equipped.instance_id.as_str(), "item_hash": equipped.item_hash.value(), "name": equipped.name,
                "slots": equipped.slots.iter().map(|slot| json!({
                    "plug_hash": slot.plug_hash.value(), "name": slot.name, "empty": slot.empty,
                })).collect::<Vec<_>>(),
            })),
        })).collect::<Vec<_>>(),
    })
}
