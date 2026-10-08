//! One explicit lock-state intent; all mutation policy stays application-owned.
use crate::command::{CliError, LockArgs};
use crate::lock_output::{LockItem, LockOutput};
use destiny_app::{
    Clock, DefinitionRepository, FreshnessPolicy, InventoryRepository, ItemInstanceId,
    ItemSelector, LockGateway, LockRequest, MoveIntentAuthorization, MutationService,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncReason, Resolution, SearchRepository,
};

pub fn set_lock_state<G, R, P, L, C>(
    args: &LockArgs,
    locked: bool,
    mutations: &mut MutationService<G, R, P>,
    gateway: &mut L,
    clock: &C,
    policy: FreshnessPolicy,
) -> Result<LockOutput, CliError>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    P: PlanIdGenerator,
    L: LockGateway,
    C: Clock,
{
    args.validate()?;
    let now = clock.now()?;
    mutations
        .sync_mut()
        .sync(ProfileSyncReason::Ordinary, policy, now)?;
    let snapshot = mutations
        .repository()
        .load_current()?
        .ok_or_else(|| CliError::from_category(destiny_app::ErrorCategory::LocalStorage))?;
    let item = match mutations
        .repository()
        .lookup_instance(&ItemInstanceId::new(&args.item))?
    {
        Resolution::Unique(_) | Resolution::Ambiguous(_) => {
            ItemSelector::instance(ItemInstanceId::new(&args.item))
        }
        Resolution::Missing if args.item.bytes().all(|b| b.is_ascii_digit()) => {
            ItemSelector::instance(ItemInstanceId::new(&args.item))
        }
        Resolution::Missing => ItemSelector::semantic(&args.item)?,
    };
    let request = LockRequest {
        membership: snapshot.metadata.membership_id,
        item,
        locked,
        intent: MoveIntentAuthorization::Explicit,
    };
    let plan = match mutations.plan_lock(&request, PlanLifetime::MAX, policy, now) {
        Ok(plan) => plan,
        Err(error) => return Ok(LockOutput::refused(error, locked)),
    };
    let item_view = match mutations.repository().lookup_instance(plan.item())? {
        Resolution::Unique(instance) => match mutations
            .repository()
            .lookup_definition(instance.item_hash)?
        {
            Resolution::Unique(definition) => Some(LockItem {
                name: definition.name,
                instance_id: instance.instance_id.as_str().into(),
            }),
            _ => None,
        },
        _ => None,
    };
    let post = clock.now()?;
    let mut output =
        match mutations.execute_lock_verified(gateway, &request, &plan, policy, now, post) {
            Ok(outcome) => LockOutput::from_shared(destiny_output::lock::LockOutput::from_outcome(
                &outcome, locked,
            )),
            Err(failure) => {
                LockOutput::refused(failure.error(), locked).with_evidence(failure.evidence())
            }
        };
    output.item = item_view;
    Ok(output)
}
