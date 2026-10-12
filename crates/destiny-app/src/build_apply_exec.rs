//! Executes a build apply plan with DIM's partial-failure semantics, adapted to
//! optimistic actions: an acknowledged step is applied to the local projection, nothing
//! is re-read, nothing is retried, and a failure only stops what depends on it.
//! DIM: each item moves in its own try/catch and the loop continues
//! (`loadout-apply.ts:540-577`); the bulk equip takes only the items that arrived
//! (`:546-552,579-626`); a failed grouped dequip fails the whole group (`:476-523`).
use crate::builds::BuildApplyPreview;
use destiny_core::build_apply::{BuildAction, BuildApplyPlan, BuildApplyStep};
use destiny_core::{
    DefinitionRepository, DestinyMembership, EquipGateway, EquipItemRequest, EquipItemStatus,
    EquipItemsGateway, EquipItemsOutcome, EquipItemsRequest, EquipOutcome, EquipRejection,
    ErrorCategory, InsertPlugGateway, InsertPlugOutcome, InsertPlugRequest, InstancedPullGateway,
    InstancedPullOutcome, InstancedPullRejection, InstancedPullRequest, InventoryEffect,
    InventoryRepository, InventorySnapshot, ItemInstance, ItemInstanceId, SyncRevision,
    TransferGateway, TransferOutcome, TransferRejection, TransferRequest, apply_inventory_effects,
};

