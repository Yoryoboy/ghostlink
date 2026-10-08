//! Fresh, consumed-plan non-instanced Postmaster pulls.
//! Quantities are published observations, not causal or atomic guarantees.
use crate::instanced_pull::{
    PullDisposition, PullObservationAttempt, PullObservationEvidence, observe_pull,
};
use crate::{ProfileSyncService, ProfileSyncWithObservationsOutcome, PublishedProfileAcquisition};
use destiny_core::ports::RetryDelay;
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
#[derive(Debug, Eq, PartialEq)]
struct Fingerprint {
    capacity: StackPullCapacityInput,
    unique_label: String,
    policy_version: DefinitionVersion,
    policy_artifact: String,
    // Sorted multiset: equal anonymous rows remain significant, never deduplicated.
    rows: Vec<StackObservation>,
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
    fingerprint: Fingerprint,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StackPullArrival {
    VerifiedArrival,
    QuantityMismatch,
    MembershipMismatch,
    DefinitionVersionDrift,
    ArithmeticOverflow,
}
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullObservation {
    metadata: SnapshotMetadata,
    stacks: Vec<StackObservation>,
    destination_quantity: Option<u64>,
    postmaster_quantity: Option<u64>,
    classification: StackPullArrival,
}
impl StackPullObservation {
    pub fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }
    pub fn stacks(&self) -> &[StackObservation] {
        &self.stacks
    }
    pub fn destination_quantity(&self) -> Option<u64> {
        self.destination_quantity
    }
    pub fn postmaster_quantity(&self) -> Option<u64> {
        self.postmaster_quantity
    }
    pub fn classification(&self) -> StackPullArrival {
        self.classification
    }
}
impl PullObservationEvidence for StackPullObservation {
    fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }
    fn stop(&self) -> bool {
        matches!(
            self.classification,
            StackPullArrival::VerifiedArrival
                | StackPullArrival::MembershipMismatch
                | StackPullArrival::DefinitionVersionDrift
        )
    }
}
pub type StackPullObservationAttempt = PullObservationAttempt<StackPullObservation>;
#[derive(Debug, Eq, PartialEq)]
pub struct StackPullExecutionOutcome {
    plan_id: PlanId,
    dispatch: StackPullAttempt,
    observations: Vec<StackPullObservationAttempt>,
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
    pub fn observations(&self) -> &[StackPullObservationAttempt] {
        &self.observations
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
    fingerprint: Fingerprint,
}

/// Provider-neutral ports and injected audited policy. Callers must serialize
/// local publication/definition use through execution, as for instanced pulls.
pub struct StackPullService<G, R, D, M, C, P> {
    sync: ProfileSyncService<G, R>,
    definitions: D,
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
    fn fresh_preflight(
        &mut self,
        freshness: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<PublishedProfileAcquisition, AppError> {
        match self
            .sync
            .sync_with_observations(ProfileSyncReason::Preflight, freshness, now)?
        {
            ProfileSyncWithObservationsOutcome::Refreshed(pair) => Ok(pair),
            ProfileSyncWithObservationsOutcome::SkippedFresh { .. } => {
                Err(AppError::new(ErrorCategory::ProviderValidation))
            }
        }
    }
    /// One forced acquisition and one fresh public index evidence fetch; no action.
    pub fn plan(
        &mut self,
        request: &StackPullPlanRequest,
        lifetime: PlanLifetime,
        freshness: FreshnessPolicy,
    ) -> Result<StackPullPlan, StackPullPlanningError> {
        let now = self.clock.now()?;
        let pair = self.fresh_preflight(freshness, now)?;
        // The existing evidence port supplies the same Manifest version/artifact
        // identity for both audited policies, not equipment capacity substitution.
        let evidence = self.manifest.fetch_equipment_capacity_evidence()?;
        let wire = StackPullRequest::new(
            request.membership.clone(),
            request.item_hash,
            request.owner.clone(),
            request.amount,
        )?;
        let eligible = self.eligibility(wire, &pair, &evidence)?;
        Ok(StackPullPlan {
            id: self.plan_ids.next_plan_id()?,
            created_at: now,
            expires_at: lifetime.expires_at(now)?,
            context: eligible.context,
            fingerprint: eligible.fingerprint,
        })
    }
    fn eligibility(
        &self,
        request: StackPullRequest,
        pair: &PublishedProfileAcquisition,
        evidence: &StackBucketCapacityEvidence,
    ) -> Result<Eligibility, StackPullPlanningError> {
        use StackPullPlanningRefusal as Refusal;
        let snapshot = pair.snapshot();
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
        let qualified = self.policy.qualify(evidence).map_err(|_| {
            if evidence.definition_version() != self.policy.definition_version() {
                Refusal::PolicyVersionMismatch
            } else {
                Refusal::PolicyArtifactMismatch
            }
        })?;
        let mut capacity = qualified
            .derive(bucket, request.item_hash(), &snapshot.stacks)
            .map_err(Refusal::Capacity)?;
        capacity.max_stack_size = d.max_stack_size;
        capacity.unique_stack = d.stack_unique_label.as_ref().map(|s| !s.is_empty());
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
        let mut rows: Vec<_> = snapshot
            .stacks
            .iter()
            .filter(|r| {
                let destination_row = r.container() == &StackContainer::ProfileInventory
                    && r.bucket_hash() == bucket;
                let same_hash_profile_row = r.container() == &StackContainer::ProfileInventory
                    && r.item_hash() == request.item_hash();
                let owner_postmaster_row = r.item_hash() == request.item_hash()
                    && matches!(r.container(), StackContainer::Postmaster(owner) if owner == request.character());
                destination_row || same_hash_profile_row || owner_postmaster_row
            })
            .cloned()
            .collect();
        rows.sort();
        let fingerprint = Fingerprint {
            capacity,
            unique_label: d.stack_unique_label.expect("eligible uniqueness is known"),
            policy_version: self.policy.definition_version().clone(),
            policy_artifact: self.policy.bucket_artifact_identity().to_owned(),
            rows,
        };
        Ok(Eligibility {
            context: StackPullContext {
                request,
                destination_bucket: bucket,
                definition_version: snapshot.metadata.definition_version.clone(),
                revision: snapshot.metadata.revision,
                pre_destination_quantity: destination,
                pre_profile_quantity: profile,
                pre_postmaster_quantity: postmaster,
            },
            fingerprint,
        })
    }
    /// Consume on every path. Reprove before one dispatch, then reuse the bounded
    /// published-evidence loop. Acknowledgement alone always remains Uncertain.
    pub fn execute_verified<T: StackPullGateway, W: RetryDelay>(
        &mut self,
        plan: StackPullPlan,
        gateway: &mut T,
        delay: &mut W,
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
        let observations = observe_pull(
            &mut self.sync,
            &self.clock,
            delay,
            freshness,
            dispatch.dispatch_at,
            |pair| classify(&dispatch.context, pair),
        );
        let verified = observations
            .last()
            .filter(|a| a.failure().is_none())
            .and_then(PullObservationAttempt::observation)
            .is_some_and(|o| o.classification == StackPullArrival::VerifiedArrival);
        let disposition = if refused {
            PullDisposition::Refused
        } else if verified {
            PullDisposition::Verified
        } else {
            PullDisposition::Uncertain
        };
        Ok(StackPullExecutionOutcome {
            plan_id,
            dispatch,
            observations,
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
        let pair = self.fresh_preflight(freshness, preflight_at)?;
        let evidence = self.manifest.fetch_equipment_capacity_evidence()?;
        let eligible = self.eligibility(plan.context.request.clone(), &pair, &evidence)?;
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
            || eligible.fingerprint != plan.fingerprint
        {
            return Err(Error::FingerprintDrift);
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
fn classify(context: &StackPullContext, pair: PublishedProfileAcquisition) -> StackPullObservation {
    let (snapshot, _) = pair.into_parts();
    let (destination, profile, postmaster) = totals(
        &context.request,
        context.destination_bucket,
        &snapshot.stacks,
    );
    let amount = context.request.quantity() as u64;
    let classification = if snapshot.metadata.membership_id
        != *context.request.membership().membership_id()
        || snapshot.metadata.membership_type != context.request.membership().membership_type()
    {
        StackPullArrival::MembershipMismatch
    } else if snapshot.metadata.definition_version != context.definition_version {
        StackPullArrival::DefinitionVersionDrift
    } else if destination.is_none() || profile.is_none() || postmaster.is_none() {
        StackPullArrival::ArithmeticOverflow
    } else if destination == context.pre_destination_quantity.checked_add(amount)
        && profile == context.pre_profile_quantity.checked_add(amount)
        && postmaster == context.pre_postmaster_quantity.checked_sub(amount)
    {
        StackPullArrival::VerifiedArrival
    } else {
        StackPullArrival::QuantityMismatch
    };
    StackPullObservation {
        metadata: snapshot.metadata,
        stacks: snapshot.stacks,
        destination_quantity: destination,
        postmaster_quantity: postmaster,
        classification,
    }
}
#[cfg(test)]
mod tests;
