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
                let (outcome, exit_code, reason) =
                    if result.receipt.applied() && !result.local_state_updated {
                        (
                            "local_state_failed",
                            42,
                            Some("local state could not be updated after the action"),
                        )
                    } else if result.receipt.applied() {
                        ("applied", 0, reason)
                    } else {
                        ("failed", 41, reason)
                    };
                Self {
                    value: json!({"kind":"apply_loadout", "character":clean(result.character.as_str()),
                        "slot":result.slot, "name":result.name.as_deref().map(clean),
                        "outcome":outcome, "receipt":receipt, "reason":reason,
                        "local_state_updated":result.local_state_updated}),
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
                        "outcome":"refused", "receipt":null, "local_state_updated":false, "code":code, "reason":clean(&reason),
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
            text.push_str(&format!("; {reason}"));
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
