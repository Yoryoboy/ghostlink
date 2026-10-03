use super::{OutputMode, render_weekly};
use destiny_app::weekly_personal::*;
use destiny_app::{Character, CharacterId, DefinitionResolution, DefinitionVersion};

pub(super) fn resolved(name: &str) -> DefinitionResolution {
    DefinitionResolution::Resolved {
        name: name.into(),
        description: String::new(),
    }
}

fn objective(name: &str, progress: i64, completion_value: i64) -> PersonalChallenge {
    PersonalChallenge {
        objective_hash: 20,
        definition: resolved(name),
        progress,
        completion_value,
        complete: progress == completion_value,
    }
}

pub(super) fn group(
    name: &str,
    objectives: Vec<PersonalChallenge>,
    rewards: &[&str],
    phases: bool,
) -> PersonalGroup {
    PersonalGroup {
        root_name: name.into(),
        activity_type_name: "Ignored type".into(),
        reward_names: Vec::new(),
        pools: Vec::new(),
        activities: vec![PersonalActivity {
            hash: 10,
            definition: resolved(&format!("{name}: Standard")),
            activity_type_hash: Some(30),
            activity_type: resolved("Ignored type"),
            rewards: rewards
                .iter()
                .enumerate()
                .map(|(i, name)| PersonalReward {
                    hash: i as u32,
                    definition: resolved(name),
                })
                .collect(),
            modifiers: Vec::new(),
            challenge_objectives: vec![PersonalObjective {
                hash: 20,
                definition: resolved("Never invent progress"),
            }],
            challenges: objectives,
            phases: phases.then_some(PhaseProgress {
                complete: 0,
                total: 4,
            }),
            focused: false,
            recommended_light: None,
            has_challenge: true,
        }],
    }
}

pub(super) fn fixture() -> (Character, WeeklyPersonal) {
    let character = Character {
        id: CharacterId::new("hunter"),
        name: None,
        class: Some("Hunter".into()),
        race: None,
        gender: None,
        light: Some(432),
        date_last_played: None,
    };
    let mut quickplay = group("Quickplay", Vec::new(), &[], false);
    quickplay.pools = vec![
        QuickplayPool {
            objectives: vec![objective("Activities", 0, 3)],
            rewards: vec!["Ops Bonus Drop".into(), "Main Ingredient".into()],
            variants: Vec::new(),
        },
        QuickplayPool {
            objectives: vec![objective("Activities", 0, 3)],
            rewards: vec!["The Spiteful Fang".into()],
            variants: Vec::new(),
        },
    ];
    let mut weekly = WeeklyPersonal {
        definition_version: DefinitionVersion::new("not displayed"),
        vanguard_alerts: VanguardAlerts {
            grandmaster: Some(group(
                "Exodus Crash",
                vec![objective("Activities", 0, 1)],
                &["The Slammer"],
                false,
            )),
            quickplay: vec![quickplay],
            crucible: vec![group(
                "Control",
                vec![objective("Activities", 0, 3)],
                &["The Helmsman"],
                false,
            )],
        },
        featured_raids: vec![
            group(
                "Crota's End",
                vec![objective("Raid completed:", 0, 1)],
                &["Raid Gear", "Deepsight Weapon (Chance)", "Deepsight Weapon"],
                true,
            ),
            group(
                "Deep Stone Crypt",
                Vec::new(),
                &["Raid Gear", "Deepsight Weapon"],
                true,
            ),
        ],
        featured_dungeons: vec![
            group(
                "Ghosts of the Deep",
                vec![objective("Dungeon completed:", 0, 1)],
                &["Powerful Gear"],
                false,
            ),
            group(
                "The Shattered Throne",
                vec![objective("Dungeon completed:", 0, 1)],
                &["Powerful Gear"],
                false,
            ),
        ],
        weekly_rotators: WeeklyRotators {
            expedition: Some(group("Expedition: Cosmodrome", Vec::new(), &[], false)),
            override_activity: Some(group("Override: Europa", Vec::new(), &[], false)),
        },
        unclassified: vec![UnclassifiedActivity {
            hash: 999,
            name: None,
            reason: UnclassifiedReason::ActivityDefinitionMissing,
            source: UnclassifiedSource::CharacterActivities,
        }],
        character_activities: Vec::new(),
        public_activities: Vec::new(),
        milestones: Vec::new(),
    };
    for raid in &mut weekly.featured_raids {
        let mut master = raid.activities[0].clone();
        master.hash = 11;
        master.definition = resolved(&format!("{}: Master", raid.root_name));
        master.modifiers.push(destiny_app::WeeklyModifier::new(
            1234,
            resolved("Hidden modifier"),
        ));
        raid.activities.push(master);
    }
    (character, weekly)
}

#[test]
fn weekly_live_like_dashboard_is_pinned() {
    let (character, weekly) = fixture();
    assert_eq!(
        render_weekly(OutputMode::Human, &character, &weekly),
        "Ghost Link — Weekly\nHunter · Power 432\n\nGRANDMASTER\nExodus Crash\n  ○ Activities 0/1\n  Reward      The Slammer\n\nFEATURED RAIDS\nCrota's End\n  ○ Raid completed: 0/1\n  Encounters  0/4\n  Rewards     Raid Gear · Deepsight Weapon (Chance) · Deepsight Weapon\n\nDeep Stone Crypt\n  Encounters  0/4\n  Rewards     Raid Gear · Deepsight Weapon\n\nFEATURED DUNGEONS\nGhosts of the Deep\n  ○ Dungeon completed: 0/1\n  Reward      Powerful Gear\n\nThe Shattered Throne\n  ○ Dungeon completed: 0/1\n  Reward      Powerful Gear\n\nVANGUARD\nQuickplay\n  ○ Activities 0/3 → Ops Bonus Drop · Main Ingredient\n  ○ Activities 0/3 → The Spiteful Fang\n\nCRUCIBLE\nControl\n  ○ Activities 0/3\n  Reward      The Helmsman\n\nSEASONAL\nExpedition: Cosmodrome\nOverride: Europa\n\n⚠ 1 activity could not be classified.\n  Run `ghostlink sync-manifest`."
    );
}

