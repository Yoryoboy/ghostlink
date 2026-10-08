//! Fresh instanced Postmaster planning and consumed-plan verified execution.
//! Published observations are not causal or atomic application guarantees.

use crate::{ProfileSyncService, ProfileSyncWithObservationsOutcome, PublishedProfileAcquisition};
use destiny_core::ports::RetryDelay;
use destiny_core::{
    AppError, BucketHash, CapacityRefusal, CharacterId, Clock, DefinitionRepository,
    DefinitionVersion, DestinyMembership, EquipmentCapacityEvidence, EquipmentCapacityPolicy,
    ErrorCategory, FreshnessPolicy, InstancedBucketObservation, InstancedPullGateway,
    InstancedPullOutcome, InstancedPullRequest, InventoryRepository, InventorySnapshotInput,
    ItemHash, ItemInstance, ItemInstanceId, ItemLocation, ManifestGateway, PlanId, PlanIdGenerator,
    PlanLifetime, ProfileAcquisition, ProfileGateway, ProfileSyncReason, Resolution,
    SnapshotMetadata, SyncRevision, Timestamp,
};
use std::time::Duration;

/// Select one genuine instance. Destination is its freshly observed Postmaster owner.
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

// Ephemeral runtime buckets do not change the inventory revision. Bind the
// complete eligibility basis separately so future preflight cannot reuse proof
// on revision equality alone. IDs are sorted, preserving occupant identity.
#[derive(Debug, Eq, PartialEq)]
struct EligibilityFingerprint {
    item_hash: ItemHash,
    instance_kind: bool,
    side_effects: bool,
    bucket: BucketHash,
    policy_version: DefinitionVersion,
    policy_artifact: String,
    occupied: usize,
    occupants: Vec<ItemInstanceId>,
}

/// Opaque bounded intent, NOT dispatch authority. Execution takes
/// this plan by value on every path and reacquires/reproves eligibility. There is
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
    fingerprint: EligibilityFingerprint,
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
// VerifiedArrival describes a published location, never causal application.
// Attempt-level timestamp failures still invalidate this location evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullArrival {
    VerifiedArrival,
    MembershipMismatch,
    DefinitionVersionDrift,
    TargetMissing,
    HashDrift,
    ObservationsUnavailable,
    StillInPostmaster,
    WrongCharacterOwner,
    WrongActualBucket,
    Elsewhere,
    UnknownLocation,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PullObservation {
    metadata: SnapshotMetadata,
    // None means absent: do not forge observed identity from planned identity.
    target: Option<ItemInstance>,
    // Retain the complete actual-bucket evidence, including explicit unavailable.
    observations: Option<Vec<InstancedBucketObservation>>,
    classification: PullArrival,
}

/// Pure classification of the exact successfully published acquisition pair.
/// PublishedProfileAcquisition guarantees unique IDs and complete, corresponding
/// observations when available; no reload, definition lookup or capacity reproof.
/// Revision equality is deliberately irrelevant. Caller serialization is still
/// required: this observation does not protect against later publications or
/// external game activity. The executor separately resolves receipt refusals.
fn classify_pull_observation(
    context: &PullContext,
    pair: PublishedProfileAcquisition,
) -> PullObservation {
    let (snapshot, observations) = pair.into_parts();
    let target = snapshot
        .items
        .into_iter()
        .find(|item| item.instance_id == context.item);
    let classification = (|| {
        if snapshot.metadata.membership_id != *context.membership.membership_id()
            || snapshot.metadata.membership_type != context.membership.membership_type()
        {
            return PullArrival::MembershipMismatch;
        }
        if snapshot.metadata.definition_version != context.definition_version {
            return PullArrival::DefinitionVersionDrift;
        }
        let Some(target) = &target else {
            return PullArrival::TargetMissing;
        };
        if target.item_hash != context.item_hash {
            return PullArrival::HashDrift;
        }
        let Some(observations) = &observations else {
            return PullArrival::ObservationsUnavailable;
        };
        let bucket = observations
            .iter()
            .find(|row| row.instance_id() == &target.instance_id)
            .expect("published available observations cover every unique instance")
            .bucket_hash();
        match &target.location {
            ItemLocation::Character(owner) if owner != &context.owner => {
                PullArrival::WrongCharacterOwner
            }
            ItemLocation::Character(_) if bucket != context.destination_bucket => {
                PullArrival::WrongActualBucket
            }
            ItemLocation::Character(_) => PullArrival::VerifiedArrival,
            ItemLocation::Postmaster(owner) if owner != &context.owner => {
                PullArrival::WrongCharacterOwner
            }
            ItemLocation::Postmaster(_) => PullArrival::StillInPostmaster,
            ItemLocation::Vault => PullArrival::Elsewhere,
            ItemLocation::Other(_) => PullArrival::UnknownLocation,
        }
    })();
    PullObservation {
        metadata: snapshot.metadata,
        target,
        observations,
        classification,
    }
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
impl PullObservation {
    pub fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }
    pub fn classification(&self) -> PullArrival {
        self.classification
    }
    pub fn target_id(&self) -> Option<&ItemInstanceId> {
        self.target.as_ref().map(|item| &item.instance_id)
    }
    pub fn target_hash(&self) -> Option<ItemHash> {
        self.target.as_ref().map(|item| item.item_hash)
    }
    pub fn target_location(&self) -> Option<&ItemLocation> {
        self.target.as_ref().map(|item| &item.location)
    }
    pub fn actual_buckets_available(&self) -> bool {
        self.observations.is_some()
    }
    pub fn target_actual_bucket(&self) -> Option<BucketHash> {
        let target = self.target.as_ref()?;
        self.observations
            .as_ref()?
            .iter()
            .find(|row| row.instance_id() == &target.instance_id)
            .map(|row| row.bucket_hash())
    }
}

