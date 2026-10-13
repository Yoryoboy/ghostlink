//! Shared sanitized CLI/MCP projection of one saved-loadout attempt.
use destiny_app::{
    ApplyLoadoutError, ApplyLoadoutResult, LoadoutActionOutcome, LoadoutActionRejection,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct ApplyLoadoutOutput {
    pub value: Value,
    pub exit_code: i32,
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
fn local_issue(issue: &destiny_app::LoadoutLocalIssue) -> Value {
    json!({"item": {
        "instance_id": clean(issue.item.instance_id.as_str()),
        "item_hash": issue.item.item_hash.map(|hash| hash.value()),
        "bucket": issue.item.bucket.as_deref().map(clean),
        "name": issue.item.name.as_deref().map(clean),
    }, "reason": issue.reason.code()})
}
impl ApplyLoadoutOutput {
    pub fn from_result(result: Result<ApplyLoadoutResult, ApplyLoadoutError>) -> Self {
        match result {
            Ok(result) => {
                let (receipt, reason) = match result.receipt {
                    LoadoutActionOutcome::Applied => ("applied", None),
                    LoadoutActionOutcome::AuthenticationRefused => (
                        "authentication_refused",
                        Some("provider authentication failed"),
                    ),
                    LoadoutActionOutcome::Transient => (
                        "transient",
                        Some("provider temporarily unavailable; action did not apply"),
                    ),
                    LoadoutActionOutcome::Uncertain => (
                        "uncertain",
                        Some("provider outcome is uncertain; do not blindly retry"),
                    ),
                    LoadoutActionOutcome::Rejected(reason) => (
                        "rejected",
                        Some(match reason {
                            LoadoutActionRejection::Provider(reason) => reason.reason(),
                            LoadoutActionRejection::RestrictedLocation => {
                                "go to orbit or a social space and retry"
                            }
                            LoadoutActionRejection::CharacterNotFound => "character was not found",
                            LoadoutActionRejection::InvalidRequest => {
                                "loadout request was rejected"
                            }
                        }),
                    ),
                };
                let (outcome, exit_code, reason) = if result.receipt.applied()
                    && !result.local_state_updated
                {
                    // Partial means items were reported, not that storage failed.
                    let reason =
                        if result.local_state.status == destiny_app::LoadoutLocalStatus::Partial {
                            "some loadout items were not applied; see local_state"
                        } else {
                            "local state could not be updated after the action"
                        };
                    ("local_state_failed", 42, Some(reason))
                } else if result.receipt.applied() {
                    ("applied", 0, reason)
                } else {
                    ("failed", 41, reason)
                };
                Self {
                    value: json!({"kind":"apply_loadout", "character":clean(result.character.as_str()),
                    "slot":result.slot, "name":result.name.as_deref().map(clean),
                    "outcome":outcome, "receipt":receipt, "reason":reason,
                    "message":reason.and_then(crate::rejection_message),
                    "local_state": {
                        "status": result.local_state.status.code(),
                        "unresolved_items": result.local_state.unresolved_items.iter().map(local_issue).collect::<Vec<_>>(),
                        "skipped": result.local_state.skipped.iter().map(local_issue).collect::<Vec<_>>(),
                        "persisted": result.local_state.persisted
                    }}),
                    exit_code,
                }
            }
            Err(error) => {
                let (code, reason, candidates) = match error {
                    ApplyLoadoutError::App(error) => (
                        "preflight_failed",
                        destiny_app::AppError::new(error.category()).to_string(),
                        Vec::new(),
                    ),
                    ApplyLoadoutError::MissingIntent => (
                        "missing_intent",
                        "explicit intent is required".into(),
                        Vec::new(),
                    ),
                    ApplyLoadoutError::InvalidSlot => (
                        "invalid_slot",
                        "loadout slot must be between 1 and 12".into(),
                        Vec::new(),
                    ),
                    ApplyLoadoutError::EmptySlot => (
                        "empty_slot",
                        "loadout slot is empty or unavailable".into(),
                        Vec::new(),
                    ),
                    ApplyLoadoutError::UnknownName(candidates) => (
                        "unknown_loadout_name",
                        "unknown loadout name; choose an available slot".into(),
                        candidates,
                    ),
                    ApplyLoadoutError::AmbiguousName(candidates) => (
                        "ambiguous_loadout_name",
                        "ambiguous loadout name; choose an explicit slot".into(),
                        candidates,
                    ),
                };
                Self {
                    value: json!({"kind":"apply_loadout", "character":null, "slot":null, "name":null,
                        "outcome":"refused", "receipt":null, "local_state":{"status":"not_attempted", "unresolved_items":[], "skipped":[], "persisted":true}, "code":code, "reason":clean(&reason),
                        "candidates":candidates.iter().map(|c| json!({"slot":c.slot,"name":c.name.as_deref().map(clean)})).collect::<Vec<_>>()}),
                    exit_code: 41,
                }
            }
        }
    }
    pub fn render_text(&self) -> String {
        let mut text = format!(
            "Apply loadout: {}",
            self.value["outcome"].as_str().unwrap_or("failed")
        );
        if let Some(character) = self.value["character"].as_str() {
            text.push_str(&format!(
                " — character {character}, slot {}",
                self.value["slot"]
            ));
            if let Some(name) = self.value["name"].as_str() {
                text.push_str(&format!(" ({name})"));
            }
            text.push_str(&format!(
                "; receipt {}",
                self.value["receipt"].as_str().unwrap_or("none")
            ));
        }
        if let Some(reason) = self.value["reason"].as_str() {
            text.push_str(&format!("; {}", crate::reason_text(reason)));
        }
        let status = self.value["local_state"]["status"]
            .as_str()
            .unwrap_or("not_attempted");
        if status == "updated" {
            text.push_str("; projected locally");
        } else {
            text.push_str(&format!("; local projection {status}"));
        }
        for (field, label) in [("unresolved_items", "unresolved"), ("skipped", "skipped")] {
            if let Some(issues) = self.value["local_state"][field].as_array() {
                for issue in issues {
                    text.push_str(&format!(
                        "\n  {label}: {} [{}] — {}",
                        issue["item"]["name"].as_str().unwrap_or("(unknown item)"),
                        issue["item"]["instance_id"].as_str().unwrap_or("unknown"),
                        issue["reason"].as_str().unwrap_or("invalid_projection")
                    ));
                }
            }
        }
        if let Some(candidates) = self.value["candidates"].as_array() {
            for candidate in candidates {
                text.push_str(&format!(
                    "\n  [{}] {}",
                    candidate["slot"],
                    candidate["name"].as_str().unwrap_or("(unnamed)")
                ));
            }
        }
        text
    }
}
