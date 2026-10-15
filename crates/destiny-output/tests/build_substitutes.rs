use destiny_app::build::{BuildSource, BuildSummary};
use destiny_app::build_substitutes::*;
use destiny_app::builds::{BuildReply, SubstitutesResult};
use destiny_app::*;
use destiny_output::builds::BuildOutput;
use serde_json::json;

fn piece(id: &str, name: &str, in_build: bool) -> SubstitutePiece {
    SubstitutePiece {
        instance_id: id.into(),
        hash: 7,
        name: name.into(),
        bucket_hash: 3448274439,
        in_build,
    }
}
fn result(report: SubstitutesReport) -> BuildOutput {
    BuildOutput::from_result(Ok(BuildReply::Substitutes(Box::new(SubstitutesResult {
        summary: BuildSummary {
            id: "id".into(),
            name: "Raid".into(),
            class_type: 1,
            source: BuildSource::Import,
            created_at: Timestamp::new(100),
            updated_at: Timestamp::new(100),
        },
        target_character: CharacterId::new("1"),
        report,
        metadata: QueryMetadata {
            sync: SyncMetadata {
                dataset: Dataset::ProfileInventory,
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
                revision: SyncRevision::new(1),
                definition_version: DefinitionVersion::new("v1"),
            },
            freshness: FreshnessStatus::Fresh,
        },
    }))))
}
fn evaluated(truncated: bool) -> SubstitutesReport {
    SubstitutesReport {
        evaluation: "evaluated",
        reason: None,
        limit: 10,
        sets: vec![SubstituteArmorSet {
            pieces: vec![
                piece("11", "Helm\u{7}et", false),
                piece("2", "Gauntlets", true),
                piece("3", "Chest", true),
                piece("4", "Legs", true),
                piece("5", "Class", true),
            ],
            stats: vec![
                SubstituteStat {
                    stat_hash: 2996146975,
                    name: Some("Weapons".into()),
                    total: 110,
                    armor_only: 60,
                },
                SubstituteStat {
                    stat_hash: 392767087,
                    name: None,
                    total: 40,
                    armor_only: 40,
                },
            ],
            stat_mods: vec![
                SubstituteMod {
                    hash: 5001,
                    name: Some("Weapons Mod".into()),
                },
                SubstituteMod {
                    hash: 5001,
                    name: Some("Weapons Mod".into()),
                },
            ],
            tuning_mods: vec![SubstituteMod {
                hash: 309000506,
                name: None,
            }],
            power: 1900,
        }],
        optimizer: optimizer::OptimizerAnalysis {
            evaluation: "evaluated",
            reason: None,
            candidates: Some([2, 1, 1, 1, 1]),
            combos_total: Some(2),
            combos_visited: Some(if truncated { 1 } else { 2 }),
            truncated,
            threads: Some(8),
            font_note: false,
        },
    }
}

#[test]
fn substitutes_json_carries_sets_stats_mods_and_the_budget_outcome() {
    let output = result(evaluated(true));
    assert_eq!(output.exit_code, 0);
    let v = &output.value;
    assert_eq!(v["kind"], "build_substitutes");
    assert_eq!(v["summary"]["name"], "Raid");
    assert_eq!(v["target_character"], "1");
    assert_eq!(
        (v["evaluation"].as_str(), v["limit"].as_u64()),
        (Some("evaluated"), Some(10))
    );
    assert_eq!(v["reason"], json!(null));
    assert_eq!(
        v["sets"][0]["pieces"][0],
        json!({"instance_id":"11", "hash":7, "name":"Helm\u{7}et", "bucket_hash":3448274439i64, "in_build":false})
    );
    assert_eq!(v["sets"][0]["rank"], 1);
    assert_eq!(v["sets"][0]["power"], 1900);
    assert_eq!(
        v["sets"][0]["stats"][0],
        json!({"stat_hash":2996146975u32, "name":"Weapons", "total":110, "armor_only":60})
    );
    assert_eq!(v["sets"][0]["stats"][1]["name"], json!(null));
    assert_eq!(
        v["sets"][0]["stat_mods"][1],
        json!({"hash":5001, "name":"Weapons Mod"})
    );
    assert_eq!(
        v["sets"][0]["tuning_mods"],
        json!([{"hash":309000506, "name":null}])
    );
    assert_eq!(
        v["optimizer"],
        json!({"evaluation":"evaluated", "reason":null, "candidates_per_bucket":[2,1,1,1,1],
            "combinations_total":2, "combinations_visited":1, "truncated":true, "threads":8})
    );
    assert_eq!(v["metadata"]["freshness"], "fresh");
}

#[test]
fn substitutes_text_lists_ranked_sets_and_says_when_the_search_was_cut_short() {
    let text = result(evaluated(true)).render_text();
    assert!(
        text.starts_with("Substitutes for Raid on character 1: 1 set (limit 10)"),
        "{text}"
    );
    assert!(text.contains("combinations visited: 1 of 2"));
    assert!(text.contains("Truncated: the time budget ran out"));
    assert!(text.contains("#1  power 1900"));
    assert!(
        text.contains("Helmet: Helmet (11) [not in build]"),
        "{text}"
    );
    assert!(text.contains("Gauntlets: Gauntlets (2)\n"));
    assert!(text.contains("Weapons 110 (armor 60)"));
    assert!(text.contains("Stat mods: Weapons Mod x2"));
    assert!(text.contains("Tuning mods: 309000506"));
    assert!(!text.contains('\u{7}'));
    assert!(!result(evaluated(false)).render_text().contains("Truncated"));
}

#[test]
fn substitutes_that_could_not_run_say_why_in_both_formats() {
    let output = result(SubstitutesReport {
        evaluation: "not_evaluated",
        reason: Some("analysis_blocked_by_finding"),
        limit: 10,
        sets: vec![],
        optimizer: optimizer::OptimizerAnalysis::not_evaluated("analysis_blocked_by_finding"),
    });
    assert_eq!(output.value["evaluation"], "not_evaluated");
    assert_eq!(output.value["reason"], "analysis_blocked_by_finding");
    assert_eq!(output.value["sets"], json!([]));
    assert!(
        output
            .render_text()
            .contains("Substitutes not evaluated: analysis_blocked_by_finding")
    );
    let empty = result(SubstitutesReport {
        sets: vec![],
        ..evaluated(false)
    });
    assert!(
        empty
            .render_text()
            .contains("No armor sets match the saved stat ranges.")
    );
}
