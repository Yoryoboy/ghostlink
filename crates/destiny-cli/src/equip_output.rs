//! CLI text and process-exit policy around the transport-neutral equip output.
use destiny_app::{
    Character, ExecutionEvidence, ItemInstance, MoveExecutionOutcome, MutationError,
};
use destiny_output::equip::EquipOutput as SharedOutput;
pub use destiny_output::equip::{
    EquipCharacter, EquipItem, EquipLocation, EquipObservation, EquipStatus, EquipStep,
    ProfileRefresh, location_view, mutation_reason, step_view,
};
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug)]
pub struct EquipOutput {
    output: SharedOutput,
    /// Process-only metadata, deliberately excluded from the parity JSON.
    pub refusal_exit_code: Option<i32>,
}
impl Deref for EquipOutput {
    type Target = SharedOutput;
    fn deref(&self) -> &Self::Target {
        &self.output
    }
}
impl DerefMut for EquipOutput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.output
    }
}
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}
impl EquipOutput {
    pub fn from_shared(output: SharedOutput) -> Self {
        Self {
            output,
            refusal_exit_code: None,
        }
    }
    pub fn refused(error: MutationError) -> Self {
        let exit_code = match error {
            MutationError::MissingItem | MutationError::MissingResolution => 32,
            MutationError::MissingDestination => 2,
            _ => crate::command::category_outcome(error.category()).exit_code,
        };
        Self {
            output: SharedOutput::refused(error),
            refusal_exit_code: Some(exit_code),
        }
    }
    pub fn with_evidence(mut self, evidence: &ExecutionEvidence, characters: &[Character]) -> Self {
        self.output = self.output.with_evidence(evidence, characters);
        self
    }
    pub fn from_outcome(
        outcome: &MoveExecutionOutcome,
        characters: &[Character],
        observed: Option<&ItemInstance>,
    ) -> Self {
        Self {
            output: SharedOutput::from_outcome(outcome, characters, observed),
            refusal_exit_code: None,
        }
    }
    pub fn exit_code(&self) -> i32 {
        match self.status {
            EquipStatus::Applied | EquipStatus::VerifiedNoop => 0,
            EquipStatus::Incomplete => 41,
            EquipStatus::VerificationFailed => 42,
            EquipStatus::Refused => self.refusal_exit_code.unwrap_or(40),
        }
    }
    pub fn render_text(&self) -> String {
        let name = clean(
            self.item
                .as_ref()
                .map(|i| i.name.as_str())
                .unwrap_or("Item"),
        );
        let character = clean(
            self.character
                .as_ref()
                .map(|c| c.label.as_str())
                .unwrap_or("character"),
        );
        if self.status == EquipStatus::VerifiedNoop {
            return format!("{name} is already equipped on {character}\n✔ verified");
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
                    .map(|r| format!(" — {}", clean(r)))
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
        if let Some(o) = &self.observed {
            lines.push(format!(
                "{}: {}; {}",
                "refreshed state",
                clean(&o.location.label),
                if o.equipped {
                    "equipped"
                } else {
                    "not equipped"
                }
            ));
        }
        lines.join("\n")
    }
}
