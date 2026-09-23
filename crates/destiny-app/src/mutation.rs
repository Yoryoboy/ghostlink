//! Shared application mutation service: semantic move planning and the
//! mandatory preflight gate.
//!
//! # Scope
//!
//! [`MutationService`] is the single adapter-neutral path that turns a
//! validated semantic [`MoveRequest`] into either a bounded [`MovePlan`] or one
//! sanitized typed [`MutationError`]. It composes the existing
//! [`ProfileSyncService`] for freshness and the core-owned pure mutation
//! contracts for policy. It holds no transfer, execution, or provider-mutation
//! seam, so planning can only read the committed projection and can never cause
//! a remote mutation as a side effect.
//!
//! # Explicit intent
//!
//! Every entry point evaluates [`MoveRequest::authorization`] *before* any side
//! effect, including a profile refresh. Absent or ambiguous intent fails closed
//! with a typed error, so omitting intent can never reach the provider through
//! planning. Explicit intent is the whole authorization fact: no preview token,
//! confirmation value, approval ceremony, or second call is accepted or
//! required, which is what lets the MCP adapter invoke a single `move_item`
//! operation directly.
//!
//! # Item and destination resolution
//!
//! Resolution never guesses. An exact [`ItemSelector::Instance`] is resolved by
//! identity through [`SearchRepository::lookup_instance`]. A semantic
//! [`ItemSelector::Semantic`] term is resolved through the existing bounded
//! definition search and then the bounded instance search: a missing, truncated,
//! or multiple match fails closed instead of ranking candidates. A
//! [`DestinationSelector`] resolves through the shared core
//! [`resolve_destination`] rule against the committed characters, so the Vault,
//! one exact character, and one normalized case-insensitive label are the only
//! accepted destinations and an unknown or ambiguous one is rejected.
//!
//! # Planning (DMP-003)
//!
//! [`MutationService::plan_move`] authorizes intent, applies the ordinary
//! freshness gate (an already-fresh projection is not re-fetched), resolves the
//! item and destination, and builds one bounded [`MovePlan`] through the shared
//! [`PlanLifetime`] policy and the injected [`PlanIdGenerator`] seam. The plan
//! binds the concrete item, its current source, the destination, the membership,
//! the committed revision, the creation time, and the bounded expiry. The
//! bounded expiry is computed before a plan identifier is spent, so an
//! overflowing clock never consumes one.
//!
//! # Preflight (DMP-004)
//!
//! [`MutationService::preflight_move`] authorizes intent again and forces
//! [`ProfileSyncReason::Preflight`], which refreshes regardless of freshness,
//! then re-resolves the stable item identity, the destination, the current
//! source, the membership, and the committed revision against the refreshed
//! projection and evaluates the shared core [`evaluate_mutation`] policy. Every
//! non-valid [`MutationPolicyOutcome`] is mapped to a specific typed error
//! instead of being reported as an approximate or aggregate failure.
//!
//! Preflight then re-binds the *request's* own selectors: the requested item and
//! destination are resolved again against the same refreshed committed
//! projection and must match the plan's item and destination uniquely. The
//! plan-anchored policy evaluation proves the plan is still internally
//! consistent; this second resolution proves the caller's intent refers to that
//! same plan, so a request for one item can never preflight a plan for another.
//! A missing, ambiguous, or mismatched selector is rejected with the existing
//! typed item/destination outcome.
//!
//! The current source is only *known* when the item re-resolves uniquely; when
//! the item cannot be re-resolved, the plan's own source is submitted so the
//! shared policy reports the item resolution failure instead of a misleading
//! source mismatch. The plan's revision is never rebound: any change to the
//! committed projection advances its revision, and the policy invalidates the
//! plan before an execution could act on changed assumptions.
//!
//! # Composed entry point
//!
//! [`MutationService::prepare_move`] is the single shared adapter entry: it
//! plans and then preflights, so a presentation adapter cannot obtain a plan
//! without passing the mandatory gate. The two phases stay independently
//! callable because they own distinct contracts.

use std::fmt;

use crate::sync::ProfileSyncService;
use destiny_core::{
    AppError, DefinitionSearchQuery, DestinationSelector, ErrorCategory, FreshnessPolicy,
    InstanceSearchQuery, IntentAuthorizationOutcome, InventoryRepository, InventorySnapshot,
    ItemHashFilter, ItemInstance, ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT,
    MovePlan, MovePlanValidation, MoveRequest, MutationPolicyInput, MutationPolicyOutcome,
    PlanIdGenerator, PlanLifetime, ProfileGateway, ProfileSyncReason, Resolution, SearchRepository,
    Timestamp, evaluate_mutation, resolve_destination,
};

/// The sanitized, typed failure of semantic move planning or preflight.
///
/// The record carries no item identifier, membership identifier, provider
/// payload, or adapter value, so it is safe to present. Every variant maps to
/// one shared [`ErrorCategory`] through [`MutationError::category`], and one
/// variant preserves a propagated seam failure category without losing it.
///
/// The core policy owns a fixed outcome precedence, so several of its defensive
/// outcomes are only reachable from an invalid or contract-violating projection
/// rather than from a healthy one. They are still mapped here, distinctly and
/// exhaustively, because a sanitized policy result must never be collapsed into
/// a generic failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationError {
    /// A seam failure propagated unchanged; the sanitized category is kept.
    Application(ErrorCategory),
    /// The request carried no explicit intent to move one item.
    MissingIntent,
    /// The request did not attribute intent to exactly one move.
    AmbiguousIntent,
    /// The requested item matched nothing in the committed projection.
    MissingItem,
    /// The requested item matched more than one concrete instance.
    AmbiguousItem,
    /// The requested destination matched nothing in the committed projection.
    MissingDestination,
    /// The requested destination matched more than one character.
    AmbiguousDestination,
    /// The requested destination is already the item's current location.
    SameLocation,
    /// A plan endpoint lies outside the Vault/character contract.
    UnsupportedLocation,
    /// The item's re-resolved current location is unsupported.
    UnsupportedSource,
    /// The item is no longer at the location the plan recorded.
    SourceMismatch,
    /// The plan's bounded lifetime elapsed before preflight.
    Expired,
    /// The committed projection changed after the plan was created.
    StaleRevision,
    /// The plan binds a different membership than the preflight request.
    WrongMembership,
    /// The plan no longer matches the requested item.
    WrongItem,
    /// The plan no longer matches the requested destination.
    WrongDestination,
    /// The planned item no longer resolves in the committed projection.
    MissingResolution,
    /// The planned item resolves ambiguously in the committed projection.
    AmbiguousResolution,
    /// The shared plan validation rejected the requested move.
    InvalidPlan,
    /// A refresh succeeded but no committed projection became visible.
    InvalidProjection,
}

