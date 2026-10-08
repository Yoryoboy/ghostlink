//! Process-only exit policy around the shared lock presentation.
use destiny_app::MutationError;
use destiny_output::lock::LockOutput as SharedOutput;
pub use destiny_output::lock::{LockItem, LockStatus, ProfileRefresh};
use std::ops::{Deref, DerefMut};

#[derive(Clone, Debug)]
pub struct LockOutput {
    output: SharedOutput,
    refusal_exit_code: Option<i32>,
}
impl Deref for LockOutput {
    type Target = SharedOutput;
    fn deref(&self) -> &Self::Target {
        &self.output
    }
}
impl DerefMut for LockOutput {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.output
    }
}
impl LockOutput {
    pub fn from_shared(output: SharedOutput) -> Self {
        Self {
            output,
            refusal_exit_code: None,
        }
    }
    pub fn refused(error: MutationError, locked: bool) -> Self {
        Self {
            output: SharedOutput::refused(error, locked),
            refusal_exit_code: Some(match error {
                MutationError::MissingItem | MutationError::MissingResolution => 32,
                _ => crate::command::category_outcome(error.category()).exit_code,
            }),
        }
    }
    pub fn with_evidence(mut self, evidence: &destiny_app::LockExecutionEvidence) -> Self {
        self.output = self.output.with_evidence(evidence);
        self
    }
    pub fn exit_code(&self) -> i32 {
        match self.status {
            LockStatus::Applied | LockStatus::VerifiedSuccess | LockStatus::VerifiedNoop => 0,
            LockStatus::Incomplete => 41,
            LockStatus::VerificationFailed => 42,
            LockStatus::AppliedPendingProviderRefresh => 43,
            LockStatus::Refused => self.refusal_exit_code.unwrap_or(40),
        }
    }
}
