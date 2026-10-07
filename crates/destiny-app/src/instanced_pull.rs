//! Fresh, planning-only eligibility for instanced Postmaster recovery.
//! No dispatch or post-action observation is exposed by this module.

use crate::{ProfileSyncService, ProfileSyncWithObservationsOutcome, PublishedProfileAcquisition};
use destiny_core::{
    AppError, BucketHash, CapacityRefusal, CharacterId, Clock, DefinitionRepository,
    DefinitionVersion, DestinyMembership, EquipmentCapacityEvidence, EquipmentCapacityPolicy,
    ErrorCategory, FreshnessPolicy, InstancedPullGateway, InstancedPullOutcome,
    InstancedPullRequest, InventoryRepository, InventorySnapshotInput, ItemHash, ItemInstanceId,
    ItemLocation, ManifestGateway, PlanId, PlanIdGenerator, PlanLifetime, ProfileAcquisition,
    ProfileGateway, ProfileSyncReason, Resolution, SyncRevision, Timestamp,
};

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

/// Opaque bounded intent, NOT dispatch authority. The future executor must take
/// this plan by value on every path and reacquire/reprove eligibility. There is
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

// GPM-008f will consume terminal evidence, not reusable plan authority.
#[allow(dead_code)]
#[derive(Debug, Eq, PartialEq)]
struct PullAttempt {
    context: PullContext,
    preflight_at: Timestamp,
    dispatch_at: Timestamp,
    outcome: Result<InstancedPullOutcome, AppError>,
}
#[derive(Debug, Eq, PartialEq)]
struct PullContext {
    membership: DestinyMembership,
    item: ItemInstanceId,
    item_hash: ItemHash,
    owner: CharacterId,
    destination_bucket: BucketHash,
    definition_version: DefinitionVersion,
    revision: SyncRevision,
}
struct Eligibility {
    context: PullContext,
    fingerprint: EligibilityFingerprint,
}
#[allow(dead_code)] // Private until GPM-008f adds the verified executor.
#[derive(Debug, Eq, PartialEq)]
enum PreDispatchError {
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
        let qualified = self.policy.qualify(evidence).map_err(|_| {
            if evidence.definition_version() != self.policy.definition_version() {
                Refusal::PolicyVersionMismatch
            } else {
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
    /// atomic against other clients or game activity. No post-action proof yet.
    #[allow(dead_code)] // Private future GPM-008f seam, deliberately not exported.
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

#[cfg(test)]
mod tests;
