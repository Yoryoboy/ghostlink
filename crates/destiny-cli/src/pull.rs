//! One fresh plan, consumed once, with acknowledgement and no redispatch.
use crate::pull_output::PullOutput;
use destiny_app::{
    Clock, DefinitionRepository, DestinyMembership, FreshnessPolicy, InstancedPullGateway,
    InstancedPullPlanRequest, InstancedPullService, InventoryRepository, ItemInstanceId,
    ManifestGateway, PlanIdGenerator, PlanLifetime, ProfileGateway,
};

/// Internal one-plan generator; even accidental re-planning cannot remint intent.
pub(crate) struct InvocationPlanId(bool);
impl InvocationPlanId {
    pub(crate) fn new() -> Self {
        Self(false)
    }
}
impl PlanIdGenerator for InvocationPlanId {
    fn next_plan_id(&mut self) -> Result<destiny_app::PlanId, destiny_app::AppError> {
        if self.0 {
            return Err(destiny_app::AppError::new(
                destiny_app::ErrorCategory::PlanIdExhausted,
            ));
        }
        self.0 = true;
        Ok(destiny_app::PlanId::new("cli-pull"))
    }
}

/// Caller owns local publication/definition-use serialization across this entire
/// call. Membership is authenticated context, not a command-line argument.
pub fn pull_item<G, R, D, M, C, P, T>(
    item: ItemInstanceId,
    membership: DestinyMembership,
    service: &mut InstancedPullService<G, R, D, M, C, P>,
    gateway: &mut T,
    freshness: FreshnessPolicy,
) -> PullOutput
where
    G: ProfileGateway,
    R: InventoryRepository,
    D: DefinitionRepository,
    M: ManifestGateway,
    C: Clock,
    P: PlanIdGenerator,
    T: InstancedPullGateway,
{
    let plan = match service.plan(
        &InstancedPullPlanRequest { membership, item },
        PlanLifetime::new(60).expect("fixed positive plan lifetime"),
        freshness,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return PullOutput::planning_failed(&error)
                .with_versions(service.definition_versions());
        }
    };
    match service.execute(plan, gateway, freshness) {
        Ok(outcome) => PullOutput::from_outcome(&outcome),
        Err(failure) => {
            PullOutput::execution_failed(&failure).with_versions(service.definition_versions())
        }
    }
}
