//! Transport-neutral single-item move contract and sanitized text presentation.
//! Equip's location/evidence vocabulary is shared, not its output schema.
use destiny_app::{
    Character, ExecutionEvidence, ItemInstance, MoveExecutionOutcome, MutationError,
    VerificationFailureReason,
};
use serde_json::{Value, json};

pub use crate::equip::{
    EquipLocation as MoveLocation, EquipStep as MoveStep, ProfileRefresh, location_view, step_view,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveStatus {
    Applied,
    VerifiedNoop,
    Incomplete,
    VerificationFailed,
    Refused,
}
impl MoveStatus {
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
pub struct MoveItem {
    pub name: String,
    pub instance_id: String,
    pub slot: Option<String>,
}
#[derive(Clone, Debug)]
pub struct MoveObservation {
    pub location: MoveLocation,
    pub equipped: bool,
}

#[derive(Clone, Debug)]
pub struct MoveOutput {
    pub status: MoveStatus,
    pub item: Option<MoveItem>,
    pub destination: Option<MoveLocation>,
    pub steps: Vec<MoveStep>,
    pub reason: Option<String>,
    pub profile_refresh: ProfileRefresh,
    pub observed: Option<MoveObservation>,
}

fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
fn location_json(location: &MoveLocation) -> Value {
    let mut value = json!({"kind":location.kind,"label":location.label});
    if let Some(id) = &location.character_id {
        value["character_id"] = json!(id);
    }
    value
}
impl MoveOutput {
    pub fn refused(error: MutationError) -> Self {
        Self {
            status: MoveStatus::Refused,
            item: None,
            destination: None,
            steps: vec![],
            reason: Some(crate::equip::mutation_reason(error)),
            profile_refresh: ProfileRefresh::NotRequired,
            observed: None,
        }
    }
    pub fn with_evidence(mut self, evidence: &ExecutionEvidence, characters: &[Character]) -> Self {
        self.steps = evidence
            .steps()
            .iter()
            .map(|r| step_view(r.step.ordinal, &r.step.action, r.outcome, characters))
            .collect();
        self
    }
    pub fn from_outcome(
        outcome: &MoveExecutionOutcome,
        characters: &[Character],
        observed: Option<&ItemInstance>,
    ) -> Self {
        let (status, evidence, refresh, reason) = match outcome {
            MoveExecutionOutcome::Applied(e) => (
                MoveStatus::Applied, e, ProfileRefresh::NotRequired,
                Some("Bungie accepted the action; not re-read".into()),
            ),
            MoveExecutionOutcome::VerifiedNoOp(e) => (
                MoveStatus::VerifiedNoop, e, ProfileRefresh::NotRequired, None,
            ),
            MoveExecutionOutcome::IncompleteExecution(e) => (
                MoveStatus::Incomplete, e.evidence(), ProfileRefresh::NotRequired,
                e.error().map(crate::equip::mutation_reason),
            ),
            MoveExecutionOutcome::VerificationFailed(e) => (
                MoveStatus::VerificationFailed, e.evidence(),
                ProfileRefresh::Failed,
                Some(match e.reason() {
                    VerificationFailureReason::RefreshFailed => "outcome is uncertain: local state could not be updated after the action",
                }.into()),
            ),
        };
        Self {
            status,
            item: None,
            destination: None,
            steps: vec![],
            reason,
            profile_refresh: refresh,
            // Never label a stale repository projection as a fresh observation.
            observed: if refresh == ProfileRefresh::Failed
                || matches!(status, MoveStatus::Applied | MoveStatus::Incomplete)
            {
                None
            } else {
                observed.map(|i| MoveObservation {
                    location: location_view(&i.location, characters),
                    equipped: i.equipped,
                })
            },
        }
        .with_evidence(evidence, characters)
    }
    pub fn json(&self) -> Value {
        json!({
            "status":self.status.as_str(),
            "item":self.item.as_ref().map(|i|json!({"name":i.name,"instance_id":i.instance_id,"slot":i.slot})),
            "destination":self.destination.as_ref().map(location_json),
            "steps":self.steps.iter().map(|s| {
                let mut value = json!({"ordinal":s.ordinal,"kind":s.kind,"to":location_json(&s.to),"outcome":s.outcome});
                if let Some(from) = &s.from { value["from"] = location_json(from); }
                if let Some(reason) = &s.reason { value["reason"] = json!(reason); if let Some(message) = crate::rejection_message(reason) { value["message"] = json!(message); } }
                value
            }).collect::<Vec<_>>(),
            "steps_completed":self.steps.iter().filter(|s| s.outcome == "completed").count(),
            "steps_uncompleted":self.steps.iter().filter(|s| s.outcome != "completed").count(),
            "verification":match self.status {
                MoveStatus::VerifiedNoop => "verified",
                MoveStatus::VerificationFailed => "failed",
                _ => "not_verified",
            },
            "reason":self.reason,
            "profile_refresh":match self.profile_refresh {
                ProfileRefresh::NotRequired => "not_required",
                ProfileRefresh::Refreshed => "refreshed",
                ProfileRefresh::Failed => "failed",
            },
            "observed":self.observed.as_ref().map(|o|json!({"location":location_json(&o.location),"equipped":o.equipped})),
        })
    }
    pub fn render_text(&self) -> String {
        let name = clean(
            self.item
                .as_ref()
                .map(|i| i.name.as_str())
                .unwrap_or("Item"),
        );
        let destination = clean(
            self.destination
                .as_ref()
                .map(|d| d.label.as_str())
                .unwrap_or("destination"),
        );
        if self.status == MoveStatus::VerifiedNoop {
            return format!("{name} is already at {destination}\n✔ verified");
        }
        let mut lines = vec![format!("{name}: {}", self.status.as_str())];
        if let Some(reason) = &self.reason {
            lines.push(clean(reason));
        }
        for step in &self.steps {
            let outcome = if step.outcome == "not_run" {
                "not run"
            } else {
                step.outcome
            };
            let route = step
                .from
                .as_ref()
                .map(|from| format!(" ({} → {})", clean(&from.label), clean(&step.to.label)))
                .unwrap_or_default();
            lines.push(format!(
                "{}. {}: {outcome}{route}{}",
                step.ordinal,
                step.kind,
                step.reason
                    .as_ref()
                    .map(|r| format!(" — {}", clean(crate::reason_text(r))))
                    .unwrap_or_default()
            ));
        }
        if self.steps.iter().any(|s| s.outcome == "uncertain") {
            lines.push("outcome is uncertain; do not blindly retry".into());
        }
        match self.profile_refresh {
            ProfileRefresh::Refreshed => lines.push("profile refreshed".into()),
            ProfileRefresh::Failed => {
                lines.push("profile refresh failed; no fresh state available".into())
            }
            ProfileRefresh::NotRequired => {}
        }
        if let Some(observed) = &self.observed {
            lines.push(format!(
                "{}: {}",
                "refreshed state",
                clean(&observed.location.label)
            ));
        }
        lines.join("\n")
    }
}
