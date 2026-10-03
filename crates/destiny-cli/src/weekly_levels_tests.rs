use super::weekly_human::{Level, render_level};
use super::weekly_human_tests::{fixture, group, resolved};
use destiny_app::{DefinitionResolution, WeeklyModifier};

#[test]
fn weekly_presentations_read_the_model_category() {
    let (character, mut weekly) = level_fixture();
    let modifier = &mut weekly.featured_raids[0].activities[0].modifiers[0];
    modifier.category = destiny_app::weekly_personal::ModifierCategory::Surge;
    let debug = render_level(&character, &weekly, Level::Debug);
    assert!(debug.contains("Surge            Shielded Foes #100"));
    let json = super::render_weekly(super::OutputMode::Json, &character, &weekly);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        json["featured_raids"][0]["variants"][0]["modifiers"][0]["category"],
        "surge"
    );
}

fn level_fixture() -> (destiny_app::Character, destiny_app::WeeklyPersonal) {
    let (character, mut weekly) = fixture();
    weekly.vanguard_alerts = Default::default();
    weekly.featured_dungeons.clear();
    weekly.weekly_rotators = Default::default();
    weekly.featured_raids.truncate(1);
    weekly.definition_version = destiny_app::DefinitionVersion::new("v1");
    let raid = &mut weekly.featured_raids[0];
    raid.activities[0].modifiers = vec![
        WeeklyModifier::new(100, resolved("Shielded Foes")),
        WeeklyModifier::new(101, resolved("Shielded Foes")),
        WeeklyModifier::new(102, DefinitionResolution::Unresolved),
    ];
    raid.activities[1].modifiers = raid.activities[0].modifiers.clone();
    for (hash, name) in [
        (103, "Arc Surge"),
        (104, "Stasis Surge"),
        (105, "Overcharged Fusion Rifle"),
        (106, "Champion Foes"),
        (107, "Master Modifiers"),
    ] {
        raid.activities[1]
            .modifiers
            .push(WeeklyModifier::new(hash, resolved(name)));
    }
    (character, weekly)
}

#[test]
fn weekly_details_is_pinned() {
    let (character, weekly) = level_fixture();
    assert_eq!(
        render_level(&character, &weekly, Level::Details),
        "Ghost Link — Weekly\nHunter · Power 432\nModifier groups are derived from names.\n\nFEATURED RAIDS\nCrota's End\n  ○ Raid completed: 0/1\n  Encounters  0/4\n  Rewards     Raid Gear · Deepsight Weapon (Chance) · Deepsight Weapon\n  Modifiers\n    Other modifiers  Shielded Foes\n  Master\n    Surge            Arc Surge · Stasis Surge\n    Overcharge       Overcharged Fusion Rifle\n    Champions        Champion Foes\n    Other modifiers  Master Modifiers\n\n1 unnamed modifier hidden; use --debug.\n\n⚠ 1 activity could not be classified.\n  Run `ghostlink sync-manifest`."
    );
}

#[test]
fn weekly_debug_is_pinned() {
    let (character, weekly) = level_fixture();
    assert_eq!(
        render_level(&character, &weekly, Level::Debug),
        "Ghost Link — Weekly\nHunter · Power 432\nModifier groups are derived from names.\nManifest v1\n\nFEATURED RAIDS\nCrota's End  #10 · #11\n  Crota's End: Standard  #10\n    ○ Raid completed: 0/1  #20\n    Encounters  0/4\n    Rewards          Raid Gear #0 · Deepsight Weapon (Chance) #1 · Deepsight Weapon #2\n    Modifiers\n      Other modifiers  Shielded Foes #100 · Shielded Foes #101 · unresolved #102 (unresolved)\n  Crota's End: Master  #11\n    ○ Raid completed: 0/1  #20\n    Encounters  0/4\n    Rewards          Raid Gear #0 · Deepsight Weapon (Chance) #1 · Deepsight Weapon #2\n    Modifiers\n      Surge            Arc Surge #103 · Stasis Surge #104\n      Overcharge       Overcharged Fusion Rifle #105\n      Champions        Champion Foes #106\n      Other modifiers  Shielded Foes #100 · Shielded Foes #101 · unresolved #102 (unresolved) · Master Modifiers #107\n\nUNCLASSIFIED\nActivity 999\n  name    unknown\n  reason  activity_definition_missing\n  source  character_activities"
    );
}

