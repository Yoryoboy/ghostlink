//! Shared lock-state planning and single-attempt optimistic execution.
//!
//! Lock is idempotent state, not equip policy: no class, exotic or equipped
//! restriction applies. Ordinary sync and committed-projection validation prove
//! a no-op or bind one dispatch. Acknowledgement updates local lock state without
//! a post-action read. Incomplete attempts or persistence failures invalidate
//! freshness; never blindly retry.
//!
//! Vault requests use the first owned character in committed projection order.
//! This deterministic provider routing choice still needs live validation.

use crate::mutation::{MutationError, MutationService, authorize};
use destiny_core::{
    CharacterId, DestinationSelector, DestinyMembership, FreshnessPolicy, InventoryRepository,
    InventorySnapshot, ItemInstance, ItemInstanceId, ItemLocation, LockGateway, LockOutcome,
    LockRequest, MoveRequest, PlanId, PlanIdGenerator, PlanLifetime, ProfileGateway,
    ProfileSyncReason, SearchRepository, SetLockStateRequest, SyncRevision, Timestamp,
};

/// Opaque bounded plan. Construction is application-owned; callers cannot forge
/// its identity, requested state, membership, source, revision or lifetime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockPlan {
    id: PlanId,
    membership: DestinyMembership,
    item: ItemInstanceId,
    source: ItemLocation,
    character: CharacterId,
    locked: bool,
    revision: SyncRevision,
    created_at: Timestamp,
    expires_at: Timestamp,
}

impl LockPlan {
    pub fn id(&self) -> &PlanId {
        &self.id
    }
    pub fn item(&self) -> &ItemInstanceId {
        &self.item
    }
    pub fn character(&self) -> &CharacterId {
        &self.character
    }
    pub const fn locked(&self) -> bool {
        self.locked
    }
}

/// Bounded evidence: None means no dispatch, Some preserves the exact sanitized
/// gateway outcome, including a refusal or uncertainty, through classification.
/// Observed state is populated only when local state already matches;
/// nothing is sent to Bungie (legacy VerifiedNoOp), not provider verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockExecutionEvidence {
    plan: PlanId,
    attempt: Option<LockOutcome>,
    observed: Option<bool>,
}

impl LockExecutionEvidence {
    pub fn plan_id(&self) -> &PlanId {
        &self.plan
    }
    pub const fn attempt(&self) -> Option<LockOutcome> {
        self.attempt
    }
    pub const fn observed(&self) -> Option<bool> {
        self.observed
    }
}

/// Pre-dispatch refusal. No remote mutation occurred; no PostAction is spent.
#[derive(Debug, Eq, PartialEq)]
pub struct LockExecutionFailure {
    error: MutationError,
    evidence: LockExecutionEvidence,
}

