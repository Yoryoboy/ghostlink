use destiny_app::weekly_reset::{
    DefinitionResolution as D, WeeklyActivity, WeeklyMilestone, WeeklyModifier, WeeklyReset,
};
use destiny_app::{DefinitionVersion, Timestamp};
use destiny_cli::output::OutputMode;

fn render_weekly_reset(mode: OutputMode, reset: &WeeklyReset) -> String {
    destiny_cli::output::render_weekly_reset(
        mode,
        reset,
        &chrono::FixedOffset::west_opt(3 * 3600).unwrap(),
    )
}

#[test]
fn invalid_dates_are_sanitized_in_both_modes() {
    for invalid in ["2026-13-01", "2026-10-6", "06/10/2026", ""] {
        for json in [false, true] {
            let mut args = vec!["ghostlink", "weekly-reset", "--date", invalid];
            if json {
                args.push("--json");
            }
            let output = destiny_cli::runner::parse_process(args).unwrap_err();
            assert_eq!(output.exit_code, 2);
            assert!(output.stdout.is_empty());
            if json {
                let value: serde_json::Value = serde_json::from_str(&output.stderr).unwrap();
                assert_eq!(
                    value,
                    serde_json::json!({"error":{"code":"invalid_request","message":"invalid arguments: invalid argument syntax; use --help for accepted arguments"}})
                );
            } else {
                assert_eq!(
                    output.stderr,
                    "error: invalid arguments: invalid argument syntax; use --help for accepted arguments\n"
                );
            }
        }
    }
}

#[test]
fn embedded_dated_week_pins_predictions_and_observation_gating() {
    use destiny_app::{
        AppError, PublicMilestone, PublicMilestonesGateway, PublicMilestonesSnapshot,
        WorldDefinitionRepository,
    };
    struct Gateway {
        calls: usize,
    }
    impl PublicMilestonesGateway for Gateway {
        fn load_public_milestones(&mut self) -> Result<PublicMilestonesSnapshot, AppError> {
            self.calls += 1;
            Ok(PublicMilestonesSnapshot {
                milestones: vec![PublicMilestone {
                    milestone_hash: 1,
                    order: 3,
                    start: None,
                    end: None,
                    activities: vec![],
                }],
            })
        }
    }
    let calendar = destiny_adapters::calendar::embedded_calendar().unwrap();
    let date = destiny_adapters::calendar::instant_for_date(
        "2026-10-06",
        &chrono::FixedOffset::west_opt(3 * 3600).unwrap(),
    )
    .unwrap();
    let version = DefinitionVersion::new("v1");
    let mut repository = destiny_app::test_support::InMemoryDefinitionRepository::default();
    repository
        .publish_manifest(
            destiny_app::DefinitionReplacementInput {
                definitions: destiny_app::ManifestDefinitions {
                    version: version.clone(),
                    definitions: vec![destiny_app::ItemDefinition {
                        hash: destiny_app::ItemHash::new(1),
                        bucket_hash: None,
                        name: "Item".into(),
                        item_type: None,
                        tier: None,
                        element: None,
                        slot: None,
                        class_type: None,
                        plug_category: None,
                        tags: vec![],
                        version: version.clone(),
                    }],
                },
                checked_at: date,
                source: destiny_app::SourceMarker::Manifest,
            },
            destiny_app::test_support::synthetic_world(version),
        )
        .unwrap();
    let mut service =
        destiny_app::weekly_reset::WeeklyResetService::new(Gateway { calls: 0 }, repository);
    let prefix = "Weekly reset (manifest v1)\nCalendar (prediction)\nWeek of 2026-10-06 14:00 -03:00 → 2026-10-13 14:00 -03:00 (exclusive end)\n  Featured raids (prediction): Salvation's Edge, Vault of Glass\n  Featured dungeons (prediction): Pit of Heresy, Warlord's Ruin\n  Featured Pantheon (Reprise, Encore) (prediction): Calus, Consecrated Mind\n  Grandmaster Vanguard Alert (prediction): The Arms Dealer";
    let predicted = serde_json::json!({"source":"prediction","start":"2026-10-06T17:00:00Z","end":"2026-10-13T17:00:00Z","rotations":[
        {"key":"raid","title":"Featured raids","status":"predicted","activities":[{"name":"Salvation's Edge","hash":1541433876u32},{"name":"Vault of Glass","hash":3881495763u32}]},
        {"key":"dungeon","title":"Featured dungeons","status":"predicted","activities":[{"name":"Pit of Heresy","hash":2582501063u32},{"name":"Warlord's Ruin","hash":2004855007u32}]},
        {"key":"pantheon","title":"Featured Pantheon (Reprise, Encore)","status":"predicted","activities":[{"name":"Calus","hash":1566552947u32},{"name":"Consecrated Mind","hash":3975235718u32}]},
        {"key":"grandmaster","title":"Grandmaster Vanguard Alert","status":"predicted","activities":[{"name":"The Arms Dealer","hash":1446478334u32}]}
    ]});
    // A different current week must never acquire public milestones.
    let later = destiny_adapters::calendar::instant_for_date(
        "2026-10-13",
        &chrono::FixedOffset::west_opt(3 * 3600).unwrap(),
    )
    .unwrap();
    let other = service.weekly_reset(&calendar, date, later).unwrap();
    assert_eq!(service.gateway().calls, 0);
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &other),
        format!("{prefix}\nLive data (observed) is only available for the current week.")
    );
    let actual: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &other)).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"kind":"weekly_reset","definition_version":"v1","current_week":false,"calendar":predicted,"milestones_source":"observed","milestones":[]})
    );
    let current = service.weekly_reset(&calendar, date, date).unwrap();
    assert_eq!(service.gateway().calls, 1);
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &current),
        format!("{prefix}\nLive milestones (observed)\nMilestone")
    );
    let actual: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &current)).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"kind":"weekly_reset","definition_version":"v1","current_week":true,"calendar":predicted,"milestones_source":"observed","milestones":[{"hash":1,"order":3,"name":"Milestone","description":"","resolved":true,"start":null,"end":null,"activities":[]}]})
    );
    let before = destiny_adapters::calendar::instant_for_date(
        "2026-06-15",
        &chrono::FixedOffset::west_opt(3 * 3600).unwrap(),
    )
    .unwrap();
    let unavailable = service.weekly_reset(&calendar, before, date).unwrap();
    assert_eq!(service.gateway().calls, 1);
    let human = render_weekly_reset(OutputMode::Human, &unavailable);
    let json: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &unavailable)).unwrap();
    for rotation in json["calendar"]["rotations"].as_array().unwrap() {
        assert_eq!(rotation["status"], "no_data");
        assert_eq!(rotation["activities"], serde_json::json!([]));
        let note = rotation["coverage_note"].as_str().unwrap();
        assert!(!note.is_empty());
        assert!(human.contains(&format!("no data — {note}")));
    }
}

