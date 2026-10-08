//! One fresh stack plan, consumed once, with bounded verified observations.
use crate::{command::StackPullSelection, pull_stack_output::StackPullOutput};
use destiny_app::*;

/// Membership is authenticated context, never a command-line override. Caller
/// serializes local inventory publication and definition use across this call.
pub fn pull_stack<G, R, D, M, C, P, T, W>(
    selection: StackPullSelection,
    membership: DestinyMembership,
    service: &mut StackPullService<G, R, D, M, C, P>,
    gateway: &mut T,
    delay: &mut W,
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
    W: RetryDelay,
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
    match service.execute_verified(plan, gateway, delay, freshness) {
        Ok(outcome) => StackPullOutput::from_outcome(&outcome),
        Err(failure) => StackPullOutput::execution_failed(&failure),
    }
}
