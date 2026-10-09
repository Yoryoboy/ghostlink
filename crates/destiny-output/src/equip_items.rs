//! Shared sanitized projection of a combined batch equip attempt.
use destiny_app::{BatchEquipResult, BatchItemStatus, EquipItemsOutcome, MutationError};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct EquipItemsOutput {
    pub value: Value,
    pub exit_code: i32,
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
impl EquipItemsOutput {
    pub fn from_result(result: Result<BatchEquipResult, MutationError>) -> Self {
        match result {
            Err(error) => Self {
                value: json!({"kind":"equip_items", "outcome":"refused", "items":[],
                    "reason":clean(&error.to_string())}),
                exit_code: 41,
            },
            Ok(result) => {
                let (outcome, exit_code, reason) = if !result.local_state_updated {
                    (
                        "local_state_failed",
                        42,
                        Some("local state could not be updated after the action"),
                    )
                } else if result.no_op() {
                    ("verified_no_op", 0, None)
                } else if result.applied() {
                    ("applied", 0, None)
                } else {
                    let reason = match &result.receipt {
                        Some(EquipItemsOutcome::Rejected(reason)) => reason.reason(),
                        Some(EquipItemsOutcome::Refused) => "provider refused batch equip",
                        Some(EquipItemsOutcome::RestrictedLocation) => {
                            "go to orbit or a social space and retry"
                        }
                        Some(EquipItemsOutcome::AuthenticationRefused) => {
                            "provider authentication failed"
                        }
                        Some(EquipItemsOutcome::Transient) => "provider temporarily unavailable",
                        Some(EquipItemsOutcome::Uncertain) => {
                            "provider outcome is uncertain; do not blindly retry"
                        }
                        _ => "batch equip was incomplete; do not blindly retry",
                    };
                    ("incomplete", 41, Some(reason))
                };
                let items: Vec<_> = result
                    .items
                    .iter()
                    .map(|item| {
                        let (status, code) = match item.status {
                            BatchItemStatus::NoOp => ("no_op", None),
                            BatchItemStatus::Success => ("success", None),
                            BatchItemStatus::Failure(code) => ("failure", Some(code)),
                            BatchItemStatus::NotReported => ("not_reported", None),
                            BatchItemStatus::NotAttempted => ("not_attempted", None),
                        };
                        json!({"item":clean(item.item.as_str()), "name":clean(&item.name),
                            "slot":item.slot.as_str(), "status":status, "code":code})
                    })
                    .collect();
                Self {
                    value: json!({"kind":"equip_items", "outcome":outcome,
                        "items":items, "reason":reason, "message":reason.and_then(crate::rejection_message)}),
                    exit_code,
                }
            }
        }
    }
    pub fn render_text(&self) -> String {
        let mut text = format!(
            "Equip items: {}",
            self.value["outcome"].as_str().unwrap_or("incomplete")
        );
        if let Some(reason) = self.value["reason"].as_str() {
            text.push_str(&format!("; {}", crate::reason_text(reason)));
        }
        if let Some(items) = self.value["items"].as_array() {
            for item in items {
                text.push_str(&format!(
                    "\n  {} [{}] — {}: {}",
                    item["name"].as_str().unwrap_or("unknown"),
                    item["item"].as_str().unwrap_or("unknown"),
                    item["slot"].as_str().unwrap_or("unknown"),
                    item["status"].as_str().unwrap_or("not_reported")
                ));
                if let Some(code) = item["code"].as_i64() {
                    text.push_str(&format!(" ({code})"));
                }
            }
        }
        text
    }
}