#[test]
fn injected_positive_offset_changes_only_the_human_window() {
    let offset = chrono::FixedOffset::east_opt(5 * 3600 + 1800).unwrap();
    let result = reset();
    assert!(
        destiny_cli::output::render_weekly_reset(OutputMode::Human, &result, &offset)
            .contains("Week of 2026-10-06 22:30 +05:30 → 2026-10-13 22:30 +05:30")
    );
    assert_eq!(
        destiny_cli::output::render_weekly_reset(OutputMode::Json, &result, &offset),
        render_weekly_reset(OutputMode::Json, &result)
    );
}

fn reset() -> WeeklyReset {
    WeeklyReset {
        current_week: true,
        calendar: destiny_app::weekly_reset::WeeklyCalendar {
            window: destiny_app::calendar::WeekWindow {
                start: Timestamp::new(1791306000),
                end: Timestamp::new(1791910800),
            },
            rotations: vec![destiny_app::weekly_reset::WeeklyRotation {
                key: "raid".into(),
                title: "Raids".into(),
                outcome: destiny_app::weekly_reset::RotationOutcome::Predicted(vec![
                    destiny_app::calendar::ActivityReference::new("alpha", "Alpha", 42).unwrap(),
                ]),
            }],
        },
        definition_version: DefinitionVersion::new("v1"),
        milestones: vec![
            WeeklyMilestone {
                hash: 1,
                order: 2,
                definition: D::Resolved {
                    name: "Raid\nName".into(),
                    description: "Description".into(),
                },
                start: Some(Timestamp::new(1790701200)),
                end: Some(Timestamp::new(1791306000)),
                activities: vec![
                    WeeklyActivity {
                        hash: 3,
                        definition: D::Resolved {
                            name: "Activity".into(),
                            description: "Ignored".into(),
                        },
                        modifiers: vec![
                            WeeklyModifier::new(
                                4,
                                D::Resolved {
                                    name: "A".into(),
                                    description: "{var:42}".into(),
                                },
                            ),
                            WeeklyModifier::new(5, D::Unresolved),
                        ],
                    },
                    WeeklyActivity {
                        hash: 6,
                        definition: D::Unresolved,
                        modifiers: vec![],
                    },
                ],
            },
            WeeklyMilestone {
                hash: 7,
                order: 3,
                definition: D::Unresolved,
                start: None,
                end: None,
                activities: vec![],
            },
        ],
    }
}

