//! Offline, provider-neutral fakes for application contract tests.
//!
//! These types are test support only. They model normalized behavior and never
//! perform network, filesystem, serialization, or credential-provider work.

use destiny_core::{
    AccessCredential, AppError, AuthorizationCallback, AuthorizationState, BrowserAuthorization,
    CallbackReceiver, Clock, CredentialStore, Dataset, DefinitionReplacementInput,
    DefinitionRepository, DefinitionSearchQuery, DefinitionVersion, DestinyMembership,
    ErrorCategory, InstanceSearchQuery, InventoryRepository, InventorySnapshot,
    InventorySnapshotInput, ItemDefinition, ItemHash, ItemInstance, ItemInstanceId,
    ManifestAcquisition, ManifestDefinitions, ManifestGateway, PlanId, PlanIdGenerator,
    ProfileGateway, Resolution, SearchRepository, SnapshotMetadata, SourceMarker, SyncMetadata,
    SyncRevision, Timestamp, TransferGateway, TransferOutcome, TransferRequest, definition_matches,
};
use std::collections::{HashSet, VecDeque};

/// One coherent sentinel set shared by every cross-boundary leak harness.
///
/// The durable SQLite bytes and sidecars, the CLI runner and its process
/// streams, the MCP stdio child process, and the serialized error shapes all
/// draw their canaries from this single definition, so a value proven absent at
/// one boundary is the exact value probed at the others. Every constant is
/// inert test data: none is a real credential, identifier, or provider value,
/// and none requires a network, a keyring, or a live account.
pub mod leak_sentinels {
    /// An API key supplied to a provider read gateway.
    pub const API_KEY: &str = "sentinel-hardening-api-key-2f9c41d7";
    /// An opaque bearer credential supplied to a provider read gateway.
    pub const BEARER_TOKEN: &str = "sentinel-hardening-bearer-token-7d41b8e2";
    /// Raw provider payload text that a failure message might otherwise echo.
    pub const RAW_PAYLOAD: &str = "sentinel-hardening-raw-payload-b3e5a0c9";
    /// A non-secret control marker expected to be observable wherever a
    /// surface is genuinely exercised, proving a clean scan or stream is not an
    /// empty one.
    pub const CONTROL: &str = "sentinel-hardening-control-1a6d50f3";

    /// The secret-bearing sentinels that must never reach a durable byte, a
    /// process stream, or a serialized error shape.
    pub const SECRETS: [&str; 3] = [API_KEY, BEARER_TOKEN, RAW_PAYLOAD];
}

#[derive(Debug)]
pub struct DeterministicRandom {
    states: VecDeque<AuthorizationState>,
}

impl DeterministicRandom {
    pub fn new(states: impl IntoIterator<Item = AuthorizationState>) -> Self {
        Self {
            states: states.into_iter().collect(),
        }
    }
}

impl destiny_core::SecureRandom for DeterministicRandom {
    fn generate_state(&mut self) -> Result<AuthorizationState, AppError> {
        self.states
            .pop_front()
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::LocalStorage))
    }
}

#[derive(Debug, Default)]
pub struct RecordingBrowser {
    pub requests: Vec<destiny_core::ports::AuthorizationRequest>,
}

impl BrowserAuthorization for RecordingBrowser {
    fn open(&mut self, request: destiny_core::ports::AuthorizationRequest) -> Result<(), AppError> {
        self.requests.push(request);
        Ok(())
    }
}

#[derive(Debug)]
pub struct FailingBrowser {
    pub error: AppError,
}

impl BrowserAuthorization for FailingBrowser {
    fn open(
        &mut self,
        _request: destiny_core::ports::AuthorizationRequest,
    ) -> Result<(), AppError> {
        Err(self.error)
    }
}

#[derive(Debug, Default)]
pub struct QueuedCallbackReceiver {
    pub callbacks: VecDeque<Result<AuthorizationCallback, AppError>>,
}

impl CallbackReceiver for QueuedCallbackReceiver {
    fn receive(&mut self) -> Result<AuthorizationCallback, AppError> {
        self.callbacks.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Debug, Default)]
pub struct QueuedTokenExchange {
    pub outcomes: VecDeque<Result<AccessCredential, AppError>>,
}

impl destiny_core::PublicTokenExchange for QueuedTokenExchange {
    fn exchange(
        &mut self,
        _code: destiny_core::AuthorizationCode,
    ) -> Result<AccessCredential, AppError> {
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

#[derive(Clone, Debug)]
pub struct FakeClock {
    now: Timestamp,
}

impl FakeClock {
    pub const fn fixed(now: Timestamp) -> Self {
        Self { now }
    }
    pub const fn now_value(&self) -> Timestamp {
        self.now
    }
    pub fn advance(&mut self, seconds: u64) -> Result<(), AppError> {
        let next = self
            .now
            .seconds()
            .checked_add(seconds)
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::ClockOverflow))?;
        self.now = Timestamp::new(next);
        Ok(())
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(self.now)
    }
}

#[derive(Clone, Debug)]
pub struct DeterministicPlanIds {
    next: u64,
    calls: u64,
}

impl DeterministicPlanIds {
    pub const fn starting_at(next: u64) -> Self {
        Self { next, calls: 0 }
    }

    /// Counts identifiers issued, so a test can prove how many were spent.
    pub const fn calls(&self) -> u64 {
        self.calls
    }
}

impl PlanIdGenerator for DeterministicPlanIds {
    fn next_plan_id(&mut self) -> Result<PlanId, AppError> {
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| AppError::new(destiny_core::ErrorCategory::PlanIdExhausted))?;
        let id = PlanId::new(format!("plan-{}", self.next));
        self.next = next;
        self.calls += 1;
        Ok(id)
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryDefinitionRepository {
    version: Option<DefinitionVersion>,
    definitions: Vec<ItemDefinition>,
    metadata: Option<SyncMetadata>,
    previous_input: Option<DefinitionReplacementInput>,
    world: Option<destiny_core::WorldDefinitionsAcquisition>,
    publication_failure: Option<AppError>,
    metadata_failure: Option<AppError>,
    publication_attempts: usize,
}

impl InMemoryDefinitionRepository {
    /// Seeds arbitrary state for read-path contract tests.
    ///
    /// This bypasses publication validation on purpose so lookup behavior such
    /// as ambiguous matches stays directly testable. Publication always
    /// validates the complete set before mutating state. Non-empty seeded
    /// catalogs carry read metadata; use `default()` for an absent catalog.
    /// Publication history remains unset so the first publish still owns its revision.
    pub fn new(version: DefinitionVersion, definitions: Vec<ItemDefinition>) -> Self {
        let metadata = (!definitions.is_empty()).then(|| SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: Timestamp::new(0),
            source: SourceMarker::Manifest,
            revision: SyncRevision::new(0),
            definition_version: version.clone(),
        });
        Self {
            version: Some(version),
            definitions,
            metadata,
            ..Self::default()
        }
    }

    /// Injects a sanitized publication failure; reads keep serving prior state.
    pub fn with_publication_failure(mut self, error: AppError) -> Self {
        self.publication_failure = Some(error);
        self
    }

    /// Injects a real metadata read failure, distinct from absent metadata.
    pub fn with_metadata_failure(mut self, error: AppError) -> Self {
        self.metadata_failure = Some(error);
        self
    }

    /// Constructs a committed overflow boundary state for repository contract tests.
    pub fn with_revision_for_test(
        replacement: DefinitionReplacementInput,
        revision: SyncRevision,
    ) -> Self {
        let metadata = SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: replacement.checked_at,
            source: replacement.source,
            revision,
            definition_version: replacement.definitions.version.clone(),
        };
        Self {
            version: Some(replacement.definitions.version.clone()),
            definitions: replacement.definitions.definitions.clone(),
            metadata: Some(metadata),
            previous_input: Some(replacement),
            ..Self::default()
        }
    }

    /// Counts publication attempts, including rejected ones.
    pub const fn publication_attempts(&self) -> usize {
        self.publication_attempts
    }

    /// Counts the definitions currently visible in the active set.
    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    /// A publishable set is non-empty, version-consistent, and duplicate-free.
    fn validate_publication(definitions: &ManifestDefinitions) -> Result<(), AppError> {
        if definitions.definitions.is_empty() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let mut seen = HashSet::with_capacity(definitions.definitions.len());
        for definition in &definitions.definitions {
            if definition.version != definitions.version || !seen.insert(definition.hash) {
                return Err(AppError::new(ErrorCategory::ProviderValidation));
            }
        }
        Ok(())
    }
}

