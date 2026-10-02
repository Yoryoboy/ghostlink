//! Deterministic human and JSON rendering for the read journey.
//!
//! Every renderer is pure and total over one shared result record. Human output
//! is concise, line-oriented text with a metadata footer; JSON output is exactly
//! one bounded object with a stable `kind` and no human prose. Neither mode can
//! invent an identifier, a display string, or a freshness concept that the
//! shared records do not already carry.

use serde_json::{Value, json};

use destiny_app::{
    AccessCredentialMetadata, Character, CharacterListResult, FreshnessStatus, InventorySearchHit,
    InventorySearchResult, ItemDefinition, ItemInspection, ItemInstance, ItemLocation, PerkSummary,
    QueryMetadata, SourceMarker,
};

use crate::command::{CallbackMode, CliError};
use crate::login::LoginInstruction;

/// The output mode selected by the global `--json` flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    /// Concise, line-oriented text for people.
    Human,
    /// Exactly one bounded JSON object for programs.
    Json,
}

/// Renders the public version label and counts, never definition payloads.
pub fn render_manifest(mode: OutputMode, report: &destiny_app::ManifestSyncReport) -> String {
    use destiny_app::ManifestSyncReport;
    let (status, version, count, skipped, world_counts, world_skipped) = match report {
        ManifestSyncReport::Updated {
            version,
            record_count,
            skipped,
            world_counts,
            world_skipped,
        } => (
            "updated",
            version,
            *record_count,
            Some(*skipped),
            world_counts,
            Some(world_skipped),
        ),
        ManifestSyncReport::Unchanged {
            version,
            record_count,
            world_counts,
        } => (
            "unchanged",
            version,
            *record_count,
            None,
            world_counts,
            None,
        ),
    };
    match mode {
        OutputMode::Human => {
            let mut text = format!(
                "manifest: {status}\nversion: {}\ndefinitions: {count}",
                sanitize(version.as_str())
            );
            if let Some(skipped) = skipped {
                text.push_str(&format!("\nskipped: {skipped}"));
            }
            for (index, label) in [
                "milestones",
                "activities",
                "activity_modifiers",
                "objectives",
                "activity_types",
            ]
            .iter()
            .enumerate()
            {
                text.push_str(&format!("\n{label}: {}", world_counts[index]));
                if let Some(skipped) = world_skipped {
                    text.push_str(&format!("\n{label}_skipped: {}", skipped[index]));
                }
            }
            text
        }
        OutputMode::Json => {
            let mut value = json!({"kind": "sync_manifest", "status": status, "version": version.as_str(), "definitions": count});
            if let Some(skipped) = skipped {
                value["skipped"] = json!(skipped);
            }
            for (index, label) in [
                "milestones",
                "activities",
                "activity_modifiers",
                "objectives",
                "activity_types",
            ]
            .iter()
            .enumerate()
            {
                value[*label] = json!(world_counts[index]);
                if let Some(skipped) = world_skipped {
                    value[format!("{label}_skipped")] = json!(skipped[index]);
                }
            }
            value.to_string()
        }
    }
}