/// The five action adapters a plan can need. Production composes the Bungie ones.
pub struct BuildGateways<T, E, B, P, S> {
    pub transfer: T,
    pub equip: E,
    pub equip_items: B,
    pub pull: P,
    pub socket: S,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepFailure {
    /// Definitive Bungie refusal (stable snake_case reason); the action did not apply.
    Rejected(&'static str),
    /// Per-item `EquipItems` status code; the item was not equipped.
    BungieCode(i32),
    /// Bungie omitted the item from a batch result; not evidence that it was equipped.
    NotReported,
    AuthenticationRefused,
    Transient,
    /// Dispatched with an unknown effect. Never retried, never projected.
    Uncertain,
    /// Refused locally or by the pre-request gate (for example the throttle fail-fast);
    /// nothing was sent.
    NotDispatched(ErrorCategory),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipCause {
    /// A step this one relies on did not succeed (index into the plan).
    DependencyFailed(usize),
    /// Bungie or the credentials refused everything; remaining steps were not sent.
    ProviderStopped,
    /// The plan was refused before the first step (stale or unreadable projection).
    PlanRefused,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemStatus {
    Succeeded,
    Failed(StepFailure),
    Skipped(SkipCause),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemOutcome {
    pub item: ItemInstanceId,
    pub status: ItemStatus,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StepReport {
    pub index: usize,
    pub step: BuildApplyStep,
    /// One entry per item: moves and pulls have one, equips one per equipped item.
    pub outcomes: Vec<ItemOutcome>,
}
impl StepReport {
    pub fn succeeded(&self) -> bool {
        self.outcomes
            .iter()
            .all(|o| o.status == ItemStatus::Succeeded)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyStatus {
    Applied,
    /// Nothing to do: the plan has no steps.
    NoOp,
    /// Some steps applied and some failed or were skipped.
    Partial,
    /// Steps were attempted and none applied.
    Failed,
    /// Nothing reached Bungie; the category exit applies.
    Refused(ErrorCategory),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyLocalState {
    Unchanged,
    Updated,
    /// Freshness was invalidated; the next ordinary read re-syncs.
    RefreshPending,
    /// Acknowledged effects could not be written to the projection.
    Failed,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanExecution {
    pub steps: Vec<StepReport>,
    pub status: ApplyStatus,
    pub local_state: ApplyLocalState,
    /// Requests that reached the provider (including uncertain ones).
    pub dispatched: usize,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuildApplyExecution {
    pub preview: BuildApplyPreview,
    pub execution: PlanExecution,
}

fn transfer_reason(r: TransferRejection) -> &'static str {
    match r {
        TransferRejection::Provider(r) => r.reason(),
        TransferRejection::NotTransferable => "not_transferable",
        TransferRejection::Equipped => "equipped",
        TransferRejection::DestinationFull => "destination_full",
        TransferRejection::InvalidRequest => "invalid_request",
    }
}
fn equip_reason(r: EquipRejection) -> &'static str {
    match r {
        EquipRejection::Provider(r) => r.reason(),
        EquipRejection::NotInSocialSpace => "not_in_social_space",
        EquipRejection::ExoticLimit => "exotic_limit",
        EquipRejection::Unequippable => "unequippable",
        EquipRejection::ItemNotFound => "item_not_found",
        EquipRejection::CharacterNotFound => "character_not_found",
        EquipRejection::InvalidRequest => "invalid_request",
    }
}
fn pull_reason(r: InstancedPullRejection) -> &'static str {
    match r {
        InstancedPullRejection::Provider(r) => r.reason(),
        InstancedPullRejection::InvalidRequest => "invalid_request",
        InstancedPullRejection::DestinationFull => "destination_full",
    }
}
/// Refusals that concern the whole session, not one item.
fn stops_everything(category: ErrorCategory) -> bool {
    matches!(
        category,
        ErrorCategory::ProviderRateLimit
            | ErrorCategory::ProviderUnavailable
            | ErrorCategory::ProviderAuthentication
            | ErrorCategory::ReauthorizationRequired
            | ErrorCategory::AuthorizationRequired
            | ErrorCategory::AuthorizationDenied
            | ErrorCategory::CredentialStoreUnavailable
    )
}
fn items_of(step: &BuildApplyStep) -> Vec<ItemInstanceId> {
    match &step.action {
        BuildAction::Move { item, .. } | BuildAction::PullFromPostmaster { item, .. } => {
            vec![item.clone()]
        }
        BuildAction::Equip { items, .. } => items.clone(),
        BuildAction::InsertPlug { item, .. } => vec![item.clone()],
    }
}
fn is_plug(step: &BuildApplyStep) -> bool {
    matches!(step.action, BuildAction::InsertPlug { .. })
}

/// What the dispatch of one step reported, per item that was sent.
type Sent = Vec<(ItemInstanceId, Result<(), StepFailure>)>;

struct Run<'a, R> {
    repository: &'a mut R,
    membership: DestinyMembership,
    projected: InventorySnapshot,
    dispatched: usize,
    persist_failed: bool,
    stopped: bool,
    /// A plug insertion was acknowledged; the projection cannot rebuild its effects.
    plugged: bool,
}
impl<R: InventoryRepository + DefinitionRepository> Run<'_, R> {
    fn instance(&self, id: &ItemInstanceId) -> Option<ItemInstance> {
        self.projected
            .items
            .iter()
            .find(|i| &i.instance_id == id)
            .cloned()
    }
    /// Applies one acknowledged effect exactly as the single actions do.
    fn persist(&mut self, effect: InventoryEffect) {
        match self
            .repository
            .apply_acknowledged_effects(&self.membership, std::slice::from_ref(&effect))
        {
            Ok(snapshot) => self.projected = snapshot,
            Err(_) => {
                self.persist_failed = true;
                if let Ok(result) = apply_inventory_effects(&self.projected, &[effect]) {
                    self.projected = result.snapshot;
                }
            }
        }
    }
    fn persist_equip(&mut self, item: &ItemInstanceId, character: &destiny_core::CharacterId) {
        let effect = self.instance(item).map(|instance| {
            crate::mutation::equip_effect(&*self.repository, &instance, character, &self.projected)
        });
        match effect {
            Some(Ok(effect)) => self.persist(effect),
            _ => self.persist_failed = true,
        }
    }
    fn refuse(&mut self, category: ErrorCategory) -> StepFailure {
        if stops_everything(category) {
            self.stopped = true;
        }
        StepFailure::NotDispatched(category)
    }
    fn dispatch(
        &mut self,
        step: &BuildApplyStep,
        ready: &[ItemInstanceId],
        planned: usize,
        gateways: &mut dyn Gateways,
    ) -> Sent {
        let one =
            |item: &ItemInstanceId, result: Result<(), StepFailure>| vec![(item.clone(), result)];
        match &step.action {
            BuildAction::Move { item, from, to } => {
                let Some(instance) = self.instance(item) else {
                    return one(
                        item,
                        Err(StepFailure::NotDispatched(ErrorCategory::StaleState)),
                    );
                };
                let request = TransferRequest::new(
                    self.membership.clone(),
                    item.clone(),
                    instance.item_hash,
                    instance.quantity,
                    from.clone(),
                    to.clone(),
                );
                let result = match request {
                    Err(e) => Err(StepFailure::NotDispatched(e.category())),
                    Ok(request) => gateways
                        .transfer(&request)
                        .map(|o| {
                            self.dispatched += 1;
                            match o {
                                TransferOutcome::Transferred => Ok(()),
                                TransferOutcome::Rejected(r) => {
                                    Err(StepFailure::Rejected(transfer_reason(r)))
                                }
                                TransferOutcome::AuthenticationRefused => {
                                    Err(StepFailure::AuthenticationRefused)
                                }
                                TransferOutcome::Transient => Err(StepFailure::Transient),
                                TransferOutcome::Uncertain => Err(StepFailure::Uncertain),
                            }
                        })
                        .unwrap_or_else(|e| Err(self.refuse(e.category()))),
                };
                if result.is_ok() {
                    self.persist(InventoryEffect::Relocate {
                        item: item.clone(),
                        to: to.clone(),
                    });
                }
                one(item, result)
            }
            BuildAction::PullFromPostmaster { item, character } => {
                let Some(instance) = self.instance(item) else {
                    return one(
                        item,
                        Err(StepFailure::NotDispatched(ErrorCategory::StaleState)),
                    );
                };
                let request = InstancedPullRequest::new(
                    self.membership.clone(),
                    item.clone(),
                    instance.item_hash,
                    character.clone(),
                );
                let result = match request {
                    Err(e) => Err(StepFailure::NotDispatched(e.category())),
                    Ok(request) => gateways
                        .pull(&request)
                        .map(|o| {
                            self.dispatched += 1;
                            match o {
                                InstancedPullOutcome::Acknowledged => Ok(()),
                                InstancedPullOutcome::Rejected(r) => {
                                    Err(StepFailure::Rejected(pull_reason(r)))
                                }
                                InstancedPullOutcome::AuthenticationRefused => {
                                    Err(StepFailure::AuthenticationRefused)
                                }
                                InstancedPullOutcome::Transient => Err(StepFailure::Transient),
                                InstancedPullOutcome::Uncertain => Err(StepFailure::Uncertain),
                            }
                        })
                        .unwrap_or_else(|e| Err(self.refuse(e.category()))),
                };
                if result.is_ok() {
                    self.persist(InventoryEffect::PullFromPostmaster {
                        item: item.clone(),
                        character: character.clone(),
                    });
                }
                one(item, result)
            }
            BuildAction::InsertPlug {
                item,
                character,
                socket_index,
                plug,
            } => {
                let result = match InsertPlugRequest::new(
                    self.membership.clone(),
                    item.clone(),
                    character.clone(),
                    *socket_index,
                    *plug,
                ) {
                    Err(e) => Err(StepFailure::NotDispatched(e.category())),
                    Ok(request) => gateways
                        .insert_plug(&request)
                        .map(|o| {
                            self.dispatched += 1;
                            match o {
                                InsertPlugOutcome::Inserted => Ok(()),
                                InsertPlugOutcome::Rejected(r) => {
                                    Err(StepFailure::Rejected(r.reason()))
                                }
                                InsertPlugOutcome::AuthenticationRefused => {
                                    Err(StepFailure::AuthenticationRefused)
                                }
                                InsertPlugOutcome::Transient => Err(StepFailure::Transient),
                                InsertPlugOutcome::Uncertain => Err(StepFailure::Uncertain),
                            }
                        })
                        .unwrap_or_else(|e| Err(self.refuse(e.category()))),
                };
                // DIM swaps in the item Bungie returns (`advanced-write-actions.ts:243-258`).
                // The projection derives perks, stats and energy from component data, so
                // it is invalidated rather than patched with a guess.
                self.plugged |= result.is_ok();
                one(item, result)
            }
            BuildAction::Equip { character, .. } => {
                // DIM bulk-equips only when several items were planned; a lone equip is
                // inline (`loadout-apply.ts:424-433,526-540`).
                let sent: Sent = if planned == 1 {
                    let item = &ready[0];
                    let result = match EquipItemRequest::new(
                        self.membership.clone(),
                        item.clone(),
                        character.clone(),
                    ) {
                        Err(e) => Err(StepFailure::NotDispatched(e.category())),
                        Ok(request) => gateways
                            .equip(&request)
                            .map(|o| {
                                self.dispatched += 1;
                                match o {
                                    EquipOutcome::Equipped => Ok(()),
                                    EquipOutcome::Rejected(r) => {
                                        Err(StepFailure::Rejected(equip_reason(r)))
                                    }
                                    EquipOutcome::AuthenticationRefused => {
                                        Err(StepFailure::AuthenticationRefused)
                                    }
                                    EquipOutcome::Transient => Err(StepFailure::Transient),
                                    EquipOutcome::Uncertain => Err(StepFailure::Uncertain),
                                }
                            })
                            .unwrap_or_else(|e| Err(self.refuse(e.category()))),
                    };
                    one(item, result)
                } else {
                    self.equip_many(character, ready, gateways)
                };
                for (item, result) in &sent {
                    if result.is_ok() {
                        self.persist_equip(item, character);
                    }
                }
                sent
            }
        }
    }
    /// DIM sends one `equipItems` request with every item and never chunks
    /// (`destiny2-api.ts:229-246`); a build holds one item per slot, so that stays small.
    /// The request type caps a request at `MAX_EQUIP_ITEMS`, so a larger group is split
    /// in plan order. Items of a chunk that was never sent are left out of the result
    /// and reported as `provider_stopped`.
    fn equip_many(
        &mut self,
        character: &destiny_core::CharacterId,
        ready: &[ItemInstanceId],
        gateways: &mut dyn Gateways,
    ) -> Sent {
        let mut sent = Sent::new();
        for chunk in ready.chunks(destiny_core::MAX_EQUIP_ITEMS) {
            if self.stopped {
                break;
            }
            sent.extend(self.equip_chunk(character, chunk, gateways));
        }
        sent
    }
    fn equip_chunk(
        &mut self,
        character: &destiny_core::CharacterId,
        ready: &[ItemInstanceId],
        gateways: &mut dyn Gateways,
    ) -> Sent {
        let all = |failure: StepFailure| {
            ready
                .iter()
                .map(|i| (i.clone(), Err(failure.clone())))
                .collect::<Sent>()
        };
        let request = match EquipItemsRequest::new(
            self.membership.clone(),
            character.clone(),
            ready.to_vec(),
        ) {
            Ok(request) => request,
            Err(e) => return all(StepFailure::NotDispatched(e.category())),
        };
        let outcome = match gateways.equip_items(&request) {
            Ok(outcome) => outcome,
            Err(e) => return all(self.refuse(e.category())),
        };
        self.dispatched += 1;
        match outcome {
            EquipItemsOutcome::Acknowledged(results) => ready
                .iter()
                .map(|item| {
                    let status = results.iter().find(|r| &r.item == item).map(|r| r.status);
                    let result = match status {
                        Some(EquipItemStatus::Success) => Ok(()),
                        Some(EquipItemStatus::Failure(code)) => Err(StepFailure::BungieCode(code)),
                        _ => Err(StepFailure::NotReported),
                    };
                    (item.clone(), result)
                })
                .collect(),
            EquipItemsOutcome::Rejected(r) => all(StepFailure::Rejected(r.reason())),
            EquipItemsOutcome::Refused => all(StepFailure::Rejected("refused")),
            EquipItemsOutcome::RestrictedLocation => {
                all(StepFailure::Rejected("not_in_social_space"))
            }
            EquipItemsOutcome::AuthenticationRefused => all(StepFailure::AuthenticationRefused),
            EquipItemsOutcome::Transient => all(StepFailure::Transient),
            EquipItemsOutcome::Uncertain => all(StepFailure::Uncertain),
        }
    }
}

/// Object-safe view of [`BuildGateways`] so the run loop is not generic over four adapters.
trait Gateways {
    fn transfer(&mut self, r: &TransferRequest) -> Result<TransferOutcome, destiny_core::AppError>;
    fn equip(&mut self, r: &EquipItemRequest) -> Result<EquipOutcome, destiny_core::AppError>;
    fn equip_items(
        &mut self,
        r: &EquipItemsRequest,
    ) -> Result<EquipItemsOutcome, destiny_core::AppError>;
    fn pull(
        &mut self,
        r: &InstancedPullRequest,
    ) -> Result<InstancedPullOutcome, destiny_core::AppError>;
    fn insert_plug(
        &mut self,
        r: &InsertPlugRequest,
    ) -> Result<InsertPlugOutcome, destiny_core::AppError>;
}
struct Paced<'a, T, E, B, P, S> {
    gateways: &'a mut BuildGateways<T, E, B, P, S>,
    pace: &'a mut dyn FnMut(),
}
impl<
    T: TransferGateway,
    E: EquipGateway,
    B: EquipItemsGateway,
    P: InstancedPullGateway,
    S: InsertPlugGateway,
> Gateways for Paced<'_, T, E, B, P, S>
{
    fn transfer(&mut self, r: &TransferRequest) -> Result<TransferOutcome, destiny_core::AppError> {
        (self.pace)();
        self.gateways.transfer.transfer(r)
    }
    fn equip(&mut self, r: &EquipItemRequest) -> Result<EquipOutcome, destiny_core::AppError> {
        (self.pace)();
        self.gateways.equip.equip(r)
    }
    fn equip_items(
        &mut self,
        r: &EquipItemsRequest,
    ) -> Result<EquipItemsOutcome, destiny_core::AppError> {
        (self.pace)();
        self.gateways.equip_items.equip_items(r)
    }
    fn pull(
        &mut self,
        r: &InstancedPullRequest,
    ) -> Result<InstancedPullOutcome, destiny_core::AppError> {
        (self.pace)();
        self.gateways.pull.pull(r)
    }
    fn insert_plug(
        &mut self,
        r: &InsertPlugRequest,
    ) -> Result<InsertPlugOutcome, destiny_core::AppError> {
        (self.pace)();
        self.gateways.socket.insert_plug(r)
    }
}

fn refused(plan: &BuildApplyPlan, category: ErrorCategory) -> PlanExecution {
    PlanExecution {
        steps: plan
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| StepReport {
                index,
                step: step.clone(),
                outcomes: items_of(step)
                    .into_iter()
                    .map(|item| ItemOutcome {
                        item,
                        status: ItemStatus::Skipped(SkipCause::PlanRefused),
                    })
                    .collect(),
            })
            .collect(),
        status: ApplyStatus::Refused(category),
        local_state: ApplyLocalState::Unchanged,
        dispatched: 0,
    }
}

/// Runs `plan` in order against the projection it was planned from. `pace` runs before
/// every request; adapters own the throttle (waits above 30 s fail before dispatch).
pub fn execute_plan<R, T, E, B, P, S>(
    repository: &mut R,
    plan: &BuildApplyPlan,
    expected_revision: SyncRevision,
    gateways: &mut BuildGateways<T, E, B, P, S>,
    pace: &mut dyn FnMut(),
) -> PlanExecution
where
    R: InventoryRepository + DefinitionRepository,
    T: TransferGateway,
    E: EquipGateway,
    B: EquipItemsGateway,
    P: InstancedPullGateway,
    S: InsertPlugGateway,
{
    if plan.steps.is_empty() {
        return PlanExecution {
            steps: vec![],
            status: ApplyStatus::NoOp,
            local_state: ApplyLocalState::Unchanged,
            dispatched: 0,
        };
    }
    let snapshot = match repository.load_current() {
        Ok(Some(snapshot)) => snapshot,
        _ => return refused(plan, ErrorCategory::LocalStorage),
    };
    // The plan is only valid for the projection it was computed from.
    if snapshot.metadata.revision != expected_revision {
        return refused(plan, ErrorCategory::StaleState);
    }
    let Ok(membership) = DestinyMembership::new(
        snapshot.metadata.membership_type,
        snapshot.metadata.membership_id.clone(),
    ) else {
        return refused(plan, ErrorCategory::LocalStorage);
    };
    let mut run = Run {
        repository,
        membership,
        projected: snapshot,
        dispatched: 0,
        persist_failed: false,
        stopped: false,
        plugged: false,
    };
    let mut gateways = Paced { gateways, pace };
    let mut reports: Vec<StepReport> = Vec::with_capacity(plan.steps.len());
    for (index, step) in plan.steps.iter().enumerate() {
        let items = items_of(step);
        let skip = |cause| {
            items
                .iter()
                .map(|item| ItemOutcome {
                    item: item.clone(),
                    status: ItemStatus::Skipped(cause),
                })
                .collect::<Vec<_>>()
        };
        let hard = step
            .needs
            .iter()
            .find(|n| !reports[**n].succeeded())
            .copied();
        let outcomes = if run.stopped {
            skip(SkipCause::ProviderStopped)
        } else if let Some(dependency) = hard {
            skip(SkipCause::DependencyFailed(dependency))
        } else {
            // Per item: the last earlier step that touched it must have applied. Plug
            // insertions only wait on their `needs`: DIM carries on after a failed plug.
            let blocked: Vec<Option<usize>> = items
                .iter()
                .map(|item| {
                    if is_plug(step) {
                        return None;
                    }
                    reports
                        .iter()
                        .rev()
                        .filter(|r| !is_plug(&r.step))
                        .find(|r| r.outcomes.iter().any(|o| &o.item == item))
                        .filter(|r| {
                            r.outcomes
                                .iter()
                                .any(|o| &o.item == item && o.status != ItemStatus::Succeeded)
                        })
                        .map(|r| r.index)
                })
                .collect();
            let ready: Vec<_> = items
                .iter()
                .zip(&blocked)
                .filter(|(_, b)| b.is_none())
                .map(|(i, _)| i.clone())
                .collect();
            let mut sent = if ready.is_empty() {
                vec![]
            } else {
                run.dispatch(step, &ready, items.len(), &mut gateways)
            };
            items
                .iter()
                .zip(&blocked)
                .map(|(item, blocked)| ItemOutcome {
                    item: item.clone(),
                    status: match blocked {
                        Some(dependency) => {
                            ItemStatus::Skipped(SkipCause::DependencyFailed(*dependency))
                        }
                        None => match sent.iter().position(|(i, _)| i == item) {
                            Some(at) => match sent.remove(at).1 {
                                Ok(()) => ItemStatus::Succeeded,
                                Err(failure) => ItemStatus::Failed(failure),
                            },
                            None => ItemStatus::Skipped(SkipCause::ProviderStopped),
                        },
                    },
                })
                .collect()
        };
        reports.push(StepReport {
            index,
            step: step.clone(),
            outcomes,
        });
    }
    let all_ok = reports.iter().all(StepReport::succeeded);
    let dispatched = run.dispatched;
    let mut persist_failed = run.persist_failed;
    let plugged = run.plugged;
    if dispatched > 0 && (!all_ok || persist_failed || plugged) {
        persist_failed |= run.repository.invalidate_freshness().is_err();
    }
    let first_refusal = reports
        .iter()
        .flat_map(|r| &r.outcomes)
        .find_map(|o| match o.status {
            ItemStatus::Failed(StepFailure::NotDispatched(category)) => Some(category),
            _ => None,
        });
    let status = if all_ok {
        ApplyStatus::Applied
    } else if let (0, Some(category)) = (dispatched, first_refusal) {
        ApplyStatus::Refused(category)
    } else if reports
        .iter()
        .flat_map(|r| &r.outcomes)
        .any(|o| o.status == ItemStatus::Succeeded)
    {
        ApplyStatus::Partial
    } else {
        ApplyStatus::Failed
    };
    let local_state = if persist_failed {
        ApplyLocalState::Failed
    } else if dispatched == 0 {
        ApplyLocalState::Unchanged
    } else if all_ok && !plugged {
        ApplyLocalState::Updated
    } else {
        ApplyLocalState::RefreshPending
    };
    PlanExecution {
        steps: reports,
        status,
        local_state,
        dispatched,
    }
}
