//! CLI exits over shared sanitized stack-pull presentation.
use destiny_app::{
    PullDisposition, StackPullExecutionFailure, StackPullExecutionFailureReason,
    StackPullExecutionOutcome, StackPullPlanningError, StackPullPlanningRefusal,
    StackPullPreDispatchError,
};

#[derive(Clone, Debug)]
pub struct StackPullOutput {
    shared: destiny_output::stack_pull::StackPullOutput,
    exit_code: i32,
    versions: Option<(
        destiny_app::DefinitionVersion,
        destiny_app::DefinitionVersion,
    )>,
}
impl StackPullOutput {
    pub fn from_outcome(outcome: &StackPullExecutionOutcome) -> Self {
        let shared = destiny_output::stack_pull::StackPullOutput::from_outcome(outcome);
        let exit_code = if shared.is_success() {
            0
        } else {
            match outcome.disposition() {
                PullDisposition::Applied => unreachable!("shared mapper preserves disposition"),
                PullDisposition::LocalStateFailed => 42,
                PullDisposition::Uncertain => 41,
                PullDisposition::Refused => 41,
            }
        };
        Self {
            shared,
            exit_code,
            versions: None,
        }
    }
    pub fn planning_failed(error: &StackPullPlanningError) -> Self {
        Self {
            shared: destiny_output::stack_pull::StackPullOutput::planning_failed(error),
            exit_code: planning_exit(error),
            versions: None,
        }
    }
    pub fn execution_failed(failure: &StackPullExecutionFailure) -> Self {
        let exit_code = match failure.reason() {
            StackPullExecutionFailureReason::PreDispatch(
                StackPullPreDispatchError::Eligibility(error),
            ) => planning_exit(error),
            StackPullExecutionFailureReason::PreDispatch(_) => 30,
            StackPullExecutionFailureReason::Gateway(attempt) => attempt
                .gateway_result()
                .as_ref()
                .err()
                .map(|error| crate::command::category_outcome(error.category()).exit_code)
                .unwrap_or(40),
        };
        Self {
            shared: destiny_output::stack_pull::StackPullOutput::execution_failed(failure),
            exit_code,
            versions: None,
        }
    }
    pub fn with_versions(
        mut self,
        versions: Option<(
            destiny_app::DefinitionVersion,
            destiny_app::DefinitionVersion,
        )>,
    ) -> Self {
        self.versions = versions;
        self
    }
    pub fn json(&self) -> serde_json::Value {
        self.shared.json()
    }
    pub fn render_text(&self) -> String {
        if self.versions.is_some()
            && self.shared.json()["failure"]["reason"] == "definition_version_mismatch"
        {
            crate::pull_output::render_with_versions(&self.shared.json(), self.versions.as_ref())
        } else {
            self.shared.render_text()
        }
    }
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
}
fn planning_exit(error: &StackPullPlanningError) -> i32 {
    match error {
        StackPullPlanningError::Infrastructure(error) => {
            crate::command::category_outcome(error.category()).exit_code
        }
        StackPullPlanningError::Refused(StackPullPlanningRefusal::DefinitionMissing) => 33,
        StackPullPlanningError::Refused(StackPullPlanningRefusal::DefinitionAmbiguous) => 31,
        StackPullPlanningError::Refused(_) => 40,
    }
}
