use destiny_app::Timestamp;
use destiny_app::build::{BuildSource, BuildSummary, DimLoadoutError};
use destiny_app::builds::{BuildError, BuildReply};
use destiny_output::builds::BuildOutput;
use serde_json::json;

#[test]
fn build_list_and_delete_snapshots() {
    let output = BuildOutput::from_result(Ok(BuildReply::List(vec![BuildSummary {
        id: "id".into(),
        name: "Raid".into(),
        class_type: 1,
        source: BuildSource::Import,
        created_at: Timestamp::new(100),
        updated_at: Timestamp::new(100),
    }])));
    assert_eq!(
        output.value,
        json!({"kind":"builds", "builds":[{"id":"id", "name":"Raid", "class_type":1, "source":"import", "created_at":100, "updated_at":100}]})
    );
    assert_eq!(output.render_text(), "id  Raid (import)");
    let empty = BuildOutput::from_result(Ok(BuildReply::List(vec![])));
    assert_eq!(empty.render_text(), "No saved builds.");
    let deleted = BuildOutput::from_result(Ok(BuildReply::Deleted(false)));
    assert_eq!(
        deleted.value,
        json!({"kind":"delete_build", "deleted":false})
    );
    assert_eq!(deleted.render_text(), "Deleted: false");
}

#[test]
fn build_analysis_preserves_specs_counts_coverage_and_human_text() {
    use destiny_app::*;
    let build = build::BuildDefinition {
        id: "fixture".into(),
        name: "Raid".into(),
        class_type: 1,
        equipped: vec![build::BuildItem {
            instance_id: Some(ItemInstanceId::new("404")),
            hash: ItemHash::new(10),
            amount: Some(1),
            socket_overrides: vec![],
            crafted_date: Some(123),
        }],
        ..Default::default()
    };
    let result = builds::BuildCheckResult {
        summary: BuildSummary {
            id: build.id.clone(),
            name: build.name.clone(),
            class_type: 1,
            source: BuildSource::Import,
            created_at: Timestamp::new(100),
            updated_at: Timestamp::new(100),
        },
        analysis: build_analysis::analyze_build(
            &build,
            &CharacterId::new("1"),
            &[],
            &[],
            &[],
            &[],
            &[],
        ),
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
    };
    let output = BuildOutput::from_result(Ok(BuildReply::Analysis(Box::new(result.clone()))));
    assert_eq!(
        output.value["counts"],
        json!({"specified":1,"resolved":0,"missing":1})
    );
    assert_eq!(output.value["items"][0]["spec"]["id"], "404");
    assert_eq!(output.value["items"][0]["spec"]["crafted_date"], 123);
    assert_eq!(output.value["findings"].as_array().unwrap().len(), 13);
    let fragments = output.value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "empty_fragment_slots")
        .unwrap();
    assert_eq!(fragments["evaluation"], "evaluated");
    assert_eq!(fragments["present"], false);
    assert!(!output.render_text().contains("empty_fragment_slots"));
    assert_eq!(
        output.value["equipped_set_bonuses"],
        json!({"evaluation":"not_evaluated", "reasons":["set_activation_not_assembled"], "sets":[]})
    );
    assert!(
        output
            .render_text()
            .contains("Equipped set bonuses: not_evaluated")
    );
    assert!(
        output
            .render_text()
            .contains("1 specified, 0 resolved, 1 missing")
    );
    assert!(output.render_text().contains("not_evaluated"));
    assert_eq!(output.value["mod_fit"]["evaluation"], "not_evaluated");
    assert!(output.value["mod_fit"]["unassigned_mods"].is_null());
    assert!(output.render_text().contains("Mods fit: not_evaluated"));
    assert_eq!(output.exit_code, 0); // findings are data, not failed transport
    let mut result = result;
    result
        .analysis
        .findings
        .iter_mut()
        .find(|f| f.kind == build_analysis::FindingKind::EmptyFragmentSlots)
        .unwrap()
        .present = Some(true);
    result.analysis.mod_fit = destiny_app::mod_assignment::ModFitAnalysis {
        evaluation: "evaluated",
        reason: None,
        invalid_mods: Some(vec![40]),
        unassigned_mods: Some(vec![42, 42]),
        needs_upgrades_for_mods: Some(false),
        uses_seasonal_mods: Some(true),
        item_mod_assignments: Some(vec![]),
        upgrade_costs: Some([0, 1, 0, 0, 100]),
    };
    result.analysis.equipped_set_bonuses = build_sets::EquippedSetBonuses {
        evaluation: "evaluated",
        reasons: vec![],
        sets: vec![build_sets::EquippedSetBonus {
            hash: 42,
            name: Some("Set\nname".into()),
            equipped_count: 2,
            perks: Some(vec![build_sets::EquippedSetPerk {
                perk: armor_catalog::SetPerk {
                    required_set_count: 2,
                    sandbox_perk_hash: 100,
                    name: Some("Perk\nname".into()),
                    icon: None,
                },
                active: Some(true),
            }]),
        }],
    };
    let output = BuildOutput::from_result(Ok(BuildReply::Analysis(Box::new(result))));
    assert!(output.render_text().contains("empty_fragment_slots"));
    assert_eq!(output.value["mod_fit"]["unassigned_mods"], json!([42, 42]));
    assert_eq!(output.value["mod_fit"]["needs_upgrades_for_mods"], false);
    assert_eq!(output.value["mod_fit"]["uses_seasonal_mods"], true);
    assert_eq!(output.value["mod_fit"]["item_mod_assignments"], json!([]));
    assert_eq!(
        output.value["mod_fit"]["upgrade_costs"],
        json!([
            {"material_hash":4257549985_u32,"amount":1},
            {"material_hash":3159615086_u32,"amount":100}
        ])
    );
    assert!(
        output
            .render_text()
            .contains("Seasonal mods required: true")
    );
    assert!(output.render_text().contains("Mod upgrade costs:"));
    assert!(output.render_text().contains("Unassigned mods: [42,42]"));
    assert_eq!(
        output.value["equipped_set_bonuses"]["sets"][0]["perks"][0]["active"],
        true
    );
    assert!(
        output
            .render_text()
            .contains("Set 42 (Setname): 2 equipped pieces")
    );
    assert!(
        output
            .render_text()
            .contains("Perk 100 (Perkname), requires 2: active")
    );
}

