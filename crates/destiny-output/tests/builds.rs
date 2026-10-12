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
    assert_eq!(output.value["stats"]["evaluation"], "not_evaluated");
    assert!(output.value["stats"]["values"][0]["as_is"].is_null());
    assert!(output.value["stats"]["needs_upgrades_for_stats"].is_null());
    assert_eq!(output.exit_code, 0); // findings are data, not failed transport
    let mut result = result;
    result
        .analysis
        .findings
        .iter_mut()
        .find(|f| f.kind == build_analysis::FindingKind::EmptyFragmentSlots)
        .unwrap()
        .present = Some(true);
    result.analysis.stats.evaluation = "evaluated";
    result.analysis.stats.reason = None;
    result.analysis.stats.as_is = Some([10; 6]);
    result.analysis.stats.assumed = Some([25; 6]);
    result.analysis.stats.satisfies_constraints = Some(true);
    result.analysis.stats.needs_upgrades_for_stats = Some(true);
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
    assert_eq!(output.value["stats"]["needs_upgrades_for_stats"], true);
    assert_eq!(
        output.value["stats"]["values"][0],
        json!({"stat_hash":2996146975_u32,"as_is":10,"assumed":25})
    );
    assert!(
        output
            .render_text()
            .contains("Stat 2996146975: as-is 10, assumed 25")
    );
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

fn apply_preview() -> destiny_app::builds::BuildApplyPreview {
    use destiny_app::build_apply::*;
    use destiny_app::builds::BuildApplyPreview;
    use destiny_app::*;
    let character = CharacterId::new("1");
    BuildApplyPreview {
        summary: BuildSummary {
            id: "b".into(),
            name: "Raid\u{7}".into(),
            class_type: 1,
            source: BuildSource::Import,
            created_at: Timestamp::new(1),
            updated_at: Timestamp::new(1),
        },
        plan: BuildApplyPlan {
            target: character.clone(),
            steps: vec![
                BuildApplyStep {
                    phase: ApplyPhase::Move,
                    action: BuildAction::PullFromPostmaster {
                        item: ItemInstanceId::new("7"),
                        character: character.clone(),
                    },
                    needs: vec![],
                },
                BuildApplyStep {
                    phase: ApplyPhase::Move,
                    action: BuildAction::Move {
                        item: ItemInstanceId::new("7"),
                        from: ItemLocation::Vault,
                        to: ItemLocation::Character(character.clone()),
                    },
                    needs: vec![0],
                },
                BuildApplyStep {
                    phase: ApplyPhase::Equip,
                    action: BuildAction::Equip {
                        items: vec![ItemInstanceId::new("7")],
                        character,
                    },
                    needs: vec![],
                },
            ],
            skipped: vec![BuildApplySkip {
                spec_index: 2,
                reason: ApplySkipReason::NoSpace,
            }],
            already_there: vec![0],
            clear_skipped: vec![(ItemInstanceId::new("9"), ApplySkipReason::NoSpace)],
            socket_skipped: vec![],
            socket_already_there: vec![],
            mod_skipped: vec![],
            mods_in_place: 0,
            not_applied: vec!["armor_mods_deferred_to_be_007b2"],
            unknown: vec!["vault_capacity"],
        },
        names: vec![(ItemInstanceId::new("7"), "Hawkmoon".into())],
        plug_names: vec![],
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
    }
}

#[test]
fn apply_plan_renders_typed_steps_in_json_and_text() {
    let preview = apply_preview();
    let output = BuildOutput::from_result(Ok(BuildReply::ApplyPlan(Box::new(preview))));
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.value["kind"], "build_apply_plan");
    assert_eq!(output.value["dry_run"], true);
    assert_eq!(output.value["dispatched"], 0);
    assert_eq!(output.value["steps"][0]["action"], "pull_from_postmaster");
    assert_eq!(output.value["steps"][1]["to"], "character:1");
    assert_eq!(output.value["steps"][2]["items"][0]["name"], "Hawkmoon");
    assert_eq!(
        output.value["skipped"],
        json!([{"spec_index":2,"reason":"no_space"}])
    );
    assert_eq!(output.value["unknown"], json!(["vault_capacity"]));
    assert_eq!(
        output.render_text(),
        "Dry run: Raid on character 1 (nothing was dispatched)\n\
1. [move] pull Hawkmoon (7) from the Postmaster of 1\n\
2. [move] move Hawkmoon (7) vault -> character:1\n\
3. [equip] equip Hawkmoon (7) on 1\n\
Skipped item 2: \"no_space\"\n\
Not cleared \"9\": \"no_space\"\n\
Already in place: [0]\n\
Sockets already in place: 0\n\
Mods already in place: 0\n\
Not applied: [\"armor_mods_deferred_to_be_007b2\"]\n\
Unknown evidence: [\"vault_capacity\"]"
    );
}