impl MutationError {
    /// The shared sanitized category for this failure.
    pub const fn category(self) -> ErrorCategory {
        match self {
            Self::Application(category) => category,
            Self::MissingIntent => ErrorCategory::AuthorizationRequired,
            Self::AmbiguousIntent
            | Self::AmbiguousItem
            | Self::AmbiguousDestination
            | Self::AmbiguousResolution => ErrorCategory::AmbiguousMatch,
            Self::MissingItem
            | Self::MissingDestination
            | Self::MissingResolution
            | Self::SourceMismatch
            | Self::Expired
            | Self::StaleRevision
            | Self::WrongMembership
            | Self::WrongItem
            | Self::WrongDestination => ErrorCategory::StaleState,
            Self::SameLocation | Self::UnsupportedLocation | Self::UnsupportedSource => {
                ErrorCategory::UnsupportedOperation
            }
            Self::InvalidPlan | Self::InvalidProjection => ErrorCategory::LocalStorage,
        }
    }

    /// Maps one shared core policy outcome to its specific typed failure.
    ///
    /// [`MutationPolicyOutcome::Valid`] has no failure meaning; it maps to
    /// [`MutationError::InvalidPlan`] only so the mapping stays total, and
    /// callers that already handled validity never observe it.
    pub const fn from_policy_outcome(outcome: MutationPolicyOutcome) -> Self {
        match outcome {
            MutationPolicyOutcome::Valid => Self::InvalidPlan,
            MutationPolicyOutcome::Expired => Self::Expired,
            MutationPolicyOutcome::StaleRevision => Self::StaleRevision,
            MutationPolicyOutcome::WrongMembership => Self::WrongMembership,
            MutationPolicyOutcome::WrongItem => Self::WrongItem,
            MutationPolicyOutcome::WrongDestination => Self::WrongDestination,
            MutationPolicyOutcome::MissingResolution => Self::MissingResolution,
            MutationPolicyOutcome::AmbiguousResolution => Self::AmbiguousResolution,
            MutationPolicyOutcome::MissingDestination => Self::MissingDestination,
            MutationPolicyOutcome::AmbiguousDestination => Self::AmbiguousDestination,
            MutationPolicyOutcome::UnsupportedLocation => Self::UnsupportedLocation,
            MutationPolicyOutcome::UnsupportedSource => Self::UnsupportedSource,
            MutationPolicyOutcome::SourceMismatch => Self::SourceMismatch,
        }
    }
}

impl From<AppError> for MutationError {
    fn from(error: AppError) -> Self {
        Self::Application(error.category())
    }
}

impl From<MutationError> for AppError {
    fn from(error: MutationError) -> Self {
        Self::new(error.category())
    }
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Application(category) => write!(formatter, "{}", AppError::new(*category)),
            Self::MissingIntent => {
                formatter.write_str("explicit intent to move one item is required")
            }
            Self::AmbiguousIntent => {
                formatter.write_str("intent does not identify exactly one move")
            }
            Self::MissingItem => {
                formatter.write_str("the requested item is not in the committed projection")
            }
            Self::AmbiguousItem => {
                formatter.write_str("the requested item matched more than one item")
            }
            Self::MissingDestination => {
                formatter.write_str("the requested destination could not be resolved")
            }
            Self::AmbiguousDestination => {
                formatter.write_str("the requested destination matched more than one character")
            }
            Self::SameLocation => {
                formatter.write_str("the item is already at the requested destination")
            }
            Self::UnsupportedLocation => {
                formatter.write_str("the source or destination location is unsupported")
            }
            Self::UnsupportedSource => {
                formatter.write_str("the item's current location is unsupported")
            }
            Self::SourceMismatch => {
                formatter.write_str("the item is no longer at its planned source")
            }
            Self::Expired => formatter.write_str("the move plan has expired"),
            Self::StaleRevision => {
                formatter.write_str("local state changed after the plan was created")
            }
            Self::WrongMembership => {
                formatter.write_str("the plan belongs to a different membership")
            }
            Self::WrongItem => formatter.write_str("the plan no longer matches the requested item"),
            Self::WrongDestination => {
                formatter.write_str("the plan no longer matches the requested destination")
            }
            Self::MissingResolution => {
                formatter.write_str("the planned item is no longer in the committed projection")
            }
            Self::AmbiguousResolution => {
                formatter.write_str("the planned item resolved ambiguously")
            }
            Self::InvalidPlan => formatter.write_str("the move plan could not be constructed"),
            Self::InvalidProjection => formatter.write_str("local projection state is unavailable"),
        }
    }
}

impl std::error::Error for MutationError {}

/// Plans and preflights one semantic `move_item` request.
///
/// The service owns the sync orchestration and the plan identifier seam by
/// value, exactly as [`crate::queries::QueryService`] owns its sync service, so
/// both presentation adapters share one policy path instead of each
/// re-implementing resolution, binding, or preflight.
pub struct MutationService<G, R, P> {
    sync: ProfileSyncService<G, R>,
    plan_ids: P,
}