impl DefinitionRepository for InMemoryDefinitionRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.version
            .clone()
            .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))
    }

    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        let matches = self
            .definitions
            .iter()
            .filter(|definition| definition.hash == hash)
            .cloned()
            .collect::<Vec<_>>();
        Ok(match matches.as_slice() {
            [] => Resolution::Missing,
            [definition] => Resolution::Unique(definition.clone()),
            _ => Resolution::Ambiguous(matches),
        })
    }

    fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
        if let Some(error) = self.metadata_failure {
            return Err(error);
        }
        Ok(self.metadata.clone())
    }

    fn replace_definitions(
        &mut self,
        replacement: DefinitionReplacementInput,
    ) -> Result<SyncMetadata, AppError> {
        self.publication_attempts += 1;
        if let Some(error) = self.publication_failure {
            return Err(error);
        }
        if replacement.source != SourceMarker::Manifest {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Self::validate_publication(&replacement.definitions)?;

        // Revision identity is the normalized definition content alone. A later
        // `checked_at` refreshes committed metadata but never advances the
        // revision; a fingerprint would collide, so content equality is the only
        // retention authority here.
        let revision = match self.previous_input.as_ref() {
            Some(previous) if previous.content_eq(&replacement) => self
                .metadata
                .as_ref()
                .map(|metadata| metadata.revision)
                .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
            _ => match self.metadata.as_ref() {
                Some(metadata) => SyncRevision::new(
                    metadata
                        .revision
                        .value()
                        .checked_add(1)
                        .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
                ),
                None => SyncRevision::new(1),
            },
        };

        let metadata = SyncMetadata {
            dataset: Dataset::ManifestDefinitions,
            checked_at: replacement.checked_at,
            source: replacement.source,
            revision,
            definition_version: replacement.definitions.version.clone(),
        };
        if self
            .previous_input
            .as_ref()
            .is_none_or(|previous| !previous.content_eq(&replacement))
        {
            self.world = None;
        }
        self.version = Some(replacement.definitions.version.clone());
        self.definitions = replacement.definitions.definitions.clone();
        self.metadata = Some(metadata.clone());
        self.previous_input = Some(replacement);
        Ok(metadata)
    }
}

impl destiny_core::ports::WorldDefinitionRepository for InMemoryDefinitionRepository {
    fn lookup_milestone(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_core::MilestoneDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|world| {
            world
                .milestones
                .definitions
                .iter()
                .find(|d| d.hash() == hash)
                .cloned()
        }))
    }
    fn lookup_activity(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_core::ActivityDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|world| {
            world
                .activities
                .definitions
                .iter()
                .find(|d| d.hash() == hash)
                .cloned()
        }))
    }
    fn lookup_activity_modifier(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_core::ActivityModifierDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|world| {
            world
                .modifiers
                .definitions
                .iter()
                .find(|d| d.hash() == hash)
                .cloned()
        }))
    }
    fn lookup_objective(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_core::ObjectiveDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|w| {
            w.objectives
                .definitions
                .iter()
                .find(|d| d.hash() == hash)
                .cloned()
        }))
    }
    fn lookup_activity_type(
        &self,
        hash: u32,
    ) -> Result<Option<destiny_core::ActivityTypeDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|w| {
            w.activity_types
                .definitions
                .iter()
                .find(|d| d.hash() == hash)
                .cloned()
        }))
    }
    fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
        Ok(self
            .world
            .as_ref()
            .map_or([0; 5], destiny_core::WorldDefinitionsAcquisition::counts))
    }
    fn publish_manifest(
        &mut self,
        items: DefinitionReplacementInput,
        mut world: destiny_core::WorldDefinitionsAcquisition,
    ) -> Result<SyncMetadata, AppError> {
        world.validate_for(&items.definitions.version)?;
        world.canonicalize();
        let world_changed = self.world.as_ref() != Some(&world);
        let items_unchanged = self
            .previous_input
            .as_ref()
            .is_some_and(|previous| previous.content_eq(&items));
        let mut candidate = self.clone();
        if world_changed && items_unchanged {
            candidate.previous_input = None;
        }
        let result = candidate.replace_definitions(items);
        self.publication_attempts = candidate.publication_attempts;
        let metadata = result?;
        candidate.world = Some(world);
        *self = candidate;
        Ok(metadata)
    }
}

impl destiny_core::ports::StatDefinitionRepository for InMemoryDefinitionRepository {
    fn lookup_stat(&self, hash: u32) -> Result<Option<destiny_core::StatDefinition>, AppError> {
        Ok(self.world.as_ref().and_then(|world| {
            world
                .stats
                .definitions
                .iter()
                .find(|stat| stat.hash() == hash)
                .cloned()
        }))
    }
    fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
        let normalized = destiny_core::normalize_stat_name(name)?;
        let mut hashes = Vec::new();
        if let Some(world) = &self.world {
            for stat in &world.stats.definitions {
                if destiny_core::normalize_stat_name(stat.name())? == normalized {
                    hashes.push(stat.hash());
                }
            }
        }
        hashes.sort_unstable();
        Ok(hashes)
    }
    fn stat_definition_count(&self) -> Result<usize, AppError> {
        Ok(self
            .world
            .as_ref()
            .map_or(0, |world| world.stats.definitions.len()))
    }
}

impl destiny_core::ports::DefinitionCatalogRepository for InMemoryDefinitionRepository {
    fn definition_count(&self) -> Result<usize, AppError> {
        Ok(self.definitions.len())
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryInventoryRepository {
    current: Option<InventorySnapshot>,
    previous_input: Option<InventorySnapshotInput>,
}

impl InMemoryInventoryRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Constructs an overflow boundary state for repository contract tests.
    pub fn with_revision_for_test(
        mut input: InventorySnapshotInput,
        revision: SyncRevision,
    ) -> Self {
        input
            .characters
            .sort_by(|left, right| left.id.cmp(&right.id));
        input
            .items
            .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id.clone(),
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version.clone(),
                checked_at: input.checked_at,
                source: input.source,
            },
            characters: input.characters.clone(),
            items: input.items.clone(),
        };
        Self {
            current: Some(snapshot),
            previous_input: Some(input),
        }
    }

    /// Exposes the committed snapshot metadata, if any, without cloning the projection.
    pub fn snapshot_metadata(&self) -> Result<Option<SnapshotMetadata>, AppError> {
        Ok(self.load_current()?.map(|snapshot| snapshot.metadata))
    }
}

impl InventoryRepository for InMemoryInventoryRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        Ok(self.current.clone())
    }

    fn replace_current(
        &mut self,
        mut input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        // Provenance and structural validity are validated before any mutation so
        // a rejected replacement leaves the prior projection fully readable.
        if input.source != SourceMarker::Profile {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        if !input.is_structurally_valid() {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        // Characters and items are unordered sets, so the committed and returned
        // vectors are canonicalized by identifier. Revision identity is the
        // normalized projection content through the shared `content_eq`:
        // freshness metadata (`checked_at`, `source`) never advances a revision,
        // but every successful replacement commits the new `checked_at`.
        input
            .characters
            .sort_by(|left, right| left.id.cmp(&right.id));
        input
            .items
            .sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        let revision = match (self.current.as_ref(), self.previous_input.as_ref()) {
            (Some(current), Some(previous)) if previous.content_eq(&input) => {
                current.metadata.revision
            }
            (Some(current), _) => SyncRevision::new(
                current
                    .metadata
                    .revision
                    .value()
                    .checked_add(1)
                    .ok_or_else(|| AppError::new(ErrorCategory::LocalStorage))?,
            ),
            (None, _) => SyncRevision::new(1),
        };
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: input.membership_id,
                membership_type: input.membership_type,
                revision,
                definition_version: input.definition_version,
                checked_at: input.checked_at,
                source: input.source,
            },
            characters: input.characters,
            items: input.items,
        };
        self.current = Some(snapshot.clone());
        self.previous_input = Some(InventorySnapshotInput::with_metadata(&snapshot));
        Ok(snapshot)
    }
}

/// A deterministic, in-memory [`SearchRepository`] over committed projection data.
///
/// The fake mirrors the durable search contract exactly: it validates the same
/// queries, applies the same [`definition_matches`]/[`instance_matches`]
/// normalization and matching rules, and returns the same
/// [`ItemHash`]/[`ItemInstanceId`] ascending order truncated to the same limit.
/// It holds only the *current* projection, so replacing the seeded definitions
/// or instances immediately changes what a later search observes and nothing
/// from the previous revision stays visible.
///
/// It performs no I/O and no serialization, which lets later `QueryService`
/// tests exercise definition-versus-instance behavior without a database.
#[derive(Clone, Debug, Default)]
pub struct InMemorySearchRepository {
    definitions: Vec<ItemDefinition>,
    instances: Vec<ItemInstance>,
}

impl InMemorySearchRepository {
    /// Builds an empty projection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a projection from one complete definition set and no instances.
    pub fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
        Self {
            definitions,
            instances: Vec::new(),
        }
    }

    /// Builds a projection from one definition set and one committed snapshot.
    pub fn with_projection(definitions: Vec<ItemDefinition>, snapshot: &InventorySnapshot) -> Self {
        Self {
            definitions,
            instances: snapshot.items.clone(),
        }
    }

    /// Replaces every visible definition, as a completed Manifest publication would.
    pub fn set_definitions(&mut self, definitions: Vec<ItemDefinition>) {
        self.definitions = definitions;
    }

    /// Replaces every visible instance, as a completed profile refresh would.
    pub fn set_instances(&mut self, instances: Vec<ItemInstance>) {
        self.instances = instances;
    }

    /// Number of definitions currently visible to search.
    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    /// Number of instances currently visible to search.
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }
}

