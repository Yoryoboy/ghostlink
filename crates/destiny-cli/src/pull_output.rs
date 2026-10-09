//! CLI exit and text policy over the committed transport-neutral pull mapper.
use destiny_app::{
    InstancedPullExecutionFailure, InstancedPullExecutionFailureReason,
    InstancedPullExecutionOutcome, InstancedPullPlanningError, InstancedPullRefusal,
    PreDispatchError, PullDisposition,
};
use destiny_output::instanced_pull::InstancedPullOutput;

#[derive(Clone, Debug)]
pub struct PullOutput {
    shared: InstancedPullOutput,
    exit_code: i32,
    versions: Option<(
        destiny_app::DefinitionVersion,
        destiny_app::DefinitionVersion,
    )>,
}
impl PullOutput {
    pub fn from_outcome(outcome: &InstancedPullExecutionOutcome) -> Self {
        let shared = InstancedPullOutput::from_outcome(outcome);
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
    pub fn planning_failed(error: &InstancedPullPlanningError) -> Self {
        Self {
            shared: InstancedPullOutput::planning_failed(error),
            exit_code: planning_exit(error),
            versions: None,
        }
    }
    pub fn execution_failed(failure: &InstancedPullExecutionFailure) -> Self {
        let exit_code = match failure.reason() {
            InstancedPullExecutionFailureReason::PreDispatch(PreDispatchError::Eligibility(
                error,
            )) => planning_exit(error),
            InstancedPullExecutionFailureReason::PreDispatch(_) => 30,
            InstancedPullExecutionFailureReason::Gateway(attempt) => attempt
                .gateway_result()
                .as_ref()
                .err()
                .map(|error| crate::command::category_outcome(error.category()).exit_code)
                .unwrap_or(40),
        };
        Self {
            shared: InstancedPullOutput::execution_failed(failure),
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
    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }
    /// Format only the sanitized shared projection and retained receipt/failure.
    pub fn render_text(&self) -> String {
        render_with_versions(&self.shared.json(), self.versions.as_ref())
    }
}
pub(crate) fn render_with_versions(
    value: &serde_json::Value,
    versions: Option<&(
        destiny_app::DefinitionVersion,
        destiny_app::DefinitionVersion,
    )>,
) -> String {
    let text = destiny_output::instanced_pull::render_pull_text(value);
    if (value["failure"]["reason"] == "definition_version_mismatch"
        || value["failure"]["capacity_reason"]["kind"] == "definition_version_mismatch")
        && let Some((snapshot, stored)) = versions
    {
        let clean = |s: &str| {
            s.chars()
                .map(|c| if c.is_control() { '?' } else { c })
                .collect::<String>()
        };
        return format!(
            "Postmaster pull: refused\nSnapshot definitions version {} differs from stored definitions version {}; run ghostlink sync and retry.",
            clean(snapshot.as_str()),
            clean(stored.as_str())
        );
    }
    text
}

fn planning_exit(error: &InstancedPullPlanningError) -> i32 {
    match error {
        InstancedPullPlanningError::Infrastructure(error) => {
            crate::command::category_outcome(error.category()).exit_code
        }
        InstancedPullPlanningError::Refused(InstancedPullRefusal::TargetMissing) => 32,
        InstancedPullPlanningError::Refused(InstancedPullRefusal::TargetAmbiguous) => 31,
        InstancedPullPlanningError::Refused(InstancedPullRefusal::DefinitionMissing) => 33,
        InstancedPullPlanningError::Refused(_) => 40,
    }
}