/// Honest terminal disposition; even observed arrival cannot override refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PullDisposition {
    Verified,
    NotYetObserved,
    Uncertain,
    Refused,
}

#[derive(Debug, Eq, PartialEq)]
pub enum PullObservationFailure {
    Clock(AppError),
    BackwardsClock,
    Refresh(AppError),
    SkippedFresh,
}

/// One ordered observation attempt. A clock failure has no invented timestamp.
/// Check `failure()` even when `observation()` exists: a backwards publication
/// timestamp preserves the pair but invalidates its otherwise exact arrival.
#[derive(Debug, Eq, PartialEq)]
pub struct PullObservationAttempt<O = PullObservation> {
    checked_at: Option<Timestamp>,
    result: Result<O, PullObservationFailure>,
    // A publication time fault retains the pair as well as the failure.
    publication_failure: Option<PullObservationFailure>,
}
impl<O> PullObservationAttempt<O> {
    pub fn checked_at(&self) -> Option<Timestamp> {
        self.checked_at
    }
    pub fn observation(&self) -> Option<&O> {
        self.result.as_ref().ok()
    }
    pub fn failure(&self) -> Option<&PullObservationFailure> {
        self.publication_failure
            .as_ref()
            .or_else(|| self.result.as_ref().err())
    }
}

/// Evidence from a received gateway outcome, retained even after local failures.
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullExecutionOutcome {
    plan_id: PlanId,
    dispatch: PullAttempt,
    observations: Vec<PullObservationAttempt>,
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
    pub fn observations(&self) -> &[PullObservationAttempt] {
        &self.observations
    }
    pub fn disposition(&self) -> PullDisposition {
        self.disposition
    }
}

/// Before-dispatch failures never incur PostAction acquisitions or waits.
#[derive(Debug, Eq, PartialEq)]
pub enum InstancedPullExecutionFailureReason {
    PreDispatch(PreDispatchError),
    Gateway(Box<PullAttempt>),
}
#[derive(Debug, Eq, PartialEq)]
pub struct InstancedPullExecutionFailure {
    plan_id: PlanId,
    reason: InstancedPullExecutionFailureReason,
}
impl InstancedPullExecutionFailure {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }
    pub fn reason(&self) -> &InstancedPullExecutionFailureReason {
        &self.reason
    }
}

struct Eligibility {
    context: PullContext,
    fingerprint: EligibilityFingerprint,
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

