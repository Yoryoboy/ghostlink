//! Pure rendering of committed observations; no pull selectors are invented.
use destiny_app::{PostmasterResult, ProfileCurrenciesCoverage};
use serde_json::{Value, json};

use crate::output::{OutputMode, metadata_json, metadata_text, sanitize};

fn coverage_text(coverage: ProfileCurrenciesCoverage) -> &'static str {
    match coverage {
        ProfileCurrenciesCoverage::NotObserved => "not_observed",
        ProfileCurrenciesCoverage::Unavailable => "unavailable",
        ProfileCurrenciesCoverage::Available => "available",
    }
}

/// Stable snake_case DTO JSON. Optional names/classes are null; real IDs are strings.
/// Duplicate stack observations remain separate, without synthetic identities.
pub fn postmaster_json(result: &PostmasterResult) -> Value {
    json!({
        "kind": "postmaster",
        "metadata": metadata_json(&result.metadata),
        "profile_currencies_coverage": coverage_text(result.profile_currencies_coverage),
        "characters": result.characters.iter().map(|character| json!({
            "character_id": character.character_id.as_str(),
            "class": character.class,
            "instances": character.instances.iter().map(|item| json!({
                "instance_id": item.instance_id.as_str(),
                "item_hash": item.item_hash.value(),
                "name": item.name,
                "quantity": item.quantity,
            })).collect::<Vec<_>>(),
            "stacks": character.stacks.iter().map(|item| json!({
                "item_hash": item.item_hash.value(),
                "name": item.name,
                "bucket_hash": item.bucket_hash.value(),
                "quantity": item.quantity,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Render one committed read using existing metadata and output sanitization.
pub fn render_postmaster(mode: OutputMode, result: &PostmasterResult) -> String {
    if mode == OutputMode::Json {
        return postmaster_json(result).to_string();
    }
    let mut lines =
        vec!["Postmaster committed observations (not a live inventory guarantee)".into()];
    for character in &result.characters {
        lines.push(format!(
            "{} ({}) — Postmaster",
            sanitize(character.class.as_deref().unwrap_or("Unknown class")),
            sanitize(character.character_id.as_str())
        ));
        lines.push(
            if character.instances.is_empty() {
                "  Instances: none"
            } else {
                "  Instances:"
            }
            .into(),
        );
        for item in &character.instances {
            let label = item
                .name
                .as_deref()
                .map(sanitize)
                .unwrap_or_else(|| format!("hash {}", item.item_hash.value()));
            lines.push(format!(
                "    {label} | item_hash: {} | quantity: {} | instance_id: {}",
                item.item_hash.value(),
                item.quantity,
                sanitize(item.instance_id.as_str())
            ));
        }
        lines.push(
            if character.stacks.is_empty() {
                "  Stacks: none"
            } else {
                "  Stacks:"
            }
            .into(),
        );
        for item in &character.stacks {
            let label = item
                .name
                .as_deref()
                .map(sanitize)
                .unwrap_or_else(|| format!("hash {}", item.item_hash.value()));
            lines.push(format!(
                "    {label} | item_hash: {} | quantity: {} | bucket_hash: {}",
                item.item_hash.value(),
                item.quantity,
                item.bucket_hash.value()
            ));
        }
    }
    if result.characters.is_empty() {
        lines.push("No owned characters in the committed observation.".into());
    }
    lines.push(
        "Stacks are anonymous observations, not stable pull selectors. No pull feature is exposed."
            .into(),
    );
    lines.push(format!(
        "profile_currencies_coverage: {} (component 103 only, not Postmaster completeness)",
        coverage_text(result.profile_currencies_coverage)
    ));
    lines.push(metadata_text(&result.metadata));
    lines.join("\n")
}
