//! Local, consumed-plan non-instanced Postmaster pulls.
//! Acknowledgement is success, not a quantity observation or atomic guarantee.
use crate::ProfileSyncService;
use crate::instanced_pull::PullDisposition;
use destiny_core::*;

#[derive(Debug, Eq, PartialEq)]
pub struct StackPullPlanRequest {
    pub membership: DestinyMembership,
    pub owner: CharacterId,
    pub item_hash: ItemHash,
    pub amount: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullPlanningRefusal {
    MembershipMismatch,
    OwnerNotOwned,
    DefinitionMissing,
    DefinitionAmbiguous,
    DefinitionVersionMismatch,
    SideEffects,
    SideEffectsUnknown,
    Instanced,
    InstanceKindUnknown,
    MissingBucket,
    UnsupportedBucket,
    PolicyVersionMismatch,
    PolicyArtifactMismatch,
    Capacity(StackPullRefusal),
}
#[derive(Debug, Eq, PartialEq)]
pub enum StackPullPlanningError {
    Refused(StackPullPlanningRefusal),
    Infrastructure(AppError),
}
impl From<AppError> for StackPullPlanningError {
    fn from(e: AppError) -> Self {
        Self::Infrastructure(e)
    }
}
impl From<StackPullPlanningRefusal> for StackPullPlanningError {
    fn from(e: StackPullPlanningRefusal) -> Self {
        Self::Refused(e)
    }
}
/// Opaque intent; no Clone, public constructor, ticket or reminting API.
/// ```compile_fail
/// use destiny_app::StackPullPlan;
/// fn duplicate(p: &StackPullPlan) -> StackPullPlan { p.clone() }
/// ```
/// ```compile_fail
/// use destiny_app::StackPullPlan;
/// fn consume(_: StackPullPlan) {}
/// fn replay(p: StackPullPlan) { consume(p); consume(p); }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullPlan {
    id: PlanId,
    created_at: Timestamp,
    expires_at: Timestamp,
    context: StackPullContext,
}
impl StackPullPlan {
    pub fn id(&self) -> &PlanId {
        &self.id
    }
    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }
    pub fn context(&self) -> &StackPullContext {
        &self.context
    }
}
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullContext {
    request: StackPullRequest,
    destination_bucket: BucketHash,
    definition_version: DefinitionVersion,
    revision: SyncRevision,
    max_stack_size: i32,
    pre_destination_quantity: u64,
    pre_profile_quantity: u64,
    pre_postmaster_quantity: u64,
}
impl StackPullContext {
    pub fn request(&self) -> &StackPullRequest {
        &self.request
    }
    pub fn destination_bucket(&self) -> BucketHash {
        self.destination_bucket
    }
    pub fn definition_version(&self) -> &DefinitionVersion {
        &self.definition_version
    }
    pub fn revision(&self) -> SyncRevision {
        self.revision
    }
    pub fn pre_destination_quantity(&self) -> u64 {
        self.pre_destination_quantity
    }
    /// Same-hash quantity across the whole ProfileInventory before dispatch.
    pub fn pre_profile_quantity(&self) -> u64 {
        self.pre_profile_quantity
    }
    pub fn pre_postmaster_quantity(&self) -> u64 {
        self.pre_postmaster_quantity
    }
}
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullAttempt {
    context: StackPullContext,
    preflight_at: Timestamp,
    dispatch_at: Timestamp,
    outcome: Result<StackPullOutcome, AppError>,
}
impl StackPullAttempt {
    pub fn context(&self) -> &StackPullContext {
        &self.context
    }
    pub fn preflight_at(&self) -> Timestamp {
        self.preflight_at
    }
    pub fn dispatch_at(&self) -> Timestamp {
        self.dispatch_at
    }
    pub fn gateway_result(&self) -> &Result<StackPullOutcome, AppError> {
        &self.outcome
    }
}
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullExecutionOutcome {
    plan_id: PlanId,
    dispatch: StackPullAttempt,
    local_state_error: Option<AppError>,
    disposition: PullDisposition,
}
impl StackPullExecutionOutcome {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn dispatch(&self) -> &StackPullAttempt {
        &self.dispatch
    }
    pub fn context(&self) -> &StackPullContext {
        &self.dispatch.context
    }
    pub fn gateway_outcome(&self) -> &StackPullOutcome {
        self.dispatch
            .outcome
            .as_ref()
            .expect("received outcome required")
    }
    pub fn local_state_error(&self) -> Option<&AppError> {
        self.local_state_error.as_ref()
    }
    pub fn disposition(&self) -> PullDisposition {
        self.disposition
    }
}
#[derive(Debug, Eq, PartialEq)]
pub enum StackPullPreDispatchError {
    Eligibility(StackPullPlanningError),
    Expired,
    BackwardsClock,
    IdentityDrift,
    DefinitionVersionDrift,
    RevisionDrift,
    FingerprintDrift,
}
impl From<AppError> for StackPullPreDispatchError {
    fn from(e: AppError) -> Self {
        Self::Eligibility(e.into())
    }
}
impl From<StackPullPlanningError> for StackPullPreDispatchError {
    fn from(e: StackPullPlanningError) -> Self {
        Self::Eligibility(e)
    }
}
#[derive(Debug, Eq, PartialEq)]
pub enum StackPullExecutionFailureReason {
    PreDispatch(StackPullPreDispatchError),
    Gateway(Box<StackPullAttempt>),
}
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullExecutionFailure {
    plan_id: PlanId,
    reason: StackPullExecutionFailureReason,
}
impl StackPullExecutionFailure {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn reason(&self) -> &StackPullExecutionFailureReason {
        &self.reason
    }
}
struct Eligibility {
    context: StackPullContext,
}

