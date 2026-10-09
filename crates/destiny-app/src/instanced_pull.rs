//! Local instanced Postmaster planning and consumed-plan optimistic execution.
//! Acknowledgement is success, not a post-action observation or atomic guarantee.

use crate::ProfileSyncService;
use destiny_core::{
    AppError, BucketHash, CapacityRefusal, CharacterId, Clock, DefinitionRepository,
    DefinitionVersion, DestinyMembership, EquipmentCapacityPolicy, ErrorCategory, FreshnessPolicy,
    InstancedPullGateway, InstancedPullOutcome, InstancedPullRequest, InventoryRepository,
    InventorySnapshot, ItemHash, ItemInstanceId, ItemLocation, ManifestGateway, PlanId,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncReason, Resolution, SyncRevision,
    Timestamp,
};

/// Select one genuine instance. Destination is its committed Postmaster owner.
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullPlanRequest {
    pub membership: DestinyMembership,
    pub item: ItemInstanceId,
}

/// Eligibility refusals carry no raw provider data or account identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstancedPullRefusal {
    MembershipMismatch,
    TargetMissing,
    TargetAmbiguous,
    NotInPostmaster,
    OwnerNotOwned,
    DefinitionMissing,
    DefinitionAmbiguous,
    DefinitionVersionMismatch,
    SideEffects,
    SideEffectsUnknown,
    NonInstanced,
    InstanceKindUnknown,
    MissingBucket,
    UnsupportedBucket,
    PolicyVersionMismatch,
    PolicyArtifactMismatch,
    Capacity(CapacityRefusal),
}

#[derive(Debug, Eq, PartialEq)]
pub enum InstancedPullPlanningError {
    Refused(InstancedPullRefusal),
    Infrastructure(AppError),
}
impl From<AppError> for InstancedPullPlanningError {
    fn from(error: AppError) -> Self {
        Self::Infrastructure(error)
    }
}
impl From<InstancedPullRefusal> for InstancedPullPlanningError {
    fn from(refusal: InstancedPullRefusal) -> Self {
        Self::Refused(refusal)
    }
}

