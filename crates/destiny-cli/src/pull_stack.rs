//! One fresh stack plan, consumed once, with acknowledgement and no retry.
use crate::{command::StackPullSelection, pull_stack_output::StackPullOutput};
use destiny_app::*;

/// Membership is authenticated context, never a command-line override. Caller
/// serializes local inventory publication and definition use across this call.
pub fn pull_stack<G, R, D, M, C, P, T>(
    selection: StackPullSelection,
    membership: DestinyMembership,
    service: &mut StackPullService<G, R, D, M, C, P>,
    gateway: &mut T,
    freshness: FreshnessPolicy,
) -> StackPullOutput
where
    G: ProfileGateway,
    R: InventoryRepository,
    D: DefinitionRepository,
    M: ManifestGateway,
    C: Clock,
    P: PlanIdGenerator,
    T: StackPullGateway,
{
    let request = StackPullPlanRequest {
        membership,
        owner: selection.owner,
        item_hash: selection.item_hash,
        amount: selection.amount,
    };
    let plan = match service.plan(
        &request,
        PlanLifetime::new(60).expect("fixed positive plan lifetime"),
        freshness,
    ) {
        Ok(plan) => plan,
        Err(error) => return StackPullOutput::planning_failed(&error),
    };
    match service.execute(plan, gateway, freshness) {
        Ok(outcome) => StackPullOutput::from_outcome(&outcome),
        Err(failure) => StackPullOutput::execution_failed(&failure),
    }
}
