//! Deterministic human and JSON rendering for the read journey.
//!
//! Every renderer is pure and total over one shared result record. Human output
//! is concise, line-oriented text with a metadata footer; JSON output is exactly
//! one bounded object with a stable `kind` and no human prose. Neither mode can
//! invent an identifier, a display string, or a freshness concept that the
//! shared records do not already carry.

use serde_json::{Value, json};

use destiny_app::{
    Character, CharacterListResult, FreshnessStatus, InventorySearchHit, InventorySearchResult,
    ItemDefinition, ItemInspection, ItemInstance, ItemLocation, PerkSummary, QueryMetadata,
    SourceMarker,
};

use crate::command::CliError;

/// The output mode selected by the global `--json` flag.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputMode {
    /// Concise, line-oriented text for people.
    Human,
    /// Exactly one bounded JSON object for programs.
    Json,
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
    use super::{OutputMode, render_characters, render_error, render_inspection, render_search};
    use crate::command::CliError;
    use destiny_app::{
        BucketHash, Character, CharacterId, CharacterListResult, Dataset, DefinitionVersion,
        FreshnessStatus, InventorySearchHit, InventorySearchResult, ItemDefinition, ItemHash,
        ItemInspection, ItemInstance, ItemInstanceId, ItemLocation, PerkHash, PerkSummary,
        QueryMetadata, SourceMarker, SyncMetadata, SyncRevision, Timestamp,
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
