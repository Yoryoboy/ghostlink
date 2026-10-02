//! Exercise the exact CLI JSON rendering entry without adding a presentation dependency.
#[path = "../../destiny-cli/src/weekly_json.rs"]
mod cli_json;

#[test]
fn weekly_cli_mcp_json_parity_on_the_same_fixture() {
    use destiny_app::*;
    let character = Character {
        id: CharacterId::new("123"),
        name: Some("name\n\"quoted\"".into()),
        class: Some("Hunter".into()),
        race: None,
        gender: None,
        light: Some(2000),
    };
    let unresolved = DefinitionResolution::Unresolved;
    let activity = PersonalActivity {
        hash: 99,
        definition: unresolved.clone(),
        activity_type_hash: Some(42),
        activity_type: DefinitionResolution::Resolved {
            name: "Raid".into(),
            description: "".into(),
        },
        rewards: vec![PersonalReward {
            hash: 1,
            definition: unresolved.clone(),
        }],
        modifiers: std::iter::once(WeeklyModifier {
            hash: 2,
            definition: unresolved.clone(),
        })
        .chain(
            [
                "Arc Surge",
                "Overcharged Fusion Rifle",
                "Champion Foes",
                "Shielded Foes",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, name)| WeeklyModifier {
                hash: 10 + i as u32,
                definition: DefinitionResolution::Resolved {
                    name: name.into(),
                    description: String::new(),
                },
            }),
        )
        .collect(),
        challenge_objectives: vec![PersonalObjective {
            hash: 3,
            definition: unresolved.clone(),
        }],
        challenges: vec![PersonalChallenge {
            objective_hash: 3,
            definition: unresolved.clone(),
            progress: 1,
            completion_value: 2,
            complete: false,
        }],
        phases: Some(PhaseProgress {
            complete: 2,
            total: 4,
        }),
        focused: true,
        recommended_light: Some(2000),
        has_challenge: true,
    };
    let group = PersonalGroup {
        root_name: "Root\n\"label\"".into(),
        activity_type_name: "Raid".into(),
        activities: vec![activity.clone()],
        reward_names: vec![],
        pools: vec![QuickplayPool {
            objectives: activity.challenges.clone(),
            rewards: vec!["Ops Bonus Drop".into(), "Main Ingredient".into()],
            variants: vec![activity.clone()],
        }],
    };
    let mut missing_type = activity.clone();
    missing_type.hash = 100;
    missing_type.definition = DefinitionResolution::Resolved {
        name: "Known activity".into(),
        description: String::new(),
    };
    missing_type.activity_type = DefinitionResolution::Unresolved;
    let inputs = [activity.clone(), missing_type];
    let weekly = WeeklyPersonal {
        definition_version: DefinitionVersion::new("v1"),
        unclassified: weekly_personal::unclassified_activities(&inputs, &inputs),
        vanguard_alerts: VanguardAlerts {
            grandmaster: Some(group.clone()),
            crucible: vec![group.clone()],
            quickplay: vec![group.clone()],
        },
        featured_raids: vec![group.clone()],
        featured_dungeons: vec![group.clone()],

        weekly_rotators: WeeklyRotators {
            expedition: Some(group.clone()),
            override_activity: Some(group.clone()),
        },

        character_activities: vec![],
        public_activities: vec![],
        milestones: vec![PersonalMilestone {
            hash: 5,
            definition: unresolved,
        }],
    };
    let cli: serde_json::Value =
        serde_json::from_str(&cli_json::render_weekly_json(&character, &weekly)).unwrap();
    let mcp = destiny_mcp::service::weekly_output(&character, &weekly).unwrap();
    assert_eq!(cli, serde_json::to_value(mcp).unwrap());
    assert_eq!(
        cli["vanguard_alerts"]["grandmaster"]["variants"][0]["name"],
        serde_json::Value::Null
    );
    assert_eq!(
        cli["vanguard_alerts"]["grandmaster"]["variants"][0]["hash"],
        99
    );
    assert_eq!(cli["unclassified_unresolved"], 4);
    assert_eq!(
        cli["unclassified"],
        serde_json::json!([
            {"hash":99,"name":null,"reason":"activity_definition_missing","source":"character_activities"},
            {"hash":100,"name":"Known activity","reason":"activity_type_missing","source":"character_activities"},
            {"hash":99,"name":null,"reason":"activity_definition_missing","source":"public_milestones"},
            {"hash":100,"name":"Known activity","reason":"activity_type_missing","source":"public_milestones"}
        ])
    );
    let pool = &cli["vanguard_alerts"]["quickplay"][0]["pools"][0];
    assert_eq!(
        pool["rewards"],
        serde_json::json!(["Ops Bonus Drop", "Main Ingredient"])
    );
    assert_eq!(pool["objectives"][0]["progress"], 1);
    assert_eq!(pool["variants"][0]["hash"], 99);
    assert_eq!(pool["variants"][0]["modifiers"][0]["category"], "other");
    assert_eq!(
        cli["featured_raids"][0]["variants"][0]["modifiers"][0]["category"],
        "other"
    );
    let categories: Vec<_> = pool["variants"][0]["modifiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["category"].as_str().unwrap())
        .collect();
    assert_eq!(
        categories,
        ["other", "surge", "overcharge", "champions", "other"]
    );
    assert!(cli.get("newest").is_none());
    assert!(cli.get("portal_focus").is_none());
}
