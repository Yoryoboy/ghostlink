use super::*;
use destiny_app::{ArtifactPoints, ArtifactResult, CharacterArtifact, CharacterId, ItemHash};

fn fixture() -> ArtifactResult {
    ArtifactResult {
        points: Some(ArtifactPoints {
            points: 12,
            points_cap: 12,
            progress_to_next_level: 34,
            next_level_at: 100,
        }),
        characters: vec![
            CharacterArtifact {
                character_id: CharacterId::new("10"),
                class: Some("Hunter".into()),
                equipped: Some(destiny_app::EquippedArtifact {
                    instance_id: destiny_app::ItemInstanceId::new("tablet"),
                    item_hash: ItemHash::new(30),
                    name: Some("Tablet of Ruin".into()),
                    slots: vec![
                        destiny_app::ArtifactSlot {
                            plug_hash: ItemHash::new(40),
                            name: Some("Perk 1".into()),
                            description: None,
                            empty: false,
                        },
                        destiny_app::ArtifactSlot {
                            plug_hash: ItemHash::new(47),
                            name: Some("Empty Artifact Mod".into()),
                            description: None,
                            empty: true,
                        },
                    ],
                }),
            },
            CharacterArtifact {
                character_id: CharacterId::new("20"),
                class: Some("Titan".into()),
                equipped: None,
            },
        ],
    }
}
#[test]
fn artifact_description_is_optional_and_visible_in_text_and_json() {
    let mut result = fixture();
    let slot = &mut result.characters[0].equipped.as_mut().unwrap().slots[0];
    slot.description = Some("Improve damage.\nWhile active.".into());
    assert_eq!(
        artifact_json(&result)["characters"][0]["equipped"]["slots"][0]["description"],
        "Improve damage.\nWhile active."
    );
    let human = render_artifact(OutputMode::Human, &result);
    assert!(human.contains("    Perk 1:\n      Improve damage.\n      While active."));
    assert!(
        artifact_json(&fixture())["characters"][0]["equipped"]["slots"][0]
            .get("description")
            .is_none()
    );
}

#[test]
fn artifact_text_uses_points_and_equipped_item() {
    assert_eq!(
        render_artifact(OutputMode::Human, &fixture()),
        "Artifact points: 12/12\nHunter: Tablet of Ruin\n  Perks: Perk 1, (empty)\nTitan: no artifact equipped"
    );
}
#[test]
fn artifact_text_shows_progress_only_below_cap() {
    let mut result = fixture();
    result.points.as_mut().unwrap().points = 5;
    assert!(
        render_artifact(OutputMode::Human, &result)
            .starts_with("Artifact points: 5/12 (34/100 to next point)\n")
    );
    result.points.as_mut().unwrap().points = 0;
    assert!(
        render_artifact(OutputMode::Human, &result)
            .starts_with("Artifact points: 0/12 (34/100 to next point)\n")
    );
}
#[test]
fn artifact_missing_points_names_and_empty_results() {
    let mut result = fixture();
    result.points = None;
    result.characters[0].class = None;
    let equipped = result.characters[0].equipped.as_mut().unwrap();
    equipped.name = None;
    equipped.slots[0].name = None;
    assert!(
        render_artifact(OutputMode::Human, &result)
            .starts_with("Artifact points: unavailable\nUnknown: #30\n  Perks: #40, (empty)")
    );
    result.characters[0].equipped.as_mut().unwrap().name = Some("Tablet\nInjected".into());
    assert!(render_artifact(OutputMode::Human, &result).contains("Unknown: Tablet?Injected"));
    result.characters.clear();
    assert_eq!(
        render_artifact(OutputMode::Human, &result),
        "Artifact points: unavailable"
    );
}
#[test]
fn artifact_json_projects_points_and_equipped_only() {
    let expected = json!({"kind":"artifact", "points":{"points":12,"points_cap":12,"progress_to_next_level":34,"next_level_at":100}, "characters":[
        {"character_id":"10","class":"Hunter","equipped":{"instance_id":"tablet","item_hash":30,"name":"Tablet of Ruin","slots":[{"plug_hash":40,"name":"Perk 1","empty":false},{"plug_hash":47,"name":"Empty Artifact Mod","empty":true}]}},
        {"character_id":"20","class":"Titan","equipped":null}
    ]});
    assert_eq!(
        serde_json::from_str::<Value>(&render_artifact(OutputMode::Json, &fixture())).unwrap(),
        expected
    );
    let mut result = fixture();
    result.points = None;
    assert_eq!(artifact_json(&result)["points"], Value::Null);
}
