//! Shared snake-case saved loadouts projection for CLI and MCP.
use destiny_app::{LoadoutItemView, LoadoutPlugView, LoadoutsResult, SubclassPlugKind};
use serde_json::{Value, json};

/// Stable full result envelope; saved slot and socket indices retain gaps.
pub fn loadouts_json(result: &LoadoutsResult) -> Value {
    json!({"kind": "loadouts", "characters": result.characters.iter().map(|character| json!({
        "character_id": character.character_id.as_str(), "class": character.class,
        "loadouts": character.loadouts.iter().map(|loadout| json!({
            "slot": loadout.slot, "name": loadout.name, "name_hash": loadout.name_hash,
            "color_hash": loadout.color_hash, "icon_hash": loadout.icon_hash,
            "items": loadout.items.iter().map(|item| json!({
                "instance_id": item.instance_id.as_str(), "item_hash": item.item_hash.map(|hash| hash.value()),
                "name": item.name, "item_type": item.item_type, "bucket_hash": item.bucket_hash,
                "missing": item.missing,
                "plugs": item.plugs.iter().map(|plug| json!({
                    "socket_index": plug.socket_index, "plug_hash": plug.plug_hash.value(),
                    "name": plug.name, "subclass_kind": plug.subclass_kind.map(kind_json), "empty": plug.empty,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
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

fn item_name(item: &LoadoutItemView) -> String {
    if item.missing {
        return format!("(missing #{})", super::sanitize(item.instance_id.as_str()));
    }
    item.name
        .as_deref()
        .map(super::sanitize)
        .unwrap_or_else(|| {
            item.item_hash
                .map(|hash| format!("#{}", hash.value()))
                .unwrap_or_else(|| format!("#{}", super::sanitize(item.instance_id.as_str())))
        })
}

fn plug_name(plug: &LoadoutPlugView) -> String {
    plug.name
        .as_deref()
        .map(super::sanitize)
        .unwrap_or_else(|| format!("#{}", plug.plug_hash.value()))
}

fn plug_names(item: &LoadoutItemView) -> String {
    item.plugs
        .iter()
        .filter(|plug| !plug.empty)
        .map(plug_name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Compact equipment summary, with optional per-item plug detail.
pub fn render_loadouts(mode: super::OutputMode, result: &LoadoutsResult, detail: bool) -> String {
    if mode == super::OutputMode::Json {
        return loadouts_json(result).to_string();
    }
    let mut lines = Vec::new();
    for character in &result.characters {
        lines.push(super::sanitize(
            character.class.as_deref().unwrap_or("Unknown"),
        ));
        if character.loadouts.is_empty() {
            lines.push("  no saved loadouts".into());
        }
        for loadout in &character.loadouts {
            let name = loadout
                .name
                .as_deref()
                .map(super::sanitize)
                .unwrap_or_else(|| format!("#{}", loadout.name_hash));
            lines.push(format!("  [{}] {name}", loadout.slot));
            for (label, buckets) in [
                ("Weapons", &[1498876634, 2465295065, 953998645][..]),
                (
                    "Armor",
                    &[3448274439, 3551918588, 14239492, 20886954, 1585787867][..],
                ),
            ] {
                let names = loadout
                    .items
                    .iter()
                    .filter(|item| item.bucket_hash.is_some_and(|hash| buckets.contains(&hash)))
                    .map(item_name)
                    .collect::<Vec<_>>();
                if !names.is_empty() {
                    lines.push(format!("      {label}: {}", names.join(", ")));
                }
            }
            for item in &loadout.items {
                match item.bucket_hash {
                    Some(3284755031) => {
                        let mut groups = Vec::new();
                        for &(kind, label) in super::SUBCLASS_LABELS {
                            let names = item
                                .plugs
                                .iter()
                                .filter(|plug| !plug.empty && plug.subclass_kind == Some(kind))
                                .map(plug_name)
                                .collect::<Vec<_>>();
                            if !names.is_empty() {
                                groups.push(format!("{label}: {}", names.join(", ")));
                            }
                        }
                        let suffix = if groups.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", groups.join("; "))
                        };
                        lines.push(format!("      Subclass: {}{suffix}", item_name(item)));
                    }
                    Some(1506418338) => {
                        let perks = plug_names(item);
                        let suffix = if perks.is_empty() {
                            String::new()
                        } else {
                            format!(" — {perks}")
                        };
                        lines.push(format!("      Artifact: {}{suffix}", item_name(item)));
                    }
                    // Unknown equipment cannot be assigned a category safely.
                    Some(
                        1498876634 | 2465295065 | 953998645 | 3448274439 | 3551918588 | 14239492
                        | 20886954 | 1585787867,
                    ) => (),
                    _ => lines.push(format!("      {}", item_name(item))),
                }
            }
            if detail {
                for item in &loadout.items {
                    let item_type = item
                        .item_type
                        .as_deref()
                        .map(|name| format!(" ({})", super::sanitize(name)))
                        .unwrap_or_default();
                    let plugs = plug_names(item);
                    let suffix = if plugs.is_empty() {
                        String::new()
                    } else {
                        format!(": {plugs}")
                    };
                    lines.push(format!("      {}{item_type}{suffix}", item_name(item)));
                }
            }
        }
    }
    if lines.is_empty() {
        "no characters".into()
    } else {
        lines.join("\n")
    }
}