/// Provider-neutral ports and injected audited policy. Callers must serialize
/// local publication/definition use through execution, as for instanced pulls.
pub struct StackPullService<G, R, D, M, C, P> {
    sync: ProfileSyncService<G, R>,
    definitions: D,
    // Retained for constructor compatibility; stack pulls never fetch it.
    #[allow(dead_code)]
    manifest: M,
    policy: StackBucketCapacityPolicy,
    clock: C,
    plan_ids: P,
}
impl<
    G: ProfileGateway,
    R: InventoryRepository,
    D: DefinitionRepository,
    M: ManifestGateway,
    C: Clock,
    P: PlanIdGenerator,
> StackPullService<G, R, D, M, C, P>
{
    pub fn new(
        sync: ProfileSyncService<G, R>,
        definitions: D,
        manifest: M,
        policy: StackBucketCapacityPolicy,
        clock: C,
        plan_ids: P,
    ) -> Self {
        Self {
            sync,
            definitions,
            manifest,
            policy,
            clock,
            plan_ids,
        }
    }
    /// Diagnostic-only versions; never acquire or mutate inventory for presentation.
    pub fn definition_versions(&self) -> Option<(DefinitionVersion, DefinitionVersion)> {
        let snapshot = self.sync.repository().load_current().ok()??;
        let stored = self.definitions.current_definition_version().ok()?;
        Some((snapshot.metadata.definition_version, stored))
    }

    /// Resolve an explicit CLI owner using the same committed snapshot as planning.
    pub fn resolve_owner(
        &mut self,
        selector: &CharacterId,
        freshness: FreshnessPolicy,
    ) -> Result<CharacterId, AppError> {
        let now = self.clock.now()?;
        let snapshot = self.committed_snapshot(freshness, now)?;
        crate::weekly_selection::select_weekly_character(&snapshot.characters, Some(selector))
            .map(|character| character.id)
            .map_err(|error| {
                AppError::new(match error {
                    crate::weekly_selection::WeeklySelectionError::InvalidRequest => {
                        ErrorCategory::ProviderValidation
                    }
                    crate::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                        ErrorCategory::AmbiguousMatch
                    }
                })
            })
    }

    fn committed_snapshot(
        &mut self,
        freshness: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<InventorySnapshot, AppError> {
        self.sync
            .sync(ProfileSyncReason::Ordinary, freshness, now)?;
        self.sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))
    }
    /// Ordinary sync reuses fresh committed stacks; no Manifest capacity fetch.
    pub fn plan(
        &mut self,
        request: &StackPullPlanRequest,
        lifetime: PlanLifetime,
        freshness: FreshnessPolicy,
    ) -> Result<StackPullPlan, StackPullPlanningError> {
        let now = self.clock.now()?;
        let snapshot = self.committed_snapshot(freshness, now)?;
        let wire = StackPullRequest::new(
            request.membership.clone(),
            request.item_hash,
            request.owner.clone(),
            request.amount,
        )?;
        let eligible = self.eligibility(wire, &snapshot)?;
        Ok(StackPullPlan {
            id: self.plan_ids.next_plan_id()?,
            created_at: now,
            expires_at: lifetime.expires_at(now)?,
            context: eligible.context,
        })
    }
    fn eligibility(
        &self,
        request: StackPullRequest,
        snapshot: &InventorySnapshot,
    ) -> Result<Eligibility, StackPullPlanningError> {
        use StackPullPlanningRefusal as Refusal;
        if snapshot.metadata.membership_id != *request.membership().membership_id()
            || snapshot.metadata.membership_type != request.membership().membership_type()
        {
            return Err(Refusal::MembershipMismatch.into());
        }
        if !snapshot
            .characters
            .iter()
            .any(|c| &c.id == request.character())
        {
            return Err(Refusal::OwnerNotOwned.into());
        }
        let d = match self.definitions.lookup_definition(request.item_hash())? {
            Resolution::Unique(d) => d,
            Resolution::Missing => return Err(Refusal::DefinitionMissing.into()),
            Resolution::Ambiguous(_) => return Err(Refusal::DefinitionAmbiguous.into()),
        };
        if self.definitions.current_definition_version()? != snapshot.metadata.definition_version
            || d.version != snapshot.metadata.definition_version
        {
            return Err(Refusal::DefinitionVersionMismatch.into());
        }
        match d.does_postmaster_pull_have_side_effects {
            Some(false) => {}
            Some(true) => return Err(Refusal::SideEffects.into()),
            None => return Err(Refusal::SideEffectsUnknown.into()),
        }
        match d.is_instance_item {
            Some(false) => {}
            Some(true) => return Err(Refusal::Instanced.into()),
            None => return Err(Refusal::InstanceKindUnknown.into()),
        }
        let bucket = d.bucket_hash.ok_or(Refusal::MissingBucket)?;
        if self.policy.capacity(bucket).is_none() {
            return Err(Refusal::UnsupportedBucket.into());
        }
        let mut capacity = self
            .policy
            .derive(bucket, request.item_hash(), &snapshot.stacks)
            .map_err(Refusal::Capacity)?;
        capacity.max_stack_size = d.max_stack_size;
        // The instance kind is known here, so the inventory block was decoded:
        // an absent label (most live stacks) means "not unique", as in DIM.
        capacity.unique_stack = Some(d.stack_unique_label.as_ref().is_some_and(|s| !s.is_empty()));
        capacity.side_effects = d.does_postmaster_pull_have_side_effects;
        if let StackPullDecision::Refused(reason) =
            evaluate_stack_pull(&request, &snapshot.stacks, &capacity)
        {
            return Err(Refusal::Capacity(reason).into());
        }
        let (destination, profile, postmaster) = totals(&request, bucket, &snapshot.stacks);
        let destination =
            destination.ok_or(Refusal::Capacity(StackPullRefusal::ArithmeticOverflow))?;
        let profile = profile.ok_or(Refusal::Capacity(StackPullRefusal::ArithmeticOverflow))?;
        let postmaster =
            postmaster.ok_or(Refusal::Capacity(StackPullRefusal::ArithmeticOverflow))?;
        // Check verification arithmetic before dispatch, not only after application.
        profile
            .checked_add(request.quantity() as u64)
            .ok_or(Refusal::Capacity(StackPullRefusal::ArithmeticOverflow))?;
        Ok(Eligibility {
            context: StackPullContext {
                request,
                destination_bucket: bucket,
                definition_version: snapshot.metadata.definition_version.clone(),
                revision: snapshot.metadata.revision,
                max_stack_size: d
                    .max_stack_size
                    .ok_or(Refusal::Capacity(StackPullRefusal::UnknownMaxStackSize))?,
                pre_destination_quantity: destination,
                pre_profile_quantity: profile,
                pre_postmaster_quantity: postmaster,
            },
        })
    }
    /// Consume intent, recheck committed eligibility and revision, dispatch once.
    /// Acknowledgements persist exactly once without invalidation; incomplete
    /// receipts or persistence failures invalidate, retaining the receipt.
    /// No post-action read or retry occurs.
    pub fn execute<T: StackPullGateway>(
        &mut self,
        plan: StackPullPlan,
        gateway: &mut T,
        freshness: FreshnessPolicy,
    ) -> Result<StackPullExecutionOutcome, StackPullExecutionFailure> {
        let plan_id = plan.id.clone();
        let dispatch = self
            .preflight_dispatch(plan, gateway, freshness)
            .map_err(|e| StackPullExecutionFailure {
                plan_id: plan_id.clone(),
                reason: StackPullExecutionFailureReason::PreDispatch(e),
            })?;
        let receipt = match &dispatch.outcome {
            Ok(receipt) => receipt,
            Err(_) => {
                return Err(StackPullExecutionFailure {
                    plan_id,
                    reason: StackPullExecutionFailureReason::Gateway(Box::new(dispatch)),
                });
            }
        };
        let refused = matches!(
            receipt,
            StackPullOutcome::Rejected(_)
                | StackPullOutcome::AuthenticationRefused
                | StackPullOutcome::Transient
        );
        let acknowledged = matches!(receipt, StackPullOutcome::Acknowledged);
        let context = &dispatch.context;
        let persistence = if acknowledged {
            self.sync
                .repository_mut()
                .apply_acknowledged_effects(
                    context.request.membership(),
                    &[InventoryEffect::PullStack {
                        item_hash: context.request.item_hash(),
                        owner: context.request.character().clone(),
                        quantity: context.request.quantity() as u32,
                        destination_bucket: context.destination_bucket,
                        max_stack_size: context.max_stack_size,
                    }],
                )
                .map(|_| ())
        } else {
            Ok(())
        };
        let local_state_error = if !acknowledged || persistence.is_err() {
            let invalidation = self.sync.repository_mut().invalidate_freshness();
            persistence.and(invalidation).err()
        } else {
            None
        };
        let disposition = if local_state_error.is_some() {
            PullDisposition::LocalStateFailed
        } else if refused {
            PullDisposition::Refused
        } else if acknowledged {
            PullDisposition::Applied
        } else {
            PullDisposition::Uncertain
        };
        Ok(StackPullExecutionOutcome {
            plan_id,
            dispatch,
            local_state_error,
            disposition,
        })
    }
    fn preflight_dispatch<T: StackPullGateway>(
        &mut self,
        plan: StackPullPlan,
        gateway: &mut T,
        freshness: FreshnessPolicy,
    ) -> Result<StackPullAttempt, StackPullPreDispatchError> {
        use StackPullPreDispatchError as Error;
        let preflight_at = self.clock.now()?;
        check_time(&plan, preflight_at, plan.created_at)?;
        let snapshot = self.committed_snapshot(freshness, preflight_at)?;
        let eligible = self.eligibility(plan.context.request.clone(), &snapshot)?;
        let context = eligible.context;
        if context.request != plan.context.request {
            return Err(Error::IdentityDrift);
        }
        if context.definition_version != plan.context.definition_version {
            return Err(Error::DefinitionVersionDrift);
        }
        if context.revision != plan.context.revision {
            return Err(Error::RevisionDrift);
        }
        if context.destination_bucket != plan.context.destination_bucket
            || context.max_stack_size != plan.context.max_stack_size
        {
            return Err(Error::IdentityDrift);
        }
        let metadata = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?
            .metadata;
        if metadata.membership_id != *context.request.membership().membership_id()
            || metadata.membership_type != context.request.membership().membership_type()
        {
            return Err(Error::IdentityDrift);
        }
        if metadata.revision != context.revision {
            return Err(Error::RevisionDrift);
        }
        if metadata.definition_version != context.definition_version
            || self.definitions.current_definition_version()? != context.definition_version
        {
            return Err(Error::DefinitionVersionDrift);
        }
        let dispatch_at = self.clock.now()?;
        check_time(&plan, dispatch_at, preflight_at)?;
        let outcome = gateway.pull(&context.request);
        Ok(StackPullAttempt {
            context,
            preflight_at,
            dispatch_at,
            outcome,
        })
    }
}
fn check_time(
    plan: &StackPullPlan,
    now: Timestamp,
    previous: Timestamp,
) -> Result<(), StackPullPreDispatchError> {
    if now < previous {
        return Err(StackPullPreDispatchError::BackwardsClock);
    }
    if now >= plan.expires_at {
        return Err(StackPullPreDispatchError::Expired);
    }
    Ok(())
}
/// Same-hash totals: destination bucket, whole ProfileInventory, owner Postmaster.
///
/// The whole-profile total guards against a same-hash change in another
/// profile bucket masking an inexact arrival in the destination bucket.
fn totals(
    request: &StackPullRequest,
    bucket: BucketHash,
    rows: &[StackObservation],
) -> (Option<u64>, Option<u64>, Option<u64>) {
    let sum = |container: &StackContainer, qualified_bucket: Option<BucketHash>| {
        rows.iter()
            .filter(|r| {
                r.item_hash() == request.item_hash()
                    && r.container() == container
                    && qualified_bucket.is_none_or(|b| r.bucket_hash() == b)
            })
            .try_fold(0u64, |total, r| total.checked_add(r.quantity() as u64))
    };
    (
        sum(&StackContainer::ProfileInventory, Some(bucket)),
        sum(&StackContainer::ProfileInventory, None),
        sum(
            &StackContainer::Postmaster(request.character().clone()),
            None,
        ),
    )
}
#[cfg(test)]
mod tests;