fn execution(
    preview: destiny_app::builds::BuildApplyPreview,
    outcomes: Vec<Vec<destiny_app::build_apply_exec::ItemStatus>>,
    status: destiny_app::build_apply_exec::ApplyStatus,
    local_state: destiny_app::build_apply_exec::ApplyLocalState,
) -> BuildReply {
    use destiny_app::build_apply_exec::*;
    let steps = preview
        .plan
        .steps
        .iter()
        .zip(outcomes)
        .enumerate()
        .map(|(index, (step, statuses))| {
            let items = match &step.action {
                destiny_app::build_apply::BuildAction::Move { item, .. }
                | destiny_app::build_apply::BuildAction::InsertPlug { item, .. }
                | destiny_app::build_apply::BuildAction::PullFromPostmaster { item, .. } => {
                    vec![item.clone()]
                }
                destiny_app::build_apply::BuildAction::Equip { items, .. } => items.clone(),
            };
            StepReport {
                index,
                step: step.clone(),
                outcomes: items
                    .into_iter()
                    .zip(statuses)
                    .map(|(item, status)| ItemOutcome { item, status })
                    .collect(),
            }
        })
        .collect();
    BuildReply::Applied(Box::new(BuildApplyExecution {
        preview,
        execution: PlanExecution {
            steps,
            status,
            local_state,
            dispatched: 1,
        },
    }))
}

#[test]
fn apply_execution_renders_typed_outcomes_text_and_exit_code() {
    use destiny_app::build_apply_exec::*;
    let reply = execution(
        apply_preview(),
        vec![
            vec![ItemStatus::Succeeded],
            vec![ItemStatus::Failed(StepFailure::Rejected(
                "not_transferable",
            ))],
            vec![ItemStatus::Skipped(SkipCause::DependencyFailed(1))],
        ],
        ApplyStatus::Partial,
        ApplyLocalState::RefreshPending,
    );
    let output = BuildOutput::from_result(Ok(reply));
    assert_eq!(output.exit_code, 41);
    let v = &output.value;
    assert_eq!(v["kind"], "build_apply");
    assert_eq!(v["dry_run"], false);
    assert_eq!(v["result"], "partial");
    assert_eq!(v["local_state"], "refresh_pending");
    assert_eq!(v["dispatched"], 1);
    assert_eq!(v["steps"][0]["outcomes"][0]["status"], "succeeded");
    assert_eq!(
        v["steps"][1]["outcomes"][0],
        json!({"item":"7","name":"Hawkmoon","status":"failed","failure":"rejected",
            "reason":"not_transferable","message":destiny_output::rejection_message("not_transferable")})
    );
    assert_eq!(
        v["steps"][2]["outcomes"][0],
        json!({"item":"7","name":"Hawkmoon","status":"skipped","cause":"dependency_failed","depends_on_step":2})
    );
    assert_eq!(v["steps"][1]["needs"], json!([1]));
    assert_eq!(v["counts"], json!({"succeeded":1,"failed":1,"skipped":1}));
    assert_eq!(
        v["not_applied"],
        json!([{"outcome":"not_applied","reason":"armor_mods_deferred_to_be_007b2"}])
    );
    assert_eq!(v["skipped"], json!([{"spec_index":2,"reason":"no_space"}]));
    assert_eq!(
        output.render_text(),
        "Applied Raid on character 1: partial (1 succeeded, 1 failed, 1 skipped)\n\
Bungie acknowledgements are not re-read; do not blindly retry.\n\
1. [move] succeeded: pull Hawkmoon (7) from the Postmaster of 1\n\
2. [move] failed (not_transferable): move Hawkmoon (7) vault -> character:1\n\
3. [equip] skipped (step 2 did not succeed): equip Hawkmoon (7) on 1\n\
Skipped item 2: \"no_space\"\n\
Not cleared \"9\": \"no_space\"\n\
Not applied: armor_mods_deferred_to_be_007b2\n\
Local state: refresh pending"
    );
}

