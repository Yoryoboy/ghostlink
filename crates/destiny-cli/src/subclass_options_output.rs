//! Shared snake-case projection and human rendering of subclass options.
use destiny_app::{OptionStatus, SubclassElement, SubclassOptionsResult};
use serde_json::{Value, json};

use super::{OutputMode, SUBCLASS_LABELS, sanitize};

/// Full, unfiltered CLI/MCP projection, including the raw provider flags.
pub fn subclass_options_json(result: &SubclassOptionsResult) -> Value {
    use destiny_app::SubclassPlugKind::*;
    json!({"kind": "subclass_options",
        "reference_character_id": result.reference_character_id.as_ref().map(|id| id.as_str()),
        "characters": result.characters.iter().map(|character| json!({
            "character_id": character.character_id.as_str(), "class": character.class,
            "status_reliable": character.status_reliable,
            "elements": character.elements.iter().map(|element| json!({
                "element": element.element.map(|element| element.as_str()),
                "groups": element.groups.iter().map(|group| json!({
                    "kind": match group.kind {
                        Super => "super", ClassAbility => "class_ability", Movement => "movement",
                        Melee => "melee", Grenade => "grenade", Aspect => "aspect",
                        Fragment => "fragment", Transcendence => "transcendence",
                    },
                    "options": group.options.iter().map(|option| super::with_insights(json!({
                        "plug_hash": option.plug_hash.value(), "name": option.name,
                        "status": match option.status {
                            OptionStatus::Unlocked => "unlocked", OptionStatus::Locked => "locked",
                            OptionStatus::Unknown => "unknown",
                        },
                        "can_insert": option.can_insert, "enabled": option.enabled,
                    }), option.description.as_deref(), option.clarity.as_deref(), option.clarity_match.as_deref())).collect::<Vec<_>>()
                })).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

/// Text-only locked filtering never treats unknown status as locked.
pub fn render_subclass_options(
    mode: OutputMode,
    result: &SubclassOptionsResult,
    locked: bool,
) -> String {
    if mode == OutputMode::Json {
        return subclass_options_json(result).to_string();
    }
    let mut lines = Vec::new();
    for character in &result.characters {
        let status = if character.status_reliable {
            "status from last-played character"
        } else {
            "status unknown: not the last-played character"
        };
        lines.push(format!(
            "{} ({status})",
            sanitize(character.class.as_deref().unwrap_or("Unknown"))
        ));
        if locked && !character.status_reliable {
            continue;
        }
        for element in &character.elements {
            let mut groups = Vec::new();
            for &(kind, label) in SUBCLASS_LABELS {
                let names: Vec<_> = element
                    .groups
                    .iter()
                    .filter(|group| group.kind == kind)
                    .flat_map(|group| &group.options)
                    .filter(|option| !locked || option.status == OptionStatus::Locked)
                    .map(|option| {
                        let mut name = option
                            .name
                            .as_deref()
                            .map(sanitize)
                            .unwrap_or_else(|| format!("#{}", option.plug_hash.value()));
                        if option.status == OptionStatus::Locked {
                            name.push_str(" [locked]");
                        }
                        name
                    })
                    .collect();
                if !names.is_empty() {
                    groups.push(format!("    {label}: {}", names.join(", ")));
                    for option in element
                        .groups
                        .iter()
                        .filter(|group| group.kind == kind)
                        .flat_map(|group| &group.options)
                        .filter(|option| !locked || option.status == OptionStatus::Locked)
                    {
                        super::push_insights(
                            &mut groups,
                            6,
                            super::description_label(names.len() > 1, option.name.as_deref()),
                            option.description.as_deref(),
                            option.clarity.as_deref(),
                            option.clarity_match.as_deref(),
                        );
                    }
                }
            }
            if !groups.is_empty() {
                let label = match element.element {
                    Some(SubclassElement::Arc) => "Arc",
                    Some(SubclassElement::Solar) => "Solar",
                    Some(SubclassElement::Void) => "Void",
                    Some(SubclassElement::Stasis) => "Stasis",
                    Some(SubclassElement::Strand) => "Strand",
                    Some(SubclassElement::Prismatic) => "Prismatic",
                    None => "Shared",
                };
                lines.push(format!("  {label}"));
                lines.extend(groups);
            }
        }
    }
    if lines.is_empty() {
        "no characters".into()
    } else {
        lines.join("\n")
    }
}