impl SearchRepository for InMemorySearchRepository {
    fn search_definitions(
        &self,
        query: &DefinitionSearchQuery,
    ) -> Result<Vec<ItemDefinition>, AppError> {
        query.validate()?;
        let mut matches: Vec<ItemDefinition> = self
            .definitions
            .iter()
            .filter(|definition| definition_matches(query, definition))
            .cloned()
            .collect();
        matches.sort_by_key(|definition| definition.hash);
        matches.truncate(query.limit);
        Ok(matches)
    }

    fn search_instances(&self, query: &InstanceSearchQuery) -> Result<Vec<ItemInstance>, AppError> {
        query.validate()?;
        let mut matches: Vec<ItemInstance> = self
            .instances
            .iter()
            .filter(|instance| {
                let mut definitions = self
                    .definitions
                    .iter()
                    .filter(|definition| definition.hash == instance.item_hash);
                let definition = definitions.next();
                let unique = if definitions.next().is_none() {
                    definition
                } else {
                    None
                };
                let plugs: Vec<_> = instance
                    .plugs
                    .iter()
                    .filter_map(|plug| {
                        let mut matches = self
                            .definitions
                            .iter()
                            .filter(|definition| definition.hash == plug.plug_hash);
                        let first = matches.next();
                        if matches.next().is_none() {
                            first.cloned()
                        } else {
                            None
                        }
                    })
                    .collect();
                destiny_core::query::instance_matches_with_definition(query, instance, unique)
                    && destiny_core::query::instance_matches_plug_names(query, &plugs)
            })
            .cloned()
            .collect();
        matches.sort_by(|left, right| left.instance_id.cmp(&right.instance_id));
        matches.truncate(query.limit);
        Ok(matches)
    }

    fn lookup_instance(
        &self,
        instance_id: &ItemInstanceId,
    ) -> Result<Resolution<ItemInstance>, AppError> {
        // A structurally valid projection carries unique instance identifiers,
        // so `Ambiguous` is only reachable from an invalid or expressly
        // test-seeded projection. The unbounded ambiguity vector is inherited
        // from the shared `Resolution` type used by definition lookup and is
        // not a bounded product-facing candidate list; only the caller-supplied
        // search inputs are bounded by the query validation above.
        let matches: Vec<ItemInstance> = self
            .instances
            .iter()
            .filter(|instance| &instance.instance_id == instance_id)
            .cloned()
            .collect();
        Ok(match matches.as_slice() {
            [] => Resolution::Missing,
            [instance] => Resolution::Unique(instance.clone()),
            _ => Resolution::Ambiguous(matches),
        })
    }
}

/// Deterministically queues membership resolution and inventory acquisition
/// outcomes and records how each port method was called.
///
/// The two queues are independent so a test can make resolution succeed while
/// acquisition fails, or exercise resolution failure without ever requesting a
/// snapshot. `requested_memberships` records the exact membership each
/// acquisition was asked for, in call order.
#[derive(Debug, Default)]
pub struct QueuedProfileGateway {
    memberships: VecDeque<Result<DestinyMembership, AppError>>,
    snapshots: VecDeque<Result<InventorySnapshotInput, AppError>>,
    membership_calls: usize,
    inventory_calls: usize,
    requested_memberships: Vec<DestinyMembership>,
}

impl QueuedProfileGateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues one membership resolution outcome.
    pub fn queue_membership(&mut self, outcome: Result<DestinyMembership, AppError>) {
        self.memberships.push_back(outcome);
    }

    /// Queues one inventory acquisition outcome.
    pub fn queue_inventory(&mut self, outcome: Result<InventorySnapshotInput, AppError>) {
        self.snapshots.push_back(outcome);
    }

    /// Counts resolution attempts, including failures and exhausted queues.
    pub const fn membership_calls(&self) -> usize {
        self.membership_calls
    }

    /// Counts acquisition attempts, including failures and exhausted queues.
    pub const fn inventory_calls(&self) -> usize {
        self.inventory_calls
    }

    /// The memberships passed to `load_inventory`, in call order.
    pub fn requested_memberships(&self) -> &[DestinyMembership] {
        &self.requested_memberships
    }
}

impl ProfileGateway for QueuedProfileGateway {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        self.membership_calls += 1;
        self.memberships.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }

    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        self.inventory_calls += 1;
        self.requested_memberships.push(membership.clone());
        self.snapshots.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

/// Deterministically queues sanitized transfer outcomes and records the exact
/// requests it was asked to issue, in call order.
///
/// The queue is consumed front-to-back like [`QueuedProfileGateway`]. An
/// exhausted queue reports a sanitized provider-unavailable failure rather than
/// inventing a success, so a test that forgets to script a step fails loudly
/// instead of silently passing. Each recorded request is a clone of the
/// sanitized [`TransferRequest`] the executor built, so a test can assert the
/// exact endpoint order and the fields (including the complete membership and
/// the definition hash and quantity) the app derived from the projection.
#[derive(Debug, Default)]
pub struct QueuedTransferGateway {
    outcomes: VecDeque<Result<TransferOutcome, AppError>>,
    requests: Vec<TransferRequest>,
    calls: usize,
}

impl QueuedTransferGateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues one sanitized transfer outcome.
    pub fn queue(&mut self, outcome: Result<TransferOutcome, AppError>) {
        self.outcomes.push_back(outcome);
    }

    /// Counts transfer attempts, including failures and exhausted queues.
    pub const fn calls(&self) -> usize {
        self.calls
    }

    /// The exact transfer requests issued, in call order.
    pub fn requests(&self) -> &[TransferRequest] {
        &self.requests
    }
}

impl TransferGateway for QueuedTransferGateway {
    fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError> {
        self.calls += 1;
        self.requests.push(request.clone());
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                destiny_core::ErrorCategory::ProviderUnavailable,
            ))
        })
    }
}

/// Minimal valid world acquisition for deterministic item-focused sync tests.
pub fn synthetic_world(version: DefinitionVersion) -> destiny_core::WorldDefinitionsAcquisition {
    use destiny_core::*;
    WorldDefinitionsAcquisition {
        version,
        stats: WorldDefinitionTable {
            definitions: vec![StatDefinition::new(5, "Weapons".into()).unwrap()],
            skipped_count: 7,
        },
        objectives: WorldDefinitionTable {
            definitions: vec![ObjectiveDefinition::new(5, "Progress".into(), 1).unwrap()],
            skipped_count: 5,
        },
        activity_types: WorldDefinitionTable {
            definitions: vec![ActivityTypeDefinition::new(4, "Type".into(), "".into()).unwrap()],
            skipped_count: 6,
        },
        milestones: WorldDefinitionTable {
            definitions: vec![
                MilestoneDefinition::new(1, "Milestone".into(), "".into(), 3).unwrap(),
            ],
            skipped_count: 2,
        },
        activities: WorldDefinitionTable {
            definitions: vec![
                ActivityDefinition::new(2, "Activity".into(), "".into(), 4, 2000).unwrap(),
            ],
            skipped_count: 3,
        },
        modifiers: WorldDefinitionTable {
            definitions: vec![
                ActivityModifierDefinition::new(3, "Modifier".into(), "".into()).unwrap(),
            ],
            skipped_count: 4,
        },
    }
}

#[derive(Debug, Default)]
pub struct QueuedManifestGateway {
    indexes: VecDeque<Result<destiny_core::ports::ManifestIndex, AppError>>,
    outcomes: VecDeque<Result<ManifestAcquisition, AppError>>,
    index_calls: usize,
    component_calls: usize,
    world_calls: usize,
    worlds: VecDeque<Result<destiny_core::WorldDefinitionsAcquisition, AppError>>,
}

impl QueuedManifestGateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compatibility shorthand: queue an index and its component together.
    pub fn queue(&mut self, outcome: Result<ManifestAcquisition, AppError>) {
        match outcome {
            Ok(batch) => {
                self.queue_index(Ok(destiny_core::ports::ManifestIndex {
                    version: batch.version.clone(),
                    component_path: "/common/destiny2_content/test.json".into(),
                    milestone_path: "/common/destiny2_content/milestones.json".into(),
                    activity_path: "/common/destiny2_content/activities.json".into(),
                    modifier_path: "/common/destiny2_content/modifiers.json".into(),
                    objective_path: "/common/destiny2_content/objectives.json".into(),
                    activity_type_path: "/common/destiny2_content/types.json".into(),
                    stat_path: "/common/destiny2_content/stats.json".into(),
                }));
                self.queue_world(Ok(synthetic_world(batch.version.clone())));
                self.queue_component(Ok(batch));
            }
            Err(error) => self.queue_index(Err(error)),
        }
    }

    pub fn queue_index(&mut self, outcome: Result<destiny_core::ports::ManifestIndex, AppError>) {
        self.indexes.push_back(outcome);
    }

    pub fn queue_component(&mut self, outcome: Result<ManifestAcquisition, AppError>) {
        self.outcomes.push_back(outcome);
    }

    pub fn queue_world(
        &mut self,
        outcome: Result<destiny_core::WorldDefinitionsAcquisition, AppError>,
    ) {
        self.worlds.push_back(outcome);
    }

    pub const fn world_calls(&self) -> usize {
        self.world_calls
    }

    pub const fn calls(&self) -> usize {
        self.index_calls
    }

    pub const fn index_calls(&self) -> usize {
        self.index_calls
    }

    pub const fn component_calls(&self) -> usize {
        self.component_calls
    }
}