#[test]
fn apply_execution_exit_codes_reuse_existing_action_exits() {
    use destiny_app::ErrorCategory as E;
    use destiny_app::build_apply_exec::*;
    let ok = || vec![vec![ItemStatus::Succeeded]; 3];
    let exit = |preview, outcomes, status, local| {
        BuildOutput::from_result(Ok(execution(preview, outcomes, status, local))).exit_code
    };
    let mut clean = apply_preview();
    clean.plan.skipped.clear();
    clean.plan.clear_skipped.clear();
    // Every step applied and nothing left unplanned.
    assert_eq!(
        exit(
            clean.clone(),
            ok(),
            ApplyStatus::Applied,
            ApplyLocalState::Updated
        ),
        0
    );
    // Unplanned build items mean the build is not fully applied.
    assert_eq!(
        exit(
            apply_preview(),
            ok(),
            ApplyStatus::Applied,
            ApplyLocalState::Updated
        ),
        41
    );
    // Nothing to send is not a failure.
    let mut empty = clean.clone();
    empty.plan.steps.clear();
    assert_eq!(
        exit(empty, vec![], ApplyStatus::NoOp, ApplyLocalState::Unchanged),
        0
    );
    let failed = || vec![vec![ItemStatus::Failed(StepFailure::Uncertain)]; 3];
    assert_eq!(
        exit(
            clean.clone(),
            failed(),
            ApplyStatus::Failed,
            ApplyLocalState::RefreshPending
        ),
        41
    );
    // Persistence failure keeps the existing verification exit.
    assert_eq!(
        exit(
            clean.clone(),
            ok(),
            ApplyStatus::Applied,
            ApplyLocalState::Failed
        ),
        42
    );
    // Nothing reached Bungie: category exits (rate limit 20, stale state 30).
    let refused = |category| {
        let outcome = ItemStatus::Failed(StepFailure::NotDispatched(category));
        exit(
            clean.clone(),
            vec![vec![outcome]; 3],
            ApplyStatus::Refused(category),
            ApplyLocalState::Unchanged,
        )
    };
    assert_eq!(refused(E::ProviderRateLimit), 20);
    assert_eq!(refused(E::StaleState), 30);
    assert_eq!(refused(E::ReauthorizationRequired), 11);
}

fn with_plug_step() -> destiny_app::builds::BuildApplyPreview {
    use destiny_app::build_apply::*;
    use destiny_app::*;
    let mut preview = apply_preview();
    preview.plan.steps.push(BuildApplyStep {
        phase: ApplyPhase::Sockets,
        action: BuildAction::InsertPlug {
            item: ItemInstanceId::new("7"),
            character: CharacterId::new("1"),
            socket_index: 3,
            plug: ItemHash::new(555),
        },
        needs: vec![2],
    });
    preview.plug_names = vec![(ItemHash::new(555), "Fusion\u{7} Rig".into())];
    preview.plan.socket_skipped = vec![destiny_app::build_sockets::SocketSkip {
        spec_index: 1,
        socket_index: Some(4),
        reason: destiny_app::build_sockets::SocketSkipReason::NotFree,
    }];
    preview.plan.socket_already_there = vec![(1, 2)];
    preview
}

#[test]
fn plug_steps_and_socket_skips_render_in_the_plan_and_the_result() {
    use destiny_app::build_apply_exec::*;
    let plan = BuildOutput::from_result(Ok(BuildReply::ApplyPlan(Box::new(with_plug_step()))));
    let step = &plan.value["steps"][3];
    assert_eq!(
        step,
        &json!({"phase":"sockets","action":"insert_plug","item":"7","name":"Hawkmoon",
            "character":"1","socket_index":3,"plug":555,"plug_name":"Fusion\u{7} Rig"})
    );
    assert_eq!(
        plan.value["socket_skipped"],
        json!([{"spec_index":1,"socket_index":4,"reason":"not_free"}])
    );
    assert_eq!(
        plan.value["socket_already_there"],
        json!([{"spec_index":1,"socket_index":2}])
    );
    let text = plan.render_text();
    assert!(text.contains("4. [sockets] insert Fusion Rig (555) into socket 3 of Hawkmoon (7)"));
    assert!(text.contains("Skipped socket 4 of item 1: \"not_free\""));
    assert!(text.contains("Sockets already in place: 1"));
    let applied = execution(
        with_plug_step(),
        vec![vec![ItemStatus::Succeeded]; 4],
        ApplyStatus::Applied,
        ApplyLocalState::RefreshPending,
    );
    let output = BuildOutput::from_result(Ok(applied));
    assert_eq!(
        output.value["steps"][3]["outcomes"][0]["status"],
        "succeeded"
    );
    assert_eq!(output.value["steps"][3]["needs"], json!([3]));
    assert!(
        output
            .render_text()
            .contains("4. [sockets] succeeded: insert Fusion Rig (555) into socket 3")
    );
}