#[test]
fn weekly_reset_text_resolved_unresolved_windows_and_empty_activities() {
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &reset()),
        "Weekly reset (manifest v1)\nCalendar (prediction)\nWeek of 2026-10-06 14:00 -03:00 → 2026-10-13 14:00 -03:00 (exclusive end)\n  Raids (prediction): Alpha\nLive milestones (observed)\nRaid?Name [2026-09-29 17:00Z -> 2026-10-06 17:00Z]\n  - Activity\n    modifiers: A, unresolved #5\n  - unresolved #6\nunresolved #7"
    );
}

#[test]
fn weekly_reset_text_start_only_window() {
    let mut reset = reset();
    reset.milestones.truncate(1);
    reset.milestones[0].end = None;
    reset.milestones[0].activities.clear();
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &reset),
        "Weekly reset (manifest v1)\nCalendar (prediction)\nWeek of 2026-10-06 14:00 -03:00 → 2026-10-13 14:00 -03:00 (exclusive end)\n  Raids (prediction): Alpha\nLive milestones (observed)\nRaid?Name [from 2026-09-29 17:00Z]"
    );
}

#[test]
fn weekly_reset_text_end_only_window() {
    let mut reset = reset();
    reset.milestones.truncate(1);
    reset.milestones[0].start = None;
    reset.milestones[0].activities.clear();
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &reset),
        "Weekly reset (manifest v1)\nCalendar (prediction)\nWeek of 2026-10-06 14:00 -03:00 → 2026-10-13 14:00 -03:00 (exclusive end)\n  Raids (prediction): Alpha\nLive milestones (observed)\nRaid?Name [until 2026-10-06 17:00Z]"
    );
}

#[test]
fn dated_non_current_week_keeps_milestones_array_empty() {
    let mut reset = reset();
    reset.current_week = false;
    reset.milestones.clear();
    assert_eq!(
        render_weekly_reset(OutputMode::Human, &reset),
        "Weekly reset (manifest v1)\nCalendar (prediction)\nWeek of 2026-10-06 14:00 -03:00 → 2026-10-13 14:00 -03:00 (exclusive end)\n  Raids (prediction): Alpha\nLive data (observed) is only available for the current week."
    );
    let actual: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &reset)).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"kind":"weekly_reset","definition_version":"v1","current_week":false,"calendar":{"source":"prediction","start":"2026-10-06T17:00:00Z","end":"2026-10-13T17:00:00Z","rotations":[{"key":"raid","title":"Raids","status":"predicted","activities":[{"name":"Alpha","hash":42}]}]},"milestones_source":"observed","milestones":[]})
    );
}

#[test]
fn weekly_reset_json_shape() {
    let actual: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &reset())).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"kind":"weekly_reset","definition_version":"v1","current_week":true,"calendar":{"source":"prediction","start":"2026-10-06T17:00:00Z","end":"2026-10-13T17:00:00Z","rotations":[{"key":"raid","title":"Raids","status":"predicted","activities":[{"name":"Alpha","hash":42}]}]},"milestones_source":"observed","milestones":[{"hash":1,"order":2,"name":"Raid\nName","description":"Description","resolved":true,"start":"2026-09-29T17:00:00Z","end":"2026-10-06T17:00:00Z","activities":[{"hash":3,"name":"Activity","description":"Ignored","resolved":true,"modifiers":[{"hash":4,"name":"A","description":"{var:42}","resolved":true},{"hash":5,"name":null,"description":null,"resolved":false}]},{"hash":6,"name":null,"description":null,"resolved":false,"modifiers":[]}]},{"hash":7,"order":3,"name":null,"description":null,"resolved":false,"start":null,"end":null,"activities":[]}]})
    );
}