/// Opaque bounded intent, NOT dispatch authority. Execution takes
/// this plan by value on every path and checks committed eligibility and revision. There is
/// no borrowed-plan ticket authorization, cloning, or reminting API.
///
/// ```compile_fail
/// use destiny_app::InstancedPullPlan;
/// fn duplicate(plan: &InstancedPullPlan) -> InstancedPullPlan {
///     Clone::clone(plan)
/// }
/// ```
/// ```compile_fail
/// use destiny_app::InstancedPullPlan;
/// let plan = InstancedPullPlan::new();
/// ```
/// ```compile_fail
/// use destiny_app::InstancedPullPlan;
/// fn consume(_: InstancedPullPlan) {}
/// fn replay(plan: InstancedPullPlan) {
///     consume(plan);
///     consume(plan);
/// }
/// ```
/// ```compile_fail
/// use destiny_app::InstancedPullService;
/// use destiny_core::*;
/// fn dispatch<G: ProfileGateway, R: InventoryRepository, D: DefinitionRepository,
///     M: ManifestGateway, C: Clock, P: PlanIdGenerator, T: InstancedPullGateway>(
///     service: &mut InstancedPullService<G, R, D, M, C, P>,
///     plan: destiny_app::InstancedPullPlan, gateway: &mut T) {
///     service.preflight_dispatch(plan, gateway, FreshnessPolicy { ttl_seconds: 60 });
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullPlan {
    id: PlanId,
    created_at: Timestamp,
    expires_at: Timestamp,
    membership: DestinyMembership,
    item: ItemInstanceId,
    item_hash: ItemHash,
    owner: CharacterId,
    destination_bucket: BucketHash,
    definition_version: DefinitionVersion,
    revision: SyncRevision,
}
impl InstancedPullPlan {
    pub fn id(&self) -> &PlanId {
        &self.id
    }
    pub fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub fn item_hash(&self) -> ItemHash {
        self.item_hash
    }
    pub fn owner(&self) -> &CharacterId {
        &self.owner
    }
    pub fn destination_bucket(&self) -> BucketHash {
        self.destination_bucket
    }
    pub fn definition_version(&self) -> &DefinitionVersion {
        &self.definition_version
    }
    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct PullAttempt {
    context: PullContext,
    preflight_at: Timestamp,
    dispatch_at: Timestamp,
    outcome: Result<InstancedPullOutcome, AppError>,
}
#[derive(Debug, Eq, PartialEq)]
pub struct PullContext {
    membership: DestinyMembership,
    item: ItemInstanceId,
    item_hash: ItemHash,
    owner: CharacterId,
    destination_bucket: BucketHash,
    definition_version: DefinitionVersion,
    revision: SyncRevision,
}
impl PullContext {
    pub fn membership(&self) -> &DestinyMembership {
        &self.membership
    }
    pub fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub fn item_hash(&self) -> ItemHash {
        self.item_hash
    }
    pub fn owner(&self) -> &CharacterId {
        &self.owner
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
}
impl PullAttempt {
    pub fn context(&self) -> &PullContext {
        &self.context
    }
    pub fn preflight_at(&self) -> Timestamp {
        self.preflight_at
    }
    pub fn dispatch_at(&self) -> Timestamp {
        self.dispatch_at
    }
    pub fn gateway_result(&self) -> &Result<InstancedPullOutcome, AppError> {
        &self.outcome
    }
}
/// Acknowledgement is success; transport uncertainty never is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullDisposition {
    Applied,
    Uncertain,
    Refused,
    LocalStateFailed,
}

/// Evidence from a received gateway outcome, retained even after local failures.
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullExecutionOutcome {
    plan_id: PlanId,
    dispatch: PullAttempt,
    local_state_error: Option<AppError>,
    disposition: PullDisposition,
}
impl InstancedPullExecutionOutcome {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn dispatch(&self) -> &PullAttempt {
        &self.dispatch
    }
    pub fn context(&self) -> &PullContext {
        self.dispatch.context()
    }
    pub fn gateway_outcome(&self) -> &InstancedPullOutcome {
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

/// Before-dispatch failures never invalidate freshness or retry.
#[derive(Debug, Eq, PartialEq)]
pub enum InstancedPullExecutionFailureReason {
    PreDispatch(PreDispatchError),
    Gateway(Box<PullAttempt>),
}
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullExecutionFailure {
    plan_id: PlanId,
    reason: InstancedPullExecutionFailureReason,
    local_state_error: Option<AppError>,
}
impl InstancedPullExecutionFailure {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn local_state_error(&self) -> Option<&AppError> {
        self.local_state_error.as_ref()
    }
    pub fn reason(&self) -> &InstancedPullExecutionFailureReason {
        &self.reason
    }
}

struct Eligibility {
    context: PullContext,
}
#[derive(Debug, Eq, PartialEq)]
pub enum PreDispatchError {
    Eligibility(InstancedPullPlanningError),
    Expired,
    BackwardsClock,
    IdentityDrift,
    DefinitionVersionDrift,
    RevisionDrift,
    FingerprintDrift,
}
impl From<InstancedPullPlanningError> for PreDispatchError {
    fn from(error: InstancedPullPlanningError) -> Self {
        Self::Eligibility(error)
    }
}
impl From<AppError> for PreDispatchError {
    fn from(error: AppError) -> Self {
        Self::Eligibility(error.into())
    }
}

/// Injected provider-neutral policy and ports; no adapter constants or pull gateway.
/// The clock and ID generator are never ambient. Definition reads must share the
/// published Manifest version; callers must serialize local publication with use.
pub struct InstancedPullService<G, R, D, M, C, P> {
    sync: ProfileSyncService<G, R>,
    definitions: D,
    // Retained for constructor compatibility; instanced pulls never fetch it.
    #[allow(dead_code)]
    manifest: M,
    policy: EquipmentCapacityPolicy,
    clock: C,
    plan_ids: P,
}
impl<G, R, D, M, C, P> InstancedPullService<G, R, D, M, C, P>
where
    G: ProfileGateway,
    R: InventoryRepository,
    D: DefinitionRepository,
    M: ManifestGateway,
    C: Clock,
    P: PlanIdGenerator,
{
    pub fn new(
        sync: ProfileSyncService<G, R>,
        definitions: D,
        manifest: M,
        policy: EquipmentCapacityPolicy,
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

    /// Ordinary sync reuses a fresh committed snapshot without acquisition.
    /// Definition-based capacity is advisory; no Manifest-index evidence fetch.
    /// Planning cannot dispatch an action.
    pub fn plan(
        &mut self,
        request: &InstancedPullPlanRequest,
        lifetime: PlanLifetime,
        freshness: FreshnessPolicy,
    ) -> Result<InstancedPullPlan, InstancedPullPlanningError> {
        let now = self.clock.now()?;
        self.sync
            .sync(ProfileSyncReason::Ordinary, freshness, now)?;
        let snapshot = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let eligibility = self.eligibility(&request.membership, &request.item, &snapshot)?;
        let expires_at = lifetime.expires_at(now)?;
        let id = self.plan_ids.next_plan_id()?;
        let context = eligibility.context;
        Ok(InstancedPullPlan {
            id,
            created_at: now,
            expires_at,
            membership: context.membership,
            item: context.item,
            item_hash: context.item_hash,
            owner: context.owner,
            destination_bucket: context.destination_bucket,
            definition_version: context.definition_version,
            revision: context.revision,
        })
    }

    /// Consume intent, validate the committed revision and dispatch at most once.
    /// Acknowledgements persist locally and retain freshness without a post-action
    /// profile request. Incomplete receipts or persistence failures invalidate,
    /// retaining the receipt. Gateway Err is pre-dispatch-only.
    /// Callers serialize local publication/definition use; no retry or rollback.
    ///
    /// ```compile_fail
    /// use destiny_app::{InstancedPullService, InstancedPullPlan};
    /// use destiny_core::*;
    /// fn replay<G: ProfileGateway, R: InventoryRepository, D: DefinitionRepository,
    /// M: ManifestGateway, C: Clock, P: PlanIdGenerator, T: InstancedPullGateway>(
    /// s: &mut InstancedPullService<G,R,D,M,C,P>,
    /// plan: InstancedPullPlan, gateway: &mut T, freshness: FreshnessPolicy) {
    ///     s.execute(plan, gateway, freshness);
    ///     s.execute(plan, gateway, freshness);
    /// }
    /// ```
    pub fn execute<T: InstancedPullGateway>(
        &mut self,
        plan: InstancedPullPlan,
        gateway: &mut T,
        freshness: FreshnessPolicy,
    ) -> Result<InstancedPullExecutionOutcome, InstancedPullExecutionFailure> {
        let plan_id = plan.id.clone();
        let dispatch = self
            .preflight_dispatch(plan, gateway, freshness)
            .map_err(|error| InstancedPullExecutionFailure {
                plan_id: plan_id.clone(),
                reason: InstancedPullExecutionFailureReason::PreDispatch(error),
                local_state_error: None,
            })?;
        let receipt = match &dispatch.outcome {
            Ok(receipt) => receipt,
            Err(_) => {
                let local_state_error = self.sync.repository_mut().invalidate_freshness().err();
                return Err(InstancedPullExecutionFailure {
                    plan_id,
                    local_state_error,
                    reason: InstancedPullExecutionFailureReason::Gateway(Box::new(dispatch)),
                });
            }
        };
        let refused = matches!(
            receipt,
            InstancedPullOutcome::Rejected(_)
                | InstancedPullOutcome::AuthenticationRefused
                | InstancedPullOutcome::Transient
        );
        let acknowledged = matches!(receipt, InstancedPullOutcome::Acknowledged);
        let persistence = if acknowledged {
            self.sync
                .repository_mut()
                .apply_acknowledged_effects(
                    dispatch.context.membership(),
                    &[destiny_core::InventoryEffect::PullFromPostmaster {
                        item: dispatch.context.item.clone(),
                        character: dispatch.context.owner.clone(),
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
        Ok(InstancedPullExecutionOutcome {
            plan_id,
            dispatch,
            local_state_error,
            disposition,
        })
    }

    fn eligibility(
        &self,
        membership: &DestinyMembership,
        item_id: &ItemInstanceId,
        snapshot: &InventorySnapshot,
    ) -> Result<Eligibility, InstancedPullPlanningError> {
        use InstancedPullRefusal as Refusal;
        if membership.membership_id() != &snapshot.metadata.membership_id
            || membership.membership_type() != snapshot.metadata.membership_type
        {
            return Err(Refusal::MembershipMismatch.into());
        }
        let mut targets = snapshot
            .items
            .iter()
            .filter(|item| &item.instance_id == item_id);
        let target = targets.next().ok_or(Refusal::TargetMissing)?;
        if targets.next().is_some() {
            return Err(Refusal::TargetAmbiguous.into());
        }
        let ItemLocation::Postmaster(owner) = &target.location else {
            return Err(Refusal::NotInPostmaster.into());
        };
        if !snapshot
            .characters
            .iter()
            .any(|character| &character.id == owner)
        {
            return Err(Refusal::OwnerNotOwned.into());
        }
        let definition = match self.definitions.lookup_definition(target.item_hash)? {
            Resolution::Unique(definition) => definition,
            Resolution::Missing => return Err(Refusal::DefinitionMissing.into()),
            Resolution::Ambiguous(_) => return Err(Refusal::DefinitionAmbiguous.into()),
        };
        if self.definitions.current_definition_version()? != snapshot.metadata.definition_version
            || definition.version != snapshot.metadata.definition_version
        {
            return Err(Refusal::DefinitionVersionMismatch.into());
        }
        match definition.does_postmaster_pull_have_side_effects {
            Some(false) => {}
            Some(true) => return Err(Refusal::SideEffects.into()),
            None => return Err(Refusal::SideEffectsUnknown.into()),
        }
        match definition.is_instance_item {
            Some(true) => {}
            Some(false) => return Err(Refusal::NonInstanced.into()),
            None => return Err(Refusal::InstanceKindUnknown.into()),
        }
        let bucket = definition.bucket_hash.ok_or(Refusal::MissingBucket)?;
        if self.policy.capacity(bucket).is_none() {
            return Err(Refusal::UnsupportedBucket.into());
        }
        self.policy
            .check_local_capacity(snapshot, &self.definitions, owner, bucket)
            .map_err(|error| match error {
                destiny_core::LocalCapacityError::Refused(reason) => {
                    InstancedPullPlanningError::Refused(Refusal::Capacity(reason))
                }
                destiny_core::LocalCapacityError::Infrastructure(error) => error.into(),
            })?;
        Ok(Eligibility {
            context: PullContext {
                membership: membership.clone(),
                item: target.instance_id.clone(),
                item_hash: target.item_hash,
                owner: owner.clone(),
                destination_bucket: bucket,
                definition_version: snapshot.metadata.definition_version.clone(),
                revision: snapshot.metadata.revision,
            },
        })
    }
    /// Consumes intent on every path, never remints or retries. Caller must
    /// serialize local publications through dispatch; these checks cannot be
    /// atomic against other clients or game activity.
    fn preflight_dispatch<T: InstancedPullGateway>(
        &mut self,
        plan: InstancedPullPlan,
        gateway: &mut T,
        freshness: FreshnessPolicy,
    ) -> Result<PullAttempt, PreDispatchError> {
        let preflight_at = self.clock.now()?;
        Self::check_time(&plan, preflight_at, plan.created_at)?;
        self.sync
            .sync(ProfileSyncReason::Ordinary, freshness, preflight_at)?;
        let snapshot = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?;
        let eligible = self.eligibility(&plan.membership, &plan.item, &snapshot)?;
        let context = eligible.context;
        if context.item_hash != plan.item_hash || context.owner != plan.owner {
            return Err(PreDispatchError::IdentityDrift);
        }
        if context.definition_version != plan.definition_version {
            return Err(PreDispatchError::DefinitionVersionDrift);
        }
        if context.revision != plan.revision {
            return Err(PreDispatchError::RevisionDrift);
        }
        if context.destination_bucket != plan.destination_bucket {
            return Err(PreDispatchError::IdentityDrift);
        }
        let metadata = self
            .sync
            .repository()
            .load_current()?
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?
            .metadata;
        if metadata.membership_id != *plan.membership.membership_id()
            || metadata.membership_type != plan.membership.membership_type()
        {
            return Err(PreDispatchError::IdentityDrift);
        }
        if metadata.revision != plan.revision {
            return Err(PreDispatchError::RevisionDrift);
        }
        if metadata.definition_version != plan.definition_version
            || self.definitions.current_definition_version()? != plan.definition_version
        {
            return Err(PreDispatchError::DefinitionVersionDrift);
        }
        let dispatch_at = self.clock.now()?;
        Self::check_time(&plan, dispatch_at, preflight_at)?;
        let request = InstancedPullRequest::new(
            context.membership.clone(),
            context.item.clone(),
            context.item_hash,
            context.owner.clone(),
        )?;
        let outcome = gateway.pull(&request); // Exactly one attempt, including Err.
        Ok(PullAttempt {
            context,
            preflight_at,
            dispatch_at,
            outcome,
        })
    }

    fn check_time(
        plan: &InstancedPullPlan,
        now: Timestamp,
        previous: Timestamp,
    ) -> Result<(), PreDispatchError> {
        if now < previous {
            return Err(PreDispatchError::BackwardsClock);
        }
        if now >= plan.expires_at {
            return Err(PreDispatchError::Expired);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