#[test]
fn mod_steps_skips_and_counts_render_in_the_plan_and_the_result() {
    use destiny_app::build_apply::*;
    use destiny_app::build_apply_exec::*;
    use destiny_app::*;
    let mut preview = with_plug_step();
    preview.plan.steps[3].phase = ApplyPhase::Mods;
    preview.plan.mod_skipped = vec![destiny_app::build_mod_apply::ModSkip {
        plug: ItemHash::new(2001),
        reason: "mod_not_unlocked",
    }];
    preview.plan.mods_in_place = 2;
    let plan = BuildOutput::from_result(Ok(BuildReply::ApplyPlan(Box::new(preview.clone()))));
    assert_eq!(plan.value["steps"][3]["phase"], "mods");
    assert_eq!(
        plan.value["mod_skipped"],
        json!([{"mod":2001,"reason":"mod_not_unlocked"}])
    );
    assert_eq!(plan.value["mods_in_place"], 2);
    let text = plan.render_text();
    assert!(text.contains("4. [mods] insert Fusion Rig (555) into socket 3 of Hawkmoon (7)"));
    assert!(text.contains("Skipped mod 2001: \"mod_not_unlocked\""));
    assert!(text.contains("Mods already in place: 2"));
    let applied = execution(
        preview,
        vec![vec![ItemStatus::Succeeded]; 4],
        ApplyStatus::Applied,
        ApplyLocalState::RefreshPending,
    );
    let output = BuildOutput::from_result(Ok(applied));
    assert_eq!(output.value["mod_skipped"][0]["reason"], "mod_not_unlocked");
    assert_eq!(output.value["mods_in_place"], 2);
    assert!(
        output
            .render_text()
            .contains("Skipped mod 2001: \"mod_not_unlocked\"")
    );
}

#[test]
fn build_analysis_reports_optimizer_evidence_additively() {
    use destiny_app::*;
    let result = |optimizer: optimizer::OptimizerAnalysis| {
        let mut analysis = build_analysis::analyze_build(
            &build::BuildDefinition::default(),
            &CharacterId::new("1"),
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        analysis.optimizer = optimizer;
        BuildOutput::from_result(Ok(BuildReply::Analysis(Box::new(
            builds::BuildCheckResult {
                summary: BuildSummary {
                    id: "id".into(),
                    name: "Raid".into(),
                    class_type: 1,
                    source: BuildSource::Import,
                    created_at: Timestamp::new(100),
                    updated_at: Timestamp::new(100),
                },
                analysis,
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
            },
        ))))
    };
    // Default: the optimizer never ran.
    let output = result(Default::default());
    assert_eq!(
        output.value["optimizer"],
        json!({"evaluation":"not_evaluated", "reason":"optimizer_not_run", "candidates_per_bucket":null,
            "combinations_total":null, "combinations_visited":null, "truncated":false, "threads":null, "font_note":false})
    );
    assert!(
        output
            .render_text()
            .contains("Optimizer: not_evaluated (reason: \"optimizer_not_run\"")
    );
    assert!(!output.render_text().contains("Font of"));
    // An evaluated run reports its counts, and the Font note appears only when set.
    let output = result(optimizer::OptimizerAnalysis {
        evaluation: "evaluated",
        reason: None,
        candidates: Some([3, 2, 1, 4, 5]),
        combos_total: Some(120),
        combos_visited: Some(7),
        truncated: false,
        threads: Some(8),
        font_note: true,
    });
    assert_eq!(
        output.value["optimizer"]["candidates_per_bucket"],
        json!([3, 2, 1, 4, 5])
    );
    assert_eq!(output.value["optimizer"]["combinations_visited"], 7);
    assert_eq!(output.value["optimizer"]["font_note"], true);
    let text = output.render_text();
    assert!(text.contains("Optimizer: evaluated"));
    assert!(text.contains("combinations visited: 7 of 120"));
    assert!(text.contains("Font of ... mods push a stat past the cap"));
    assert_eq!(output.exit_code, 0);
}
