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
impl BuildOutput {
    pub fn from_result(result: Result<BuildReply, BuildError>) -> Self {
        match result {
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
                // Known source limitations remain visible on subsequent show.
                if result.build.summary.source != BuildSource::Import {
                    omissions
                        .extend(["socket_categories_unavailable", "crafted_dates_unavailable"]);
                    if result.build.summary.source == BuildSource::Equipped {
                        omissions.extend([
                            "inventory_socket_indices_unavailable",
                            "artifact_unlocks_unavailable",
                        ]);
                    }
                }
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
