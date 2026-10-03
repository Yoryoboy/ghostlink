use destiny_app::weekly_reset::{
    DefinitionResolution as D, WeeklyActivity, WeeklyMilestone, WeeklyModifier, WeeklyReset,
};
use destiny_app::{DefinitionVersion, Timestamp};
use destiny_cli::output::{OutputMode, render_weekly_reset};

fn reset() -> WeeklyReset {
    WeeklyReset {
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
        "Weekly reset (manifest v1)\nRaid?Name [2026-09-29 17:00Z -> 2026-10-06 17:00Z]\n  - Activity\n    modifiers: A, unresolved #5\n  - unresolved #6\nunresolved #7"
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
        "Weekly reset (manifest v1)\nRaid?Name [from 2026-09-29 17:00Z]"
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
        "Weekly reset (manifest v1)\nRaid?Name [until 2026-10-06 17:00Z]"
    );
}

#[test]
fn weekly_reset_json_shape() {
    let actual: serde_json::Value =
        serde_json::from_str(&render_weekly_reset(OutputMode::Json, &reset())).unwrap();
    assert_eq!(
        actual,
        serde_json::json!({"kind":"weekly_reset","definition_version":"v1","milestones":[{"hash":1,"order":2,"name":"Raid\nName","description":"Description","resolved":true,"start":"2026-09-29T17:00:00Z","end":"2026-10-06T17:00:00Z","activities":[{"hash":3,"name":"Activity","description":"Ignored","resolved":true,"modifiers":[{"hash":4,"name":"A","description":"{var:42}","resolved":true},{"hash":5,"name":null,"description":null,"resolved":false}]},{"hash":6,"name":null,"description":null,"resolved":false,"modifiers":[]}]},{"hash":7,"order":3,"name":null,"description":null,"resolved":false,"start":null,"end":null,"activities":[]}]})
    );
}
