//! Shared local-build envelopes and stable category exits.
use destiny_app::build::{BuildSource, BuildSummary, DimLoadoutError};
use destiny_app::builds::{BuildError, BuildOmission, BuildReply};
use serde_json::{Value, json};

pub struct BuildOutput {
    pub value: Value,
    pub exit_code: i32,
    /// Export deliberately bypasses the ordinary envelope.
    pub export: Option<String>,
}
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}
fn source(source: BuildSource) -> &'static str {
    match source {
        BuildSource::Import => "import",
        BuildSource::Equipped => "equipped",
        BuildSource::InGameLoadout => "in_game_loadout",
    }
}
fn summary(s: &BuildSummary) -> Value {
    json!({"id": s.id, "name": s.name, "class_type": s.class_type,
        "source": source(s.source), "created_at": s.created_at.seconds(),
        "updated_at": s.updated_at.seconds()})
}
fn omission(o: BuildOmission) -> &'static str {
    match o {
        BuildOmission::InventorySocketIndicesUnavailable => "inventory_socket_indices_unavailable",
        BuildOmission::ArtifactUnlocksUnavailable => "artifact_unlocks_unavailable",
        BuildOmission::SocketCategoriesUnavailable => "socket_categories_unavailable",
        BuildOmission::CraftedDatesUnavailable => "crafted_dates_unavailable",
        BuildOmission::ItemDefinitionsUnavailable => "item_definitions_unavailable",
    }
}
fn location(l: &destiny_app::ItemLocation) -> String {
    match l {
        destiny_app::ItemLocation::Vault => "vault".into(),
        destiny_app::ItemLocation::Character(id) => format!("character:{}", id.as_str()),
        destiny_app::ItemLocation::Postmaster(id) => format!("postmaster:{}", id.as_str()),
        destiny_app::ItemLocation::Other(_) => "other".into(),
    }
}
fn apply_plan(preview: &destiny_app::builds::BuildApplyPreview) -> Value {
    use destiny_app::build_apply::{ApplyPhase, BuildAction};
    let plan = &preview.plan;
    let name = |id: &destiny_app::ItemInstanceId| {
        preview
            .names
            .iter()
            .find(|(i, _)| i == id)
            .map(|(_, n)| n.clone())
    };
    let steps: Vec<_> = plan.steps.iter().map(|step| {
        let phase = match step.phase {
            ApplyPhase::Dequip => "dequip",
            ApplyPhase::Move => "move",
            ApplyPhase::Equip => "equip",
            ApplyPhase::ClearSpace => "clear_space",
        };
        match &step.action {
            BuildAction::Move { item, from, to } => json!({"phase":phase, "action":"move",
                "item":item.as_str(), "name":name(item), "from":location(from), "to":location(to)}),
            BuildAction::PullFromPostmaster { item, character } => json!({"phase":phase,
                "action":"pull_from_postmaster", "item":item.as_str(), "name":name(item),
                "character":character.as_str()}),
            BuildAction::Equip { items, character } => json!({"phase":phase, "action":"equip",
                "items":items.iter().map(|i| json!({"item":i.as_str(), "name":name(i)})).collect::<Vec<_>>(),
                "character":character.as_str()}),
        }
    }).collect();
    json!({"kind":"build_apply_plan", "dry_run":true, "dispatched":0, "summary":summary(&preview.summary),
        "target_character":plan.target.as_str(), "steps":steps,
        "skipped":plan.skipped.iter().map(|s| json!({"spec_index":s.spec_index, "reason":s.reason.as_str()})).collect::<Vec<_>>(),
        "already_there":plan.already_there,
        "clear_skipped":plan.clear_skipped.iter().map(|(i, r)| json!({"item":i.as_str(), "reason":r.as_str()})).collect::<Vec<_>>(),
        "not_applied":plan.not_applied, "unknown":plan.unknown,
        "metadata":{"checked_at":preview.metadata.sync.checked_at.seconds(),
            "revision":preview.metadata.sync.revision.value()}})
}
impl BuildOutput {
    pub fn from_result(result: Result<BuildReply, BuildError>) -> Self {
        match result {
            Ok(BuildReply::Analysis(result)) => {
                let analysis = &result.analysis;
                let items: Vec<_> = analysis.items.iter().map(|i| {
                    let inventory = i.inventory.as_ref().map(|item| {
                        let (location, owner) = match &item.location {
                            destiny_app::ItemLocation::Character(id) => ("character", Some(id.as_str())),
                            destiny_app::ItemLocation::Postmaster(id) => ("postmaster", Some(id.as_str())),
                            destiny_app::ItemLocation::Vault => ("vault", None),
                            destiny_app::ItemLocation::Other(_) => ("other", None),
                        };
                        json!({"instance_id":item.instance_id.as_str(), "hash":item.item_hash.value(),
                            "location":location, "owning_character":owner, "equipped":item.equipped})
                    });
                    json!({"status":if inventory.is_some() {"resolved"} else {"missing"},
                        "spec":{"id":i.spec.instance_id.as_ref().map(|id| id.as_str()), "hash":i.spec.hash.value(),
                            "amount":i.spec.amount, "crafted_date":i.spec.crafted_date,
                            "socket_overrides":i.spec.socket_overrides.iter().map(|(index, hash)| (index.to_string(), json!(hash.value()))).collect::<serde_json::Map<_, _>>()},
                        "should_equip":i.should_equip, "inventory":inventory,
                        "class_compatible":i.class_compatible, "resolution_reason":i.resolution_reason})
                }).collect();
                let findings: Vec<_> = analysis
                    .findings
                    .iter()
                    .map(|f| {
                        json!({"kind":f.kind.as_str(),
                    "evaluation":f.evaluation, "present":f.present, "reason":f.reason,
                    "blocks_optimization":f.kind.blocks_optimization()})
                    })
                    .collect();
                let bonuses = &analysis.equipped_set_bonuses;
                let sets: Vec<_> = bonuses
                    .sets
                    .iter()
                    .map(|set| {
                        let perks = set.perks.as_ref().map(|perks| {
                            perks
                                .iter()
                                .map(|p| {
                                    json!({"sandbox_perk_hash":p.perk.sandbox_perk_hash,
                            "required_set_count":p.perk.required_set_count, "name":p.perk.name,
                            "icon":p.perk.icon, "active":p.active})
                                })
                                .collect::<Vec<_>>()
                        });
                        json!({"hash":set.hash, "name":set.name,
                        "equipped_count":set.equipped_count, "perks":perks})
                    })
                    .collect();
                Self::success(
                    json!({"kind":"build_analysis", "summary":summary(&result.summary),
                    "target_character":analysis.target_character.as_str(), "counts":{"specified":analysis.specified,
                        "resolved":analysis.resolved,"missing":analysis.missing}, "items":items,
                    "findings":findings, "not_evaluated":analysis.not_evaluated,
                    "stats":{"evaluation":analysis.stats.evaluation, "reason":analysis.stats.reason,
                        "values":destiny_app::ARMOR_STATS.iter().enumerate().map(|(i, hash)| json!({"stat_hash":hash,
                            "as_is":analysis.stats.as_is.map(|v| v[i]), "assumed":analysis.stats.assumed.map(|v| v[i])})).collect::<Vec<_>>(),
                        "satisfies_constraints":analysis.stats.satisfies_constraints,
                        "needs_upgrades_for_stats":analysis.stats.needs_upgrades_for_stats},
                    "mod_fit":{"evaluation":analysis.mod_fit.evaluation, "reason":analysis.mod_fit.reason,
                        "invalid_mods":analysis.mod_fit.invalid_mods,
                        "unassigned_mods":analysis.mod_fit.unassigned_mods,
                        "needs_upgrades_for_mods":analysis.mod_fit.needs_upgrades_for_mods,
                        "uses_seasonal_mods":analysis.mod_fit.uses_seasonal_mods,
                        "item_mod_assignments":analysis.mod_fit.item_mod_assignments.as_ref().map(|items| items.iter().map(|(id, mods)| json!({"instance_id":id.as_str(),"mods":mods})).collect::<Vec<_>>()),
                        "upgrade_costs":analysis.mod_fit.upgrade_costs.map(|costs| destiny_app::mod_assignment::RANKED_MATERIALS.iter().zip(costs).filter(|(_, amount)| *amount > 0).map(|(hash, amount)| json!({"material_hash":hash,"amount":amount})).collect::<Vec<_>>())},
                    "equipped_set_bonuses":{"evaluation":bonuses.evaluation,
                        "reasons":bonuses.reasons, "sets":sets},
                    "metadata":{"checked_at":result.metadata.sync.checked_at.seconds(),
                        "revision":result.metadata.sync.revision.value(), "definition_version":result.metadata.sync.definition_version.as_str(),
                        "freshness":match result.metadata.freshness { destiny_app::FreshnessStatus::Fresh => "fresh", destiny_app::FreshnessStatus::Stale => "stale", destiny_app::FreshnessStatus::Missing => "missing", destiny_app::FreshnessStatus::InvalidSource => "invalid_source"}}}),
                )
            }
            Ok(BuildReply::ApplyPlan(preview)) => Self::success(apply_plan(&preview)),
            Ok(BuildReply::List(builds)) => Self::success(
                json!({"kind":"builds", "builds":builds.iter().map(summary).collect::<Vec<_>>()}),
            ),
            Ok(BuildReply::Deleted(deleted)) => {
                Self::success(json!({"kind":"delete_build", "deleted":deleted}))
            }
            Ok(BuildReply::Export(dim)) => Self {
                value: json!({"kind":"build_export", "dim_json":dim}),
                exit_code: 0,
                export: Some(dim),
            },
            Ok(BuildReply::Build {
                result,
                dim_json,
                source_capture,
            }) => {
                let mut omissions = result
                    .omissions
                    .iter()
                    .copied()
                    .map(omission)
                    .collect::<Vec<_>>();
                // Stored DIM JSON keeps overrides, not acquisition diagnostics.
                // Do not manufacture source-specific missing-data claims on show.
                if !source_capture && result.build.summary.source != BuildSource::Import {
                    omissions.push("source_capture_evidence_not_persisted");
                }
                let overrides: Vec<_> = result
                    .build
                    .definition
                    .equipped
                    .iter()
                    .map(|item| {
                        json!({"id": item.instance_id.as_ref().map(|id| id.as_str()),
                        "hash": item.hash.value(),
                        "socket_overrides": item.socket_overrides.iter()
                            .map(|(index, hash)| (index.to_string(), json!(hash.value())))
                            .collect::<serde_json::Map<_, _>>()})
                    })
                    .collect();
                omissions.sort_unstable();
                omissions.dedup();
                let unresolved = if !source_capture
                    && result.build.summary.source == BuildSource::InGameLoadout
                {
                    Value::Null // Discarded source entries were never persisted.
                } else {
                    json!(
                        result
                            .unresolved_items
                            .iter()
                            .map(|i| i.as_str())
                            .collect::<Vec<_>>()
                    )
                };
                Self::success(json!({
                    "kind":"build", "summary":summary(&result.build.summary),
                    "dim_json":dim_json, "omissions":omissions,
                    "equipped_socket_overrides":overrides,
                    "unresolved_items":unresolved,
                }))
            }
            Err(error) => Self::error(error),
        }
    }
    fn success(value: Value) -> Self {
        Self {
            value,
            exit_code: 0,
            export: None,
        }
    }
    fn error(error: BuildError) -> Self {
        use destiny_app::ErrorCategory as E;
        let not_found = matches!(error, BuildError::NotFound);
        let (category, reason, message, candidates) = match error {
            BuildError::App(e) => (e.category(), None, destiny_app::AppError::new(e.category()).to_string(), vec![]),
            BuildError::NotFound => (E::ProviderValidation, None, "Build not found.".into(), vec![]),
            BuildError::AmbiguousName(v) => (E::AmbiguousMatch, None, "Build name is ambiguous; select an ID.".into(), v.iter().map(summary).collect()),
            BuildError::Codec(DimLoadoutError::ShareLinkRequiresOnlineImport) => (E::UnsupportedOperation, Some("share_link_requires_online_import"), "dim.gg share links require online import, deferred to BE-009. Use DIM JSON or a loadouts?loadout= URL.".into(), vec![]),
            BuildError::Codec(e) => (E::ProviderValidation, None, e.to_string(), vec![]),
            BuildError::Loadout(destiny_app::ApplyLoadoutError::App(e)) => (e.category(), None, destiny_app::AppError::new(e.category()).to_string(), vec![]),
            BuildError::Loadout(destiny_app::ApplyLoadoutError::AmbiguousName(v)) => (E::AmbiguousMatch, None, "In-game loadout name is ambiguous; select a slot.".into(), v.iter().map(|c| json!({"slot":c.slot, "name":c.name})).collect()),
            BuildError::Loadout(destiny_app::ApplyLoadoutError::UnknownName(v)) => (E::ProviderValidation, None, "Unknown in-game loadout name; select a slot.".into(), v.iter().map(|c| json!({"slot":c.slot, "name":c.name})).collect()),
            BuildError::Loadout(_) => (E::ProviderValidation, None, "Could not resolve in-game loadout; use an explicit slot (1..12).".into(), vec![]),
            BuildError::SavedLoadoutsUnavailable => (E::StaleState, None, "Saved loadouts unavailable; sync profile first.".into(), vec![]),
        };
        let (code, exit_code) = if not_found {
            ("item_not_found", 32)
        } else {
            match category {
                E::ProviderValidation => ("invalid_request", 2),
                E::AmbiguousMatch => ("ambiguous_match", 31),
                E::UnsupportedOperation => ("unsupported_operation", 40),
                E::StaleState => ("stale_state", 30),
                E::DefinitionsUnavailable => ("definitions_unavailable", 33),
                E::LocalStorage => ("local_storage", 51),
                E::ConfigurationError => ("configuration_error", 52),
                E::AuthorizationRequired => ("authorization_required", 10),
                E::AuthorizationDenied => ("authorization_denied", 10),
                E::AlreadyAuthorizing | E::AlreadyAuthorized | E::CsrfStateMismatch => {
                    ("authorization_state", 10)
                }
                E::ReauthorizationRequired | E::ProviderAuthentication => {
                    ("reauthorization_required", 11)
                }
                E::ProviderRateLimit => ("provider_rate_limit", 20),
                E::ProviderUnavailable | E::CallbackTimeout => ("provider_unavailable", 21),
                E::CredentialStoreUnavailable => ("credential_store_unavailable", 50),
                E::PartialFailure => ("partial_failure", 41),
                E::VerificationFailed => ("verification_failed", 42),
                E::UnknownStatName => ("unknown_stat_name", 2),
                E::UnknownSearchName => ("unknown_search_name", 2),
                E::AmbiguousSearchName => ("ambiguous_search_name", 2),
                E::ClockOverflow | E::PlanIdExhausted => ("internal_error", 70),
            }
        };
        Self {
            value: json!({"error":{"code":code, "message":message, "reason":reason, "candidates":candidates}}),
            exit_code,
            export: None,
        }
    }
    pub fn render_text(&self) -> String {
        if let Some(export) = &self.export {
            return export.clone();
        }
        if let Some(error) = self.value.get("error") {
            return format!(
                "{}\n{}",
                clean(error["message"].as_str().unwrap_or("Build failed.")),
                self.value
            );
        }
        if let Some(builds) = self.value.get("builds").and_then(Value::as_array) {
            return if builds.is_empty() {
                "No saved builds.".into()
            } else {
                builds
                    .iter()
                    .map(|b| {
                        format!(
                            "{}  {} ({})",
                            clean(b["id"].as_str().unwrap_or("")),
                            clean(b["name"].as_str().unwrap_or("")),
                            b["source"].as_str().unwrap_or("")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
        }
        if self.value["kind"] == "build_apply_plan" {
            let v = &self.value;
            let mut lines = vec![format!(
                "Dry run: {} on character {} (nothing was dispatched)",
                clean(v["summary"]["name"].as_str().unwrap_or("")),
                clean(v["target_character"].as_str().unwrap_or(""))
            )];
            for (n, step) in v["steps"].as_array().into_iter().flatten().enumerate() {
                let label = |item: &Value| {
                    format!(
                        "{} ({})",
                        clean(item["name"].as_str().unwrap_or("item")),
                        clean(item["item"].as_str().unwrap_or(""))
                    )
                };
                let text = match step["action"].as_str() {
                    Some("move") => format!(
                        "move {} {} -> {}",
                        label(step),
                        clean(step["from"].as_str().unwrap_or("")),
                        clean(step["to"].as_str().unwrap_or(""))
                    ),
                    Some("pull_from_postmaster") => format!(
                        "pull {} from the Postmaster of {}",
                        label(step),
                        clean(step["character"].as_str().unwrap_or(""))
                    ),
                    _ => format!(
                        "equip {} on {}",
                        step["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(label)
                            .collect::<Vec<_>>()
                            .join(", "),
                        clean(step["character"].as_str().unwrap_or(""))
                    ),
                };
                lines.push(format!(
                    "{}. [{}] {text}",
                    n + 1,
                    step["phase"].as_str().unwrap_or("")
                ));
            }
            for skip in v["skipped"].as_array().into_iter().flatten() {
                lines.push(format!(
                    "Skipped item {}: {}",
                    skip["spec_index"], skip["reason"]
                ));
            }
            for skip in v["clear_skipped"].as_array().into_iter().flatten() {
                lines.push(format!("Not cleared {}: {}", skip["item"], skip["reason"]));
            }
            lines.push(format!("Already in place: {}", v["already_there"]));
            lines.push(format!("Not applied: {}", v["not_applied"]));
            lines.push(format!("Unknown evidence: {}", v["unknown"]));
            return lines.join("\n");
        }
        if self.value["kind"] == "build_analysis" {
            let counts = &self.value["counts"];
            let mut lines = vec![format!(
                "{}: {} specified, {} resolved, {} missing (character {})",
                clean(self.value["summary"]["name"].as_str().unwrap_or("")),
                counts["specified"],
                counts["resolved"],
                counts["missing"],
                clean(self.value["target_character"].as_str().unwrap_or(""))
            )];
            if let Some(items) = self.value["items"].as_array() {
                for item in items {
                    lines.push(format!(
                        "{}: {} {}",
                        item["spec"]["hash"], item["status"], item["inventory"]["location"]
                    ));
                }
            }
            if let Some(findings) = self.value["findings"].as_array() {
                for finding in findings {
                    if finding["present"] == true || finding["evaluation"] == "not_evaluated" {
                        lines.push(format!(
                            "{}: {} ({})",
                            finding["kind"], finding["evaluation"], finding["reason"]
                        ));
                    }
                }
            }
            let stats = &self.value["stats"];
            lines.push(format!(
                "Build stats: {} (reason: {})",
                stats["evaluation"].as_str().unwrap_or("not_evaluated"),
                stats["reason"]
            ));
            if let Some(values) = stats["values"].as_array() {
                for value in values {
                    lines.push(format!(
                        "  Stat {}: as-is {}, assumed {}",
                        value["stat_hash"], value["as_is"], value["assumed"]
                    ));
                }
            }
            let mods = &self.value["mod_fit"];
            lines.push(format!(
                "Mods fit: {} (reason: {})",
                mods["evaluation"].as_str().unwrap_or("not_evaluated"),
                mods["reason"]
            ));
            lines.push(format!("Invalid mods: {}", mods["invalid_mods"]));
            lines.push(format!("Unassigned mods: {}", mods["unassigned_mods"]));
            lines.push(format!(
                "Armor upgrades needed for mods: {}",
                mods["needs_upgrades_for_mods"]
            ));
            lines.push(format!(
                "Seasonal mods required: {}",
                mods["uses_seasonal_mods"]
            ));
            lines.push(format!("Mod upgrade costs: {}", mods["upgrade_costs"]));
            lines.push(format!(
                "Item mod assignments: {}",
                mods["item_mod_assignments"]
            ));
            let bonuses = &self.value["equipped_set_bonuses"];
            lines.push(format!(
                "Equipped set bonuses: {} (reasons: {})",
                bonuses["evaluation"].as_str().unwrap_or("not_evaluated"),
                bonuses["reasons"]
            ));
            if let Some(sets) = bonuses["sets"].as_array() {
                for set in sets {
                    lines.push(format!(
                        "Set {} ({}): {} equipped pieces",
                        set["hash"],
                        clean(set["name"].as_str().unwrap_or("Unknown")),
                        set["equipped_count"]
                    ));
                    if let Some(perks) = set["perks"].as_array() {
                        for perk in perks {
                            lines.push(format!(
                                "  Perk {} ({}), requires {}: {}",
                                perk["sandbox_perk_hash"],
                                clean(perk["name"].as_str().unwrap_or("Unknown")),
                                perk["required_set_count"],
                                match perk["active"].as_bool() {
                                    Some(true) => "active",
                                    Some(false) => "inactive",
                                    None => "unknown",
                                }
                            ));
                        }
                    } else {
                        lines.push("  Perks: unknown (set catalog unavailable)".into());
                    }
                }
            }
            lines.push(format!("Not evaluated: {}", self.value["not_evaluated"]));
            return lines.join("\n");
        }
        if let Some(deleted) = self.value.get("deleted") {
            return format!("Deleted: {deleted}");
        }
        let s = &self.value["summary"];
        format!(
            "{}  {} ({})\nOmissions: {}\nUnresolved items: {}\nDIM JSON: {}",
            clean(s["id"].as_str().unwrap_or("")),
            clean(s["name"].as_str().unwrap_or("")),
            s["source"].as_str().unwrap_or(""),
            self.value["omissions"],
            self.value["unresolved_items"],
            self.value["dim_json"]
        )
    }
}
