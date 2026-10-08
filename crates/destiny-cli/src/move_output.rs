//! CLI-only exit policy for the transport-neutral move presentation.
use destiny_app::{
    Character, ExecutionEvidence, ItemInstance, MoveExecutionOutcome, MutationError,
};
use destiny_output::move_item::MoveOutput as SharedOutput;
pub use destiny_output::move_item::{
    MoveItem, MoveLocation, MoveObservation, MoveStatus, MoveStep, ProfileRefresh, location_view,
    step_view,
};
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug)]
pub struct MoveOutput {
    output: SharedOutput,
    pub refusal_exit_code: Option<i32>,
}
impl Deref for MoveOutput {
    type Target = SharedOutput;
    fn deref(&self) -> &Self::Target {
        &self.output
    }
}
impl DerefMut for MoveOutput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.output
    }
}
impl MoveOutput {
    pub fn from_shared(output: SharedOutput) -> Self {
        Self {
            output,
            refusal_exit_code: None,
        }
    }
    pub fn refused(error: MutationError) -> Self {
        let code = match error {
            MutationError::MissingItem | MutationError::MissingResolution => 32,
            MutationError::MissingDestination => 2,
            _ => crate::command::category_outcome(error.category()).exit_code,
        };
        Self {
            output: SharedOutput::refused(error),
            refusal_exit_code: Some(code),
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
        Self::from_shared(SharedOutput::from_outcome(outcome, characters, observed))
    }
    pub fn exit_code(&self) -> i32 {
        match self.status {
            MoveStatus::Applied | MoveStatus::VerifiedNoop => 0,
            MoveStatus::Incomplete => 41,
            MoveStatus::VerificationFailed => 42,
            MoveStatus::Refused => self.refusal_exit_code.unwrap_or(40),
        }
    }
}
