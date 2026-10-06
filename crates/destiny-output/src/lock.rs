//! Transport-neutral, sanitized lock-state presentation contract.
use destiny_app::{
    LockExecutionEvidence, LockExecutionOutcome, LockOutcome, LockRejection,
    LockVerificationFailureReason, MutationError,
};
use serde_json::{Value, json};

pub use crate::equip::{EquipStatus as LockStatus, ProfileRefresh};

#[derive(Clone, Debug)]
pub struct LockItem {
    pub name: String,
    pub instance_id: String,
}
#[derive(Clone, Debug)]
pub struct LockOutput {
    pub status: LockStatus,
    pub item: Option<LockItem>,
    pub locked: bool,
    pub attempt: Option<&'static str>,
    pub reason: Option<String>,
    pub profile_refresh: ProfileRefresh,
    pub observed: Option<bool>,
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
impl LockOutput {
    pub fn refused(error: MutationError, locked: bool) -> Self {
        Self {
            status: LockStatus::Refused,
            item: None,
            locked,
            attempt: None,
            reason: Some(crate::equip::mutation_reason(error)),
            profile_refresh: ProfileRefresh::NotRequired,
            observed: None,
        }
    }
    pub fn with_evidence(mut self, evidence: &LockExecutionEvidence) -> Self {
        let (attempt, reason) = match evidence.attempt() {
            None => (None, None),
            Some(LockOutcome::Applied) => (Some("applied"), None),
            Some(LockOutcome::Uncertain) => (
                Some("uncertain"),
                Some("provider outcome is uncertain; do not blindly retry"),
            ),
            Some(LockOutcome::Transient) => (
                Some("failed"),
                Some("provider temporarily unavailable; action did not apply"),
            ),
            Some(LockOutcome::AuthenticationRefused) => (
                Some("authentication_refused"),
                Some("provider authentication failed"),
            ),
            Some(LockOutcome::Rejected(reason)) => (
                Some("rejected"),
                Some(match reason {
                    LockRejection::NotInSocialSpace => {
                        "character must be in orbit, a social space, or offline"
                    }
                    LockRejection::ItemNotFound => "item was not found",
                    LockRejection::CharacterNotFound => "character was not found",
                    LockRejection::InvalidRequest => "lock request was rejected",
                }),
            ),
        };
        self.attempt = attempt;
        self.observed = evidence.observed();
        if self.reason.is_none() {
            self.reason = reason.map(str::to_owned);
        }
        self
    }
    pub fn from_outcome(outcome: &LockExecutionOutcome, locked: bool) -> Self {
        let (status, refresh, reason) = match outcome {
            LockExecutionOutcome::VerifiedSuccess(_) => {
                (LockStatus::VerifiedSuccess, ProfileRefresh::Refreshed, None)
            }
            LockExecutionOutcome::VerifiedNoOp(_) => {
                (LockStatus::VerifiedNoop, ProfileRefresh::NotRequired, None)
            }
            LockExecutionOutcome::IncompleteExecution(_) => {
                (LockStatus::Incomplete, ProfileRefresh::Refreshed, None)
            }
            LockExecutionOutcome::AppliedPendingProviderRefresh(_) => (
                LockStatus::AppliedPendingProviderRefresh,
                ProfileRefresh::Refreshed,
                Some(
                    "provider confirmed the action, but observed state is stale/unconfirmed; do not retry, run `ghostlink sync` later to confirm",
                ),
            ),
            LockExecutionOutcome::VerificationFailed(failure) => (
                LockStatus::VerificationFailed,
                if failure.reason() == LockVerificationFailureReason::RefreshFailed {
                    ProfileRefresh::Failed
                } else {
                    ProfileRefresh::Refreshed
                },
                Some(match failure.reason() {
                    LockVerificationFailureReason::RefreshFailed => {
                        "outcome is uncertain: post-action profile refresh failed; do not blindly retry"
                    }
                    LockVerificationFailureReason::Unresolved => {
                        "outcome is uncertain: refreshed item could not be uniquely resolved; do not blindly retry"
                    }
                    LockVerificationFailureReason::Contradiction => {
                        "refreshed state contradicts the action; lock state was not verified; do not blindly retry"
                    }
                }),
            ),
        };
        Self {
            status,
            item: None,
            locked,
            attempt: None,
            reason: reason.map(str::to_owned),
            profile_refresh: refresh,
            observed: None,
        }
        .with_evidence(outcome.evidence())
    }
    pub fn json(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "item": self.item.as_ref().map(|i| json!({"name":clean(&i.name),"instance_id":clean(&i.instance_id)})),
            "locked": self.locked,
            "attempt": self.attempt,
            "reason": self.reason.as_deref().map(clean),
            "profile_refresh": match self.profile_refresh { ProfileRefresh::NotRequired => "not_required", ProfileRefresh::Refreshed => "refreshed", ProfileRefresh::Failed => "failed" },
            "observed": self.observed.map(|locked| json!({"locked":locked})),
        })
    }
    pub fn render_text(&self) -> String {
        let name = clean(
            self.item
                .as_ref()
                .map(|i| i.name.as_str())
                .unwrap_or("Item"),
        );
        let state = if self.locked { "locked" } else { "unlocked" };
        if matches!(
            self.status,
            LockStatus::VerifiedSuccess | LockStatus::VerifiedNoop
        ) {
            return format!(
                "{name}: {}{state}\n✔ verified",
                if self.status == LockStatus::VerifiedNoop {
                    "already "
                } else {
                    ""
                }
            );
        }
        let mut lines = vec![format!("{name}: {}", self.status.as_str())];
        if let Some(reason) = &self.reason {
            lines.push(clean(reason));
        }
        if let Some(attempt) = self.attempt {
            lines.push(format!("set_lock_state: {attempt}"));
        }
        match self.profile_refresh {
            ProfileRefresh::NotRequired => {}
            ProfileRefresh::Refreshed => lines.push("profile refreshed".into()),
            ProfileRefresh::Failed => {
                lines.push("profile refresh failed; no fresh state available".into())
            }
        }
        if let Some(locked) = self.observed {
            lines.push(format!(
                "{}: {}",
                if self.status == LockStatus::AppliedPendingProviderRefresh {
                    "stale/unconfirmed state"
                } else {
                    "observed state"
                },
                if locked { "locked" } else { "unlocked" }
            ));
        }
        lines.join("\n")
    }
}