impl<G, R, P> MutationService<G, R, P>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository,
    P: PlanIdGenerator,
{
    pub fn new(sync: ProfileSyncService<G, R>, plan_ids: P) -> Self {
        Self { sync, plan_ids }
    }

    /// Borrows the composed synchronization service.
    pub fn sync(&self) -> &ProfileSyncService<G, R> {
        &self.sync
    }

    /// Mutably borrows the composed synchronization service.
    pub fn sync_mut(&mut self) -> &mut ProfileSyncService<G, R> {
        &mut self.sync
    }

    /// Borrows the repository so callers can read or seed the projection.
    pub fn repository(&self) -> &R {
        self.sync.repository()
    }

    /// Mutably borrows the repository for test or composition setup.
    pub fn repository_mut(&mut self) -> &mut R {
        self.sync.repository_mut()
    }

    /// Borrows the injected plan identifier generator.
    pub fn plan_ids(&self) -> &P {
        &self.plan_ids
    }

    /// Mutably borrows the plan identifier generator for test or composition setup.
    pub fn plan_ids_mut(&mut self) -> &mut P {
        &mut self.plan_ids
    }

    /// DMP-003: authorizes intent, resolves the request, and builds one plan.
    ///
    /// Explicit intent is validated before any side effect, the ordinary
    /// freshness gate runs next, and the item and destination are resolved
    /// against the committed projection without guessing. Nothing here calls a
    /// provider mutation endpoint: the only seams touched are the profile
    /// gateway through the sync service and the read-only repository.
    pub fn plan_move(
        &mut self,
        request: &MoveRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        authorize(request)?;
        let outcome = self
            .sync
            .sync(ProfileSyncReason::Ordinary, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self.committed_snapshot()?;
        let item = self.resolve_item(request)?;
        let destination = self.resolve_plan_destination(request, &snapshot.characters)?;
        self.build_plan(
            request,
            &item,
            destination,
            outcome.revision(),
            lifetime,
            now,
        )
    }

    /// DMP-004: re-checks one plan against a mandatory fresh preflight.
    ///
    /// Intent is validated again before the refresh, then
    /// [`ProfileSyncReason::Preflight`] forces one synchronized projection
    /// regardless of freshness. The stable item identity, the destination, the
    /// current source, the membership, and the committed revision are
    /// re-resolved, and the shared core policy is evaluated. `Ok(())` means the
    /// plan survived preflight; every non-valid policy outcome is a specific
    /// typed error.
    ///
    /// The request's own selectors are then resolved against the same refreshed
    /// projection and must match the plan's item and destination uniquely, so a
    /// caller cannot carry intent for one move and preflight a plan for another.
    pub fn preflight_move(
        &mut self,
        request: &MoveRequest,
        plan: &MovePlan,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<(), MutationError> {
        authorize(request)?;
        let outcome = self
            .sync
            .sync(ProfileSyncReason::Preflight, policy, now)
            .map_err(MutationError::from)?;
        let snapshot = self.committed_snapshot()?;
        let (resolved_item, current_source) = self.reresolve_item(plan)?;
        let resolved_destination = match &plan.destination {
            ItemLocation::Vault => Resolution::Unique(ItemLocation::Vault),
            ItemLocation::Character(id) => resolve_destination(
                &DestinationSelector::Character(id.clone()),
                &snapshot.characters,
            ),
            // An unsupported planned destination is rejected by the shared core
            // policy before the re-resolved destination is consulted.
            ItemLocation::Other(_) => Resolution::Missing,
        };
        let input = MutationPolicyInput {
            now,
            membership: request.membership.clone(),
            current_revision: outcome.revision(),
            current_source,
            resolved_item,
            resolved_destination,
        };
        match evaluate_mutation(plan, &input) {
            MutationPolicyOutcome::Valid => {}
            outcome => return Err(MutationError::from_policy_outcome(outcome)),
        }
        // The plan-anchored evaluation above proves the plan is still internally
        // consistent; this request-anchored resolution proves the caller's intent
        // refers to that same plan. Both selectors are resolved against the same
        // refreshed projection and must match the plan uniquely, so a request for
        // one item can never preflight a plan for another.
        let requested_item = self.resolve_item(request)?;
        if requested_item.instance_id != plan.item {
            return Err(MutationError::WrongItem);
        }
        let requested_destination = self.resolve_plan_destination(request, &snapshot.characters)?;
        if requested_destination != plan.destination {
            return Err(MutationError::WrongDestination);
        }
        Ok(())
    }

    /// The single shared adapter entry: plan, then pass the mandatory preflight.
    ///
    /// Both freshness gates run: planning uses the ordinary gate (a fresh
    /// projection is not re-fetched) and preflight always refreshes. A
    /// presentation adapter that calls this cannot obtain a plan that skipped
    /// the preflight policy.
    pub fn prepare_move(
        &mut self,
        request: &MoveRequest,
        lifetime: PlanLifetime,
        policy: FreshnessPolicy,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        let plan = self.plan_move(request, lifetime, policy, now)?;
        self.preflight_move(request, &plan, policy, now)?;
        Ok(plan)
    }

    /// Returns the committed projection, or reports that none is visible.
    fn committed_snapshot(&self) -> Result<InventorySnapshot, MutationError> {
        self.repository()
            .load_current()
            .map_err(MutationError::from)?
            .ok_or(MutationError::InvalidProjection)
    }

    /// Resolves the requested item to exactly one concrete instance.
    fn resolve_item(&self, request: &MoveRequest) -> Result<ItemInstance, MutationError> {
        match &request.item {
            ItemSelector::Instance(instance_id) => {
                match self
                    .repository()
                    .lookup_instance(instance_id)
                    .map_err(MutationError::from)?
                {
                    Resolution::Unique(instance) => Ok(instance),
                    Resolution::Missing => Err(MutationError::MissingItem),
                    Resolution::Ambiguous(_) => Err(MutationError::AmbiguousItem),
                }
            }
            ItemSelector::Semantic(term) => {
                let definitions = self
                    .repository()
                    .search_definitions(&DefinitionSearchQuery {
                        text: Some(term.clone()),
                        item_type: None,
                        tags: Vec::new(),
                        limit: MAX_SEARCH_LIMIT,
                    })
                    .map_err(MutationError::from)?;
                if definitions.is_empty() {
                    return Err(MutationError::MissingItem);
                }
                // A term that matches more definitions than the bounded
                // instance filter can carry is ambiguous, never truncated. The
                // term itself never appears in the error.
                if definitions.len() > MAX_HASH_FILTERS {
                    return Err(MutationError::AmbiguousItem);
                }
                let instances = self
                    .repository()
                    .search_instances(&InstanceSearchQuery {
                        item_hashes: ItemHashFilter::AnyOf(
                            definitions
                                .iter()
                                .map(|definition| definition.hash)
                                .collect(),
                        ),
                        perk_hash: None,
                        character_id: None,
                        limit: MAX_SEARCH_LIMIT,
                    })
                    .map_err(MutationError::from)?;
                match instances.as_slice() {
                    [] => Err(MutationError::MissingItem),
                    [instance] => Ok(instance.clone()),
                    _ => Err(MutationError::AmbiguousItem),
                }
            }
        }
    }

    /// Resolves the requested destination to exactly one supported location.
    fn resolve_plan_destination(
        &self,
        request: &MoveRequest,
        characters: &[destiny_core::Character],
    ) -> Result<ItemLocation, MutationError> {
        match resolve_destination(&request.destination, characters) {
            Resolution::Unique(location) => Ok(location),
            Resolution::Missing => Err(MutationError::MissingDestination),
            Resolution::Ambiguous(_) => Err(MutationError::AmbiguousDestination),
        }
    }

    /// Re-resolves the plan's stable item identity and its current location.
    ///
    /// A unique resolution gives both the identity and the current source. When
    /// the item does not resolve, the plan's own source is submitted so the
    /// shared policy reports the resolution failure rather than a misleading
    /// source mismatch.
    fn reresolve_item(
        &self,
        plan: &MovePlan,
    ) -> Result<(Resolution<destiny_core::ItemInstanceId>, ItemLocation), MutationError> {
        match self
            .repository()
            .lookup_instance(&plan.item)
            .map_err(MutationError::from)?
        {
            Resolution::Unique(instance) => {
                let source = instance.location.clone();
                Ok((Resolution::Unique(instance.instance_id), source))
            }
            Resolution::Missing => Ok((Resolution::Missing, plan.source.clone())),
            Resolution::Ambiguous(instances) => Ok((
                Resolution::Ambiguous(
                    instances
                        .into_iter()
                        .map(|instance| instance.instance_id)
                        .collect(),
                ),
                plan.source.clone(),
            )),
        }
    }

    /// Builds one bounded, revision-bound plan through the injected id seam.
    fn build_plan(
        &mut self,
        request: &MoveRequest,
        item: &ItemInstance,
        destination: ItemLocation,
        revision: destiny_core::SyncRevision,
        lifetime: PlanLifetime,
        now: Timestamp,
    ) -> Result<MovePlan, MutationError> {
        // The bounded expiry is computed before a plan identifier is spent.
        let expires_at = lifetime.expires_at(now).map_err(MutationError::from)?;
        let plan_id = self.plan_ids.next_plan_id().map_err(MutationError::from)?;
        MovePlan::new(
            plan_id,
            request.membership.clone(),
            item.instance_id.clone(),
            item.location.clone(),
            destination,
            revision,
            now,
            expires_at,
        )
        .map_err(|validation| match validation {
            MovePlanValidation::SameLocation => MutationError::SameLocation,
            MovePlanValidation::UnsupportedLocation => MutationError::UnsupportedLocation,
            MovePlanValidation::InvalidTimestamps | MovePlanValidation::LifetimeExceeded => {
                MutationError::InvalidPlan
            }
        })
    }
}

/// Rejects absent or ambiguous explicit intent before any side effect.
fn authorize(request: &MoveRequest) -> Result<(), MutationError> {
    match request.authorization() {
        IntentAuthorizationOutcome::Authorized => Ok(()),
        IntentAuthorizationOutcome::MissingIntent => Err(MutationError::MissingIntent),
        IntentAuthorizationOutcome::AmbiguousIntent => Err(MutationError::AmbiguousIntent),
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::{MutationError, MutationService};
    use crate::sync::ProfileSyncService;
    use crate::test_support::{
        DeterministicPlanIds, InMemoryInventoryRepository, InMemorySearchRepository,
        QueuedProfileGateway,
    };
    use destiny_core::{
        AppError, Character, CharacterId, DefinitionSearchQuery, DefinitionVersion,
        DestinationSelector, ErrorCategory, FreshnessPolicy, InstanceSearchQuery,
        InventoryRepository, InventorySnapshot, InventorySnapshotInput, ItemDefinition, ItemHash,
        ItemInstance, ItemInstanceId, ItemLocation, ItemSelector, MAX_HASH_FILTERS,
        MoveIntentAuthorization, MoveRequest, MutationPolicyOutcome, PlanId, PlanIdGenerator,
        PlanLifetime, ProfileGateway, Resolution, SearchRepository, SourceMarker, SyncRevision,
        Timestamp,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(50_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    const LIFETIME: PlanLifetime = PlanLifetime::MAX;
    const DEFINITION_VERSION: &str = "definitions-v1";
    const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

    /// A narrow test-local composite exposing one repository for both seams.
    ///
    /// Committing a profile refresh also mirrors its instances into the search
    /// projection, so a plan resolved after a refresh observes the new
    /// projection. Every read-seam call is recorded so tests can prove the exact
    /// sync-then-resolve ordering and that no mutation seam exists.
    #[derive(Clone, Debug, Default)]
    struct CompositeRepository {
        inventory: InMemoryInventoryRepository,
        search: InMemorySearchRepository,
        hides_current: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions),
                hides_current: false,
                calls: RefCell::new(Vec::new()),
            }
        }

        /// Hides every committed projection to model an unavailable local store.
        fn hiding_current() -> Self {
            Self {
                hides_current: true,
                ..Self::default()
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        fn recorded_calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }

        /// Commits one valid projection, mirrors its instances, and reports the revision.
        fn commit(&mut self, input: InventorySnapshotInput) -> SyncRevision {
            let snapshot = self
                .inventory
                .replace_current(input)
                .expect("valid seed snapshot");
            self.search.set_instances(snapshot.items.clone());
            snapshot.metadata.revision
        }

        /// Seeds the search projection with structural duplicates.
        ///
        /// A structurally valid projection can never carry duplicate instance
        /// identifiers, so this models the contract-violating state the search
        /// port documents as the only route to an ambiguous exact lookup.
        fn seed_instances(&mut self, instances: Vec<ItemInstance>) {
            self.search.set_instances(instances);
        }
    }

    impl InventoryRepository for CompositeRepository {
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.record("load_current");
            if self.hides_current {
                return Ok(None);
            }
            self.inventory.load_current()
        }

        fn replace_current(
            &mut self,
            input: InventorySnapshotInput,
        ) -> Result<InventorySnapshot, AppError> {
            self.record("replace_current");
            let snapshot = self.inventory.replace_current(input)?;
            self.search.set_instances(snapshot.items.clone());
            Ok(snapshot)
        }
    }

    impl SearchRepository for CompositeRepository {
        fn search_definitions(
            &self,
            query: &DefinitionSearchQuery,
        ) -> Result<Vec<ItemDefinition>, AppError> {
            self.record("search_definitions");
            self.search.search_definitions(query)
        }

        fn search_instances(
            &self,
            query: &InstanceSearchQuery,
        ) -> Result<Vec<ItemInstance>, AppError> {
            self.record("search_instances");
            self.search.search_instances(query)
        }

        fn lookup_instance(
            &self,
            instance_id: &ItemInstanceId,
        ) -> Result<Resolution<ItemInstance>, AppError> {
            self.record("lookup_instance");
            self.search.lookup_instance(instance_id)
        }
    }

    /// A read-only spy over the fake gateway the sync service owns by value.
    struct SharedGateway(Rc<RefCell<QueuedProfileGateway>>);

    impl ProfileGateway for SharedGateway {
        fn resolve_membership(&mut self) -> Result<destiny_core::DestinyMembership, AppError> {
            self.0.borrow_mut().resolve_membership()
        }

        fn load_inventory(
            &mut self,
            membership: &destiny_core::DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.borrow_mut().load_inventory(membership)
        }
    }

    /// A plan identifier seam that reports exhaustion immediately.
    #[derive(Debug, Default)]
    struct ExhaustedPlanIds;

    impl PlanIdGenerator for ExhaustedPlanIds {
        fn next_plan_id(&mut self) -> Result<PlanId, AppError> {
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        }
    }

    type Service = MutationService<SharedGateway, CompositeRepository, DeterministicPlanIds>;

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
    }

    fn service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        repository: CompositeRepository,
    ) -> Service {
        MutationService::new(
            ProfileSyncService::new(SharedGateway(Rc::clone(shared)), repository),
            DeterministicPlanIds::starting_at(1),
        )
    }

    fn membership_id() -> destiny_core::MembershipId {
        destiny_core::MembershipId::new(MEMBERSHIP_IDENTIFIER)
    }

    fn membership() -> destiny_core::DestinyMembership {
        destiny_core::DestinyMembership::new(destiny_core::MembershipType::new(3), membership_id())
            .expect("valid membership")
    }

    fn character(id: &str, name: Option<&str>, class: Option<&str>) -> Character {
        Character {
            id: CharacterId::new(id),
            name: name.map(str::to_owned),
            class: class.map(str::to_owned),
            race: None,
            gender: None,
            light: None,
        }
    }

    fn warlock_character() -> Character {
        character("warlock", Some("Warlock"), None)
    }

    fn definition(hash: i64, name: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(DEFINITION_VERSION),
        }
    }

    fn item_id(value: &str) -> ItemInstanceId {
        ItemInstanceId::new(value)
    }

    fn instance(instance_id: &str, item_hash: i64, location: ItemLocation) -> ItemInstance {
        ItemInstance {
            instance_id: item_id(instance_id),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            perks: Vec::new(),
        }
    }

    fn warlock() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("warlock"))
    }

    fn hunter() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("hunter"))
    }

    fn projection(
        checked_at: Timestamp,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: membership_id(),
            membership_type: destiny_core::MembershipType::new(3),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at,
            source: SourceMarker::Profile,
            characters,
            items,
        }
    }

    fn fresh_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - 100)
    }

    fn stale_checked_at() -> Timestamp {
        Timestamp::new(NOW.seconds() - TTL.ttl_seconds - 1)
    }

    fn queue_refresh(shared: &Rc<RefCell<QueuedProfileGateway>>, input: InventorySnapshotInput) {
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared.borrow_mut().queue_inventory(Ok(input));
    }

    fn request(item: ItemSelector, destination: DestinationSelector) -> MoveRequest {
        MoveRequest {
            membership: membership_id(),
            item,
            destination,
            intent: MoveIntentAuthorization::Explicit,
        }
    }

    fn exact(instance_id: &str) -> ItemSelector {
        ItemSelector::instance(self::item_id(instance_id))
    }

    fn semantic(term: &str) -> ItemSelector {
        ItemSelector::semantic(term).expect("valid semantic term")
    }

    fn exact_character(id: &str) -> DestinationSelector {
        DestinationSelector::character(CharacterId::new(id))
    }

    fn character_label(label: &str) -> DestinationSelector {
        DestinationSelector::character_label(label).expect("valid destination label")
    }

    /// One fresh projection holding a Vault item and one warlock character.
    fn fresh_vault_projection() -> InventorySnapshotInput {
        projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        )
    }

    #[test]
    fn plan_move_builds_one_direct_step_from_the_vault_to_an_exact_character() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        let revision = repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.plan_id, PlanId::new("plan-1"));
        assert_eq!(plan.membership, membership_id());
        assert_eq!(plan.item, item_id("item-1"));
        assert_eq!(plan.source, ItemLocation::Vault);
        assert_eq!(plan.destination, warlock());
        assert_eq!(plan.revision, revision);
        assert_eq!(plan.created_at(), NOW);
        assert_eq!(
            plan.expires_at(),
            Timestamp::new(NOW.seconds() + LIFETIME.ttl_seconds())
        );
        assert_eq!(plan.step_count(), 1);
        assert_eq!(plan.steps()[0].ordinal, 1);
        assert_eq!(plan.steps()[0].from, ItemLocation::Vault);
        assert_eq!(plan.steps()[0].to, warlock());
        assert_eq!(plan.effects().item, item_id("item-1"));
        assert!(!plan.effects().via_vault);
        assert_eq!(service.plan_ids().calls(), 1);
        // The projection was already fresh, so the ordinary gate skipped
        // acquisition entirely and planning stayed a pure read.
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(
            service.repository().recorded_calls(),
            ["load_current", "load_current", "lookup_instance"]
        );
    }

    #[test]
    fn plan_move_resolves_a_semantic_item_through_bounded_search() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.item, item_id("item-1"));
        assert_eq!(plan.source, ItemLocation::Vault);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "search_definitions",
                "search_instances"
            ]
        );
    }

    #[test]
    fn plan_move_rejects_a_semantic_item_that_matches_nothing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("thorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing item");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_a_semantic_item_that_matches_two_instances() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 1, warlock()),
            ],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous item");

        assert_eq!(error, MutationError::AmbiguousItem);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_treats_a_term_matching_too_many_definitions_as_ambiguous() {
        let shared = gateway();
        let definitions: Vec<ItemDefinition> = (0..(MAX_HASH_FILTERS + 1))
            .map(|index| definition(index as i64 + 1, &format!("Shared Name {index}")))
            .collect();
        let mut repository = CompositeRepository::with_definitions(definitions);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(semantic("shared"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("too many definitions");

        assert_eq!(error, MutationError::AmbiguousItem);
        // The instance search was never reached, so no partial candidate list
        // could have been truncated into an accidental selection.
        assert_eq!(
            service.repository().recorded_calls(),
            ["load_current", "load_current", "search_definitions"]
        );
    }

    #[test]
    fn plan_move_rejects_a_missing_exact_instance() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("absent"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing instance");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_an_ambiguous_exact_instance() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        repository.seed_instances(vec![
            instance("item-1", 1, ItemLocation::Vault),
            instance("item-1", 1, warlock()),
        ]);
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous instance");

        assert_eq!(error, MutationError::AmbiguousItem);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_builds_one_direct_step_to_the_vault() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), DestinationSelector::vault()),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.source, warlock());
        assert_eq!(plan.destination, ItemLocation::Vault);
        assert_eq!(plan.step_count(), 1);
        assert_eq!(plan.steps()[0].from, warlock());
        assert_eq!(plan.steps()[0].to, ItemLocation::Vault);
        assert!(!plan.effects().via_vault);
    }

    #[test]
    fn plan_move_resolves_a_character_label_case_insensitively() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), character_label("WARLOCK")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.destination, warlock());
    }

    #[test]
    fn plan_move_rejects_an_unknown_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("titan")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("unknown destination");

        assert_eq!(error, MutationError::MissingDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_an_ambiguous_destination_label() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                character("titan-a", Some("Alpha"), Some("Titan")),
                character("titan-b", Some("Bravo"), Some("Titan")),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), character_label("titan")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("ambiguous destination");

        assert_eq!(error, MutationError::AmbiguousDestination);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_routes_a_character_to_character_move_through_the_vault() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("hunter")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.step_count(), 2);
        assert_eq!(plan.steps()[0].from, warlock());
        assert_eq!(plan.steps()[0].to, ItemLocation::Vault);
        assert_eq!(plan.steps()[1].from, ItemLocation::Vault);
        assert_eq!(plan.steps()[1].to, hunter());
        assert!(plan.effects().via_vault);
    }

    #[test]
    fn plan_move_rejects_a_no_op_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, warlock())],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("no-op move");

        assert_eq!(error, MutationError::SameLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn plan_move_rejects_an_unsupported_source_location() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![instance(
                "item-1",
                1,
                ItemLocation::other("future-location"),
            )],
        ));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("unsupported source");

        assert_eq!(error, MutationError::UnsupportedLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn plan_move_rejects_absent_intent_before_any_side_effect() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let mut move_request = request(exact("item-1"), exact_character("warlock"));
        move_request.intent = MoveIntentAuthorization::Absent;

        let error = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect_err("absent intent");

        assert_eq!(error, MutationError::MissingIntent);
        assert_eq!(error.category(), ErrorCategory::AuthorizationRequired);
        // Nothing was read, refreshed, or planned: intent is validated first.
        assert!(service.repository().recorded_calls().is_empty());
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_rejects_ambiguous_intent_before_any_side_effect() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let mut move_request = request(exact("item-1"), exact_character("warlock"));
        move_request.intent = MoveIntentAuthorization::Ambiguous;

        let error = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect_err("ambiguous intent");

        assert_eq!(error, MutationError::AmbiguousIntent);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
        assert!(service.repository().recorded_calls().is_empty());
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_refreshes_a_stale_projection_and_binds_the_new_revision() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        queue_refresh(
            &shared,
            // One extra instance is a real content change, so the repository
            // advances its revision while item-1 stays in the Vault.
            projection(
                NOW,
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, warlock()),
                ],
            ),
        );
        let mut service = service(&shared, repository);

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.revision, SyncRevision::new(2));
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn plan_move_bounds_the_plan_window_by_the_supplied_lifetime() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");

        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                lifetime,
                TTL,
                NOW,
            )
            .expect("plan");

        assert_eq!(plan.created_at(), NOW);
        assert_eq!(plan.expires_at(), Timestamp::new(NOW.seconds() + 30));
        assert_eq!(
            plan.expires_at().seconds() - plan.created_at().seconds(),
            30
        );
    }

    #[test]
    fn plan_move_reports_clock_overflow_without_spending_a_plan_id() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        // The overflowing timestamp is far in the future, so the ordinary gate
        // still refreshes before the bounded expiry is computed.
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                Timestamp::new(u64::MAX),
            )
            .expect_err("overflowing clock");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ClockOverflow)
        );
        assert_eq!(error.category(), ErrorCategory::ClockOverflow);
        assert_eq!(service.plan_ids().calls(), 0);
    }

    #[test]
    fn plan_move_propagates_plan_id_exhaustion() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service: MutationService<SharedGateway, CompositeRepository, ExhaustedPlanIds> =
            MutationService::new(
                ProfileSyncService::new(SharedGateway(Rc::clone(&shared)), repository),
                ExhaustedPlanIds,
            );

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("exhausted ids");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::PlanIdExhausted)
        );
    }

    #[test]
    fn plan_move_propagates_a_refresh_failure_with_its_category() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("provider failure");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ProviderUnavailable)
        );
        assert_eq!(error.category(), ErrorCategory::ProviderUnavailable);
    }

    #[test]
    fn plan_move_touches_only_read_seams() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            stale_checked_at(),
            vec![warlock_character()],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );
        let mut service = service(&shared, repository);

        service
            .plan_move(
                &request(semantic("gjallarhorn"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");

        // The whole planning path is one refresh plus reads: the only write is
        // the repository-owned projection publication, and there is no
        // transfer, execution, or provider-mutation seam to call.
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "replace_current",
                "load_current",
                "search_definitions",
                "search_instances"
            ]
        );
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn plan_move_reports_an_invalid_projection_when_no_snapshot_is_visible() {
        let shared = gateway();
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, CompositeRepository::hiding_current());

        let error = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("no visible projection");

        assert_eq!(error, MutationError::InvalidProjection);
        assert_eq!(error.category(), ErrorCategory::LocalStorage);
    }

    #[test]
    fn preflight_confirms_a_valid_plan_and_forces_a_refresh() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        assert_eq!(service.repository().recorded_calls().len(), 3);
        // The preflight refresh re-commits identical content, so the revision is
        // retained and the plan stays valid.
        queue_refresh(&shared, fresh_vault_projection());

        service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect("valid preflight");

        // Preflight refreshed even though the projection was fresh: an ordinary
        // gate would have skipped acquisition here.
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "lookup_instance",
                "load_current",
                "replace_current",
                "load_current",
                "lookup_instance",
                "lookup_instance"
            ]
        );
        assert_eq!(plan.revision, SyncRevision::new(1));
    }

    #[test]
    fn preflight_rejects_absent_intent_before_syncing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut unauthorized = move_request.clone();
        unauthorized.intent = MoveIntentAuthorization::Absent;

        let error = service
            .preflight_move(&unauthorized, &plan, TTL, NOW)
            .expect_err("absent intent");

        assert_eq!(error, MutationError::MissingIntent);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn preflight_rejects_ambiguous_intent_before_syncing() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut unauthorized = move_request.clone();
        unauthorized.intent = MoveIntentAuthorization::Ambiguous;

        let error = service
            .preflight_move(&unauthorized, &plan, TTL, NOW)
            .expect_err("ambiguous intent");

        assert_eq!(error, MutationError::AmbiguousIntent);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn preflight_rejects_an_expired_plan() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let lifetime = PlanLifetime::new(30).expect("valid lifetime");
        let plan = service
            .plan_move(&move_request, lifetime, TTL, NOW)
            .expect("plan");
        queue_refresh(&shared, fresh_vault_projection());
        let later = Timestamp::new(NOW.seconds() + 30);

        let error = service
            .preflight_move(&move_request, &plan, TTL, later)
            .expect_err("expired plan");

        assert_eq!(error, MutationError::Expired);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_plan_whose_projection_changed() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        // The item moved to a character, so the committed content changed and
        // the repository advances its revision.
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, warlock())],
            ),
        );

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("changed revision");

        assert_eq!(error, MutationError::StaleRevision);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_vanished_item_resolution() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        // A plan whose item is not in the refreshed projection must be reported
        // as an unresolvable identity instead of being trusted.
        plan.item = item_id("vanished");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("vanished item");

        assert_eq!(error, MutationError::MissingResolution);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_destination_character_that_no_longer_exists() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        plan.destination = hunter();
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("missing destination");

        assert_eq!(error, MutationError::MissingDestination);
    }

    #[test]
    fn preflight_reports_an_unsupported_plan_endpoint() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let mut plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        plan.destination = ItemLocation::other("future-location");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("unsupported endpoint");

        assert_eq!(error, MutationError::UnsupportedLocation);
        assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    }

    #[test]
    fn preflight_rejects_a_plan_for_another_membership() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        let mut other = move_request.clone();
        other.membership = destiny_core::MembershipId::new("99999999999999999");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(&other, &plan, TTL, NOW)
            .expect_err("wrong membership");

        assert_eq!(error, MutationError::WrongMembership);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_exact_item() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 2, ItemLocation::Vault),
            ],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, ItemLocation::Vault),
                ],
            ),
        );

        // The plan is internally valid, but the request names a different
        // item, so intent for item-2 must not preflight a plan for item-1.
        let error = service
            .preflight_move(
                &request(exact("item-2"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong exact item");

        assert_eq!(error, MutationError::WrongItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_semantic_item() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(vec![
            definition(1, "Gjallarhorn"),
            definition(2, "Thorn"),
        ]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![warlock_character()],
            vec![
                instance("item-1", 1, ItemLocation::Vault),
                instance("item-2", 2, ItemLocation::Vault),
            ],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![warlock_character()],
                vec![
                    instance("item-1", 1, ItemLocation::Vault),
                    instance("item-2", 2, ItemLocation::Vault),
                ],
            ),
        );

        // The semantic term resolves uniquely, but to a different concrete
        // instance than the plan binds.
        let error = service
            .preflight_move(
                &request(semantic("thorn"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong semantic item");

        assert_eq!(error, MutationError::WrongItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_exact_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("hunter", Some("Hunter"), None),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        // The request names a different destination than the plan binds.
        let error = service
            .preflight_move(
                &request(exact("item-1"), exact_character("hunter")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong exact destination");

        assert_eq!(error, MutationError::WrongDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_rejects_a_request_for_a_different_semantic_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("hunter", Some("Hunter"), None),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("hunter", Some("Hunter"), None),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        // The label resolves uniquely, but to a different character than the
        // plan binds.
        let error = service
            .preflight_move(
                &request(exact("item-1"), character_label("hunter")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("wrong semantic destination");

        assert_eq!(error, MutationError::WrongDestination);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_a_request_item_absent_from_the_projection() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(&shared, fresh_vault_projection());

        let error = service
            .preflight_move(
                &request(exact("absent"), exact_character("warlock")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("missing requested item");

        assert_eq!(error, MutationError::MissingItem);
        assert_eq!(error.category(), ErrorCategory::StaleState);
    }

    #[test]
    fn preflight_reports_an_ambiguous_request_destination() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(projection(
            fresh_checked_at(),
            vec![
                warlock_character(),
                character("titan-a", Some("Alpha"), Some("Titan")),
                character("titan-b", Some("Bravo"), Some("Titan")),
            ],
            vec![instance("item-1", 1, ItemLocation::Vault)],
        ));
        let mut service = service(&shared, repository);
        let plan = service
            .plan_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("plan");
        queue_refresh(
            &shared,
            projection(
                fresh_checked_at(),
                vec![
                    warlock_character(),
                    character("titan-a", Some("Alpha"), Some("Titan")),
                    character("titan-b", Some("Bravo"), Some("Titan")),
                ],
                vec![instance("item-1", 1, ItemLocation::Vault)],
            ),
        );

        let error = service
            .preflight_move(
                &request(exact("item-1"), character_label("titan")),
                &plan,
                TTL,
                NOW,
            )
            .expect_err("ambiguous requested destination");

        assert_eq!(error, MutationError::AmbiguousDestination);
        assert_eq!(error.category(), ErrorCategory::AmbiguousMatch);
    }

    #[test]
    fn preflight_propagates_a_refresh_failure() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);
        let move_request = request(exact("item-1"), exact_character("warlock"));
        let plan = service
            .plan_move(&move_request, LIFETIME, TTL, NOW)
            .expect("plan");
        shared
            .borrow_mut()
            .queue_membership(Err(AppError::new(ErrorCategory::ProviderRateLimit)));
        let mut failing = service;
        let error = failing
            .preflight_move(&move_request, &plan, TTL, NOW)
            .expect_err("rate limit");

        assert_eq!(
            error,
            MutationError::Application(ErrorCategory::ProviderRateLimit)
        );
    }

    #[test]
    fn policy_outcomes_map_to_specific_typed_errors() {
        let cases = [
            (MutationPolicyOutcome::Valid, MutationError::InvalidPlan),
            (MutationPolicyOutcome::Expired, MutationError::Expired),
            (
                MutationPolicyOutcome::StaleRevision,
                MutationError::StaleRevision,
            ),
            (
                MutationPolicyOutcome::WrongMembership,
                MutationError::WrongMembership,
            ),
            (MutationPolicyOutcome::WrongItem, MutationError::WrongItem),
            (
                MutationPolicyOutcome::WrongDestination,
                MutationError::WrongDestination,
            ),
            (
                MutationPolicyOutcome::MissingResolution,
                MutationError::MissingResolution,
            ),
            (
                MutationPolicyOutcome::AmbiguousResolution,
                MutationError::AmbiguousResolution,
            ),
            (
                MutationPolicyOutcome::MissingDestination,
                MutationError::MissingDestination,
            ),
            (
                MutationPolicyOutcome::AmbiguousDestination,
                MutationError::AmbiguousDestination,
            ),
            (
                MutationPolicyOutcome::UnsupportedLocation,
                MutationError::UnsupportedLocation,
            ),
            (
                MutationPolicyOutcome::UnsupportedSource,
                MutationError::UnsupportedSource,
            ),
            (
                MutationPolicyOutcome::SourceMismatch,
                MutationError::SourceMismatch,
            ),
        ];

        for (outcome, expected) in cases {
            assert_eq!(MutationError::from_policy_outcome(outcome), expected);
        }
    }

    #[test]
    fn prepare_move_plans_and_preflights_in_one_shared_call() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        // Only the mandatory preflight refresh acquires anything: the ordinary
        // planning gate finds the projection fresh and skips acquisition.
        queue_refresh(&shared, fresh_vault_projection());
        let mut service = service(&shared, repository);

        let plan = service
            .prepare_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect("prepared plan");

        assert_eq!(plan.plan_id, PlanId::new("plan-1"));
        assert_eq!(plan.revision, SyncRevision::new(1));
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
        assert_eq!(service.plan_ids().calls(), 1);
        assert_eq!(
            service.repository().recorded_calls(),
            [
                "load_current",
                "load_current",
                "lookup_instance",
                "load_current",
                "replace_current",
                "load_current",
                "lookup_instance",
                "lookup_instance"
            ]
        );
    }

    #[test]
    fn prepare_move_rejects_a_plan_the_preflight_invalidated() {
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        queue_refresh(
            &shared,
            projection(
                NOW,
                vec![warlock_character()],
                vec![instance("item-1", 1, warlock())],
            ),
        );
        let mut service = service(&shared, repository);

        let error = service
            .prepare_move(
                &request(exact("item-1"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("invalidated plan");

        assert_eq!(error, MutationError::StaleRevision);
        // The invalidated plan is never handed to a caller.
        assert_eq!(service.plan_ids().calls(), 1);
    }

    #[test]
    fn errors_are_sanitized() {
        assert_eq!(
            MutationError::MissingItem.to_string(),
            "the requested item is not in the committed projection"
        );
        assert_eq!(
            MutationError::Application(ErrorCategory::ProviderUnavailable).to_string(),
            "the provider is unavailable"
        );
        let shared = gateway();
        let mut repository =
            CompositeRepository::with_definitions(vec![definition(1, "Gjallarhorn")]);
        repository.commit(fresh_vault_projection());
        let mut service = service(&shared, repository);

        let error = service
            .plan_move(
                &request(exact("secret-item"), exact_character("warlock")),
                LIFETIME,
                TTL,
                NOW,
            )
            .expect_err("missing item");
        let rendered = format!("{error} {error:?}");

        for secret in ["secret-item", MEMBERSHIP_IDENTIFIER, "warlock"] {
            assert!(!rendered.contains(secret), "diagnostic leaked {secret}");
        }
        assert_eq!(AppError::from(error).category(), error.category());
        assert_eq!(
            MutationError::from(AppError::new(ErrorCategory::LocalStorage)),
            { MutationError::Application(ErrorCategory::LocalStorage) }
        );
    }

    #[test]
    fn move_request_carries_no_approval_artifact() {
        let move_request = request(exact("item-1"), exact_character("warlock"));
        // Exhaustive binding without a rest pattern: the request shape is
        // exactly the membership, the two selectors, and the intent fact, so no
        // preview token, confirmation value, or approval artifact can exist.
        let MoveRequest {
            membership,
            item,
            destination,
            intent,
        } = move_request;
        assert_eq!(membership, membership_id());
        assert_eq!(item.instance_id().expect("exact item"), &item_id("item-1"));
        assert_eq!(
            destination,
            DestinationSelector::character(CharacterId::new("warlock"))
        );
        assert_eq!(
            intent.authorize(),
            destiny_core::IntentAuthorizationOutcome::Authorized
        );
    }
}
