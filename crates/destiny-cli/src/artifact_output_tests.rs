use super::*;
use destiny_app::{
    ArtifactPerkView, ArtifactResult, ArtifactSummary, ArtifactTierView, CharacterArtifact,
    CharacterArtifactState, CharacterId, ItemHash,
};

fn fixture() -> ArtifactResult {
    let active = ArtifactPerkView {
        item_hash: ItemHash::new(2),
        name: Some("Perk".into()),
        active: true,
    };
    ArtifactResult {
        artifact: Some(ArtifactSummary {
            artifact_hash: ItemHash::new(1),
            name: Some("Compass".into()),
            points_acquired: 12,
            power_bonus: 5,
        }),
        characters: vec![
            CharacterArtifact {
                character_id: CharacterId::new("10"),
                class: Some("Hunter".into()),
                state: Some(CharacterArtifactState {
                    points_used: 3,
                    reset_count: 2,
                    active_perks: vec![active.clone()],
                    tiers: vec![
                        ArtifactTierView {
                            index: 1,
                            unlocked: true,
                            points_to_unlock: 0,
                            perks: vec![
                                active,
                                ArtifactPerkView {
                                    item_hash: ItemHash::new(3),
                                    name: None,
                                    active: false,
                                },
                            ],
                        },
                        ArtifactTierView {
                            index: 2,
                            unlocked: false,
                            points_to_unlock: 4,
                            perks: vec![],
                        },
                    ],
                }),
            },
            CharacterArtifact {
                character_id: CharacterId::new("20"),
                class: Some("Titan".into()),
                state: None,
            },
        ],
    }
}

#[test]
fn artifact_text_with_and_without_tiers() {
    let base =
        "Artifact: Compass — 12 points, +5 power\nHunter: 3 points used, 2 resets\n  Active: Perk";
    assert_eq!(
        render_artifact(OutputMode::Human, &fixture(), false),
        format!("{base}\nTitan: no artifact data")
    );
    assert_eq!(
        render_artifact(OutputMode::Human, &fixture(), true),
        format!(
            "{base}\n  Tier 1 (unlocked): Perk*, #3\n  Tier 2 (locked): \nTitan: no artifact data"
        )
    );
}

#[test]
fn artifact_missing_summary_names_and_empty_results() {
    let mut result = fixture();
    result.artifact.as_mut().unwrap().name = None;
    assert!(render_artifact(OutputMode::Human, &result, false).starts_with("Artifact: #1 —"));
    result.artifact = None;
    result.characters[0].class = None;
    let state = result.characters[0].state.as_mut().unwrap();
    state.active_perks[0].name = Some("Perk\nInjected".into());
    assert!(
        render_artifact(OutputMode::Human, &result, false).starts_with(
            "Artifact: no artifact data\nUnknown: 3 points used, 2 resets\n  Active: Perk?Injected"
        )
    );
    state_clear(&mut result);
    assert!(render_artifact(OutputMode::Human, &result, false).contains("  Active: none"));
    result.characters.clear();
    assert_eq!(
        render_artifact(OutputMode::Human, &result, false),
        "Artifact: no artifact data"
    );
}

fn state_clear(result: &mut ArtifactResult) {
    result.characters[0]
        .state
        .as_mut()
        .unwrap()
        .active_perks
        .clear();
}

#[test]
fn artifact_json_projects_full_result_with_null_state() {
    let expected = json!({"kind":"artifact", "artifact":{"artifact_hash":1,"name":"Compass","points_acquired":12,"power_bonus":5}, "characters":[
        {"character_id":"10","class":"Hunter","state":{"points_used":3,"reset_count":2,"active_perks":[{"item_hash":2,"name":"Perk","active":true}],"tiers":[
            {"index":1,"unlocked":true,"points_to_unlock":0,"perks":[{"item_hash":2,"name":"Perk","active":true},{"item_hash":3,"name":null,"active":false}]},
            {"index":2,"unlocked":false,"points_to_unlock":4,"perks":[]}
        ]}}, {"character_id":"20","class":"Titan","state":null}
    ]});
    for tiers in [false, true] {
        assert_eq!(
            serde_json::from_str::<Value>(&render_artifact(OutputMode::Json, &fixture(), tiers))
                .unwrap(),
            expected
        );
    }
    let mut result = fixture();
    result.artifact = None;
    assert_eq!(artifact_json(&result)["artifact"], Value::Null);
}
