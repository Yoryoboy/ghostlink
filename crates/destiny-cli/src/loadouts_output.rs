//! Shared snake-case saved loadouts projection for CLI and MCP.
use destiny_app::{BucketHash, ItemSlot};
use destiny_app::{LoadoutItemView, LoadoutPlugView, LoadoutsResult};

pub use destiny_output::loadouts::{loadout_json, loadouts_json};

/// Render only the selected loadout, using the unchanged detailed text renderer.
pub fn render_loadout(
    mode: super::OutputMode,
    result: &destiny_app::loadouts::LoadoutDetailResult,
) -> String {
    if mode == super::OutputMode::Json {
        return loadout_json(result).to_string();
    }
    let envelope = LoadoutsResult {
        characters: vec![destiny_app::CharacterLoadoutsView {
            character_id: result.character_id.clone(),
            class: result.class.clone(),
            loadouts: vec![result.loadout.clone()],
        }],
    };
    render_loadouts(mode, &envelope, true)
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
            for (label, weapons) in [("Weapons", true), ("Armor", false)] {
                let names = loadout
                    .items
                    .iter()
                    .filter(|item| {
                        item.bucket_hash
                            .and_then(|hash| {
                                ItemSlot::from_equipment_bucket(BucketHash::new(i64::from(hash)))
                            })
                            .is_some_and(|slot| {
                                matches!(
                                    slot,
                                    ItemSlot::Kinetic | ItemSlot::Energy | ItemSlot::Power
                                ) == weapons
                            })
                    })
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
                    Some(hash)
                        if ItemSlot::from_equipment_bucket(BucketHash::new(i64::from(hash)))
                            .is_some() => {}
                    _ => lines.push(format!("      {}", item_name(item))),
                }
                if !detail && matches!(item.bucket_hash, Some(3284755031 | 1506418338)) {
                    let multiple = item.plugs.iter().filter(|plug| !plug.empty).count() > 1;
                    for plug in item.plugs.iter().filter(|plug| !plug.empty) {
                        super::push_insights(
                            &mut lines,
                            8,
                            super::description_label(multiple, plug.name.as_deref()),
                            plug.description.as_deref(),
                            plug.clarity.as_deref(),
                            plug.clarity_match.as_deref(),
                        );
                    }
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
                    let multiple = item.plugs.iter().filter(|plug| !plug.empty).count() > 1;
                    for plug in item.plugs.iter().filter(|plug| !plug.empty) {
                        super::push_insights(
                            &mut lines,
                            8,
                            super::description_label(multiple, plug.name.as_deref()),
                            plug.description.as_deref(),
                            plug.clarity.as_deref(),
                            plug.clarity_match.as_deref(),
                        );
                    }
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