    /// Always performs exactly one forced Preflight acquisition, even over an
    /// already-fresh committed snapshot. One evidence fetch follows successful
    /// publication; no request here can dispatch an action.
    pub fn plan(
        &mut self,
        request: &InstancedPullPlanRequest,
        lifetime: PlanLifetime,
        freshness: FreshnessPolicy,
    ) -> Result<InstancedPullPlan, InstancedPullPlanningError> {
        let now = self.clock.now()?;
        let pair =
            match self
                .sync
                .sync_with_observations(ProfileSyncReason::Preflight, freshness, now)?
            {
                ProfileSyncWithObservationsOutcome::Refreshed(pair) => pair,
                ProfileSyncWithObservationsOutcome::SkippedFresh { .. } => {
                    return Err(AppError::new(ErrorCategory::ProviderValidation).into());
                }
            };
        // Fetch once even for an eligibility refusal. Never retry or substitute
        // cached index evidence. Qualification still follows the definition gates.
        let evidence = self.manifest.fetch_equipment_capacity_evidence()?;
        let eligibility = self.eligibility(&request.membership, &request.item, &pair, &evidence)?;
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
            fingerprint: eligibility.fingerprint,
        })
    }

    /// Consume a plan exactly once, reacquire preflight proof, dispatch at most
    /// once and observe at most three forced successful-publication pairs.
    /// No action retry, reminting, stale fallback or rollback is performed.
    /// Callers must serialize local inventory publication and definition use
    /// through execution; these observations are not causal or atomic proof
    /// against other clients or game activity. Plan expiry gates dispatch only.
    /// The gateway contract requires Err to mean failure before dispatch; it
    /// incurs no PostAction expense. Any received outcome is retained even when
    /// subsequent clocks or publications fail. Confirmed refusal always wins
    /// over arrival evidence; acknowledgement alone never proves recovery.
    ///
    /// ```compile_fail
    /// use destiny_app::{InstancedPullService, InstancedPullPlan};
    /// use destiny_core::*;
    /// use destiny_core::ports::RetryDelay;
    /// fn replay<G: ProfileGateway, R: InventoryRepository, D: DefinitionRepository,
    /// M: ManifestGateway, C: Clock, P: PlanIdGenerator, T: InstancedPullGateway,
    /// W: RetryDelay>(s: &mut InstancedPullService<G,R,D,M,C,P>,
    /// plan: InstancedPullPlan, gateway: &mut T, delay: &mut W, freshness: FreshnessPolicy) {
    ///     s.execute_verified(plan, gateway, delay, freshness);
    ///     s.execute_verified(plan, gateway, delay, freshness);
    /// }
    /// ```
    pub fn execute_verified<T: InstancedPullGateway, W: RetryDelay>(
        &mut self,
        plan: InstancedPullPlan,
        gateway: &mut T,
        delay: &mut W,
        freshness: FreshnessPolicy,
    ) -> Result<InstancedPullExecutionOutcome, InstancedPullExecutionFailure> {
        let plan_id = plan.id.clone();
        let dispatch = self
            .preflight_dispatch(plan, gateway, freshness)
            .map_err(|error| InstancedPullExecutionFailure {
                plan_id: plan_id.clone(),
                reason: InstancedPullExecutionFailureReason::PreDispatch(error),
            })?;
        let receipt = match &dispatch.outcome {
            Ok(receipt) => receipt,
            Err(_) => {
                return Err(InstancedPullExecutionFailure {
                    plan_id,
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
        let observations = observe_pull(
            &mut self.sync,
            &self.clock,
            delay,
            freshness,
            dispatch.dispatch_at,
            |pair| classify_pull_observation(&dispatch.context, pair),
        );
        let last = observations
            .last()
            .filter(|attempt| attempt.failure().is_none())
            .and_then(PullObservationAttempt::observation)
            .map(PullObservation::classification);
        // Confirmed refusals stay Refused even if arrival is later observed: this
        // request did not apply, so that arrival has another cause (GPM-008f2).
        let disposition = if refused {
            PullDisposition::Refused
        } else if last == Some(PullArrival::VerifiedArrival) {
            PullDisposition::Verified
        } else if acknowledged && last == Some(PullArrival::StillInPostmaster) {
            PullDisposition::NotYetObserved
        } else {
            PullDisposition::Uncertain
        };
        Ok(InstancedPullExecutionOutcome {
            plan_id,
            dispatch,
            observations,
            disposition,
        })
    }

    fn eligibility(
        &self,
        membership: &DestinyMembership,
        item_id: &ItemInstanceId,
        pair: &PublishedProfileAcquisition,
        evidence: &EquipmentCapacityEvidence,
    ) -> Result<Eligibility, InstancedPullPlanningError> {
        use InstancedPullRefusal as Refusal;
        let snapshot = pair.snapshot();
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
        let qualified = self
            .policy
            .qualify(evidence)
            .map_err(|reason| match reason {
                destiny_core::CapacityPolicyMismatch::VersionMismatch => {
                    Refusal::PolicyVersionMismatch
                }
                destiny_core::CapacityPolicyMismatch::ArtifactMismatch => {
                    Refusal::PolicyArtifactMismatch
                }
            })?;
        let acquisition = ProfileAcquisition::new(
            InventorySnapshotInput::with_metadata(snapshot),
            pair.observations().map(<[_]>::to_vec),
        )?;
        let proof = qualified
            .prove(&acquisition, owner, Some(bucket))
            .map_err(Refusal::Capacity)?;
        let mut occupants: Vec<_> = acquisition
            .observations()
            .ok_or(Refusal::Capacity(CapacityRefusal::ObservationsUnavailable))?
            .iter()
            .filter(|row| {
                row.bucket_hash() == bucket
                    && matches!(row.location(), ItemLocation::Character(id) if id == owner)
            })
            .map(|row| row.instance_id().clone())
            .collect();
        occupants.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let fingerprint = EligibilityFingerprint {
            item_hash: target.item_hash,
            instance_kind: true,
            side_effects: false,
            bucket,
            policy_version: self.policy.definition_version().clone(),
            policy_artifact: self.policy.bucket_artifact_identity().to_owned(),
            occupied: proof.occupied(),
            occupants,
        };
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
            fingerprint,
        })
    }
    /// Consumes intent on every path, never remints or retries. Caller must
    /// serialize local publications through dispatch; these checks cannot be
    /// atomic against other clients or game activity. Observations follow only
    /// in the public consumed-plan executor.
    fn preflight_dispatch<T: InstancedPullGateway>(
        &mut self,
        plan: InstancedPullPlan,
        gateway: &mut T,
        freshness: FreshnessPolicy,
    ) -> Result<PullAttempt, PreDispatchError> {
        let preflight_at = self.clock.now()?;
        Self::check_time(&plan, preflight_at, plan.created_at)?;
        let pair = match self.sync.sync_with_observations(
            ProfileSyncReason::Preflight,
            freshness,
            preflight_at,
        )? {
            ProfileSyncWithObservationsOutcome::Refreshed(pair) => pair,
            ProfileSyncWithObservationsOutcome::SkippedFresh { .. } => {
                return Err(AppError::new(ErrorCategory::ProviderValidation).into());
            }
        };
        let evidence = self.manifest.fetch_equipment_capacity_evidence()?;
        let eligible = self.eligibility(&plan.membership, &plan.item, &pair, &evidence)?;
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
        if eligible.fingerprint != plan.fingerprint {
            return Err(PreDispatchError::FingerprintDrift);
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

/// Shared bounded published-evidence loop. There is deliberately no action port.
pub(crate) trait PullObservationEvidence {
    fn metadata(&self) -> &SnapshotMetadata;
    fn stop(&self) -> bool;
}
impl PullObservationEvidence for PullObservation {
    fn metadata(&self) -> &SnapshotMetadata {
        &self.metadata
    }
    fn stop(&self) -> bool {
        matches!(
            self.classification,
            PullArrival::VerifiedArrival
                | PullArrival::MembershipMismatch
                | PullArrival::DefinitionVersionDrift
                | PullArrival::HashDrift
        )
    }
}
pub(crate) fn observe_pull<G, R, C, W, O>(
    sync: &mut ProfileSyncService<G, R>,
    clock: &C,
    delay: &mut W,
    freshness: FreshnessPolicy,
    dispatch_at: Timestamp,
    classify: impl Fn(PublishedProfileAcquisition) -> O,
) -> Vec<PullObservationAttempt<O>>
where
    G: ProfileGateway,
    R: InventoryRepository,
    C: Clock,
    W: RetryDelay,
    O: PullObservationEvidence,
{
    let mut observations = Vec::with_capacity(3);
    let mut previous = dispatch_at;
    for index in 0..3 {
        if index > 0 {
            delay.wait(Duration::from_secs(2));
        }
        let now = match clock.now() {
            Ok(now) => now,
            Err(error) => {
                observations.push(PullObservationAttempt {
                    checked_at: None,
                    result: Err(PullObservationFailure::Clock(error)),
                    publication_failure: None,
                });
                break;
            }
        };
        if now < previous {
            observations.push(PullObservationAttempt {
                checked_at: Some(now),
                result: Err(PullObservationFailure::BackwardsClock),
                publication_failure: None,
            });
            break;
        }
        previous = now;
        let result =
            match sync.sync_with_observations(ProfileSyncReason::PostAction, freshness, now) {
                Ok(ProfileSyncWithObservationsOutcome::Refreshed(pair)) => Ok(classify(pair)),
                Ok(ProfileSyncWithObservationsOutcome::SkippedFresh { .. }) => {
                    Err(PullObservationFailure::SkippedFresh)
                }
                Err(error) => Err(PullObservationFailure::Refresh(error)),
            };
        let publication_failure = match &result {
            Ok(observation) if observation.metadata().checked_at < previous => {
                Some(PullObservationFailure::BackwardsClock)
            }
            Ok(observation) => {
                previous = observation.metadata().checked_at;
                None
            }
            Err(_) => None,
        };
        let stop = publication_failure.is_some()
            || result.as_ref().is_ok_and(PullObservationEvidence::stop);
        observations.push(PullObservationAttempt {
            checked_at: Some(now),
            result,
            publication_failure,
        });
        if stop {
            break;
        }
    }
    observations
}

#[cfg(test)]
mod tests;