#[test]
fn weekly_distinct_progress_and_pool_rewards_preserve_provider_order() {
    let (character, mut weekly) = fixture();
    let gm = weekly.vanguard_alerts.grandmaster.as_mut().unwrap();
    let mut variant = gm.activities[0].clone();
    variant.challenges[0].progress = 1;
    variant.challenges[0].complete = true;
    variant.challenges.push(objective("Activities", 0, 2));
    gm.activities.push(variant);
    let pool = &mut weekly.vanguard_alerts.quickplay[0].pools[0];
    let variant = group(
        "Quickplay",
        pool.objectives.clone(),
        &["Ops Bonus Drop", "Ops Bonus Drop", "Main Ingredient"],
        false,
    )
    .activities
    .remove(0);
    let mut unresolved = variant.clone();
    unresolved.rewards.push(PersonalReward {
        hash: 99,
        definition: DefinitionResolution::Unresolved,
    });
    pool.variants = vec![variant, unresolved];
    let rendered = render_weekly(OutputMode::Human, &character, &weekly);
    assert!(
        rendered
            .contains("Exodus Crash\n  ○ Activities 0/1\n  ✓ Activities 1/1\n  ○ Activities 0/2")
    );
    assert!(
        rendered.contains("○ Activities 0/3 → Ops Bonus Drop · Main Ingredient · unresolved #99")
    );
    assert!(!rendered.contains("Hidden modifier"));
    assert!(!rendered.contains("Master"));
}

#[test]
fn weekly_empty_and_unknown_light() {
    let (mut character, mut weekly) = fixture();
    character.light = None;
    weekly.vanguard_alerts = VanguardAlerts::default();
    weekly.featured_raids.clear();
    weekly.featured_dungeons.clear();
    weekly.weekly_rotators = WeeklyRotators::default();
    weekly.unclassified.clear();
    assert_eq!(
        render_weekly(OutputMode::Human, &character, &weekly),
        "Ghost Link — Weekly\nHunter"
    );
}

#[test]
fn weekly_variant_union_and_unresolved_names() {
    let (character, mut weekly) = fixture();
    let raid = &mut weekly.featured_raids[0];
    let mut variant = raid.activities[0].clone();
    variant.hash = 11;
    variant.definition = resolved("Crota's End: Master");
    variant.challenges[0].objective_hash = 21; // Different hash, same description and progress.
    variant.rewards[0].hash = 123; // Exact-name dedupe ignores hashes.
    variant.challenges.push(objective("Bonus", 1, 1));
    variant.phases = Some(PhaseProgress {
        complete: 2,
        total: 4,
    });
    variant.rewards.push(PersonalReward {
        hash: 99,
        definition: DefinitionResolution::Unresolved,
    });
    raid.activities.push(variant);
    weekly.unclassified.push(weekly.unclassified[0].clone());
    let rendered = render_weekly(OutputMode::Human, &character, &weekly);
    assert!(rendered.contains("Crota's End\n  ○ Raid completed: 0/1\n  ✓ Bonus 1/1\n  Encounters  0/4\n  Encounters  2/4\n  Rewards     Raid Gear · Deepsight Weapon (Chance) · Deepsight Weapon · unresolved #99"));
    assert!(!rendered.contains(": Master"));
    assert!(
        rendered
            .ends_with("⚠ 2 activities could not be classified.\n  Run `ghostlink sync-manifest`.")
    );
    let gm = weekly.vanguard_alerts.grandmaster.as_mut().unwrap();
    gm.root_name = "unresolved #10".into();
    gm.activities[0].definition = DefinitionResolution::Unresolved;
    let duplicate = gm.activities[0].rewards[0].clone();
    gm.activities[0].rewards.push(duplicate);
    gm.activities[0].challenges[0].complete = true;
    let rendered = render_weekly(OutputMode::Human, &character, &weekly);
    assert!(rendered.contains("unresolved #10\n  ✓ Activities 0/1\n  Reward      The Slammer"));
    weekly.vanguard_alerts.quickplay[0].pools[0].objectives[0].complete = true;
    assert!(
        render_weekly(OutputMode::Human, &character, &weekly)
            .contains("✓ Activities 0/3 → Ops Bonus Drop · Main Ingredient")
    );
}

#[test]
fn weekly_objectives_with_same_counts_but_different_completion_stay_distinct() {
    let (character, mut weekly) = fixture();
    let gm = weekly.vanguard_alerts.grandmaster.as_mut().unwrap();
    let mut variant = gm.activities[0].clone();
    variant.challenges[0].complete = !variant.challenges[0].complete;
    gm.activities.push(variant);
    let rendered = super::render_weekly(super::OutputMode::Human, &character, &weekly);
    assert!(rendered.contains("  ○ Activities 0/1\n"));
    assert!(rendered.contains("  ✓ Activities 0/1\n"));
}