impl ManifestGateway for QueuedManifestGateway {
    fn load_world_components(
        &mut self,
        _index: &destiny_core::ports::ManifestIndex,
    ) -> Result<destiny_core::WorldDefinitionsAcquisition, AppError> {
        self.world_calls += 1;
        self.worlds
            .pop_front()
            .unwrap_or_else(|| Err(AppError::new(ErrorCategory::ProviderUnavailable)))
    }

    fn fetch_index(&mut self) -> Result<destiny_core::ports::ManifestIndex, AppError> {
        self.index_calls += 1;
        self.indexes
            .pop_front()
            .unwrap_or_else(|| Err(AppError::new(ErrorCategory::ProviderUnavailable)))
    }

    fn load_component(
        &mut self,
        _index: &destiny_core::ports::ManifestIndex,
    ) -> Result<ManifestAcquisition, AppError> {
        self.component_calls += 1;
        self.outcomes
            .pop_front()
            .unwrap_or_else(|| Err(AppError::new(ErrorCategory::ProviderUnavailable)))
    }
}

#[derive(Clone, Debug, Default)]
pub struct InMemoryCredentialStore {
    credential: Option<AccessCredential>,
}

impl InMemoryCredentialStore {
    pub fn contains_credential(&self) -> bool {
        self.credential.is_some()
    }
}

impl CredentialStore for InMemoryCredentialStore {
    fn save(&mut self, credential: AccessCredential) -> Result<(), AppError> {
        self.credential = Some(credential);
        Ok(())
    }
    fn load(&self) -> Result<Option<AccessCredential>, AppError> {
        Ok(self.credential.clone())
    }
    fn delete(&mut self) -> Result<(), AppError> {
        self.credential = None;
        Ok(())
    }
}

#[derive(Debug)]
pub struct FailingCredentialStore {
    pub error: AppError,
}

impl CredentialStore for FailingCredentialStore {
    fn save(&mut self, _credential: AccessCredential) -> Result<(), AppError> {
        Err(self.error)
    }

    fn load(&self) -> Result<Option<AccessCredential>, AppError> {
        Err(self.error)
    }

