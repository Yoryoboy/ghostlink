//! Thin single-intent move orchestration; the shared app owns all side effects.
use crate::command::{CliError, MoveArgs};
use crate::move_output::{MoveItem, MoveOutput, location_view};
use destiny_app::{
    Clock, DefinitionRepository, DestinationSelector, FreshnessPolicy, InventoryRepository,
    ItemInstanceId, ItemLocation, ItemSelector, MoveIntentAuthorization, MoveRequest,
    MutationError, MutationService, PlanId, PlanIdGenerator, PlanLifetime, ProfileGateway,
    ProfileSyncReason, Resolution, SearchRepository, TransferGateway,
};

pub(crate) struct InvocationPlanId(bool);
impl InvocationPlanId {
    pub(crate) fn new() -> Self {
        Self(false)
    }
}
impl PlanIdGenerator for InvocationPlanId {
    fn next_plan_id(&mut self) -> Result<PlanId, destiny_app::AppError> {
        if self.0 {
            return Err(destiny_app::AppError::new(
                destiny_app::ErrorCategory::PlanIdExhausted,
            ));
        }
        self.0 = true;
        Ok(PlanId::new("cli-move"))
    }
}

/// No defaults, retries, or transport-specific mutation logic.
pub fn move_item<G, R, P, T, C>(
    args: &MoveArgs,
    mutations: &mut MutationService<G, R, P>,
    transfer: &mut T,
    clock: &C,
    policy: FreshnessPolicy,
) -> Result<MoveOutput, CliError>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    P: PlanIdGenerator,
    T: TransferGateway,
    C: Clock,
{
    args.validate()?;
    let preflight_now = clock.now()?;
    mutations
        .sync_mut()
        .sync(ProfileSyncReason::Ordinary, policy, preflight_now)?;
    let snapshot = mutations
        .repository()
        .load_current()?
        .ok_or_else(|| CliError::from_category(destiny_app::ErrorCategory::LocalStorage))?;
    let location = if let Some(selector) = &args.character {
        let exact: Vec<_> = snapshot
            .characters
            .iter()
            .filter(|c| c.id.as_str() == selector)
            .collect();
        let matches = if exact.is_empty() {
            snapshot
                .characters
                .iter()
                .filter(|c| {
                    c.class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(selector))
                })
                .collect::<Vec<_>>()
        } else {
            exact
        };
        match matches.as_slice() {
            [] => return Ok(MoveOutput::refused(MutationError::MissingDestination)),
            [character] => ItemLocation::Character(character.id.clone()),
            _ => return Ok(MoveOutput::refused(MutationError::AmbiguousDestination)),
        }
    } else {
        ItemLocation::Vault
    };
    let destination_view = location_view(&location, &snapshot.characters);
    let destination = match location {
        ItemLocation::Character(id) => DestinationSelector::character(id),
        _ => DestinationSelector::vault(),
    };
    let instance = mutations
        .repository()
        .lookup_instance(&ItemInstanceId::new(&args.item))?;
    let item = match instance {
        Resolution::Unique(_) | Resolution::Ambiguous(_) => {
            ItemSelector::instance(ItemInstanceId::new(&args.item))
        }
        Resolution::Missing if args.item.bytes().all(|b| b.is_ascii_digit()) => {
            ItemSelector::instance(ItemInstanceId::new(&args.item))
        }
        Resolution::Missing => ItemSelector::semantic(&args.item)?,
    };
    let request = MoveRequest {
        membership: snapshot.metadata.membership_id,
        item,
        destination,
        intent: MoveIntentAuthorization::Explicit,
    };
    let plan = match mutations.plan_move(&request, PlanLifetime::MAX, policy, preflight_now) {
        Ok(plan) => plan,
        Err(error) => {
            let mut output = MoveOutput::refused(error);
            if error == MutationError::SameLocation {
                output.reason = Some(format!("item is already in {}", destination_view.label));
            }
            output.destination = Some(destination_view);
            return Ok(output);
        }
    };
    let item_view = match mutations.repository().lookup_instance(&plan.item)? {
        Resolution::Unique(instance) => match mutations
            .repository()
            .lookup_definition(instance.item_hash)?
        {
            Resolution::Unique(definition) => Some(MoveItem {
                name: definition.name,
                instance_id: instance.instance_id.as_str().into(),
                slot: definition.slot.map(|s| s.as_str().into()),
            }),
            _ => None,
        },
        _ => None,
    };
    let post_action_now = clock.now()?;
    let execution = mutations.execute_move_action(
        transfer,
        &request,
        &plan,
        policy,
        preflight_now,
        post_action_now,
    );
    let mut output = match execution {
        Ok(outcome) => {
            let observed = match mutations.repository().lookup_instance(&plan.item) {
                Ok(Resolution::Unique(instance)) => Some(instance),
                _ => None,
            };
            MoveOutput::from_outcome(&outcome, &snapshot.characters, observed.as_ref())
        }
        Err(failure) => MoveOutput::refused(failure.error())
            .with_evidence(failure.evidence(), &snapshot.characters),
    };
    output.item = item_view;
    output.destination = Some(destination_view);
    Ok(output)
}