#[test]
fn captured_socket_overrides_are_exposed_without_fabricated_omissions() {
    use destiny_app::{build::*, builds::*, *};
    let build = StoredBuild {
        definition: BuildDefinition {
            equipped: vec![BuildItem {
                instance_id: Some(ItemInstanceId::new("1")),
                hash: ItemHash::new(10),
                amount: Some(1),
                socket_overrides: vec![(7, ItemHash::new(20))],
                crafted_date: None,
            }],
            ..Default::default()
        },
        summary: BuildSummary {
            id: "id".into(),
            name: "Raid".into(),
            class_type: 1,
            source: BuildSource::Equipped,
            created_at: Timestamp::new(100),
            updated_at: Timestamp::new(100),
        },
    };
    for source_capture in [true, false] {
        let output = BuildOutput::from_result(Ok(BuildReply::Build {
            result: Box::new(BuildSourceResult {
                build: build.clone(),
                unresolved_items: vec![],
                omissions: vec![],
            }),
            dim_json: "{}".into(),
            source_capture,
        }));
        assert_eq!(
            output.value["equipped_socket_overrides"][0]["socket_overrides"],
            json!({"7":20})
        );
        assert_eq!(
            output.value["omissions"],
            if source_capture {
                json!([])
            } else {
                json!(["source_capture_evidence_not_persisted"])
            }
        );
    }
}

#[test]
fn export_and_typed_offline_error() {
    let export = BuildOutput::from_result(Ok(BuildReply::Export("{\"name\":\"Raid\"}".into())));
    assert_eq!(export.render_text(), "{\"name\":\"Raid\"}");
    let share = BuildOutput::from_result(Err(BuildError::Codec(
        DimLoadoutError::ShareLinkRequiresOnlineImport,
    )));
    assert_eq!(share.exit_code, 40);
    assert_eq!(
        share.value["error"]["reason"],
        "share_link_requires_online_import"
    );
    assert!(share.render_text().contains("BE-009"));
    let missing = BuildOutput::from_result(Err(BuildError::NotFound));
    assert_eq!(missing.exit_code, 32);
    assert_eq!(missing.value["error"]["code"], "item_not_found");
}