    fn delete(&mut self) -> Result<(), AppError> {
        Err(self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use destiny_core::{
        AccessCredentialMetadata, Character, CharacterId, CredentialMaterial, ErrorCategory,
        InstanceSearchQuery, ItemHashFilter, ItemInstance, ItemLocation, MAX_HASH_FILTERS,
        MAX_SEARCH_LIMIT, MembershipId, MembershipType, PerkHash, PerkSummary, SearchTerm,
    };

    #[test]
    fn instance_plugs_round_trip_through_inventory_and_search_fakes() {
        let mut candidate = input(1);
        candidate.items[0].plugs = [60, 61, 60]
            .into_iter()
            .map(|hash| destiny_core::PlugSummary {
                plug_hash: ItemHash::new(hash),
                enabled: true,
            })
            .collect();
        let mut repository = InMemoryInventoryRepository::new();
        let snapshot = repository.replace_current(candidate.clone()).unwrap();
        assert_eq!(snapshot.items[0].plugs, candidate.items[0].plugs);
        assert_eq!(
            repository.load_current().unwrap().unwrap().items,
            snapshot.items
        );
        let search = InMemorySearchRepository::with_projection(vec![], &snapshot);
        let found = search
            .lookup_instance(&candidate.items[0].instance_id)
            .unwrap();
        assert_eq!(found, Resolution::Unique(candidate.items[0].clone()));
        let unchanged = repository.replace_current(candidate.clone()).unwrap();
        assert_eq!(unchanged.metadata.revision, snapshot.metadata.revision);
        candidate.items[0].plugs.swap(0, 1);
        let changed = repository.replace_current(candidate).unwrap();
        assert_eq!(
            changed.metadata.revision.value(),
            snapshot.metadata.revision.value() + 1
        );
    }

    type RevisionMutation = (&'static str, fn(&mut InventorySnapshotInput));

    fn input(quantity: u32) -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(1),
            definition_version: DefinitionVersion::new("definitions-v1"),
            checked_at: Timestamp::new(200),
            source: SourceMarker::Profile,
            characters: Vec::<Character>::new(),
            items: vec![ItemInstance {
                instance_id: "instance".into(),
                item_hash: ItemHash::new(7),
                location: ItemLocation::Vault,
                quantity,
                equipped: false,
                locked: false,
                power: None,
                plugs: Vec::new(),
                perks: Vec::new(),
            }],
        }
    }

    fn multi_input() -> InventorySnapshotInput {
        InventorySnapshotInput {
            membership_id: MembershipId::new("membership"),
            membership_type: MembershipType::new(1),
            definition_version: DefinitionVersion::new("definitions-v1"),
            checked_at: Timestamp::new(200),
            source: SourceMarker::Profile,
            characters: vec![
                Character {
                    id: "character-1".into(),
                    name: Some("name-1".into()),
                    class: Some("class-1".into()),
                    race: Some("race-1".into()),
                    gender: Some("gender-1".into()),
                    light: Some(100),
                },
                Character {
                    id: "character-2".into(),
                    name: Some("name-2".into()),
                    class: Some("class-2".into()),
                    race: Some("race-2".into()),
                    gender: Some("gender-2".into()),
                    light: Some(200),
                },
            ],
            items: vec![
                ItemInstance {
                    instance_id: "instance-1".into(),
                    item_hash: ItemHash::new(7),
                    location: ItemLocation::Vault,
                    quantity: 1,
                    equipped: false,
                    locked: false,
                    power: Some(100),
                    plugs: Vec::new(),
                    perks: vec![
                        PerkSummary {
                            perk_hash: PerkHash::new(10),
                            active: true,
                            visible: true,
                        },
                        PerkSummary {
                            perk_hash: PerkHash::new(20),
                            active: false,
                            visible: false,
                        },
                    ],
                },
                ItemInstance {
                    instance_id: "instance-2".into(),
                    item_hash: ItemHash::new(8),
                    location: ItemLocation::Other("postmaster".into()),
                    quantity: 2,
                    equipped: true,
                    locked: true,
                    power: Some(200),
                    plugs: Vec::new(),
                    perks: Vec::new(),
                },
            ],
        }
    }

    fn character_ids(characters: &[Character]) -> Vec<&str> {
        characters
            .iter()
            .map(|character| character.id.as_str())
            .collect()
    }

    fn instance_ids(items: &[ItemInstance]) -> Vec<&str> {
        items.iter().map(|item| item.instance_id.as_str()).collect()
    }

    fn item_definition(hash: i64, name: &str, version: &str) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new(version),
        }
    }

    fn typed_definition(
        hash: i64,
        name: &str,
        item_type: Option<&str>,
        tags: Vec<&str>,
    ) -> ItemDefinition {
        let mut definition = item_definition(hash, name, "manifest-v1");
        definition.item_type = item_type.map(str::to_owned);
        definition.tags = tags.into_iter().map(str::to_owned).collect();
        definition
    }

    fn search_instance(
        instance_id: &str,
        item_hash: i64,
        location: ItemLocation,
        perk_hashes: Vec<u32>,
    ) -> ItemInstance {
        ItemInstance {
            instance_id: instance_id.into(),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            plugs: Vec::new(),
            perks: perk_hashes
                .into_iter()
                .map(|hash| PerkSummary {
                    perk_hash: PerkHash::new(hash),
                    active: true,
                    visible: true,
                })
                .collect(),
        }
    }

    fn definition_query(
        text: Option<&str>,
        item_type: Option<&str>,
        tags: Vec<&str>,
        limit: usize,
    ) -> DefinitionSearchQuery {
        DefinitionSearchQuery {
            text: text.map(|value| SearchTerm::new(value).unwrap()),
            item_type: item_type.map(|value| SearchTerm::new(value).unwrap()),
            tags: tags
                .into_iter()
                .map(|tag| SearchTerm::new(tag).unwrap())
                .collect(),
            element: None,
            slot: None,
            class_type: None,
            limit,
        }
    }

    fn instance_query(
        item_hashes: ItemHashFilter,
        perk_hash: Option<u32>,
        character: Option<&str>,
        limit: usize,
    ) -> InstanceSearchQuery {
        InstanceSearchQuery {
            item_hashes,
            perk_hash: perk_hash.map(PerkHash::new),
            perk_name: None,
            mod_name: None,
            character_id: character.map(CharacterId::new),
            definitions: None,
            min_power: None,
            limit,
        }
    }

    fn definition_hashes(definitions: &[ItemDefinition]) -> Vec<i64> {
        definitions
            .iter()
            .map(|definition| definition.hash.value())
            .collect()
    }

    fn instance_names(instances: &[ItemInstance]) -> Vec<&str> {
        instances
            .iter()
            .map(|instance| instance.instance_id.as_str())
            .collect()
    }

    fn definition_set(version: &str, definitions: Vec<ItemDefinition>) -> ManifestDefinitions {
        ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions,
        }
    }

    fn replacement(version: &str, definitions: Vec<ItemDefinition>) -> DefinitionReplacementInput {
        DefinitionReplacementInput {
            definitions: definition_set(version, definitions),
            checked_at: Timestamp::new(200),
            source: SourceMarker::Manifest,
        }
    }

    #[test]
    fn instance_definition_filters_apply_before_limit_and_match_core() {
        let definitions = vec![
            typed_definition(1, "Other", Some("Rocket Launcher"), vec![]),
            typed_definition(2, "Wanted", Some("Hand Cannon"), vec!["exotic"]),
        ];
        let mut repo = InMemorySearchRepository::with_definitions(definitions.clone());
        let items = vec![
            search_instance("a", 1, ItemLocation::Vault, vec![]),
            search_instance("b", 99, ItemLocation::Vault, vec![]),
            search_instance("c", 2, ItemLocation::Vault, vec![]),
        ];
        repo.set_instances(items.clone());
        let mut query = instance_query(ItemHashFilter::Any, None, None, 1);
        query.definitions = Some(Box::new(definition_query(
            None,
            Some("hand cannon"),
            vec![],
            100,
        )));
        let actual = repo.search_instances(&query).unwrap();
        assert_eq!(instance_names(&actual), ["c"]);
        let expected: Vec<_> = items
            .into_iter()
            .filter(|item| {
                destiny_core::query::instance_matches_with_definition(
                    &query,
                    item,
                    definitions
                        .iter()
                        .find(|definition| definition.hash == item.item_hash),
                )
            })
            .take(query.limit)
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn clock_and_ids_are_deterministic() {
        let mut clock = FakeClock::fixed(Timestamp::new(10));
        assert_eq!(clock.now().unwrap(), Timestamp::new(10));
        clock.advance(5).unwrap();
        assert_eq!(clock.now().unwrap(), Timestamp::new(15));
        let mut ids = DeterministicPlanIds::starting_at(4);
        assert_eq!(ids.next_plan_id().unwrap().as_str(), "plan-4");
        assert_eq!(ids.next_plan_id().unwrap().as_str(), "plan-5");
    }

    #[test]
    fn inventory_revisions_retain_then_advance() {
        let mut repo = InMemoryInventoryRepository::new();
        assert_eq!(
            repo.replace_current(input(1))
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
        assert_eq!(
            repo.replace_current(input(1))
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
        assert_eq!(
            repo.replace_current(input(2))
                .unwrap()
                .metadata
                .revision
                .value(),
            2
        );
    }

    #[test]
    fn every_normalized_field_participates_in_revision_equality() {
        let base = multi_input();

        let mutations: [RevisionMutation; 22] = [
            ("membership_id", |value| {
                value.membership_id = MembershipId::new("membership-2");
            }),
            ("membership_type", |value| {
                value.membership_type = MembershipType::new(2);
            }),
            ("definition_version", |value| {
                value.definition_version = DefinitionVersion::new("definitions-v2");
            }),
            ("character.id", |value| {
                value.characters[0].id = "character-1-changed".into();
            }),
            ("character.name", |value| {
                value.characters[0].name = Some("name-1-changed".into());
            }),
            ("character.class", |value| {
                value.characters[0].class = Some("class-1-changed".into());
            }),
            ("character.race", |value| {
                value.characters[0].race = Some("race-1-changed".into());
            }),
            ("character.gender", |value| {
                value.characters[0].gender = Some("gender-1-changed".into());
            }),
            ("character.light", |value| {
                value.characters[0].light = Some(101);
            }),
            ("item.instance_id", |value| {
                value.items[0].instance_id = "instance-1-changed".into();
            }),
            ("item.item_hash", |value| {
                value.items[0].item_hash = ItemHash::new(9);
            }),
            ("item.location", |value| {
                value.items[0].location = ItemLocation::Character("character-1".into());
            }),
            ("item.quantity", |value| {
                value.items[0].quantity = 3;
            }),
            ("item.equipped", |value| {
                value.items[0].equipped = true;
            }),
            ("item.locked", |value| {
                value.items[0].locked = true;
            }),
            ("item.power", |value| {
                value.items[0].power = Some(101);
            }),
            ("item.perks.order", |value| {
                value.items[0].perks.reverse();
            }),
            ("item.perks.perk_hash", |value| {
                value.items[0].perks[0].perk_hash = PerkHash::new(11);
            }),
            ("item.perks.active", |value| {
                value.items[0].perks[0].active = false;
            }),
            ("item.perks.visible", |value| {
                value.items[0].perks[0].visible = false;
            }),
            ("characters.membership", |value| {
                value.characters.pop();
            }),
            ("items.membership", |value| {
                value.items.pop();
            }),
        ];

        for (field, mutate) in mutations {
            let mut repo = InMemoryInventoryRepository::new();
            repo.replace_current(base.clone()).unwrap();
            let mut changed = base.clone();
            mutate(&mut changed);

            assert_eq!(
                repo.replace_current(changed)
                    .unwrap()
                    .metadata
                    .revision
                    .value(),
                2,
                "changing {field} must advance the revision"
            );
        }
    }

    #[test]
    fn unchanged_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
        let mut repo = InMemoryInventoryRepository::new();
        let first = repo.replace_current(input(1)).unwrap();
        assert_eq!(first.metadata.revision.value(), 1);
        assert_eq!(first.metadata.checked_at, Timestamp::new(200));

        let mut refreshed = input(1);
        refreshed.checked_at = Timestamp::new(9_000);
        let second = repo.replace_current(refreshed).unwrap();

        assert_eq!(second.metadata.revision.value(), 1);
        assert_eq!(second.metadata.checked_at, Timestamp::new(9_000));
        assert_eq!(second.metadata.source, SourceMarker::Profile);
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(second.metadata));
    }

    #[test]
    fn inventory_overflow_requires_a_content_change() {
        let original = input(1);
        let mut repo = InMemoryInventoryRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );

        // An unchanged refresh at a later timestamp must not overflow; it
        // retains the revision and refreshes the committed timestamp.
        let mut refreshed = original.clone();
        refreshed.checked_at = Timestamp::new(9_000);
        let retained = repo.replace_current(refreshed).unwrap();
        assert_eq!(retained.metadata.revision.value(), u64::MAX);
        assert_eq!(retained.metadata.checked_at, Timestamp::new(9_000));

        // A genuine content change at the boundary fails atomically.
        assert_eq!(
            repo.replace_current(input(2)),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(repo.load_current().unwrap().unwrap(), retained);
    }

    #[test]
    fn revision_overflow_is_atomic() {
        let original = input(1);
        let mut repo = InMemoryInventoryRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );
        let changed = input(2);
        assert_eq!(
            repo.replace_current(changed),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            repo.load_current()
                .unwrap()
                .unwrap()
                .metadata
                .revision
                .value(),
            u64::MAX
        );
        assert_eq!(
            repo.replace_current(original)
                .unwrap()
                .metadata
                .revision
                .value(),
            u64::MAX
        );
    }

    #[test]
    fn invalid_source_is_rejected_and_preserves_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut mismatched = input(2);
        mismatched.source = SourceMarker::Manifest;
        assert_eq!(
            repo.replace_current(mismatched),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn character_and_item_permutations_retain_revision_and_canonical_order() {
        let mut repo = InMemoryInventoryRepository::new();
        let first = repo.replace_current(multi_input()).unwrap();
        assert_eq!(first.metadata.revision.value(), 1);
        assert_eq!(
            character_ids(&first.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&first.items), ["instance-1", "instance-2"]);

        let mut permuted = multi_input();
        permuted.characters.reverse();
        permuted.items.reverse();
        let second = repo.replace_current(permuted).unwrap();

        assert_eq!(second.metadata.revision.value(), 1);
        assert_eq!(
            character_ids(&second.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&second.items), ["instance-1", "instance-2"]);
        assert_eq!(repo.load_current().unwrap(), Some(second));
    }

    #[test]
    fn permuted_content_with_later_checked_at_retains_revision_and_refreshes_metadata() {
        let mut repo = InMemoryInventoryRepository::new();
        repo.replace_current(multi_input()).unwrap();

        let mut permuted = multi_input();
        permuted.characters.reverse();
        permuted.items.reverse();
        permuted.checked_at = Timestamp::new(9_000);
        let refreshed = repo.replace_current(permuted).unwrap();

        assert_eq!(refreshed.metadata.revision.value(), 1);
        assert_eq!(refreshed.metadata.checked_at, Timestamp::new(9_000));
        assert_eq!(
            character_ids(&refreshed.characters),
            ["character-1", "character-2"]
        );
        assert_eq!(instance_ids(&refreshed.items), ["instance-1", "instance-2"]);
    }

    #[test]
    fn snapshot_input_structural_validity_covers_emptiness_duplicates_and_locations() {
        let mut candidate = multi_input();
        assert!(candidate.is_structurally_valid());

        candidate.characters.clear();
        candidate.items.clear();
        assert!(candidate.is_structurally_valid());

        let mut duplicate_characters = multi_input();
        duplicate_characters
            .characters
            .push(duplicate_characters.characters[0].clone());
        assert!(!duplicate_characters.is_structurally_valid());

        let mut duplicate_items = multi_input();
        duplicate_items.items.push(duplicate_items.items[0].clone());
        assert!(!duplicate_items.is_structurally_valid());

        let mut dangling = multi_input();
        dangling.items[0].location = ItemLocation::Character("missing-character".into());
        assert!(!dangling.is_structurally_valid());
    }

    #[test]
    fn duplicate_identifiers_are_rejected_and_preserve_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut duplicate_characters = multi_input();
        duplicate_characters
            .characters
            .push(duplicate_characters.characters[0].clone());
        assert_eq!(
            repo.replace_current(duplicate_characters),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        let mut duplicate_items = multi_input();
        duplicate_items.items.push(duplicate_items.items[0].clone());
        assert_eq!(
            repo.replace_current(duplicate_items),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn dangling_item_location_is_rejected_and_preserves_the_prior_snapshot() {
        let mut repo = InMemoryInventoryRepository::new();
        let snapshot = repo.replace_current(input(1)).unwrap();
        let metadata = snapshot.metadata.clone();

        let mut dangling = multi_input();
        dangling.items[0].location = ItemLocation::Character("missing-character".into());
        assert_eq!(
            repo.replace_current(dangling),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repo.load_current().unwrap(), Some(snapshot));
        assert_eq!(repo.snapshot_metadata().unwrap(), Some(metadata));
    }

    #[test]
    fn present_character_location_is_accepted() {
        let mut repo = InMemoryInventoryRepository::new();
        repo.replace_current(input(1)).unwrap();

        let mut referenced = multi_input();
        referenced.items[0].location = ItemLocation::Character("character-2".into());
        let committed = repo.replace_current(referenced).unwrap();

        assert_eq!(committed.metadata.revision.value(), 2);
        assert_eq!(
            committed.items[0].location,
            ItemLocation::Character("character-2".into())
        );
    }

    #[test]
    fn clock_and_plan_ids_report_exhaustion_without_wrap() {
        let mut clock = FakeClock::fixed(Timestamp::new(u64::MAX));
        assert_eq!(
            clock.advance(1),
            Err(AppError::new(ErrorCategory::ClockOverflow))
        );
        assert_eq!(clock.now().unwrap().seconds(), u64::MAX);

        let mut ids = DeterministicPlanIds::starting_at(u64::MAX);
        assert_eq!(
            ids.next_plan_id(),
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        );
        assert_eq!(
            ids.next_plan_id(),
            Err(AppError::new(ErrorCategory::PlanIdExhausted))
        );
    }

    #[test]
    fn gateways_sequence_queued_failures_without_remote_side_effects() {
        let mut gateway = QueuedProfileGateway::new();
        gateway.queue_membership(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        assert_eq!(
            gateway.resolve_membership().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(gateway.membership_calls(), 1);
        assert_eq!(
            gateway.resolve_membership().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(gateway.membership_calls(), 2);
        // An exhausted acquisition queue fails closed without inventing a snapshot.
        let membership =
            DestinyMembership::new(MembershipType::new(3), MembershipId::new("7")).unwrap();
        assert_eq!(
            gateway.load_inventory(&membership).unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(gateway.inventory_calls(), 1);
    }

    #[test]
    fn repositories_and_gateways_cover_contract_outcomes() {
        let hash = ItemHash::new(7);
        let definition = ItemDefinition {
            hash,
            bucket_hash: None,
            name: "Item".into(),
            item_type: None,
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: Vec::new(),
            version: DefinitionVersion::new("definitions-v1"),
        };
        let repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("definitions-v1"),
            vec![definition.clone(), definition.clone()],
        );
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "definitions-v1"
        );
        assert!(matches!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            Resolution::Missing
        ));
        assert!(matches!(
            repository.lookup_definition(hash).unwrap(),
            Resolution::Ambiguous(_)
        ));
        let unique = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("definitions-v1"),
            vec![definition],
        );
        assert!(matches!(
            unique.lookup_definition(hash).unwrap(),
            Resolution::Unique(_)
        ));

        let mut manifest = QueuedManifestGateway::new();
        let acquisition = ManifestAcquisition {
            version: DefinitionVersion::new("v1"),
            definitions: Vec::new(),
            skipped_count: 0,
        };
        manifest.queue(Ok(acquisition.clone()));
        manifest.queue(Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(manifest.load_definitions().unwrap(), acquisition);
        assert_eq!(
            manifest.load_definitions().unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(
            manifest.load_definitions().unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(manifest.calls(), 3);

        let mut profile = QueuedProfileGateway::new();
        let membership = DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new("4611686018484639825"),
        )
        .unwrap();
        profile.queue_membership(Ok(membership.clone()));
        profile.queue_inventory(Ok(input(1)));
        profile.queue_inventory(Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(profile.resolve_membership().unwrap(), membership);
        assert_eq!(profile.load_inventory(&membership).unwrap(), input(1));
        assert_eq!(
            profile.load_inventory(&membership).unwrap_err().category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(
            profile.load_inventory(&membership).unwrap_err().category(),
            ErrorCategory::ProviderUnavailable
        );
        assert_eq!(profile.membership_calls(), 1);
        assert_eq!(profile.inventory_calls(), 3);
        let requested = vec![membership.clone(), membership.clone(), membership.clone()];
        assert_eq!(profile.requested_memberships(), requested.as_slice());

        let mut inventory = InMemoryInventoryRepository::new();
        assert_eq!(inventory.load_current().unwrap(), None);
        inventory.replace_current(input(1)).unwrap();
        assert_eq!(
            inventory
                .load_current()
                .unwrap()
                .unwrap()
                .metadata
                .revision
                .value(),
            1
        );
    }

    #[test]
    fn definition_publication_replaces_the_complete_set() {
        let mut repository = InMemoryDefinitionRepository::default();
        assert_eq!(
            repository.current_definition_version(),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(repository.definition_count(), 0);

        repository
            .replace_definitions(replacement(
                "manifest-v1",
                vec![
                    item_definition(7, "Alpha", "manifest-v1"),
                    item_definition(8, "Beta", "manifest-v1"),
                ],
            ))
            .unwrap();
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 2);
        assert_eq!(
            repository
                .current_metadata()
                .unwrap()
                .unwrap()
                .revision
                .value(),
            1
        );

        repository
            .replace_definitions(replacement(
                "manifest-v2",
                vec![item_definition(9, "Gamma", "manifest-v2")],
            ))
            .unwrap();
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v2"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Unique(item_definition(9, "Gamma", "manifest-v2"))
        );
        assert_eq!(repository.publication_attempts(), 2);
        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.dataset, Dataset::ManifestDefinitions);
        assert_eq!(metadata.source, SourceMarker::Manifest);
        assert_eq!(metadata.checked_at, Timestamp::new(200));
        assert_eq!(metadata.revision.value(), 2);
    }

    #[test]
    fn definition_publication_is_idempotent_for_an_equal_normalized_set() {
        let mut repository = InMemoryDefinitionRepository::default();
        let input = replacement(
            "manifest-v1",
            vec![
                item_definition(7, "Alpha", "manifest-v1"),
                item_definition(8, "Beta", "manifest-v1"),
            ],
        );

        let first = repository.replace_definitions(input.clone()).unwrap();
        let version = repository.current_definition_version().unwrap();
        let alpha = repository.lookup_definition(ItemHash::new(7)).unwrap();
        let beta = repository.lookup_definition(ItemHash::new(8)).unwrap();

        let second = repository.replace_definitions(input).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.revision.value(), 1);
        assert_eq!(repository.current_definition_version().unwrap(), version);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            alpha
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(8)).unwrap(),
            beta
        );
        assert_eq!(repository.definition_count(), 2);
        assert!(matches!(alpha, Resolution::Unique(_)));
        assert!(matches!(beta, Resolution::Unique(_)));
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_advances_on_normalized_content_change() {
        let base = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let mut content_changed = base.clone();
        content_changed.definitions.definitions[0].name = "Alpha Changed".into();

        let mut version_changed = base.clone();
        version_changed.definitions = definition_set(
            "manifest-v2",
            vec![item_definition(7, "Alpha", "manifest-v2")],
        );

        for (label, changed) in [("content", content_changed), ("version", version_changed)] {
            let mut repository = InMemoryDefinitionRepository::default();
            repository.replace_definitions(base.clone()).unwrap();

            let metadata = repository.replace_definitions(changed).unwrap();

            assert_eq!(
                metadata.revision.value(),
                2,
                "changing {label} must advance"
            );
            assert_eq!(repository.publication_attempts(), 2);
        }
    }

    #[test]
    fn definition_revision_ignores_definition_order_but_not_tag_order() {
        let mut alpha = item_definition(7, "Alpha", "manifest-v1");
        alpha.tags = vec!["first".to_owned(), "second".to_owned()];
        let beta = item_definition(8, "Beta", "manifest-v1");

        let mut repository = InMemoryDefinitionRepository::default();
        let base = replacement("manifest-v1", vec![alpha.clone(), beta.clone()]);
        let first = repository.replace_definitions(base).unwrap();
        assert_eq!(first.revision.value(), 1);

        // A pure permutation of the complete set retains the revision: the port
        // publishes a set and the durable schema stores no definition ordinal,
        // so definition order is not content identity.
        let permuted = replacement("manifest-v1", vec![beta.clone(), alpha.clone()]);
        let retained = repository.replace_definitions(permuted).unwrap();
        assert_eq!(retained.revision.value(), 1);

        // Each record is compared in full, so tag order inside a record remains
        // identity and advancing the revision exactly once.
        let mut retagged = alpha;
        retagged.tags = vec!["second".to_owned(), "first".to_owned()];
        let advanced = repository
            .replace_definitions(replacement("manifest-v1", vec![retagged, beta]))
            .unwrap();
        assert_eq!(advanced.revision.value(), 2);
    }

    #[test]
    fn definition_publication_retains_revision_and_refreshes_checked_at() {
        let mut repository = InMemoryDefinitionRepository::default();
        let base = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let first = repository.replace_definitions(base.clone()).unwrap();
        assert_eq!(first.revision.value(), 1);
        assert_eq!(first.checked_at, Timestamp::new(200));

        let mut refreshed = base;
        refreshed.checked_at = Timestamp::new(9_000);
        let second = repository.replace_definitions(refreshed).unwrap();

        assert_eq!(second.revision.value(), 1);
        assert_eq!(second.checked_at, Timestamp::new(9_000));
        assert_eq!(second.source, SourceMarker::Manifest);
        assert_eq!(repository.current_metadata().unwrap(), Some(second));
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_rejects_invalid_source_and_preserves_metadata() {
        let mut repository = InMemoryDefinitionRepository::default();
        let committed = repository
            .replace_definitions(replacement(
                "manifest-v1",
                vec![item_definition(7, "Alpha", "manifest-v1")],
            ))
            .unwrap();

        let mut mismatched = replacement(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        );
        mismatched.source = SourceMarker::Profile;

        assert_eq!(
            repository.replace_definitions(mismatched),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        assert_eq!(repository.current_metadata().unwrap(), Some(committed));
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Missing
        );
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn definition_publication_overflow_is_atomic() {
        let original = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let mut repository = InMemoryDefinitionRepository::with_revision_for_test(
            original.clone(),
            SyncRevision::new(u64::MAX),
        );

        // An unchanged refresh at the boundary retains the revision and refreshes
        // the committed timestamp rather than overflowing.
        let mut refreshed = original.clone();
        refreshed.checked_at = Timestamp::new(9_000);
        let retained = repository.replace_definitions(refreshed).unwrap();
        assert_eq!(retained.revision.value(), u64::MAX);
        assert_eq!(retained.checked_at, Timestamp::new(9_000));

        let changed = replacement(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        );
        assert_eq!(
            repository.replace_definitions(changed),
            Err(AppError::new(ErrorCategory::LocalStorage))
        );

        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.revision.value(), u64::MAX);
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(repository.publication_attempts(), 2);

        let retained = repository.replace_definitions(original).unwrap();
        assert_eq!(retained.revision.value(), u64::MAX);
        assert_eq!(repository.publication_attempts(), 3);
    }

    #[test]
    fn definition_publication_rejects_duplicate_hashes_before_mutation() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let error = repository.replace_definitions(replacement(
            "manifest-v2",
            vec![
                item_definition(7, "Alpha", "manifest-v2"),
                item_definition(7, "Alpha duplicate", "manifest-v2"),
            ],
        ));

        assert_eq!(error, Err(AppError::new(ErrorCategory::ProviderValidation)));
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(repository.publication_attempts(), 1);
    }

    #[test]
    fn definition_publication_rejects_mixed_versions_and_empty_sets_before_mutation() {
        let mut repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );

        let candidates = [
            replacement(
                "manifest-v2",
                vec![
                    item_definition(7, "Alpha", "manifest-v2"),
                    item_definition(8, "Beta", "manifest-v1"),
                ],
            ),
            replacement("manifest-v2", Vec::new()),
        ];

        for candidate in candidates {
            assert_eq!(
                repository.replace_definitions(candidate),
                Err(AppError::new(ErrorCategory::ProviderValidation))
            );
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                "manifest-v1"
            );
            assert_eq!(repository.definition_count(), 1);
        }
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(repository.publication_attempts(), 2);
    }

    #[test]
    fn injected_publication_failure_preserves_prior_state() {
        let seeded = replacement(
            "manifest-v1",
            vec![item_definition(7, "Alpha", "manifest-v1")],
        );
        let mut repository =
            InMemoryDefinitionRepository::with_revision_for_test(seeded, SyncRevision::new(3))
                .with_publication_failure(AppError::new(ErrorCategory::ProviderUnavailable));

        let error = repository.replace_definitions(replacement(
            "manifest-v2",
            vec![item_definition(9, "Gamma", "manifest-v2")],
        ));

        assert_eq!(
            error,
            Err(AppError::new(ErrorCategory::ProviderUnavailable))
        );
        assert_eq!(
            repository.current_definition_version().unwrap().as_str(),
            "manifest-v1"
        );
        assert_eq!(repository.definition_count(), 1);
        assert_eq!(
            repository.lookup_definition(ItemHash::new(7)).unwrap(),
            Resolution::Unique(item_definition(7, "Alpha", "manifest-v1"))
        );
        assert_eq!(
            repository.lookup_definition(ItemHash::new(9)).unwrap(),
            Resolution::Missing
        );
        let metadata = repository.current_metadata().unwrap().unwrap();
        assert_eq!(metadata.revision.value(), 3);
        assert_eq!(metadata.checked_at, Timestamp::new(200));
        assert_eq!(metadata.source, SourceMarker::Manifest);
        assert_eq!(repository.publication_attempts(), 1);
    }

    #[test]
    fn published_lookup_and_version_reads_are_deterministic() {
        let alpha = item_definition(7, "Alpha", "manifest-v1");
        let beta = item_definition(7, "Beta", "manifest-v1");
        let repository = InMemoryDefinitionRepository::new(
            DefinitionVersion::new("manifest-v1"),
            vec![alpha.clone(), beta.clone()],
        );

        for _ in 0..3 {
            assert_eq!(
                repository.current_definition_version().unwrap().as_str(),
                "manifest-v1"
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(7)).unwrap(),
                Resolution::Ambiguous(vec![alpha.clone(), beta.clone()])
            );
            assert_eq!(
                repository.lookup_definition(ItemHash::new(99)).unwrap(),
                Resolution::Missing
            );
        }
    }

    #[test]
    fn in_memory_credential_store_round_trips_complete_credentials_and_redacts_material() {
        let material = CredentialMaterial::from_bytes(b"secret");
        assert_eq!(format!("{material:?}"), "CredentialMaterial");
        let credential = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(42),
            },
            material,
            refresh_grant: None,
        };
        let mut store = InMemoryCredentialStore::default();
        store.save(credential.clone()).unwrap();
        assert!(store.contains_credential());
        let loaded = store.load().unwrap().expect("stored credential");
        assert_eq!(loaded, credential);
        assert_eq!(loaded.metadata.expires_at, Timestamp::new(42));
        assert_eq!(loaded.material.expose_secret(), b"secret");
        store.delete().unwrap();
        assert!(!store.contains_credential());
    }

    #[test]
    fn search_repository_matches_normalized_name_type_and_tags() {
        let repository = InMemorySearchRepository::with_definitions(vec![
            typed_definition(
                5,
                "Gjallarhorn",
                Some("Rocket Launcher"),
                vec!["exotic", "solar"],
            ),
            typed_definition(
                2,
                "Gjallarswift",
                Some("Rocket Launcher"),
                vec!["legendary"],
            ),
            typed_definition(9, "Ghost Shell", Some("Ghost Shell"), vec!["exotic"]),
        ]);

        // Name matching is a case-insensitive normalized substring.
        let by_name = repository
            .search_definitions(&definition_query(Some("  GJALLAR "), None, Vec::new(), 10))
            .unwrap();
        assert_eq!(definition_hashes(&by_name), [2, 5]);

        // Type matching is exact normalized equality, not a substring.
        let by_type = repository
            .search_definitions(&definition_query(
                None,
                Some("rocket launcher"),
                Vec::new(),
                10,
            ))
            .unwrap();
        assert_eq!(definition_hashes(&by_type), [2, 5]);

        // Tag matching requires every listed tag after normalization.
        let by_tags = repository
            .search_definitions(&definition_query(None, None, vec!["EXOTIC", " solar "], 10))
            .unwrap();
        assert_eq!(definition_hashes(&by_tags), [5]);

        // An unfiltered query matches every definition, bounded by the limit.
        let unfiltered = repository
            .search_definitions(&definition_query(None, None, Vec::new(), 10))
            .unwrap();
        assert_eq!(definition_hashes(&unfiltered), [2, 5, 9]);
    }

    #[test]
    fn definition_search_orders_by_hash_then_truncates_to_the_limit() {
        let repository = InMemorySearchRepository::with_definitions(vec![
            typed_definition(30, "Alpha", None, Vec::new()),
            typed_definition(10, "Beta", None, Vec::new()),
            typed_definition(20, "Gamma", None, Vec::new()),
        ]);

        let limited = repository
            .search_definitions(&definition_query(None, None, Vec::new(), 2))
            .unwrap();
        assert_eq!(definition_hashes(&limited), [10, 20]);

        let full = repository
            .search_definitions(&definition_query(None, None, Vec::new(), 3))
            .unwrap();
        assert_eq!(definition_hashes(&full), [10, 20, 30]);

        // Repeated reads are byte-for-byte deterministic.
        for _ in 0..3 {
            assert_eq!(
                definition_hashes(
                    &repository
                        .search_definitions(&definition_query(None, None, Vec::new(), 3))
                        .unwrap()
                ),
                [10, 20, 30]
            );
        }
    }

    #[test]
    fn empty_definition_search_is_an_explicit_no_match() {
        let repository = InMemorySearchRepository::with_definitions(vec![typed_definition(
            1,
            "Gjallarhorn",
            Some("Rocket Launcher"),
            vec!["exotic"],
        )]);
        let empty = repository
            .search_definitions(&definition_query(
                Some("vex mythoclast"),
                None,
                Vec::new(),
                10,
            ))
            .unwrap();
        assert!(empty.is_empty());

        let empty_projection = InMemorySearchRepository::new();
        assert!(
            empty_projection
                .search_definitions(&definition_query(None, None, Vec::new(), 10))
                .unwrap()
                .is_empty()
        );
        assert!(
            empty_projection
                .search_instances(&instance_query(ItemHashFilter::Any, None, None, 10))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn instance_search_filters_by_hash_perk_and_character() {
        let mut repository = InMemorySearchRepository::new();
        repository.set_instances(vec![
            search_instance(
                "instance-1",
                7,
                ItemLocation::Character(CharacterId::new("warlock")),
                vec![10, 20],
            ),
            search_instance("instance-2", 7, ItemLocation::Vault, vec![30]),
            search_instance("instance-3", 8, ItemLocation::Vault, vec![10]),
        ]);

        let by_hash = repository
            .search_instances(&instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(7)]),
                None,
                None,
                10,
            ))
            .unwrap();
        assert_eq!(instance_names(&by_hash), ["instance-1", "instance-2"]);

        // A perk filter is exact and combines with the definition constraint.
        let by_perk = repository
            .search_instances(&instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(7)]),
                Some(10),
                None,
                10,
            ))
            .unwrap();
        assert_eq!(instance_names(&by_perk), ["instance-1"]);

        // A perk-only search uses the explicit `Any` constraint.
        let perk_only = repository
            .search_instances(&instance_query(ItemHashFilter::Any, Some(10), None, 10))
            .unwrap();
        assert_eq!(instance_names(&perk_only), ["instance-1", "instance-3"]);

        // A character filter excludes the Vault and unknown locations.
        let by_character = repository
            .search_instances(&instance_query(
                ItemHashFilter::Any,
                None,
                Some("warlock"),
                10,
            ))
            .unwrap();
        assert_eq!(instance_names(&by_character), ["instance-1"]);

        // An empty hash list is a fail-closed no-match, not an unfiltered search.
        assert!(
            repository
                .search_instances(&instance_query(
                    ItemHashFilter::AnyOf(Vec::new()),
                    None,
                    None,
                    10
                ))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn instance_search_orders_by_identifier_then_truncates() {
        let mut repository = InMemorySearchRepository::new();
        repository.set_instances(vec![
            search_instance("instance-c", 7, ItemLocation::Vault, Vec::new()),
            search_instance("instance-a", 7, ItemLocation::Vault, Vec::new()),
            search_instance("instance-b", 7, ItemLocation::Vault, Vec::new()),
        ]);

        let ordered = repository
            .search_instances(&instance_query(ItemHashFilter::Any, None, None, 10))
            .unwrap();
        assert_eq!(
            instance_names(&ordered),
            ["instance-a", "instance-b", "instance-c"]
        );

        let limited = repository
            .search_instances(&instance_query(ItemHashFilter::Any, None, None, 2))
            .unwrap();
        assert_eq!(instance_names(&limited), ["instance-a", "instance-b"]);
    }

    #[test]
    fn lookup_instance_reports_missing_unique_and_ambiguous() {
        // Duplicate instance identifiers cannot occur in a structurally valid
        // projection, so this seeds an expressly invalid state to prove the
        // inherited `Resolution::Ambiguous` outcome is still reported rather
        // than silently collapsed.
        let duplicate_first = search_instance("instance-1", 7, ItemLocation::Vault, Vec::new());
        let duplicate_second = search_instance(
            "instance-1",
            8,
            ItemLocation::Other("postmaster".into()),
            Vec::new(),
        );

        let mut repository = InMemorySearchRepository::new();
        repository.set_instances(vec![duplicate_first.clone(), duplicate_second.clone()]);
        assert_eq!(
            repository
                .lookup_instance(&ItemInstanceId::new("instance-1"))
                .unwrap(),
            Resolution::Ambiguous(vec![duplicate_first.clone(), duplicate_second])
        );
        assert_eq!(
            repository
                .lookup_instance(&ItemInstanceId::new("missing"))
                .unwrap(),
            Resolution::Missing
        );

        repository.set_instances(vec![duplicate_first.clone()]);
        assert_eq!(
            repository
                .lookup_instance(&ItemInstanceId::new("instance-1"))
                .unwrap(),
            Resolution::Unique(duplicate_first)
        );
    }

    #[test]
    fn search_repository_exposes_only_the_current_projection() {
        let mut repository = InMemorySearchRepository::with_definitions(vec![typed_definition(
            1,
            "Old Name",
            None,
            vec!["old-tag"],
        )]);
        repository.set_instances(vec![search_instance(
            "instance-1",
            1,
            ItemLocation::Vault,
            Vec::new(),
        )]);

        repository.set_definitions(vec![typed_definition(2, "New Name", None, vec!["new-tag"])]);
        repository.set_instances(vec![search_instance(
            "instance-2",
            2,
            ItemLocation::Vault,
            Vec::new(),
        )]);

        assert_eq!(repository.definition_count(), 1);
        assert_eq!(repository.instance_count(), 1);
        assert!(
            repository
                .search_definitions(&definition_query(Some("old name"), None, Vec::new(), 10))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            definition_hashes(
                &repository
                    .search_definitions(&definition_query(None, None, Vec::new(), 10))
                    .unwrap()
            ),
            [2]
        );
        assert_eq!(
            repository
                .lookup_instance(&ItemInstanceId::new("instance-1"))
                .unwrap(),
            Resolution::Missing
        );
        assert!(matches!(
            repository
                .lookup_instance(&ItemInstanceId::new("instance-2"))
                .unwrap(),
            Resolution::Unique(_)
        ));
    }

    #[test]
    fn search_repository_rejects_invalid_queries_with_a_sanitized_error() {
        let repository = InMemorySearchRepository::with_definitions(vec![typed_definition(
            1,
            "Gjallarhorn",
            None,
            Vec::new(),
        )]);

        let mut invalid_definitions = definition_query(Some("ghost"), None, Vec::new(), 10);
        invalid_definitions.limit = 0;
        assert_eq!(
            repository.search_definitions(&invalid_definitions),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        let mut invalid_instances = instance_query(ItemHashFilter::Any, None, None, 10);
        invalid_instances.limit = MAX_SEARCH_LIMIT + 1;
        assert_eq!(
            repository.search_instances(&invalid_instances),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );
    }

    #[test]
    fn search_repository_bounds_the_instance_hash_filter_cardinality() {
        let repository = InMemorySearchRepository::with_definitions(vec![typed_definition(
            1,
            "Gjallarhorn",
            None,
            Vec::new(),
        )]);

        // The boundary is accepted and the over-bound list is rejected with a
        // sanitized error before any projection read.
        let boundary: Vec<ItemHash> = (0..MAX_HASH_FILTERS as i64).map(ItemHash::new).collect();
        let at_boundary = instance_query(ItemHashFilter::AnyOf(boundary), None, None, 10);
        assert_eq!(repository.search_instances(&at_boundary), Ok(Vec::new()));

        let over: Vec<ItemHash> = (0..=MAX_HASH_FILTERS as i64).map(ItemHash::new).collect();
        let over_bound = instance_query(ItemHashFilter::AnyOf(over), None, None, 10);
        assert_eq!(
            repository.search_instances(&over_bound),
            Err(AppError::new(ErrorCategory::ProviderValidation))
        );

        // An empty `AnyOf` stays valid and still matches nothing, so the
        // `Any` versus `AnyOf(empty)` distinction survives the bound.
        let empty_filter = instance_query(ItemHashFilter::AnyOf(Vec::new()), None, None, 10);
        assert_eq!(repository.search_instances(&empty_filter), Ok(Vec::new()));
    }

    #[test]
    fn search_repository_seeds_from_a_committed_snapshot() {
        let snapshot = InventorySnapshot {
            metadata: SnapshotMetadata {
                membership_id: MembershipId::new("membership"),
                membership_type: MembershipType::new(1),
                revision: SyncRevision::new(1),
                definition_version: DefinitionVersion::new("manifest-v1"),
                checked_at: Timestamp::new(100),
                source: SourceMarker::Profile,
            },
            characters: Vec::new(),
            items: vec![search_instance(
                "instance-1",
                7,
                ItemLocation::Vault,
                vec![10],
            )],
        };
        let repository = InMemorySearchRepository::with_projection(
            vec![typed_definition(7, "Gjallarhorn", None, Vec::new())],
            &snapshot,
        );

        assert_eq!(repository.definition_count(), 1);
        assert_eq!(repository.instance_count(), 1);
        assert_eq!(
            instance_names(
                &repository
                    .search_instances(&instance_query(ItemHashFilter::Any, Some(10), None, 10))
                    .unwrap()
            ),
            ["instance-1"]
        );
    }
}