#[test]
fn weekly_debug_keeps_unresolved_and_duplicate_entries_and_pool_variants() {
    let (character, mut weekly) = level_fixture();
    let raid = &mut weekly.featured_raids[0];
    let activity = &mut raid.activities[0];
    let mut duplicate = activity.challenges[0].clone();
    duplicate.objective_hash = 21;
    activity.challenges.push(duplicate);
    activity
        .challenges
        .push(destiny_app::weekly_personal::PersonalChallenge {
            objective_hash: 22,
            definition: DefinitionResolution::Unresolved,
            progress: 0,
            completion_value: 1,
            complete: false,
        });
    let mut reward = activity.rewards[0].clone();
    reward.hash = 23;
    activity.rewards.push(reward);
    activity
        .rewards
        .push(destiny_app::weekly_personal::PersonalReward {
            hash: 24,
            definition: DefinitionResolution::Unresolved,
        });
    activity.challenge_objectives[0].definition = DefinitionResolution::Unresolved;
    let mut quickplay = raid.clone();
    quickplay.root_name = "Quickplay".into();
    quickplay.pools = quickplay
        .activities
        .iter()
        .map(|variant| destiny_app::weekly_personal::QuickplayPool {
            objectives: variant.challenges.clone(),
            rewards: vec![
                if variant.hash == 10 {
                    "Main Ingredient"
                } else {
                    "The Spiteful Fang"
                }
                .into(),
            ],
            variants: vec![variant.clone()],
        })
        .collect();
    weekly.vanguard_alerts.quickplay = vec![quickplay];
    let text = render_level(&character, &weekly, Level::Debug);
    for expected in [
        "Raid completed: 0/1  #20",
        "Raid completed: 0/1  #21",
        "unresolved 0/1  #22 (unresolved)",
        "Raid Gear #23",
        "unresolved #24 (unresolved)",
        "Pool 1  #10",
        "Pool 2  #11",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    let details = render_level(&character, &weekly, Level::Details);
    assert!(details.contains("  → Main Ingredient\n  Modifiers\n"));
    assert!(details.contains("  → The Spiteful Fang\n  Modifiers\n"));
    assert!(!text.contains("    Objective"));
    assert_eq!(
        super::render_weekly(super::OutputMode::Json, &character, &weekly),
        super::render_weekly_level(super::OutputMode::Json, &character, &weekly, Level::Default)
    );
}

#[test]
fn weekly_details_hidden_hashes_are_distinct_and_quickplay_shared_sets_merge() {
    let (character, mut weekly) = level_fixture();
    let mut quickplay = weekly.featured_raids[0].clone();
    let mut first = quickplay.activities[0].clone();
    first.rewards.clear();
    first.modifiers.push(WeeklyModifier::new(
        1783825372,
        DefinitionResolution::Unresolved,
    ));
    let mut second = first.clone();
    second.modifiers.reverse();
    quickplay.pools = vec![
        destiny_app::weekly_personal::QuickplayPool {
            objectives: vec![],
            rewards: vec!["Main Ingredient".into()],
            variants: vec![first],
        },
        destiny_app::weekly_personal::QuickplayPool {
            objectives: vec![],
            rewards: vec!["The Spiteful Fang".into()],
            variants: vec![second],
        },
    ];
    quickplay.root_name = "Quickplay".into();
    weekly.vanguard_alerts.quickplay = vec![quickplay];
    let text = render_level(&character, &weekly, Level::Details);
    assert!(text.contains("Quickplay\n  Main Ingredient\n  The Spiteful Fang\n  Modifiers\n    Other modifiers  Shielded Foes"));
    assert!(!text.contains("Pool "));
    // Shared pools render exactly one Modifiers block for the whole Quickplay entity.
    let quickplay_block = &text[text.find("Quickplay\n").unwrap()..];
    let quickplay_block = &quickplay_block[..quickplay_block
        .find("\n\n")
        .unwrap_or(quickplay_block.len())];
    assert_eq!(quickplay_block.matches("Modifiers").count(), 1);
    assert!(!text.contains("unresolved #"));
    assert!(text.contains("2 unnamed modifiers hidden; use --debug.\n\n⚠"));
    weekly.unclassified.clear();
    assert!(
        render_level(&character, &weekly, Level::Details)
            .ends_with("2 unnamed modifiers hidden; use --debug.")
    );
}

#[test]
fn weekly_shared_modifier_sets_have_one_block() {
    let (character, mut weekly) = level_fixture();
    let raid = &mut weekly.featured_raids[0];
    raid.activities[1].modifiers = raid.activities[0].modifiers.clone();
    raid.activities[1].modifiers.reverse();
    let text = render_level(&character, &weekly, Level::Details);
    assert_eq!(text.matches("  Modifiers\n").count(), 1);
    assert!(!text.contains("  Master\n"));
    assert!(text.contains("Other modifiers  Shielded Foes"));
    assert!(!text.contains("unresolved #102"));
    // Empty modifier lists do not invent a block.
    weekly.featured_raids = vec![group("Empty", vec![], &[], false)];
    assert!(!render_level(&character, &weekly, Level::Details).contains("  Modifiers"));
}
