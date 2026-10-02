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
        modifiers: vec![WeeklyModifier {
            hash: 2,
            definition: unresolved.clone(),
        }],
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
    };
    let weekly = WeeklyPersonal {
        definition_version: DefinitionVersion::new("v1"),
        unclassified_unresolved: 2,
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
    assert!(cli.get("newest").is_none());
    assert!(cli.get("portal_focus").is_none());
}
