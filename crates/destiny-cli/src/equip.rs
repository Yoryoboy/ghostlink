//! Single-intent CLI equip orchestration over the shared mutation service.
use crate::command::{CliError, EquipArgs};
use crate::equip_output::{EquipCharacter, EquipItem, EquipOutput, location_view};
use destiny_app::{
    Clock, DefinitionRepository, DestinationSelector, EquipGateway, EquipRequest, FreshnessPolicy,
    InventoryRepository, ItemInstanceId, ItemSelector, MoveIntentAuthorization, MutationError,
    MutationService, PlanId, PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncReason,
    Resolution, SearchRepository, TransferGateway,
};

/// An invocation creates only one plan; the id is internal and never persisted.
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
        Ok(PlanId::new("cli-equip"))
    }
}

/// Offline-testable adapter boundary. Fresh preflight, dispatch, refresh and
/// equipped-state verification remain entirely application-owned; no retries.
pub fn equip_item<G, R, P, T, E, C>(
    args: &EquipArgs,
    mutations: &mut MutationService<G, R, P>,
    transfer: &mut T,
    equip: &mut E,
    clock: &C,
    policy: FreshnessPolicy,
) -> Result<EquipOutput, CliError>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    P: PlanIdGenerator,
    T: TransferGateway,
    E: EquipGateway,
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
    // Existing CLI convention: exact opaque id wins over a class label. Never
    // select a default character for a mutation, or rank duplicate classes.
    let exact: Vec<_> = snapshot
        .characters
        .iter()
        .filter(|c| c.id.as_str() == args.character)
        .collect();
    let matches = if exact.is_empty() {
        snapshot
            .characters
            .iter()
            .filter(|c| {
                c.class
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&args.character))
            })
            .collect::<Vec<_>>()
    } else {
        exact
    };
    let character = match matches.as_slice() {
        [] => return Ok(EquipOutput::refused(MutationError::MissingDestination)),
        [character] => *character,
        _ => return Ok(EquipOutput::refused(MutationError::AmbiguousDestination)),
    };
    let character_view = EquipCharacter {
        character_id: character.id.as_str().into(),
        label: location_view(
            &destiny_app::ItemLocation::Character(character.id.clone()),
            &snapshot.characters,
        )
        .label,
    };
    // Concrete identities take precedence; otherwise use the exact shared
    // semantic selector, whose bounded resolver fails closed on ambiguity.
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
    let request = EquipRequest {
        membership: snapshot.metadata.membership_id,
        item,
        destination: DestinationSelector::character(character.id.clone()),
        intent: MoveIntentAuthorization::Explicit,
    };
    let plan = match mutations.plan_equip(&request, PlanLifetime::MAX, policy, preflight_now) {
        Ok(plan) => plan,
        Err(error) => {
            let mut output = EquipOutput::refused(error);
            output.character = Some(character_view);
            return Ok(output);
        }
    };
    let item_view = match mutations
        .repository()
        .lookup_instance(&plan.mutation_plan().item)?
    {
        Resolution::Unique(instance) => match mutations
            .repository()
            .lookup_definition(instance.item_hash)?
        {
            Resolution::Unique(definition) => {
                let slot = definition
                    .slot
                    .or_else(|| {
                        definition
                            .bucket_hash
                            .and_then(destiny_app::ItemSlot::from_equipment_bucket)
                    })
                    .map(|s| {
                        let value = s.as_str();
                        format!("{}{}", value[..1].to_ascii_uppercase(), &value[1..])
                    });
                Some(EquipItem {
                    name: definition.name,
                    instance_id: instance.instance_id.as_str().into(),
                    slot,
                })
            }
            _ => None,
        },
        _ => None,
    };
    // Match the existing MCP move composition's timestamp policy. The shared
    // executor invalidates local freshness after dispatch; it does not re-read.
    let post_action_now = clock.now()?;
    let execution = mutations.execute_equip_action(
        (transfer, equip),
        &request,
        &plan,
        policy,
        preflight_now,
        post_action_now,
    );
    let mut output = match execution {
        Ok(outcome) => {
            // A failed refresh must never expose the stale projection as fresh.
            let observed = match mutations
                .repository()
                .lookup_instance(&plan.mutation_plan().item)
            {
                Ok(Resolution::Unique(i)) => Some(i),
                _ => None,
            };
            EquipOutput::from_outcome(&outcome, &snapshot.characters, observed.as_ref())
        }
        Err(failure) => EquipOutput::refused(failure.error())
            .with_evidence(failure.evidence(), &snapshot.characters),
    };
    output.item = item_view;
    output.character = Some(character_view);
    Ok(output)
}