/// Renders the resolved public reset; unknown hashes stay explicitly unresolved.
pub fn render_weekly_reset(
    mode: OutputMode,
    reset: &destiny_app::weekly_reset::WeeklyReset,
) -> String {
    use destiny_adapters::format_reset_timestamp;
    use destiny_app::weekly_reset::DefinitionResolution;
    let fields = |definition: &DefinitionResolution| match definition {
        DefinitionResolution::Resolved { name, description } => {
            (Some(name.clone()), Some(description.clone()), true)
        }
        DefinitionResolution::Unresolved => (None, None, false),
    };
    let label = |hash: u32, definition: &DefinitionResolution| match definition {
        DefinitionResolution::Resolved { name, .. } => sanitize(name),
        DefinitionResolution::Unresolved => format!("unresolved #{hash}"),
    };
    match mode {
        OutputMode::Human => {
            let mut lines = vec![format!("Weekly reset (manifest {})", sanitize(reset.definition_version.as_str()))];
            for milestone in &reset.milestones {
                let mut header = label(milestone.hash, &milestone.definition);
                match (
                    milestone.start.and_then(|t| format_reset_timestamp(t, true)),
                    milestone.end.and_then(|t| format_reset_timestamp(t, true)),
                ) {
                    (Some(start), Some(end)) => {
                        header.push_str(&format!(" [{start} -> {end}]"));
                    }
                    (Some(start), None) => header.push_str(&format!(" [from {start}]")),
                    (None, Some(end)) => header.push_str(&format!(" [until {end}]")),
                    (None, None) => {}
                }
                lines.push(header);
                for activity in &milestone.activities {
                    lines.push(format!("  - {}", label(activity.hash, &activity.definition)));
                    if !activity.modifiers.is_empty() {
                        lines.push(format!("    modifiers: {}", activity.modifiers.iter().map(|modifier| label(modifier.hash, &modifier.definition)).collect::<Vec<_>>().join(", ")));
                    }
                }
            }
            lines.join("\n")
        }
        OutputMode::Json => json!({
            "kind": "weekly_reset", "definition_version": reset.definition_version.as_str(),
            "milestones": reset.milestones.iter().map(|milestone| {
                let (name, description, resolved) = fields(&milestone.definition);
                json!({"hash": milestone.hash, "order": milestone.order, "name": name, "description": description, "resolved": resolved,
                    "start": milestone.start.and_then(|t| format_reset_timestamp(t, false)),
                    "end": milestone.end.and_then(|t| format_reset_timestamp(t, false)),
                    "activities": milestone.activities.iter().map(|activity| {
                        let (name, description, resolved) = fields(&activity.definition);
                        json!({"hash": activity.hash, "name": name, "description": description, "resolved": resolved,
                            "modifiers": activity.modifiers.iter().map(|modifier| {
                                let (name, description, resolved) = fields(&modifier.definition);
                                json!({"hash": modifier.hash, "name": name, "description": description, "resolved": resolved})
                            }).collect::<Vec<_>>()})
                    }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()
        }).to_string(),
    }
}

/// Renders only classified weekly sections; progress always comes from character data.
#[path = "weekly_json.rs"]
mod weekly_json;

pub fn render_weekly(
    mode: OutputMode,
    character: &Character,
    weekly: &destiny_app::weekly_personal::WeeklyPersonal,
) -> String {
    use destiny_app::DefinitionResolution;
    use destiny_app::weekly_personal::PersonalActivity;
    let label = |hash: u32, definition: &DefinitionResolution| match definition {
        DefinitionResolution::Resolved { name, .. } => sanitize(name),
        DefinitionResolution::Unresolved => format!("unresolved #{hash}"),
    };
    if mode == OutputMode::Json {
        return weekly_json::render_weekly_json(character, weekly);
    }
    let progress = |activity: &PersonalActivity| {
        let mut parts = activity
            .challenges
            .iter()
            .map(|c| {
                format!(
                    "challenge {} {}/{}",
                    label(c.objective_hash, &c.definition),
                    c.progress,
                    c.completion_value
                )
            })
            .collect::<Vec<_>>();
        if let Some(phases) = &activity.phases {
            parts.push(format!("phases {}/{}", phases.complete, phases.total));
        }
        parts.join("; ")
    };
    let mut lines = vec![format!(
        "Weekly ({}, manifest {})",
        sanitize(character.class.as_deref().unwrap_or("unknown")),
        sanitize(weekly.definition_version.as_str())
    )];
    for (heading, groups) in [
        (
            "Grandmaster alert",
            &weekly
                .vanguard_alerts
                .grandmaster
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        ),
        ("Featured raids", &weekly.featured_raids),
        ("Featured dungeons", &weekly.featured_dungeons),
        ("Crucible alert", &weekly.vanguard_alerts.crucible),
        ("Quickplay alert", &weekly.vanguard_alerts.quickplay),
    ] {
        if groups.is_empty() {
            continue;
        }
        lines.push(format!(
            "{heading}: {}",
            groups
                .iter()
                .map(|group| {
                    format!(
                        "{} ({})",
                        sanitize(&group.root_name),
                        sanitize(&group.activity_type_name)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        ));
        for group in groups {
            let mut batches: Vec<Vec<&destiny_app::PersonalActivity>> = Vec::new();
            for activity in &group.activities {
                if let Some(batch) = batches.iter_mut().find(|b| {
                    b[0].challenges == activity.challenges
                        && b[0].rewards == activity.rewards
                        && b[0].phases == activity.phases
                        && b[0].challenge_objectives == activity.challenge_objectives
                }) {
                    batch.push(activity);
                } else {
                    batches.push(vec![activity]);
                }
            }
            for batch in batches {
                let activity = batch[0];
                let display = if batch.len() > 1 {
                    let suffixes = batch
                        .iter()
                        .map(|a| {
                            let full = label(a.hash, &a.definition);
                            full.strip_prefix(&sanitize(&group.root_name))
                                .unwrap_or(&full)
                                .trim_start_matches(": ")
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{} ({suffixes})", sanitize(&group.root_name))
                } else {
                    label(activity.hash, &activity.definition)
                };
                let rewards = activity
                    .rewards
                    .iter()
                    .map(|r| label(r.hash, &r.definition))
                    .collect::<Vec<_>>();
                let mut detail = progress(activity);
                if !rewards.is_empty() {
                    if !detail.is_empty() {
                        detail.push_str(" — ");
                    }
                    detail.push_str(&format!("reward {}", rewards.join(", ")));
                }
                if !detail.is_empty() {
                    lines.push(format!("  {} — {detail}", display));
                }
            }
        }
    }
    let rotators = [
        weekly.weekly_rotators.expedition.as_ref(),
        weekly.weekly_rotators.override_activity.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(|group| sanitize(&group.root_name))
    .collect::<Vec<_>>();
    if !rotators.is_empty() {
        lines.push(rotators.join(" / "));
    }
    if weekly.unclassified_unresolved > 0 {
        lines.push(format!(
            "note: {} activities could not be classified (missing definitions; run ghostlink sync-manifest)",
            weekly.unclassified_unresolved
        ));
    }
    lines.join("\n")
}

/// Renders one character listing in the selected mode.
pub fn render_characters(mode: OutputMode, result: &CharacterListResult) -> String {
    match mode {
        OutputMode::Human => characters_human(result),
        OutputMode::Json => characters_json(result),
    }
}

/// Renders one inventory search in the selected mode.
pub fn render_search(mode: OutputMode, result: &InventorySearchResult) -> String {
    match mode {
        OutputMode::Human => search_human(result),
        OutputMode::Json => search_json(result),
    }
}

/// Renders one exact inspection in the selected mode.
pub fn render_inspection(mode: OutputMode, inspection: &ItemInspection) -> String {
    match mode {
        OutputMode::Human => inspection_human(inspection),
        OutputMode::Json => inspection_json(inspection),
    }
}

/// Renders one failure in the selected mode.
///
/// Human output is exactly one sanitized line prefixed with `error:`, and JSON
/// output is exactly one sanitized error object.
pub fn render_error(mode: OutputMode, error: &CliError) -> String {
    match mode {
        OutputMode::Human => format!("error: {error}"),
        OutputMode::Json => json!({
            "error": {
                "code": error.code(),
                "message": error.to_string(),
            }
        })
        .to_string(),
    }
}

/// Renders the operator instruction one interactive login produced.
///
/// The instruction is emitted before the terminal read blocks, because a human
/// cannot act on a URL that only appears after the program is already waiting.
/// Human output is a short labelled block and JSON output is exactly one bounded
/// `authorizing` object, so a program can read the URL it has to present and a
/// reader of the terminal still sees a bounded stream of objects.
///
/// The authorization URL carries the CSRF state the provider sends to the browser
/// anyway; the callback the human pastes back is never part of an instruction, and
/// no credential, code, or token value is ever rendered.
pub fn render_login_instruction(mode: OutputMode, instruction: &LoginInstruction) -> String {
    match mode {
        OutputMode::Human => match instruction.callback_mode {
            CallbackMode::Paste => format!(
                "authorization required\nopen this url in a browser:\n  {}\nafter authorizing, copy the full callback address from the browser and paste it here\npaste the callback url and press enter:\nwaiting for the pasted callback",
                sanitize(&instruction.authorization_url),
            ),
            CallbackMode::Handler => format!(
                "authorization required\nopen this url in a browser:\n  {}\nafter authorizing, the browser callback is delivered to this window automatically; keep it open\nwaiting for the browser callback",
                sanitize(&instruction.authorization_url),
            ),
        },
        OutputMode::Json => json!({
            "authorization_url": instruction.authorization_url,
            "callback_mode": instruction.callback_mode.as_str(),
            "kind": "login",
            "status": "authorizing",
        })
        .to_string(),
    }
}

/// Renders one completed authorization.
///
/// Access-only compatibility output. Credential material never reaches the renderer.
pub fn render_login_authorized(mode: OutputMode, access: &AccessCredentialMetadata) -> String {
    match mode {
        OutputMode::Human => format!(
            "authorized\nexpires_at: {}\na public client receives no refresh token; run login again after expiry",
            access.expires_at.seconds(),
        ),
        OutputMode::Json => json!({
            "expires_at": access.expires_at.seconds(),
            "kind": "login",
            "refresh_token": false,
            "status": "authorized",
        })
        .to_string(),
    }
}

/// Renders only the persisted grant's expiry, never its token material.
pub fn render_login_renewable(
    mode: OutputMode,
    access: &AccessCredentialMetadata,
    refresh_expires_at: destiny_app::Timestamp,
) -> String {
    match mode {
        OutputMode::Human => format!("authorized\nexpires_at: {}\nsession renews automatically until {}", access.expires_at.seconds(), refresh_expires_at.seconds()),
        OutputMode::Json => json!({ "expires_at": access.expires_at.seconds(), "kind": "login", "refresh_expires_at": refresh_expires_at.seconds(), "refresh_token": true, "status": "authorized" }).to_string(),
    }
}

fn characters_human(result: &CharacterListResult) -> String {
    let mut lines = vec![format!("characters: {}", result.characters.len())];
    for character in &result.characters {
        lines.push(character_line(character));
    }
    lines.push(metadata_text(&result.metadata));
    lines.join("\n")
}

fn search_human(result: &InventorySearchResult) -> String {
    let mut lines = vec![format!("candidates: {}", result.candidates.len())];
    for candidate in &result.candidates {
        lines.push(format!("- {}", definition_line(candidate)));
    }
    lines.push(format!("hits: {}", result.hits.len()));
    for hit in &result.hits {
        lines.push(instance_line(hit));
    }
    lines.push(metadata_text(&result.metadata));
    lines.join("\n")
}

fn inspection_human(inspection: &ItemInspection) -> String {
    let instance = &inspection.instance;
    let mut lines = vec![format!(
        "instance: {}",
        sanitize(instance.instance_id.as_str())
    )];
    lines.push("definition:".to_string());
    lines.push(format!("  {}", definition_line(&inspection.definition)));
    lines.push("instance state:".to_string());
    lines.push(format!("  item_hash: {}", instance.item_hash.value()));
    lines.push(format!("  location: {}", location_text(&instance.location)));
    lines.push(format!("  quantity: {}", instance.quantity));
    lines.push(format!("  equipped: {}", instance.equipped));
    lines.push(format!("  locked: {}", instance.locked));
    if let Some(power) = instance.power {
        lines.push(format!("  power: {power}"));
    }
    lines.push(format!("  perks: {}", perks_text(&instance.perks)));
    lines.push(metadata_text(&inspection.metadata));
    lines.join("\n")
}

fn characters_json(result: &CharacterListResult) -> String {
    json!({
        "characters": result
            .characters
            .iter()
            .map(character_json)
            .collect::<Vec<_>>(),
        "kind": "character_list",
        "metadata": metadata_json(&result.metadata),
    })
    .to_string()
}

fn search_json(result: &InventorySearchResult) -> String {
    json!({
        "candidates": result
            .candidates
            .iter()
            .map(definition_json)
            .collect::<Vec<_>>(),
        "hits": result
            .hits
            .iter()
            .map(|hit| json!({
                "definition": definition_json(&hit.definition),
                "instance": instance_json(&hit.instance),
            }))
            .collect::<Vec<_>>(),
        "kind": "inventory_search",
        "metadata": metadata_json(&result.metadata),
    })
    .to_string()
}

fn inspection_json(inspection: &ItemInspection) -> String {
    json!({
        "definition": definition_json(&inspection.definition),
        "instance": instance_json(&inspection.instance),
        "kind": "item_inspection",
        "metadata": metadata_json(&inspection.metadata),
    })
    .to_string()
}

/// Replaces every control character with `?`.
///
/// Human output is line-oriented, so a provider or caller value that contains a
/// newline, carriage return, or escape must never be able to forge an extra
/// output line or a metadata footer. JSON output relies on the JSON encoder's
/// escaping instead.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

fn character_line(character: &Character) -> String {
    let mut parts = vec![format!("id: {}", sanitize(character.id.as_str()))];
    if let Some(name) = &character.name {
        parts.push(format!("name: {}", sanitize(name)));
    }
    if let Some(class) = &character.class {
        parts.push(format!("class: {}", sanitize(class)));
    }
    if let Some(race) = &character.race {
        parts.push(format!("race: {}", sanitize(race)));
    }
    if let Some(gender) = &character.gender {
        parts.push(format!("gender: {}", sanitize(gender)));
    }
    if let Some(light) = character.light {
        parts.push(format!("light: {light}"));
    }
    format!("- {}", parts.join("; "))
}

fn definition_line(definition: &ItemDefinition) -> String {
    let mut parts = vec![
        format!("hash: {}", definition.hash.value()),
        format!("name: {}", sanitize(&definition.name)),
    ];
    if let Some(item_type) = &definition.item_type {
        parts.push(format!("type: {}", sanitize(item_type)));
    }
    if let Some(tier) = &definition.tier {
        parts.push(format!("tier: {}", sanitize(tier)));
    }
    if let Some(element) = &definition.element {
        parts.push(format!("element: {}", sanitize(element)));
    }
    if let Some(bucket) = definition.bucket_hash {
        parts.push(format!("bucket: {}", bucket.value()));
    }
    if !definition.tags.is_empty() {
        let tags: Vec<String> = definition
            .tags
            .iter()
            .map(|tag| sanitize(tag.as_str()))
            .collect();
        parts.push(format!("tags: {}", tags.join(", ")));
    }
    parts.join("; ")
}

fn instance_line(hit: &InventorySearchHit) -> String {
    let instance = &hit.instance;
    let mut parts = vec![
        format!("instance: {}", sanitize(instance.instance_id.as_str())),
        format!("hash: {}", instance.item_hash.value()),
        format!("name: {}", sanitize(&hit.definition.name)),
        format!("location: {}", location_text(&instance.location)),
        format!("quantity: {}", instance.quantity),
        format!("equipped: {}", instance.equipped),
        format!("locked: {}", instance.locked),
    ];
    if let Some(power) = instance.power {
        parts.push(format!("power: {power}"));
    }
    format!("- {}", parts.join("; "))
}

fn location_text(location: &ItemLocation) -> String {
    match location {
        ItemLocation::Vault => "vault".to_string(),
        ItemLocation::Character(character) => {
            format!("character:{}", sanitize(character.as_str()))
        }
        ItemLocation::Other(value) => format!("other:{}", sanitize(value)),
    }
}

fn perks_text(perks: &[PerkSummary]) -> String {
    if perks.is_empty() {
        return "none".to_string();
    }
    perks
        .iter()
        .map(|perk| format!("active={} visible={}", perk.active, perk.visible))
        .collect::<Vec<_>>()
        .join("; ")
}

fn metadata_text(metadata: &QueryMetadata) -> String {
    format!(
        "revision: {}\nchecked_at: {}\nfreshness: {}",
        metadata.sync.revision.value(),
        metadata.sync.checked_at.seconds(),
        freshness_text(metadata.freshness),
    )
}

fn metadata_json(metadata: &QueryMetadata) -> Value {
    json!({
        "checked_at": metadata.sync.checked_at.seconds(),
        "freshness": freshness_text(metadata.freshness),
        "revision": metadata.sync.revision.value(),
        "source": source_text(metadata.sync.source),
    })
}

fn character_json(character: &Character) -> Value {
    json!({
        "class": character.class.as_deref(),
        "gender": character.gender.as_deref(),
        "id": character.id.as_str(),
        "light": character.light,
        "name": character.name.as_deref(),
        "race": character.race.as_deref(),
    })
}

fn definition_json(definition: &ItemDefinition) -> Value {
    json!({
        "bucket_hash": definition.bucket_hash.map(|bucket| bucket.value()),
        "element": definition.element.as_deref(),
        "hash": definition.hash.value(),
        "item_type": definition.item_type.as_deref(),
        "name": definition.name.as_str(),
        "tags": &definition.tags,
        "tier": definition.tier.as_deref(),
    })
}

fn instance_json(instance: &ItemInstance) -> Value {
    json!({
        "equipped": instance.equipped,
        "instance_id": instance.instance_id.as_str(),
        "item_hash": instance.item_hash.value(),
        "location": location_json(&instance.location),
        "locked": instance.locked,
        "perks": instance.perks.iter().map(perk_json).collect::<Vec<_>>(),
        "power": instance.power,
        "quantity": instance.quantity,
    })
}

fn perk_json(perk: &PerkSummary) -> Value {
    json!({
        "active": perk.active,
        "visible": perk.visible,
    })
}

fn location_json(location: &ItemLocation) -> Value {
    match location {
        ItemLocation::Vault => json!({ "kind": "vault" }),
        ItemLocation::Character(character) => json!({
            "character_id": character.as_str(),
            "kind": "character",
        }),
        ItemLocation::Other(value) => json!({
            "kind": "other",
            "value": value,
        }),
    }
}

const fn freshness_text(freshness: FreshnessStatus) -> &'static str {
    match freshness {
        FreshnessStatus::Missing => "missing",
        FreshnessStatus::Fresh => "fresh",
        FreshnessStatus::Stale => "stale",
        FreshnessStatus::InvalidSource => "invalid_source",
    }
}

const fn source_text(source: SourceMarker) -> &'static str {
    match source {
        SourceMarker::Manifest => "manifest",
        SourceMarker::Profile => "profile",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputMode, render_characters, render_error, render_inspection, render_login_authorized,
        render_login_instruction, render_search,
    };
    use crate::command::CallbackMode;
    use crate::command::CliError;
    use crate::login::LoginInstruction;
    use destiny_app::{
        AccessCredentialMetadata, BucketHash, Character, CharacterId, CharacterListResult, Dataset,
        DefinitionVersion, FreshnessStatus, InventorySearchHit, InventorySearchResult,
        ItemDefinition, ItemHash, ItemInspection, ItemInstance, ItemInstanceId, ItemLocation,
        PerkHash, PerkSummary, QueryMetadata, SourceMarker, SyncMetadata, SyncRevision, Timestamp,
    };

    const DEFINITION_JSON: &str = concat!(
        r#"{"bucket_hash":953998645,"element":"Solar","hash":1,"item_type":"Rocket Launcher","#,
        r#""name":"Gjallarhorn","tags":["exotic"],"tier":"Exotic"}"#
    );

    const INSTANCE_JSON: &str = concat!(
        r#"{"equipped":false,"instance_id":"inst-1","item_hash":1,"location":{"kind":"vault"},"#,
        r#""locked":true,"perks":[{"active":true,"visible":true},"#,
        r#"{"active":true,"visible":false},{"active":false,"visible":false}],"#,
        r#""power":1350,"quantity":1}"#
    );

    const METADATA_JSON: &str =
        r#"{"checked_at":100,"freshness":"fresh","revision":3,"source":"profile"}"#;

    const METADATA_TEXT: &str = "revision: 3\nchecked_at: 100\nfreshness: fresh";

    fn metadata(freshness: FreshnessStatus) -> QueryMetadata {
        QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(3),
                definition_version: DefinitionVersion::new("manifest-v1"),
            },
            freshness,
        }
    }

    #[test]
    fn weekly_sections_and_json_are_pinned() {
        use destiny_app::DefinitionResolution;
        use destiny_app::weekly_personal::*;
        let resolved = |name: &str| DefinitionResolution::Resolved {
            name: name.into(),
            description: String::new(),
        };
        let activity = PersonalActivity {
            hash: 10,
            activity_type_hash: Some(30),
            definition: resolved("Raid: Standard"),
            activity_type: resolved("Raid"),
            rewards: vec![PersonalReward {
                hash: 99,
                definition: DefinitionResolution::Unresolved,
            }],
            modifiers: Vec::new(),
            challenge_objectives: Vec::new(),
            challenges: vec![PersonalChallenge {
                objective_hash: 20,
                definition: resolved("Activities"),
                progress: 0,
                completion_value: 1,
                complete: false,
            }],
            phases: Some(PhaseProgress {
                complete: 0,
                total: 4,
            }),
            focused: true,
            recommended_light: Some(2000),
            has_challenge: true,
        };
        let group = PersonalGroup {
            root_name: "Raid".into(),
            activity_type_name: "Raid".into(),
            activities: vec![activity.clone()],
            reward_names: Vec::new(),
        };
        let mut weekly = WeeklyPersonal {
            definition_version: destiny_app::DefinitionVersion::new("v1"),
            unclassified_unresolved: 0,
            vanguard_alerts: VanguardAlerts {
                grandmaster: Some(group.clone()),
                crucible: vec![group.clone()],
                quickplay: vec![group.clone()],
            },
            featured_raids: vec![group.clone()],
            featured_dungeons: vec![group.clone()],

            weekly_rotators: WeeklyRotators {
                expedition: Some(PersonalGroup {
                    root_name: "Expedition: Cosmodrome".into(),
                    ..group.clone()
                }),
                override_activity: Some(PersonalGroup {
                    root_name: "Override: Europa".into(),
                    ..group.clone()
                }),
            },

            character_activities: Vec::new(),
            public_activities: Vec::new(),
            milestones: Vec::new(),
        };
        let human = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        let projected: serde_json::Value = serde_json::from_str(&super::render_weekly(
            OutputMode::Json,
            &full_character(),
            &weekly,
        ))
        .unwrap();
        assert!(projected.get("newest").is_none());
        assert!(projected.get("portal_focus").is_none());
        assert!(projected.get("vanguard_alerts").is_some());
        assert_eq!(
            human,
            "Weekly (Warlock, manifest v1)\nGrandmaster alert: Raid (Raid)\n  Raid: Standard — challenge Activities 0/1; phases 0/4 — reward unresolved #99\nFeatured raids: Raid (Raid)\n  Raid: Standard — challenge Activities 0/1; phases 0/4 — reward unresolved #99\nFeatured dungeons: Raid (Raid)\n  Raid: Standard — challenge Activities 0/1; phases 0/4 — reward unresolved #99\nCrucible alert: Raid (Raid)\n  Raid: Standard — challenge Activities 0/1; phases 0/4 — reward unresolved #99\nQuickplay alert: Raid (Raid)\n  Raid: Standard — challenge Activities 0/1; phases 0/4 — reward unresolved #99\nExpedition: Cosmodrome / Override: Europa"
        );
        let value: serde_json::Value = serde_json::from_str(&super::render_weekly(
            OutputMode::Json,
            &full_character(),
            &weekly,
        ))
        .unwrap();
        let item = serde_json::json!({"hash":10,"name":"Raid: Standard","resolved":true,"activity_type":{"hash":30,"name":"Raid","resolved":true},"rewards":[{"hash":99,"name":null,"resolved":false}],"modifiers":[],"challenge_objectives":[],"challenges":[{"hash":20,"name":"Activities","resolved":true,"progress":0,"completion_value":1,"complete":false}],"phases":{"complete":0,"total":4}});
        let groups = serde_json::json!([{"root":"Raid","reward_names":[],"activity_type":{"hash":30,"name":"Raid","resolved":true},"variants":[item.clone()]}]);
        assert_eq!(
            value,
            serde_json::json!({
                "kind":"weekly", "character":super::character_json(&full_character()), "definition_version":"v1",
                "featured_raids":groups, "featured_dungeons":groups,
                "vanguard_alerts":{"grandmaster":groups[0],"crucible":groups,"quickplay":groups},
                "expedition":{"root":"Expedition: Cosmodrome","reward_names":[],"activity_type":{"hash":30,"name":"Raid","resolved":true},"variants":[item]},
                "override":{"root":"Override: Europa","reward_names":[],"activity_type":{"hash":30,"name":"Raid","resolved":true},"variants":[item]},
                "milestones":[], "unclassified_unresolved":0
            })
        );
        let mut variant = weekly.vanguard_alerts.quickplay[0].activities[0].clone();
        variant.hash = 11;
        variant.definition = resolved("Raid: Customize");
        weekly.vanguard_alerts.quickplay[0].activities.push(variant);
        let collapsed = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        assert!(collapsed.contains("Raid (Standard, Customize) — challenge Activities 0/1"));
        assert!(!collapsed.contains("Raid: Customize —"));
        weekly.vanguard_alerts.quickplay[0].activities[1].challenges[0].progress = 1;
        let distinct = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        assert!(distinct.contains("Raid: Customize — challenge Activities 1/1"));
        // Same progress but different rewards must not collapse either.
        weekly.vanguard_alerts.quickplay[0].activities[1].challenges[0].progress = 0;
        weekly.vanguard_alerts.quickplay[0].activities[1]
            .rewards
            .clear();
        let different_rewards = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        assert!(different_rewards.contains("Raid: Customize — challenge Activities 0/1"));
        assert!(!different_rewards.contains("Raid (Standard, Customize)"));
        weekly.vanguard_alerts = VanguardAlerts::default();
        weekly.featured_raids.clear();
        weekly.featured_dungeons.clear();

        weekly.weekly_rotators = WeeklyRotators::default();
        assert_eq!(
            super::render_weekly(OutputMode::Human, &full_character(), &weekly),
            "Weekly (Warlock, manifest v1)"
        );
        let value: serde_json::Value = serde_json::from_str(&super::render_weekly(
            OutputMode::Json,
            &full_character(),
            &weekly,
        ))
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind":"weekly", "character":super::character_json(&full_character()), "definition_version":"v1",
                "featured_raids":[], "featured_dungeons":[],
                "vanguard_alerts":{"grandmaster":null,"crucible":[],"quickplay":[]}, "expedition":null, "override":null,
                "milestones":[], "unclassified_unresolved":0
            })
        );
    }

    #[test]
    fn weekly_json_preserves_entity_text_and_reports_unclassified() {
        use destiny_app::DefinitionResolution;
        use destiny_app::weekly_personal::*;
        let raw = "name\n\"quoted\"";
        let activity = PersonalActivity {
            hash: 1,
            activity_type_hash: Some(2),
            definition: DefinitionResolution::Resolved {
                name: raw.into(),
                description: String::new(),
            },
            activity_type: DefinitionResolution::Unresolved,
            rewards: Vec::new(),
            modifiers: Vec::new(),
            challenge_objectives: Vec::new(),
            challenges: Vec::new(),
            phases: None,
            focused: true,
            recommended_light: None,
            has_challenge: false,
        };
        let weekly = WeeklyPersonal {
            definition_version: DefinitionVersion::new("v1"),
            featured_raids: Vec::new(),
            featured_dungeons: Vec::new(),
            weekly_rotators: WeeklyRotators::default(),
            vanguard_alerts: VanguardAlerts {
                grandmaster: Some(PersonalGroup {
                    root_name: raw.into(),
                    activity_type_name: raw.into(),
                    activities: vec![activity],
                    reward_names: Vec::new(),
                }),
                ..VanguardAlerts::default()
            },
            character_activities: Vec::new(),
            public_activities: Vec::new(),
            milestones: vec![PersonalMilestone {
                hash: 3,
                definition: DefinitionResolution::Unresolved,
            }],
            unclassified_unresolved: 2,
        };
        let value: serde_json::Value = serde_json::from_str(&super::render_weekly(
            OutputMode::Json,
            &full_character(),
            &weekly,
        ))
        .unwrap();
        assert_eq!(
            value["vanguard_alerts"]["grandmaster"]["variants"][0]["name"],
            raw
        );
        assert_eq!(
            value["vanguard_alerts"]["grandmaster"]["variants"][0]["activity_type"],
            serde_json::json!({"hash":2,"name":null,"resolved":false})
        );
        assert_eq!(value["vanguard_alerts"]["grandmaster"]["root"], raw);
        assert_eq!(
            value["milestones"],
            serde_json::json!([{"hash":3,"name":null,"resolved":false}])
        );
        assert_eq!(value["unclassified_unresolved"], 2);
        let human = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        assert!(human.ends_with("note: 2 activities could not be classified (missing definitions; run ghostlink sync-manifest)"));
        assert!(!human.contains(raw));
    }

    #[test]
    fn weekly_rotator_json_uses_real_variant_and_objective_entities() {
        use destiny_app::DefinitionResolution;
        use destiny_app::weekly_personal::*;
        let mut activity = PersonalActivity {
            hash: 11,
            activity_type_hash: Some(12),
            definition: DefinitionResolution::Resolved {
                name: "Expedition: Europa (Epic)".into(),
                description: String::new(),
            },
            activity_type: DefinitionResolution::Resolved {
                name: "Mission".into(),
                description: String::new(),
            },
            rewards: vec![PersonalReward {
                hash: 13,
                definition: DefinitionResolution::Resolved {
                    name: "reward\nname".into(),
                    description: String::new(),
                },
            }],
            modifiers: Vec::new(),
            challenge_objectives: vec![PersonalObjective {
                hash: 14,
                definition: DefinitionResolution::Unresolved,
            }],
            challenges: Vec::new(),
            phases: None,
            focused: false,
            recommended_light: None,
            has_challenge: false,
        };
        let mut weekly = WeeklyPersonal {
            definition_version: DefinitionVersion::new("v1"),
            unclassified_unresolved: 0,
            featured_raids: Vec::new(),
            featured_dungeons: Vec::new(),
            vanguard_alerts: VanguardAlerts::default(),
            weekly_rotators: WeeklyRotators::default(),
            character_activities: vec![activity.clone()],
            public_activities: Vec::new(),
            milestones: Vec::new(),
        };
        activity.hash = 15;
        activity.definition = DefinitionResolution::Resolved {
            name: "Override: Europa".into(),
            description: String::new(),
        };
        weekly.character_activities.push(activity);
        let mut unresolved = weekly.character_activities[0].clone();
        unresolved.hash = 99;
        unresolved.definition = DefinitionResolution::Resolved {
            name: "Expedition: Europa (Unresolved)".into(),
            description: String::new(),
        };
        unresolved.activity_type = DefinitionResolution::Unresolved;
        weekly.character_activities.push(unresolved);
        weekly.weekly_rotators = classify_rotators(&weekly.character_activities);
        weekly.unclassified_unresolved =
            count_unclassified_unresolved(&weekly.character_activities, &weekly.public_activities);
        let value: serde_json::Value = serde_json::from_str(&super::render_weekly(
            OutputMode::Json,
            &full_character(),
            &weekly,
        ))
        .unwrap();
        assert_eq!(value["expedition"]["root"], "Expedition: Europa");
        assert_eq!(value["expedition"]["variants"][0]["hash"], 11);
        assert_eq!(
            value["expedition"]["variants"][0]["name"],
            "Expedition: Europa (Epic)"
        );
        assert_eq!(
            value["expedition"]["variants"][0]["rewards"],
            serde_json::json!([{"hash":13,"name":"reward\nname","resolved":true}])
        );
        assert_eq!(
            value["expedition"]["variants"][0]["challenge_objectives"],
            serde_json::json!([{"hash":14,"name":null,"resolved":false}])
        );
        assert_eq!(value["override"]["variants"][0]["hash"], 15);
        assert_eq!(value["expedition"]["variants"].as_array().unwrap().len(), 1);
        assert_eq!(value["unclassified_unresolved"], 1);
        let human = super::render_weekly(OutputMode::Human, &full_character(), &weekly);
        assert!(human.contains("Expedition: Europa"));
        assert!(!human.contains("Unresolved"));
        assert!(human.ends_with("note: 1 activities could not be classified (missing definitions; run ghostlink sync-manifest)"));
    }

    fn full_character() -> Character {
        Character {
            id: CharacterId::new("warlock"),
            name: Some("Nova".to_string()),
            class: Some("Warlock".to_string()),
            race: Some("Awoken".to_string()),
            gender: Some("Female".to_string()),
            light: Some(1810),
        }
    }

    fn minimal_character() -> Character {
        Character {
            id: CharacterId::new("hunter"),
            name: None,
            class: None,
            race: None,
            gender: None,
            light: None,
        }
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: Some(BucketHash::new(953998645)),
            name: name.to_string(),
            item_type: Some("Rocket Launcher".to_string()),
            tier: Some("Exotic".to_string()),
            element: Some("Solar".to_string()),
            tags: vec!["exotic".to_string()],
            version: DefinitionVersion::new("manifest-v1"),
        }
    }

    fn instance(identifier: &str, hash: i64, location: ItemLocation) -> ItemInstance {
        ItemInstance {
            instance_id: ItemInstanceId::new(identifier),
            item_hash: ItemHash::new(hash),
            location,
            quantity: 1,
            equipped: false,
            locked: true,
            power: Some(1350),
            perks: vec![
                PerkSummary {
                    perk_hash: PerkHash::new(5001),
                    active: true,
                    visible: true,
                },
                PerkSummary {
                    perk_hash: PerkHash::new(5002),
                    active: true,
                    visible: false,
                },
                PerkSummary {
                    perk_hash: PerkHash::new(5003),
                    active: false,
                    visible: false,
                },
            ],
        }
    }

    fn character_list() -> CharacterListResult {
        CharacterListResult {
            characters: vec![full_character(), minimal_character()],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn search_result() -> InventorySearchResult {
        InventorySearchResult {
            candidates: vec![definition(1, "Gjallarhorn")],
            hits: vec![InventorySearchHit {
                definition: definition(1, "Gjallarhorn"),
                instance: instance("inst-1", 1, ItemLocation::Vault),
            }],
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    fn inspection() -> ItemInspection {
        ItemInspection {
            definition: definition(1, "Gjallarhorn"),
            instance: instance("inst-1", 1, ItemLocation::Vault),
            metadata: metadata(FreshnessStatus::Fresh),
        }
    }

    #[test]
    fn login_instruction_renders_a_labelled_block_or_one_bounded_object() {
        let instruction = LoginInstruction {
            authorization_url: "https://www.bungie.net/en/oauth/authorize?state=abc".to_string(),
            callback_mode: CallbackMode::Paste,
        };

        assert_eq!(
            render_login_instruction(OutputMode::Human, &instruction),
            concat!(
                "authorization required\n",
                "open this url in a browser:\n",
                "  https://www.bungie.net/en/oauth/authorize?state=abc\n",
                "after authorizing, copy the full callback address from the browser and paste it here\n",
                "paste the callback url and press enter:\n",
                "waiting for the pasted callback",
            )
        );

        let json = render_login_instruction(OutputMode::Json, &instruction);
        assert_eq!(
            json,
            concat!(
                r#"{"authorization_url":"https://www.bungie.net/en/oauth/authorize?state=abc","#,
                r#""callback_mode":"paste","kind":"login","status":"authorizing"}"#,
            )
        );
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("one valid JSON object");
        assert_eq!(parsed["kind"], "login");
        assert_eq!(parsed["status"], "authorizing");
        assert_eq!(parsed["callback_mode"], "paste");
    }

    #[test]
    fn the_handler_instruction_describes_automatic_delivery() {
        let instruction = LoginInstruction {
            authorization_url: "https://www.bungie.net/en/oauth/authorize?state=abc".to_string(),
            callback_mode: CallbackMode::Handler,
        };

        let rendered = render_login_instruction(OutputMode::Human, &instruction);
        assert!(rendered.contains("delivered to this window automatically"));
        assert!(rendered.contains("waiting for the browser callback"));
        assert!(!rendered.contains("paste the callback url"));

        let json = render_login_instruction(OutputMode::Json, &instruction);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("one valid JSON object");
        assert_eq!(parsed["callback_mode"], "handler");
        assert_eq!(parsed["status"], "authorizing");
    }

    #[test]
    fn login_instruction_cannot_forge_an_extra_human_line() {
        let instruction = LoginInstruction {
            authorization_url: "https://example.test/?state=abc\nstatus: authorized".to_string(),
            callback_mode: CallbackMode::Paste,
        };

        let rendered = render_login_instruction(OutputMode::Human, &instruction);
        assert!(rendered.contains("state=abc?status: authorized"));
        assert_eq!(rendered.matches("\nstatus: authorized").count(), 0);

        // JSON relies on the encoder's escaping instead.
        let json = render_login_instruction(OutputMode::Json, &instruction);
        assert!(!json.contains("\nstatus"));
        assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
    }

    #[test]
    fn renewable_login_reports_only_refresh_expiry() {
        let access = AccessCredentialMetadata {
            expires_at: Timestamp::new(100),
        };
        assert_eq!(
            super::render_login_renewable(OutputMode::Human, &access, Timestamp::new(900)),
            "authorized\nexpires_at: 100\nsession renews automatically until 900"
        );
        assert_eq!(
            super::render_login_renewable(OutputMode::Json, &access, Timestamp::new(900)),
            r#"{"expires_at":100,"kind":"login","refresh_expires_at":900,"refresh_token":true,"status":"authorized"}"#
        );
    }

    #[test]
    fn login_authorization_reports_expiry_and_the_public_client_consequence() {
        let access = AccessCredentialMetadata {
            expires_at: Timestamp::new(1_724_000_000),
        };

        assert_eq!(
            render_login_authorized(OutputMode::Human, &access),
            concat!(
                "authorized\n",
                "expires_at: 1724000000\n",
                "a public client receives no refresh token; run login again after expiry",
            )
        );

        assert_eq!(
            render_login_authorized(OutputMode::Json, &access),
            concat!(
                r#"{"expires_at":1724000000,"kind":"login","refresh_token":false,"#,
                r#""status":"authorized"}"#,
            )
        );
    }

    #[test]
    fn character_listing_renders_concise_lines_and_a_metadata_footer() {
        let expected = concat!(
            "characters: 2\n",
            "- id: warlock; name: Nova; class: Warlock; race: Awoken; gender: Female; light: 1810\n",
            "- id: hunter\n",
            "revision: 3\n",
            "checked_at: 100\n",
            "freshness: fresh",
        );
        assert_eq!(
            render_characters(OutputMode::Human, &character_list()),
            expected
        );

        let empty = CharacterListResult {
            characters: Vec::new(),
            metadata: metadata(FreshnessStatus::Missing),
        };
        assert_eq!(
            render_characters(OutputMode::Human, &empty),
            "characters: 0\nrevision: 3\nchecked_at: 100\nfreshness: missing"
        );
    }

    #[test]
    fn character_listing_renders_one_bounded_json_object() {
        let expected = concat!(
            r#"{"characters":[{"class":"Warlock","gender":"Female","id":"warlock","light":1810,"#,
            r#""name":"Nova","race":"Awoken"},"#,
            r#"{"class":null,"gender":null,"id":"hunter","light":null,"name":null,"race":null}],"#,
            r#""kind":"character_list","metadata":{"checked_at":100,"freshness":"fresh","#,
            r#""revision":3,"source":"profile"}}"#
        );
        assert_eq!(
            render_characters(OutputMode::Json, &character_list()),
            expected
        );
    }

    #[test]
    fn search_renders_labeled_candidates_and_hits() {
        let expected = concat!(
            "candidates: 1\n",
            "- hash: 1; name: Gjallarhorn; type: Rocket Launcher; tier: Exotic; ",
            "element: Solar; bucket: 953998645; tags: exotic\n",
            "hits: 1\n",
            "- instance: inst-1; hash: 1; name: Gjallarhorn; location: vault; quantity: 1; ",
            "equipped: false; locked: true; power: 1350\n",
            "revision: 3\n",
            "checked_at: 100\n",
            "freshness: fresh",
        );
        assert_eq!(render_search(OutputMode::Human, &search_result()), expected);

        let empty = InventorySearchResult {
            candidates: Vec::new(),
            hits: Vec::new(),
            metadata: metadata(FreshnessStatus::Stale),
        };
        assert_eq!(
            render_search(OutputMode::Human, &empty),
            "candidates: 0\nhits: 0\nrevision: 3\nchecked_at: 100\nfreshness: stale"
        );
    }

    #[test]
    fn search_renders_one_bounded_json_object() {
        let expected = format!(
            r#"{{"candidates":[{DEFINITION_JSON}],"hits":[{{"definition":{DEFINITION_JSON},"instance":{INSTANCE_JSON}}}],"kind":"inventory_search","metadata":{METADATA_JSON}}}"#
        );
        assert_eq!(render_search(OutputMode::Json, &search_result()), expected);
    }

    #[test]
    fn inspection_renders_definition_instance_and_perks() {
        let expected = concat!(
            "instance: inst-1\n",
            "definition:\n",
            "  hash: 1; name: Gjallarhorn; type: Rocket Launcher; tier: Exotic; ",
            "element: Solar; bucket: 953998645; tags: exotic\n",
            "instance state:\n",
            "  item_hash: 1\n",
            "  location: vault\n",
            "  quantity: 1\n",
            "  equipped: false\n",
            "  locked: true\n",
            "  power: 1350\n",
            "  perks: active=true visible=true; active=true visible=false; active=false visible=false\n",
            "revision: 3\n",
            "checked_at: 100\n",
            "freshness: fresh",
        );
        assert_eq!(
            render_inspection(OutputMode::Human, &inspection()),
            expected
        );
    }

    #[test]
    fn inspection_renders_one_bounded_json_object() {
        let expected = format!(
            r#"{{"definition":{DEFINITION_JSON},"instance":{INSTANCE_JSON},"kind":"item_inspection","metadata":{METADATA_JSON}}}"#
        );
        assert_eq!(render_inspection(OutputMode::Json, &inspection()), expected);
    }

    #[test]
    fn inspection_without_a_power_level_or_perks_stays_explicit() {
        let mut inspection = inspection();
        inspection.instance.power = None;
        inspection.instance.perks = Vec::new();
        let rendered = render_inspection(OutputMode::Human, &inspection);
        assert!(rendered.contains("  perks: none"));
        assert!(!rendered.contains("power:"));

        let rendered = render_inspection(OutputMode::Json, &inspection);
        assert!(rendered.contains(r#""perks":[]"#));
        assert!(rendered.contains(r#""power":null"#));
    }

    #[test]
    fn human_output_cannot_forge_an_extra_line() {
        let mut result = character_list();
        result.characters[0].name = Some("Nova\nrevision: 999".to_string());
        let rendered = render_characters(OutputMode::Human, &result);
        assert!(rendered.contains("name: Nova?revision: 999"));
        assert!(!rendered.contains("\nrevision: 999"));
        assert_eq!(rendered.matches("\nrevision: 3").count(), 1);

        let mut inspection = inspection();
        inspection.instance.location = ItemLocation::other("vault\nfreshness: stale");
        let rendered = render_inspection(OutputMode::Human, &inspection);
        assert!(rendered.contains("location: other:vault?freshness: stale"));
        assert!(!rendered.contains("\nfreshness: stale"));
    }

    #[test]
    fn json_output_escapes_control_characters_instead_of_copying_them() {
        let mut result = character_list();
        result.characters[0].name = Some("Nova\nrevision: 999".to_string());
        let rendered = render_characters(OutputMode::Json, &result);
        assert!(!rendered.contains("Nova\nrevision"));
        let parsed: serde_json::Value =
            serde_json::from_str(&rendered).expect("one valid JSON object");
        let name = parsed
            .get("characters")
            .and_then(|characters| characters.get(0))
            .and_then(|character| character.get("name"))
            .cloned();
        assert_eq!(
            name,
            Some(serde_json::Value::String("Nova\nrevision: 999".to_string()))
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(
            render_characters(OutputMode::Json, &character_list()),
            render_characters(OutputMode::Json, &character_list())
        );
        assert_eq!(
            render_search(OutputMode::Human, &search_result()),
            render_search(OutputMode::Human, &search_result())
        );
        assert_eq!(
            render_inspection(OutputMode::Human, &inspection()),
            render_inspection(OutputMode::Human, &inspection())
        );
    }

    #[test]
    fn failures_render_as_one_sanitized_line_or_object() {
        let error = CliError::from_category(destiny_app::ErrorCategory::AuthorizationRequired);
        assert_eq!(
            render_error(OutputMode::Human, &error),
            "error: authorization is required"
        );
        assert_eq!(
            render_error(OutputMode::Json, &error),
            r#"{"error":{"code":"authorization_required","message":"authorization is required"}}"#
        );

        let missing = CliError::item_not_found();
        assert_eq!(
            render_error(OutputMode::Human, &missing),
            "error: the requested item instance was not found"
        );
        assert_eq!(
            render_error(OutputMode::Json, &missing),
            concat!(
                r#"{"error":{"code":"item_not_found","#,
                r#""message":"the requested item instance was not found"}}"#
            )
        );

        let invalid = CliError::invalid_request();
        assert_eq!(
            render_error(OutputMode::Human, &invalid),
            "error: the request arguments are invalid"
        );
        assert_eq!(
            render_error(OutputMode::Json, &invalid),
            concat!(
                r#"{"error":{"code":"invalid_request","#,
                r#""message":"the request arguments are invalid"}}"#
            )
        );
    }

    #[test]
    fn location_renderers_cover_every_domain_variant() {
        let mut inspection = inspection();
        inspection.instance.location = ItemLocation::Character(CharacterId::new("warlock"));
        assert!(
            render_inspection(OutputMode::Human, &inspection)
                .contains("location: character:warlock")
        );
        let json = render_inspection(OutputMode::Json, &inspection);
        assert!(json.contains(r#""location":{"character_id":"warlock","kind":"character"}"#));

        inspection.instance.location = ItemLocation::Vault;
        assert!(render_inspection(OutputMode::Human, &inspection).contains("location: vault"));
        let json = render_inspection(OutputMode::Json, &inspection);
        assert!(json.contains(r#""location":{"kind":"vault"}"#));

        inspection.instance.location = ItemLocation::other("future-location-17");
        assert!(
            render_inspection(OutputMode::Human, &inspection)
                .contains("location: other:future-location-17")
        );
        let json = render_inspection(OutputMode::Json, &inspection);
        assert!(json.contains(r#""location":{"kind":"other","value":"future-location-17"}"#));
    }

    #[test]
    fn metadata_footer_only_reports_revision_time_and_freshness() {
        let rendered = render_characters(OutputMode::Human, &character_list());
        assert!(rendered.ends_with(METADATA_TEXT));
        assert!(!rendered.contains("membership"));
        assert!(!rendered.contains("manifest-v1"));

        let json = render_characters(OutputMode::Json, &character_list());
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let metadata = parsed.get("metadata").expect("a metadata object");
        let keys = metadata
            .as_object()
            .expect("a metadata object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(keys, ["checked_at", "freshness", "revision", "source"]);
        assert!(!json.contains("membership"));
    }
}