impl LockExecutionFailure {
    pub const fn error(&self) -> MutationError {
        self.error
    }
    pub fn evidence(&self) -> &LockExecutionEvidence {
        &self.evidence
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockVerificationFailureReason {
    /// Local effects or freshness could not be persisted; wire reason retained.
    RefreshFailed,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LockVerificationFailure {
    reason: LockVerificationFailureReason,
    error: Option<MutationError>,
    evidence: LockExecutionEvidence,
}

impl LockVerificationFailure {
    pub const fn reason(&self) -> LockVerificationFailureReason {
        self.reason
    }
    pub const fn error(&self) -> Option<MutationError> {
        self.error
    }
    pub fn evidence(&self) -> &LockExecutionEvidence {
        &self.evidence
    }
}

/// Mirrors equip's terminal vocabulary, but evidence records lock state rather
/// than pretending a lock was a transfer/equip step. IncompleteExecution keeps
/// provider refusal/uncertainty without claiming rollback or safe retry.
#[derive(Debug, Eq, PartialEq)]
pub enum LockExecutionOutcome {
    /// Bungie acknowledged the requested state; not a post-action observation.
    Applied(LockExecutionEvidence),
    /// Committed projection already proves the requested state; no dispatch.
    VerifiedNoOp(LockExecutionEvidence),
    IncompleteExecution(LockExecutionEvidence),
    VerificationFailed(LockVerificationFailure),
}

impl LockExecutionOutcome {
    pub fn evidence(&self) -> &LockExecutionEvidence {
        match self {
            Self::Applied(evidence)
            | Self::VerifiedNoOp(evidence)
            | Self::IncompleteExecution(evidence) => evidence,
            Self::VerificationFailed(failure) => failure.evidence(),
        }
    }
}

impl<G, R, P> MutationService<G, R, P>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository,
    P: PlanIdGenerator,
{
    /// Resolve explicit intent against ordinary freshness, with no mutation.
    pub fn plan_lock(
        &mut self,
        request: &LockRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<LockPlan, MutationError> {
        let selector = selector_request(request);
        authorize(&selector)?;
        let observation = self
            .sync_mut()
            .sync(ProfileSyncReason::Ordinary, policy, now)?;
        let snapshot = self.committed_snapshot()?;
        if request.membership != snapshot.metadata.membership_id {
            return Err(MutationError::WrongMembership);
        }
        let item = self.resolve_item(&selector)?;
        let character = lock_character(&item, &snapshot)?;
        let membership = DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id,
        )?;
        let expires_at = lifetime.expires_at(now)?;
        let id = self.plan_ids_mut().next_plan_id()?;
        Ok(LockPlan {
            id,
            membership,
            item: item.instance_id,
            source: item.location,
            character,
            locked: request.locked,
            revision: observation.revision(),
            created_at: now,
            expires_at,
        })
    }

    /// Ordinary sync → local validation → local-state no-op or one dispatch.
    /// Timestamp ordering is checked without a post-action profile request.
    /// Acknowledged effects retain freshness; storage failure fails closed.
    pub fn execute_lock<L: LockGateway>(
        &mut self,
        gateway: &mut L,
        request: &LockRequest,
        plan: &LockPlan,
        policy: FreshnessPolicy,
        preflight_now: Timestamp,
        post_action_now: Timestamp,
    ) -> Result<LockExecutionOutcome, LockExecutionFailure> {
        let mut evidence = LockExecutionEvidence {
            plan: plan.id.clone(),
            attempt: None,
            observed: None,
        };
        let failure = |error| LockExecutionFailure {
            error,
            evidence: evidence.clone(),
        };
        if post_action_now < preflight_now {
            return Err(failure(MutationError::InvalidTimestamps));
        }
        let selector = selector_request(request);
        authorize(&selector).map_err(failure)?;
        if preflight_now < plan.created_at || preflight_now >= plan.expires_at {
            return Err(failure(MutationError::Expired));
        }
        if request.membership != *plan.membership.membership_id() {
            return Err(failure(MutationError::WrongMembership));
        }
        if request.locked != plan.locked {
            return Err(failure(MutationError::InvalidPlan));
        }
        let observation = self
            .sync_mut()
            .sync(ProfileSyncReason::Ordinary, policy, preflight_now)
            .map_err(|error| failure(error.into()))?;
        let snapshot = self.committed_snapshot().map_err(failure)?;
        if snapshot.metadata.membership_id != request.membership
            || snapshot.metadata.membership_type != plan.membership.membership_type()
        {
            return Err(failure(MutationError::WrongMembership));
        }
        let item = self.resolve_item(&selector).map_err(failure)?;
        if item.instance_id != plan.item {
            return Err(failure(MutationError::WrongItem));
        }
        let character = lock_character(&item, &snapshot).map_err(failure)?;
        // Recheck publication after selector resolution. As with equip, runtime
        // composition must serialize local publication across check-to-dispatch.
        let current = self.committed_snapshot().map_err(failure)?;
        if current.metadata.membership_id != request.membership
            || current.metadata.membership_type != plan.membership.membership_type()
        {
            return Err(failure(MutationError::WrongMembership));
        }
        if current.metadata.revision != observation.revision() {
            return Err(failure(MutationError::StaleRevision));
        }
        if item.locked == request.locked {
            evidence.observed = Some(item.locked);
            return Ok(LockExecutionOutcome::VerifiedNoOp(evidence));
        }
        // As equip does, a locally satisfied state precedes stale-plan refusal.
        // Otherwise changed assumptions require a new plan, not a blind rebind.
        if observation.revision() != plan.revision {
            return Err(failure(MutationError::StaleRevision));
        }
        if item.location != plan.source || character != plan.character {
            return Err(failure(MutationError::SourceMismatch));
        }
        let provider_request = SetLockStateRequest::new(
            plan.membership.clone(),
            plan.item.clone(),
            character,
            plan.locked,
        )
        .map_err(|error| failure(error.into()))?;
        let attempt = gateway
            .set_lock_state(&provider_request)
            .map_err(|error| failure(error.into()))?;
        evidence.attempt = Some(attempt);
        Ok(self.acknowledge_lock_attempt(plan, evidence))
    }

    fn acknowledge_lock_attempt(
        &mut self,
        plan: &LockPlan,
        evidence: LockExecutionEvidence,
    ) -> LockExecutionOutcome {
        let applied = evidence.attempt == Some(LockOutcome::Applied);
        let effects = if applied {
            vec![destiny_core::InventoryEffect::SetLocked {
                item: plan.item.clone(),
                locked: plan.locked,
            }]
        } else {
            Vec::new()
        };
        if let Err(error) = self.persist_attempt_effects(&plan.membership, &effects, applied) {
            return LockExecutionOutcome::VerificationFailed(LockVerificationFailure {
                reason: LockVerificationFailureReason::RefreshFailed,
                error: Some(error.into()),
                evidence,
            });
        }
        if applied {
            LockExecutionOutcome::Applied(evidence)
        } else {
            LockExecutionOutcome::IncompleteExecution(evidence)
        }
    }
}

fn selector_request(request: &LockRequest) -> MoveRequest {
    MoveRequest {
        membership: request.membership.clone(),
        item: request.item.clone(),
        destination: DestinationSelector::vault(),
        intent: request.intent,
    }
}

fn lock_character(
    item: &ItemInstance,
    snapshot: &InventorySnapshot,
) -> Result<CharacterId, MutationError> {
    // Profile projection contains only owned instanced items. Defensive refusal
    // also excludes the provider's non-instanced sentinel rather than posting it.
    if item.instance_id.as_str().is_empty() || item.instance_id.as_str() == "0" {
        return Err(MutationError::MissingItem);
    }
    match &item.location {
        ItemLocation::Character(id)
            if snapshot
                .characters
                .iter()
                .any(|character| character.id == *id) =>
        {
            Ok(id.clone())
        }
        ItemLocation::Vault => snapshot
            .characters
            .first()
            .map(|character| character.id.clone())
            .ok_or(MutationError::MissingDestination),
        _ => Err(MutationError::UnsupportedSource),
    }
}
