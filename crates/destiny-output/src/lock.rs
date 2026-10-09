//! Transport-neutral, sanitized lock-state presentation contract.
use destiny_app::{
    LockExecutionEvidence, LockExecutionOutcome, LockOutcome, LockRejection,
    LockVerificationFailureReason, MutationError,
};
use serde_json::{Value, json};

pub use crate::equip::ProfileRefresh;

/// Acknowledgement-based success, sharing equip's refresh vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockStatus {
    Applied,
    VerifiedNoop,
    Incomplete,
    VerificationFailed,
    Refused,
}
impl LockStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::VerifiedNoop => "verified_noop",
            Self::Incomplete => "incomplete",
            Self::VerificationFailed => "verification_failed",
            Self::Refused => "refused",
        }
    }
}

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
                    LockRejection::Provider(reason) => reason.reason(),
                    LockRejection::NotInSocialSpace => {
                        "character must be in orbit, a social space, or offline"
                    }
                    LockRejection::ItemNotFound => "item_not_found",
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
            LockExecutionOutcome::Applied(_) => {
                (LockStatus::Applied, ProfileRefresh::NotRequired, None)
            }
            LockExecutionOutcome::VerifiedNoOp(_) => {
                (LockStatus::VerifiedNoop, ProfileRefresh::NotRequired, None)
            }
            LockExecutionOutcome::IncompleteExecution(_) => {
                (LockStatus::Incomplete, ProfileRefresh::NotRequired, None)
            }
            LockExecutionOutcome::VerificationFailed(failure) => (
                LockStatus::VerificationFailed,
                ProfileRefresh::Failed,
                Some(match failure.reason() {
                    LockVerificationFailureReason::RefreshFailed => {
                        "local state could not be updated after the action; do not blindly retry"
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
    pub fn local_state(&self) -> &'static str {
        match (self.status, self.profile_refresh) {
            (_, ProfileRefresh::Failed)
            | (LockStatus::Incomplete | LockStatus::VerificationFailed, _) => "refresh_pending",
            (LockStatus::Applied, _) => "updated",
            _ => "unchanged",
        }
    }
    pub fn json(&self) -> Value {
        json!({
            "status": self.status.as_str(),
            "item": self.item.as_ref().map(|i| json!({"name":clean(&i.name),"instance_id":clean(&i.instance_id)})),
            "locked": self.locked,
            "attempt": self.attempt,
            "reason": self.reason.as_deref().map(clean),
            "message": self.reason.as_deref().and_then(crate::rejection_message),
            "local_state": self.local_state(),
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
        if self.status == LockStatus::Applied {
            return format!(
                "{name}: {state}\n✔ applied (acknowledged by Bungie; not post-action verified)"
            );
        }
        if self.status == LockStatus::VerifiedNoop {
            return format!(
                "{name}: already {state} according to local state; nothing sent to Bungie"
            );
        }
        let mut lines = vec![format!("{name}: {}", self.status.as_str())];
        if let Some(reason) = &self.reason {
            lines.push(clean(crate::reason_text(reason)));
        }
        if let Some(attempt) = self.attempt {
            lines.push(format!("set_lock_state: {attempt}"));
        }
        if self.local_state() == "refresh_pending" {
            lines.push("local state refresh pending".into());
        }
        if let Some(locked) = self.observed {
            lines.push(format!(
                "{}: {}",
                "observed state",
                if locked { "locked" } else { "unlocked" }
            ));
        }
        lines.join("\n")
    }
}
