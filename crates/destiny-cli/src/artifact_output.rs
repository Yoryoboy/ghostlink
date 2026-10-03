//! Shared snake-case seasonal artifact projection for CLI and MCP.
use destiny_app::{ArtifactPerkView, ArtifactResult};
use serde_json::{Value, json};

fn perk_json(perk: &ArtifactPerkView) -> Value {
    json!({"item_hash": perk.item_hash.value(), "name": perk.name, "active": perk.active})
}

/// Stable full result envelope, retaining provider tier and perk order.
pub fn artifact_json(result: &ArtifactResult) -> Value {
    json!({
        "kind": "artifact",
        "artifact": result.artifact.as_ref().map(|artifact| json!({
            "artifact_hash": artifact.artifact_hash.value(), "name": artifact.name,
            "points_acquired": artifact.points_acquired, "power_bonus": artifact.power_bonus,
        })),
        "characters": result.characters.iter().map(|character| json!({
            "character_id": character.character_id.as_str(), "class": character.class,
            "equipped": character.equipped.as_ref().map(|equipped| json!({
                "instance_id": equipped.instance_id.as_str(), "item_hash": equipped.item_hash.value(), "name": equipped.name,
                "slots": equipped.slots.iter().map(|slot| json!({
                    "plug_hash": slot.plug_hash.value(), "name": slot.name, "empty": slot.empty,
                })).collect::<Vec<_>>(),
            })),
            "state": character.state.as_ref().map(|state| json!({
                "points_used": state.points_used, "reset_count": state.reset_count,
                "active_perks": state.active_perks.iter().map(perk_json).collect::<Vec<_>>(),
                "tiers": state.tiers.iter().map(|tier| json!({
                    "index": tier.index, "unlocked": tier.unlocked, "points_to_unlock": tier.points_to_unlock,
                    "perks": tier.perks.iter().map(perk_json).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })),
        })).collect::<Vec<_>>(),
    })
}
