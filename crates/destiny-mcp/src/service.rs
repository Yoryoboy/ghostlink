//! Injectable read and mutation service seams and their translation.
//!
//! [`InventoryReadService`] is the narrow, object-safe read seam the stdio
//! runtime calls: one method per semantic read, each taking an already-validated
//! request and returning a bounded MCP output or a sanitized [`McpError`].
//! [`QueryReadService`] is the production read implementation. It composes the
//! shared application [`QueryService`] with an injected clock and freshness
//! policy, so the MCP layer adds no freshness, search, storage, or provider
//! policy of its own.
//!
//! [`MutationToolService`] is the matching object-safe mutation seam, and
//! [`ComposedMutationService`] is its production implementation. One
//! [`MutationToolService::move_item`] call derives the provider membership
//! internally, plans against the shared planner, forces the fresh preflight,
//! issues only the ordered steps, refreshes the projection after the action, and
//! verifies the destination, so the MCP layer adds no planning, execution, or
//! verification policy of its own and no caller supplies a membership or a
//! credential.
//!
//! # Argument bounds
//!
//! Converting the raw MCP arguments into a shared read request is where the
//! adapter bounds live. Definition and instance limits, tag counts, hash-candidate
//! counts, and search text lengths are all checked against the shared read
//! contract before the service is invoked, and the shared query types validate
//! again once constructed. A rejected argument produces the sanitized
//! [`McpError`] for [`ErrorCategory::ProviderValidation`] and never echoes the
//! offending value.
//!
//! # Identity
//!
//! [`DefinitionView`] carries a definition hash and [`InstanceView`] carries a
//! concrete instance identifier; the two are never interchanged. Search reports
//! definition candidates separately from instance hits, and inspection reports
//! the exact definition beside the concrete instance.

use destiny_app::{
    AppError, Character, CharacterId, CharacterListResult, Clock, Dataset, DefinitionRepository,
    DefinitionSearchQuery, DestinationSelector, ErrorCategory, ExecutionEvidence, FreshnessPolicy,
    FreshnessStatus, InspectItemRequest, InstanceSearchQuery, InventoryRepository,
    InventorySearchResult, ItemDefinition, ItemHash, ItemHashFilter, ItemInspection, ItemInstance,
    ItemInstanceId, ItemLocation, ItemSelector, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT,
    MAX_TAG_FILTERS, MembershipId, MoveEffects, MoveExecutionOutcome, MoveIntentAuthorization,
    MovePlan, MoveRequest, MutationError, MutationService, PerkHash, PlanIdGenerator, PlanLifetime,
    ProfileGateway, ProfileSyncReason, QueryMetadata, QueryService, Resolution,
    SearchInventoryRequest, SearchRepository, SearchTerm, SourceMarker, StepAction, StepOutcome,
    Timestamp, TransferGateway, TransferRejection, VerificationFailureReason,
};

use crate::schema::{
    CharacterView, DefinitionView, FoundInspection, IncompleteMoveView, InspectItemArgs,
    InspectItemOutput, InstanceQueryArgs, InstanceView, InventorySearchHitView, ListCharactersArgs,
    ListCharactersOutput, LocationView, MAX_IDENTIFIER_LENGTH, McpError, MoveEffectsView,
    MoveItemArgs, MoveItemOutput, MoveSuccessView, PerkView, QueryMetadataView,
    SearchInventoryArgs, SearchInventoryOutput, StepEvidenceView, StepOutcomeView,
    TransferRejectionView, VerificationFailureReasonView, VerificationFailureView,
};

/// Weekly caller-selection errors remain distinct from application/tool failures.
#[derive(Debug)]
pub enum WeeklyReadError {
    InvalidArguments,
    Tool(McpError),
}
impl From<McpError> for WeeklyReadError {
    fn from(error: McpError) -> Self {
        Self::Tool(error)
    }
}
impl From<destiny_app::weekly_selection::WeeklySelectionError> for WeeklyReadError {
    fn from(error: destiny_app::weekly_selection::WeeklySelectionError) -> Self {
        match error {
            destiny_app::weekly_selection::WeeklySelectionError::InvalidRequest => {
                Self::InvalidArguments
            }
            destiny_app::weekly_selection::WeeklySelectionError::AmbiguousMatch => {
                Self::Tool(McpError::new(ErrorCategory::AmbiguousMatch))
            }
        }
    }
}

/// A validated `list_characters` request. The tool takes no arguments.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ListCharactersRequest;

/// The narrow read seam the MCP runtime calls.
///
/// The trait is deliberately object-safe and free of SDK types, so a server can
/// hold `Box<dyn InventoryReadService>` and tests can inject any implementation
/// without touching `rmcp`. Requests are already validated and bounded; outputs
/// are the bounded MCP projections defined in [`crate::schema`].
pub trait InventoryReadService {
    /// Reads saved in-game loadouts when composed with a loadout service.
    fn get_loadouts(
        &mut self,
        _character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetLoadoutsOutput, McpError> {
        Err(McpError::new(ErrorCategory::DefinitionsUnavailable))
    }
    /// Reads live artifact state when composed with an artifact service.
    fn get_artifact(
        &mut self,
        _character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetArtifactOutput, McpError> {
        Err(McpError::new(ErrorCategory::DefinitionsUnavailable))
    }
    /// Reads available subclass options with the shared status policy.
    fn get_subclass_options(
        &mut self,
        _character: Option<&CharacterId>,
        _element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        Err(McpError::new(ErrorCategory::DefinitionsUnavailable))
    }
    /// Reads equipped subclasses for one selector, or all characters.
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError>;
    fn weekly(
        &mut self,
        _args: &crate::schema::WeeklyArgs,
    ) -> Result<crate::schema::WeeklyOutput, WeeklyReadError> {
        Err(McpError::new(ErrorCategory::DefinitionsUnavailable).into())
    }
    /// Reads the current public rotation; no account state is required.
    fn weekly_reset(&mut self) -> Result<crate::schema::WeeklyResetOutput, McpError> {
        Err(McpError::new(ErrorCategory::DefinitionsUnavailable))
    }

    /// Lists the committed characters.
    fn list_characters(
        &mut self,
        request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError>;

    /// Runs one bounded definition search and optional instance search.
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError>;

    /// Inspects exactly one concrete instance.
    fn inspect_item(&mut self, request: &InspectItemRequest)
    -> Result<InspectItemOutput, McpError>;
}

/// Adds the live artifact sibling service without changing inventory policy.
pub struct ArtifactReadService<A, I, C> {
    artifact: A,
    inventory: I,
    clock: C,
    policy: FreshnessPolicy,
}

impl<A, I, C> ArtifactReadService<A, I, C> {
    pub fn new(artifact: A, inventory: I, clock: C, policy: FreshnessPolicy) -> Self {
        Self {
            artifact,
            inventory,
            clock,
            policy,
        }
    }
}

impl<A: destiny_app::ArtifactOperations, I: InventoryReadService, C: Clock> InventoryReadService
    for ArtifactReadService<A, I, C>
{
    fn get_subclass_options(
        &mut self,
        character: Option<&CharacterId>,
        element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        self.inventory.get_subclass_options(character, element)
    }
    fn get_artifact(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetArtifactOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .artifact
            .artifact(character, self.policy, now)
            .map_err(McpError::from)?;
        Ok(artifact_output(&result))
    }
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError> {
        self.inventory.get_subclass(character)
    }
    fn weekly(
        &mut self,
        args: &crate::schema::WeeklyArgs,
    ) -> Result<crate::schema::WeeklyOutput, WeeklyReadError> {
        self.inventory.weekly(args)
    }
    fn weekly_reset(&mut self) -> Result<crate::schema::WeeklyResetOutput, McpError> {
        self.inventory.weekly_reset()
    }
    fn list_characters(
        &mut self,
        request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        self.inventory.list_characters(request)
    }
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        self.inventory.search_inventory(request)
    }
    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<InspectItemOutput, McpError> {
        self.inventory.inspect_item(request)
    }
}

/// Adds live saved loadouts without changing sibling inventory or artifact policy.
pub struct LoadoutsReadService<L, I, C> {
    loadouts: L,
    inventory: I,
    clock: C,
    policy: FreshnessPolicy,
}

impl<L, I, C> LoadoutsReadService<L, I, C> {
    pub fn new(loadouts: L, inventory: I, clock: C, policy: FreshnessPolicy) -> Self {
        Self {
            loadouts,
            inventory,
            clock,
            policy,
        }
    }
}

impl<L: destiny_app::LoadoutOperations, I: InventoryReadService, C: Clock> InventoryReadService
    for LoadoutsReadService<L, I, C>
{
    fn get_subclass_options(
        &mut self,
        character: Option<&CharacterId>,
        element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        self.inventory.get_subclass_options(character, element)
    }
    fn get_loadouts(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetLoadoutsOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .loadouts
            .loadouts(character, self.policy, now)
            .map_err(McpError::from)?;
        Ok(loadouts_output(&result))
    }
    fn get_artifact(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetArtifactOutput, McpError> {
        self.inventory.get_artifact(character)
    }
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError> {
        self.inventory.get_subclass(character)
    }
    fn weekly(
        &mut self,
        args: &crate::schema::WeeklyArgs,
    ) -> Result<crate::schema::WeeklyOutput, WeeklyReadError> {
        self.inventory.weekly(args)
    }
    fn weekly_reset(&mut self) -> Result<crate::schema::WeeklyResetOutput, McpError> {
        self.inventory.weekly_reset()
    }
    fn list_characters(
        &mut self,
        request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        self.inventory.list_characters(request)
    }
    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        self.inventory.search_inventory(request)
    }
    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<InspectItemOutput, McpError> {
        self.inventory.inspect_item(request)
    }
}

/// Adds a credential-free weekly service to an independent account read seam.
pub struct WeeklyReadService<G, R, I> {
    weekly: destiny_app::weekly_reset::WeeklyResetService<G, R>,
    inventory: I,
    timestamp: fn(Timestamp) -> Option<String>,
}

impl<G, R, I> WeeklyReadService<G, R, I> {
    pub fn new(
        weekly: destiny_app::weekly_reset::WeeklyResetService<G, R>,
        inventory: I,
        timestamp: fn(Timestamp) -> Option<String>,
    ) -> Self {
        Self {
            weekly,
            inventory,
            timestamp,
        }
    }
}

impl<G, R, I> InventoryReadService for WeeklyReadService<G, R, I>
where
    G: destiny_app::PublicMilestonesGateway,
    R: destiny_app::WorldDefinitionRepository,
    I: InventoryReadService,
{
    fn get_subclass_options(
        &mut self,
        character: Option<&CharacterId>,
        element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        self.inventory.get_subclass_options(character, element)
    }
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError> {
        self.inventory.get_subclass(character)
    }

    fn weekly(
        &mut self,
        args: &crate::schema::WeeklyArgs,
    ) -> Result<crate::schema::WeeklyOutput, WeeklyReadError> {
        self.inventory.weekly(args)
    }

    fn weekly_reset(&mut self) -> Result<crate::schema::WeeklyResetOutput, McpError> {
        let reset = self.weekly.weekly_reset().map_err(McpError::from)?;
        Ok(weekly_reset_output(&reset, self.timestamp))
    }

    fn list_characters(
        &mut self,
        request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        self.inventory.list_characters(request)
    }

    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        self.inventory.search_inventory(request)
    }

    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<InspectItemOutput, McpError> {
        self.inventory.inspect_item(request)
    }
}

/// Personal weekly reads retain the existing authenticated query/refresh path.
pub struct PersonalWeeklyReadService<P, G, R, Q, S, C> {
    weekly: destiny_app::WeeklyPersonalService<P, G, R>,
    inventory: QueryReadService<Q, S, C>,
}
impl<P, G, R, Q, S, C> PersonalWeeklyReadService<P, G, R, Q, S, C> {
    pub fn new(
        weekly: destiny_app::WeeklyPersonalService<P, G, R>,
        inventory: QueryReadService<Q, S, C>,
    ) -> Self {
        Self { weekly, inventory }
    }
}
impl<P, G, R, Q, S, C> InventoryReadService for PersonalWeeklyReadService<P, G, R, Q, S, C>
where
    P: destiny_app::PublicMilestonesGateway,
    G: destiny_app::WeeklyProfileGateway,
    R: destiny_app::WorldDefinitionRepository,
    Q: ProfileGateway,
    S: InventoryRepository
        + DefinitionRepository
        + SearchRepository
        + destiny_app::StatDefinitionRepository
        + destiny_app::WorldDefinitionRepository,
    C: Clock,
{
    fn weekly(
        &mut self,
        args: &crate::schema::WeeklyArgs,
    ) -> Result<crate::schema::WeeklyOutput, WeeklyReadError> {
        let repository = self.weekly.repository();
        if repository
            .current_metadata()
            .map_err(McpError::from)?
            .is_none()
            || repository
                .world_definition_counts()
                .map_err(McpError::from)?
                .contains(&0)
        {
            return Err(McpError::new(ErrorCategory::DefinitionsUnavailable).into());
        }
        let now = self.inventory.clock.now().map_err(McpError::from)?;
        let characters = self
            .inventory
            .queries
            .list_characters(self.inventory.policy, now)
            .map_err(McpError::from)?;
        let selector = args.character.as_ref().map(CharacterId::new);
        let character = destiny_app::weekly_selection::select_weekly_character(
            &characters.characters,
            selector.as_ref(),
        )
        .map_err(WeeklyReadError::from)?;
        let snapshot = self
            .inventory
            .queries
            .repository()
            .load_current()
            .map_err(McpError::from)?
            .ok_or_else(|| McpError::new(ErrorCategory::LocalStorage))?;
        let membership = destiny_app::DestinyMembership::new(
            snapshot.metadata.membership_type,
            snapshot.metadata.membership_id,
        )
        .map_err(McpError::from)?;
        let weekly = self
            .weekly
            .weekly(&destiny_app::WeeklyCharacter {
                membership,
                character_id: character.id.clone(),
            })
            .map_err(McpError::from)?;
        weekly_output(&character, &weekly).map_err(WeeklyReadError::from)
    }
    fn get_subclass_options(
        &mut self,
        character: Option<&CharacterId>,
        element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        self.inventory.get_subclass_options(character, element)
    }
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError> {
        self.inventory.get_subclass(character)
    }

    fn list_characters(
        &mut self,
        r: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        self.inventory.list_characters(r)
    }
    fn search_inventory(
        &mut self,
        r: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        self.inventory.search_inventory(r)
    }
    fn inspect_item(&mut self, r: &InspectItemRequest) -> Result<InspectItemOutput, McpError> {
        self.inventory.inspect_item(r)
    }
}
struct WeeklyJsonAdapter;
impl destiny_app::weekly_projection::WeeklyJson for WeeklyJsonAdapter {
    type Value = serde_json::Value;
    fn object(fields: Vec<(&'static str, Self::Value)>) -> Self::Value {
        serde_json::Value::Object(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
    fn array(values: Vec<Self::Value>) -> Self::Value {
        serde_json::Value::Array(values)
    }
    fn string(value: &str) -> Self::Value {
        serde_json::json!(value)
    }
    fn integer(value: i64) -> Self::Value {
        serde_json::json!(value)
    }
    fn boolean(value: bool) -> Self::Value {
        serde_json::json!(value)
    }
    fn null() -> Self::Value {
        serde_json::Value::Null
    }
}
pub fn weekly_output(
    character: &Character,
    weekly: &destiny_app::WeeklyPersonal,
) -> Result<crate::schema::WeeklyOutput, McpError> {
    serde_json::from_value(destiny_app::weekly_projection::weekly_projection::<
        WeeklyJsonAdapter,
    >(character, weekly))
    .map_err(|_| McpError::new(ErrorCategory::ProviderValidation))
}

/// Provider text has the shared world-definition bounds; strip non-whitespace
/// controls defensively before serialization, without expanding templates.
pub fn weekly_reset_output(
    reset: &destiny_app::weekly_reset::WeeklyReset,
    timestamp: fn(Timestamp) -> Option<String>,
) -> crate::schema::WeeklyResetOutput {
    use crate::schema::{
        WeeklyActivityView, WeeklyMilestoneView, WeeklyModifierView, WeeklyResetOutput,
    };
    use destiny_app::weekly_reset::DefinitionResolution;
    fn fields(definition: &DefinitionResolution) -> (Option<String>, Option<String>, bool) {
        let clean = |text: &str, cap| {
            text.chars()
                .filter(|c| !c.is_control() || c.is_whitespace())
                .take(cap)
                .collect::<String>()
        };
        match definition {
            DefinitionResolution::Resolved { name, description } => {
                (Some(clean(name, 256)), Some(clean(description, 2048)), true)
            }
            DefinitionResolution::Unresolved => (None, None, false),
        }
    }
    WeeklyResetOutput {
        definition_version: reset.definition_version.as_str().to_owned(),
        milestones: reset
            .milestones
            .iter()
            .take(64)
            .map(|milestone| {
                let (name, description, resolved) = fields(&milestone.definition);
                WeeklyMilestoneView {
                    hash: milestone.hash,
                    order: milestone.order,
                    name,
                    description,
                    resolved,
                    start: milestone.start.and_then(timestamp),
                    end: milestone.end.and_then(timestamp),
                    activities: milestone
                        .activities
                        .iter()
                        .take(64)
                        .map(|activity| {
                            let (name, description, resolved) = fields(&activity.definition);
                            WeeklyActivityView {
                                hash: activity.hash,
                                name,
                                description,
                                resolved,
                                modifiers: activity
                                    .modifiers
                                    .iter()
                                    .take(64)
                                    .map(|modifier| {
                                        let (name, description, resolved) =
                                            fields(&modifier.definition);
                                        WeeklyModifierView {
                                            hash: modifier.hash,
                                            name,
                                            description,
                                            resolved,
                                        }
                                    })
                                    .collect(),
                            }
                        })
                        .collect(),
                }
            })
            .collect(),
    }
}

/// Composes the shared query service with an injected clock and freshness policy.
///
/// The clock supplies `now` and the policy supplies the freshness TTL, so the
/// MCP layer never reads an ambient clock and never chooses a TTL itself. Both
/// are injected by the caller that owns production composition.
pub struct QueryReadService<G, R, C> {
    queries: QueryService<G, R>,
    clock: C,
    policy: FreshnessPolicy,
}

impl<G, R, C> QueryReadService<G, R, C> {
    /// Wraps one composed query service with its clock and freshness policy.
    pub fn new(queries: QueryService<G, R>, clock: C, policy: FreshnessPolicy) -> Self {
        Self {
            queries,
            clock,
            policy,
        }
    }

    /// Borrows the composed query service.
    pub fn queries(&self) -> &QueryService<G, R> {
        &self.queries
    }

    /// Mutably borrows the composed query service for test or composition setup.
    pub fn queries_mut(&mut self) -> &mut QueryService<G, R> {
        &mut self.queries
    }
}

impl<G, R, C> InventoryReadService for QueryReadService<G, R, C>
where
    G: ProfileGateway,
    R: InventoryRepository
        + SearchRepository
        + DefinitionRepository
        + destiny_app::StatDefinitionRepository
        + destiny_app::WorldDefinitionRepository,
    C: Clock,
{
    fn get_subclass_options(
        &mut self,
        character: Option<&CharacterId>,
        element: Option<destiny_app::SubclassElement>,
    ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .subclass_options(character, element, self.policy, now)
            .map_err(McpError::from)?;
        Ok(subclass_options_output(&result))
    }
    fn get_subclass(
        &mut self,
        character: Option<&CharacterId>,
    ) -> Result<crate::schema::GetSubclassOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .character_subclasses(character, self.policy, now)
            .map_err(McpError::from)?;
        Ok(subclass_output(&result))
    }

    fn list_characters(
        &mut self,
        _request: &ListCharactersRequest,
    ) -> Result<ListCharactersOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .list_characters(self.policy, now)
            .map_err(McpError::from)?;
        Ok(list_characters_output(&result))
    }

    fn search_inventory(
        &mut self,
        request: &SearchInventoryRequest,
    ) -> Result<SearchInventoryOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let result = self
            .queries
            .search_inventory(request, self.policy, now)
            .map_err(McpError::from)?;
        Ok(search_inventory_output(&result))
    }

    fn inspect_item(
        &mut self,
        request: &InspectItemRequest,
    ) -> Result<InspectItemOutput, McpError> {
        let now = self.clock.now().map_err(McpError::from)?;
        let resolution = self
            .queries
            .inspect_item(request, self.policy, now)
            .map_err(McpError::from)?;
        inspect_item_output(&resolution)
    }
}

impl ListCharactersArgs {
    /// Validates and converts raw arguments into a `list_characters` request.
    pub fn into_request(self) -> ListCharactersRequest {
        ListCharactersRequest
    }
}

/// A validated, bounded semantic move request carrying no provider identity.
///
/// The request is the whole contribution of the MCP boundary: one validated item
/// selector and one validated destination selector. It carries no membership,
/// credential, revision, or plan, because one explicit, validated call is itself
/// the intent fact and the shared application service derives every
/// provider-bound value from the committed projection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveItemRequest {
    pub item: ItemSelector,
    pub destination: DestinationSelector,
}

/// The narrow mutation seam the MCP runtime calls.
///
/// The trait is deliberately object-safe and free of SDK types, so a server can
/// hold `Box<dyn MutationToolService>` and tests can inject any implementation
/// without touching `rmcp`. One call performs the entire one-call contract:
/// derive the provider membership internally, plan with the shared planner,
/// force the fresh preflight, issue only the ordered steps, refresh the
/// projection after the action, and verify the destination. Outputs are the
/// bounded MCP projections defined in [`crate::schema`]; a failure is a stable
/// sanitized [`McpError`].
pub trait MutationToolService {
    fn equip_item(
        &mut self,
        _request: &crate::schema::EquipItemArgs,
    ) -> Result<destiny_output::equip::EquipOutput, McpError> {
        Err(McpError::new(ErrorCategory::ConfigurationError))
    }
    /// Plans, executes, refreshes, and verifies exactly one semantic move.
    fn move_item(&mut self, request: &MoveItemRequest) -> Result<MoveItemOutput, McpError>;
}

/// Composes the shared mutation service and transfer port into one MCP call.
///
/// The service owns the composed planner/executor, the transfer port, the
/// injected clock, the freshness policy, and the plan lifetime, so the MCP layer
/// adds no freshness, planning, execution, or verification policy of its own and
/// reads no ambient clock.
///
/// [`MutationToolService::move_item`] derives the provider membership from the
/// committed projection through the application/profile boundary. A caller can
/// therefore never supply a membership or a credential, and the same derived
/// identity is what the shared planner and the executor's ticket checks compare
/// against.
pub struct ComposedMutationService<G, R, P, C, T> {
    mutations: MutationService<G, R, P>,
    transfer: T,
    clock: C,
    policy: FreshnessPolicy,
    lifetime: PlanLifetime,
}

impl<G, R, P, C, T> ComposedMutationService<G, R, P, C, T> {
    /// Wraps one composed mutation service with its ports and injected policy.
    pub fn new(
        mutations: MutationService<G, R, P>,
        transfer: T,
        clock: C,
        policy: FreshnessPolicy,
        lifetime: PlanLifetime,
    ) -> Self {
        Self {
            mutations,
            transfer,
            clock,
            policy,
            lifetime,
        }
    }

    /// Adds the exact equip port; both mutations share the same planner and lock.
    pub fn with_equip<E>(self, equip: E) -> EquipMutationService<G, R, P, C, T, E> {
        EquipMutationService { inner: self, equip }
    }

    /// Borrows the composed mutation service.
    pub fn mutations(&self) -> &MutationService<G, R, P> {
        &self.mutations
    }

    /// Mutably borrows the composed mutation service for test or composition setup.
    pub fn mutations_mut(&mut self) -> &mut MutationService<G, R, P> {
        &mut self.mutations
    }
}

impl<G, R, P, C, T> ComposedMutationService<G, R, P, C, T>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository,
    P: PlanIdGenerator,
    C: Clock,
{
    /// Resolves the effective provider membership from the committed projection.
    ///
    /// One ordinary synchronization establishes or reuses the projection through
    /// the application/profile boundary, and the effective membership comes from
    /// that projection's own metadata. The value never comes from caller input,
    /// and a caller-supplied membership is rejected by the closed argument
    /// schema before this runs.
    ///
    /// `Ok(None)` cannot follow a successful synchronization: a skipped-fresh or
    /// refreshed outcome always leaves a committed projection behind. The
    /// sanitized local-storage error covers only that unreachable invariant
    /// violation rather than guessing a membership.
    fn committed_membership(&mut self, now: Timestamp) -> Result<MembershipId, McpError> {
        self.mutations
            .sync_mut()
            .sync(ProfileSyncReason::Ordinary, self.policy, now)
            .map_err(McpError::from)?;
        let snapshot = self
            .mutations
            .repository()
            .load_current()
            .map_err(McpError::from)?
            .ok_or_else(|| McpError::new(ErrorCategory::LocalStorage))?;
        Ok(snapshot.metadata.membership_id.clone())
    }
}

impl<G, R, P, C, T> MutationToolService for ComposedMutationService<G, R, P, C, T>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository,
    P: PlanIdGenerator,
    C: Clock,
    T: TransferGateway,
{
    fn move_item(&mut self, request: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
        let preflight_now = self.clock.now().map_err(McpError::from)?;
        let membership = self.committed_membership(preflight_now)?;
        let move_request = MoveRequest {
            membership,
            item: request.item.clone(),
            destination: request.destination.clone(),
            // The single validated call is the explicit intent fact; the shared
            // planner rejects any other intent state before a side effect.
            intent: MoveIntentAuthorization::Explicit,
        };

        let plan = self
            .mutations
            .plan_move(&move_request, self.lifetime, self.policy, preflight_now)
            .map_err(mutation_failure)?;

        // The post-action observation is read after the plan, so a real clock
        // stamps it later than the preflight it observes. `plan_move` already
        // ran the ordinary freshness gate, and the executor forces the fresh
        // preflight, the ordered dispatch, the post-action refresh, and the
        // destination verification itself.
        let post_action_now = self.clock.now().map_err(McpError::from)?;
        let outcome = self
            .mutations
            .execute_move_verified(
                &mut self.transfer,
                &move_request,
                &plan,
                self.policy,
                preflight_now,
                post_action_now,
            )
            // A first-step pre-dispatch failure carries zero attempted steps, so
            // no remote mutation occurred and there is nothing to verify; the
            // sanitized category is the whole honest answer.
            .map_err(|failure| mutation_failure(failure.error()))?;

        Ok(move_item_output(&plan, &outcome))
    }
}

/// Equip composition over the same mutation service used for moves.
pub struct EquipMutationService<G, R, P, C, T, E> {
    inner: ComposedMutationService<G, R, P, C, T>,
    equip: E,
}
impl<G, R, P, C, T, E> MutationToolService for EquipMutationService<G, R, P, C, T, E>
where
    G: ProfileGateway,
    R: InventoryRepository + SearchRepository + DefinitionRepository,
    P: PlanIdGenerator,
    C: Clock,
    T: TransferGateway,
    E: destiny_app::EquipGateway,
{
    fn move_item(&mut self, request: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
        self.inner.move_item(request)
    }
    fn equip_item(
        &mut self,
        args: &crate::schema::EquipItemArgs,
    ) -> Result<destiny_output::equip::EquipOutput, McpError> {
        use destiny_output::equip::{EquipCharacter, EquipItem, EquipOutput, location_view};
        args.validate()?;
        let preflight_now = self.inner.clock.now().map_err(McpError::from)?;
        self.inner.committed_membership(preflight_now)?;
        let mutations = &mut self.inner.mutations;
        let snapshot = mutations
            .repository()
            .load_current()
            .map_err(McpError::from)?
            .ok_or_else(|| McpError::new(ErrorCategory::LocalStorage))?;
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
            [c] => *c,
            _ => return Ok(EquipOutput::refused(MutationError::AmbiguousDestination)),
        };
        let character_view = EquipCharacter {
            character_id: character.id.as_str().into(),
            label: location_view(
                &ItemLocation::Character(character.id.clone()),
                &snapshot.characters,
            )
            .label,
        };
        let instance = mutations
            .repository()
            .lookup_instance(&ItemInstanceId::new(&args.item))
            .map_err(McpError::from)?;
        let item = match instance {
            Resolution::Unique(_) | Resolution::Ambiguous(_) => {
                ItemSelector::instance(ItemInstanceId::new(&args.item))
            }
            Resolution::Missing if args.item.bytes().all(|b| b.is_ascii_digit()) => {
                ItemSelector::instance(ItemInstanceId::new(&args.item))
            }
            Resolution::Missing => ItemSelector::semantic(&args.item).map_err(McpError::from)?,
        };
        let request = destiny_app::EquipRequest {
            membership: snapshot.metadata.membership_id,
            item,
            destination: DestinationSelector::character(character.id.clone()),
            intent: MoveIntentAuthorization::Explicit,
        };
        let plan = match mutations.plan_equip(
            &request,
            self.inner.lifetime,
            self.inner.policy,
            preflight_now,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let mut output = EquipOutput::refused(error);
                output.character = Some(character_view);
                return Ok(output);
            }
        };
        let item_view = match mutations
            .repository()
            .lookup_instance(&plan.mutation_plan().item)
            .map_err(McpError::from)?
        {
            Resolution::Unique(instance) => match mutations
                .repository()
                .lookup_definition(instance.item_hash)
                .map_err(McpError::from)?
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
        let post_action_now = self.inner.clock.now().map_err(McpError::from)?;
        let mut output = match mutations.execute_equip_verified(
            (&mut self.inner.transfer, &mut self.equip),
            &request,
            &plan,
            self.inner.policy,
            preflight_now,
            post_action_now,
        ) {
            Ok(outcome) => {
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
}

impl SearchInventoryArgs {
    /// Validates and converts raw arguments into a shared search request.
    ///
    /// All adapter bounds are enforced here, before any service call. The shared
    /// query types validate the constructed request again, so the two layers can
    /// never disagree on what is acceptable.
    pub fn into_request(self) -> Result<SearchInventoryRequest, McpError> {
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(AppError::invalid_argument(
                "limit",
                format!("must be between 1 and {MAX_SEARCH_LIMIT}"),
            )
            .into());
        }
        if self.tags.len() > MAX_TAG_FILTERS {
            return Err(AppError::invalid_argument(
                "tags",
                format!("at most {MAX_TAG_FILTERS} filters are allowed"),
            )
            .into());
        }

        let definitions = DefinitionSearchQuery {
            text: optional_term(self.text.as_deref(), "text")?,
            item_type: optional_term(self.item_type.as_deref(), "item_type")?,
            tier: optional_term(self.tier.as_deref(), "tier")?,
            tags: self
                .tags
                .iter()
                .map(|value| SearchTerm::new(value).map_err(|_| McpError::from(AppError::invalid_argument("tags", "must be non-empty, at most 256 characters, and contain no prohibited control characters"))))
                .collect::<Result<Vec<_>, _>>()?,
            element: self
                .element
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| McpError::from(AppError::invalid_argument("element", "expected one of kinetic, arc, solar, void, stasis, strand")))?,
            slot: self
                .slot
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| McpError::from(AppError::invalid_argument("slot", "expected one of kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown")))?,
            class_type: self
                .class_type
                .as_deref()
                .map(str::parse)
                .transpose()
                .map_err(|_| McpError::from(AppError::invalid_argument("class_type", "expected one of titan, hunter, warlock, any, unknown")))?,
            limit: self.limit,
        };
        definitions.validate().map_err(McpError::from)?;

        let location = self
            .location
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(McpError::from)?;
        let mut instances = self
            .instances
            .map(InstanceQueryArgs::into_query)
            .transpose()?;
        let perk_name = repeated_terms(
            self.perk_name.as_deref(),
            &self.perk_names,
            "perk_name",
            "perk",
        )?;
        let mod_name =
            repeated_terms(self.mod_name.as_deref(), &self.mod_names, "mod_name", "mod")?;
        let archetype = optional_term(self.archetype.as_deref(), "archetype")?;
        let sort = self
            .sort
            .as_deref()
            .map(str::parse)
            .transpose()
            .map_err(McpError::from)?;
        let stats = self
            .stats
            .iter()
            .map(|value| value.parse::<destiny_app::StatCondition>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(McpError::from)?;
        if stats.len() > destiny_app::MAX_STAT_CONDITIONS {
            return Err(AppError::invalid_argument(
                "stats",
                format!(
                    "at most {} conditions are allowed",
                    destiny_app::MAX_STAT_CONDITIONS
                ),
            )
            .into());
        }
        if instances.is_none()
            && (self.include_available
                || !stats.is_empty()
                || sort.is_some()
                || definitions.has_filters()
                || self.min_power.is_some()
                || location.is_some()
                || self.equipped.is_some()
                || perk_name.is_some()
                || mod_name.is_some()
                || archetype.is_some())
        {
            instances = Some(
                InstanceQueryArgs {
                    limit: self.limit,
                    ..Default::default()
                }
                .into_query()?,
            );
        }
        if let Some(query) = &mut instances {
            query.definitions = definitions
                .has_filters()
                .then(|| Box::new(definitions.clone()));
            query.min_power = query.min_power.max(self.min_power);
            query.perk_name = perk_name;
            query.mod_name = mod_name;
            query.archetype = archetype;
            query.include_available = self.include_available;
            query.stats = stats;
            query.sort = sort;
            query.location = location;
            query.equipped = self.equipped;
            query.validate().map_err(McpError::from)?;
        }

        Ok(SearchInventoryRequest {
            definitions,
            instances,
        })
    }
}

impl InstanceQueryArgs {
    /// Validates and converts one instance query against the shared bounds.
    fn into_query(self) -> Result<InstanceSearchQuery, McpError> {
        if !(1..=MAX_SEARCH_LIMIT).contains(&self.limit) {
            return Err(AppError::invalid_argument(
                "instances.limit",
                format!("must be between 1 and {MAX_SEARCH_LIMIT}"),
            )
            .into());
        }
        if let Some(character_id) = &self.character_id {
            validate_identifier(character_id, "instances.character_id")?;
        }

        let item_hashes = match self.item_hashes {
            None => ItemHashFilter::Any,
            Some(hashes) => {
                if hashes.len() > MAX_HASH_FILTERS {
                    return Err(AppError::invalid_argument(
                        "instances.item_hashes",
                        format!("at most {MAX_HASH_FILTERS} hashes are allowed"),
                    )
                    .into());
                }
                ItemHashFilter::AnyOf(hashes.into_iter().map(ItemHash::new).collect())
            }
        };

        let query = InstanceSearchQuery {
            sort: None,
            include_available: false,
            item_hashes,
            perk_hash: self.perk_hash.map(PerkHash::new),
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: Vec::new(),
            character_id: self.character_id.map(CharacterId::new),
            location: None,
            equipped: None,
            definitions: None,
            min_power: self.min_power,
            limit: self.limit,
        };
        query.validate().map_err(McpError::from)?;
        Ok(query)
    }
}

impl crate::schema::GetLoadoutsArgs {
    /// Bounds the selector before invoking the shared loadout operations.
    pub fn into_request(self) -> Result<Option<CharacterId>, McpError> {
        if let Some(character) = &self.character {
            validate_identifier(character, "character")?;
        }
        Ok(self.character.map(CharacterId::new))
    }
}

/// Full CLI-compatible loadouts envelope, retaining saved slot and socket gaps.
pub fn loadouts_output(result: &destiny_app::LoadoutsResult) -> crate::schema::GetLoadoutsOutput {
    use crate::schema::{
        CharacterLoadoutsView, GetLoadoutsOutput, LoadoutItemView, LoadoutPlugView, LoadoutView,
        SubclassPlugKindView,
    };
    GetLoadoutsOutput {
        characters: result
            .characters
            .iter()
            .map(|character| CharacterLoadoutsView {
                character_id: character.character_id.as_str().to_owned(),
                class: character.class.clone(),
                loadouts: character
                    .loadouts
                    .iter()
                    .map(|loadout| LoadoutView {
                        slot: loadout.slot,
                        name: loadout.name.clone(),
                        name_hash: loadout.name_hash,
                        color_hash: loadout.color_hash,
                        icon_hash: loadout.icon_hash,
                        items: loadout
                            .items
                            .iter()
                            .map(|item| LoadoutItemView {
                                instance_id: item.instance_id.as_str().to_owned(),
                                item_hash: item.item_hash.map(|hash| hash.value()),
                                name: item.name.clone(),
                                item_type: item.item_type.clone(),
                                bucket_hash: item.bucket_hash,
                                missing: item.missing,
                                plugs: item
                                    .plugs
                                    .iter()
                                    .map(|plug| LoadoutPlugView {
                                        socket_index: plug.socket_index,
                                        plug_hash: plug.plug_hash.value(),
                                        name: plug.name.clone(),
                                        description: plug.description.clone(),
                                        empty: plug.empty,
                                        subclass_kind: plug.subclass_kind.map(|kind| match kind {
                                            destiny_app::SubclassPlugKind::Super => {
                                                SubclassPlugKindView::Super
                                            }
                                            destiny_app::SubclassPlugKind::Aspect => {
                                                SubclassPlugKindView::Aspect
                                            }
                                            destiny_app::SubclassPlugKind::Fragment => {
                                                SubclassPlugKindView::Fragment
                                            }
                                            destiny_app::SubclassPlugKind::Grenade => {
                                                SubclassPlugKindView::Grenade
                                            }
                                            destiny_app::SubclassPlugKind::Melee => {
                                                SubclassPlugKindView::Melee
                                            }
                                            destiny_app::SubclassPlugKind::ClassAbility => {
                                                SubclassPlugKindView::ClassAbility
                                            }
                                            destiny_app::SubclassPlugKind::Movement => {
                                                SubclassPlugKindView::Movement
                                            }
                                            destiny_app::SubclassPlugKind::Transcendence => {
                                                SubclassPlugKindView::Transcendence
                                            }
                                        }),
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

impl crate::schema::GetArtifactArgs {
    pub fn into_request(self) -> Result<Option<CharacterId>, McpError> {
        if let Some(character) = &self.character {
            validate_identifier(character, "character")?;
        }
        Ok(self.character.map(CharacterId::new))
    }
}

/// Full CLI-compatible artifact envelope, retaining equipped socket order.
pub fn artifact_output(result: &destiny_app::ArtifactResult) -> crate::schema::GetArtifactOutput {
    use crate::schema::{ArtifactPointsView, CharacterArtifactView, GetArtifactOutput};
    GetArtifactOutput {
        points: result.points.as_ref().map(|points| ArtifactPointsView {
            points: points.points,
            points_cap: points.points_cap,
            progress_to_next_level: points.progress_to_next_level,
            next_level_at: points.next_level_at,
        }),
        characters: result
            .characters
            .iter()
            .map(|character| CharacterArtifactView {
                character_id: character.character_id.as_str().to_owned(),
                class: character.class.clone(),
                equipped: character.equipped.as_ref().map(|equipped| {
                    crate::schema::EquippedArtifactView {
                        instance_id: equipped.instance_id.as_str().to_owned(),
                        item_hash: equipped.item_hash.value(),
                        name: equipped.name.clone(),
                        slots: equipped
                            .slots
                            .iter()
                            .map(|slot| crate::schema::ArtifactSlotView {
                                plug_hash: slot.plug_hash.value(),
                                name: slot.name.clone(),
                                description: slot.description.clone(),
                                empty: slot.empty,
                            })
                            .collect(),
                    }
                }),
            })
            .collect(),
    }
}

impl From<crate::schema::SubclassElementView> for destiny_app::SubclassElement {
    fn from(value: crate::schema::SubclassElementView) -> Self {
        use crate::schema::SubclassElementView as V;
        match value {
            V::Arc => Self::Arc,
            V::Solar => Self::Solar,
            V::Void => Self::Void,
            V::Stasis => Self::Stasis,
            V::Strand => Self::Strand,
            V::Prismatic => Self::Prismatic,
        }
    }
}

impl crate::schema::GetSubclassOptionsArgs {
    pub fn into_request(
        self,
    ) -> Result<(Option<CharacterId>, Option<destiny_app::SubclassElement>), McpError> {
        let character = crate::schema::GetSubclassArgs {
            character: self.character,
        }
        .into_request()?;
        Ok((character, self.element.map(Into::into)))
    }
}

/// Projects every option and raw flag without filtering or reordering.
pub fn subclass_options_output(
    result: &destiny_app::SubclassOptionsResult,
) -> crate::schema::GetSubclassOptionsOutput {
    use crate::schema::*;
    GetSubclassOptionsOutput {
        reference_character_id: result
            .reference_character_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        characters: result
            .characters
            .iter()
            .map(|character| CharacterSubclassOptionsView {
                character_id: character.character_id.as_str().to_owned(),
                class: character.class.clone(),
                status_reliable: character.status_reliable,
                elements: character
                    .elements
                    .iter()
                    .map(|element| ElementOptionsView {
                        element: element.element.map(|value| match value {
                            destiny_app::SubclassElement::Arc => SubclassElementView::Arc,
                            destiny_app::SubclassElement::Solar => SubclassElementView::Solar,
                            destiny_app::SubclassElement::Void => SubclassElementView::Void,
                            destiny_app::SubclassElement::Stasis => SubclassElementView::Stasis,
                            destiny_app::SubclassElement::Strand => SubclassElementView::Strand,
                            destiny_app::SubclassElement::Prismatic => {
                                SubclassElementView::Prismatic
                            }
                        }),
                        groups: element
                            .groups
                            .iter()
                            .map(|group| OptionGroupView {
                                kind: match group.kind {
                                    destiny_app::SubclassPlugKind::Super => {
                                        SubclassPlugKindView::Super
                                    }
                                    destiny_app::SubclassPlugKind::Aspect => {
                                        SubclassPlugKindView::Aspect
                                    }
                                    destiny_app::SubclassPlugKind::Fragment => {
                                        SubclassPlugKindView::Fragment
                                    }
                                    destiny_app::SubclassPlugKind::Grenade => {
                                        SubclassPlugKindView::Grenade
                                    }
                                    destiny_app::SubclassPlugKind::Melee => {
                                        SubclassPlugKindView::Melee
                                    }
                                    destiny_app::SubclassPlugKind::ClassAbility => {
                                        SubclassPlugKindView::ClassAbility
                                    }
                                    destiny_app::SubclassPlugKind::Movement => {
                                        SubclassPlugKindView::Movement
                                    }
                                    destiny_app::SubclassPlugKind::Transcendence => {
                                        SubclassPlugKindView::Transcendence
                                    }
                                },
                                options: group
                                    .options
                                    .iter()
                                    .map(|option| SubclassOptionView {
                                        plug_hash: option.plug_hash.value(),
                                        name: option.name.clone(),
                                        description: option.description.clone(),
                                        status: match option.status {
                                            destiny_app::OptionStatus::Unlocked => {
                                                OptionStatusView::Unlocked
                                            }
                                            destiny_app::OptionStatus::Locked => {
                                                OptionStatusView::Locked
                                            }
                                            destiny_app::OptionStatus::Unknown => {
                                                OptionStatusView::Unknown
                                            }
                                        },
                                        can_insert: option.can_insert,
                                        enabled: option.enabled,
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

impl crate::schema::GetSubclassArgs {
    /// Bounds the optional selector before invoking the shared query.
    pub fn into_request(self) -> Result<Option<CharacterId>, McpError> {
        if let Some(character) = &self.character {
            validate_identifier(character, "character")?;
        }
        Ok(self.character.map(CharacterId::new))
    }
}

/// Projects equipped subclasses without reordering slots or omitting empty slots.
pub fn subclass_output(result: &destiny_app::SubclassResult) -> crate::schema::GetSubclassOutput {
    use crate::schema::{
        CharacterSubclassView, GetSubclassOutput, SubclassConfigView, SubclassPlugKindView,
        SubclassSlotView,
    };
    GetSubclassOutput {
        characters: result
            .characters
            .iter()
            .map(|character| CharacterSubclassView {
                character_id: character.character_id.as_str().to_owned(),
                class: character.class.clone(),
                subclass: character
                    .subclass
                    .as_ref()
                    .map(|config| SubclassConfigView {
                        instance_id: config.instance_id.as_str().to_owned(),
                        item_hash: config.item_hash.value(),
                        name: config.name.clone(),
                        element: config.element.clone(),
                        slots: config
                            .slots
                            .iter()
                            .map(|slot| SubclassSlotView {
                                kind: match slot.kind {
                                    destiny_app::SubclassPlugKind::Super => {
                                        SubclassPlugKindView::Super
                                    }
                                    destiny_app::SubclassPlugKind::Aspect => {
                                        SubclassPlugKindView::Aspect
                                    }
                                    destiny_app::SubclassPlugKind::Fragment => {
                                        SubclassPlugKindView::Fragment
                                    }
                                    destiny_app::SubclassPlugKind::Grenade => {
                                        SubclassPlugKindView::Grenade
                                    }
                                    destiny_app::SubclassPlugKind::Melee => {
                                        SubclassPlugKindView::Melee
                                    }
                                    destiny_app::SubclassPlugKind::ClassAbility => {
                                        SubclassPlugKindView::ClassAbility
                                    }
                                    destiny_app::SubclassPlugKind::Movement => {
                                        SubclassPlugKindView::Movement
                                    }
                                    destiny_app::SubclassPlugKind::Transcendence => {
                                        SubclassPlugKindView::Transcendence
                                    }
                                },
                                plug_hash: slot.plug_hash.value(),
                                name: slot.name.clone(),
                                description: slot.description.clone(),
                                empty: slot.empty,
                            })
                            .collect(),
                    }),
            })
            .collect(),
    }
}

impl InspectItemArgs {
    /// Validates and converts raw arguments into a shared inspection request.
    ///
    /// The identifier stays an [`ItemInstanceId`]; a display name never reaches
    /// this request. An empty or over-long identifier is rejected with the
    /// sanitized [`ErrorCategory::ProviderValidation`] category before the
    /// service is invoked.
    pub fn into_request(self) -> Result<InspectItemRequest, McpError> {
        validate_identifier(&self.instance_id, "instance_id")?;
        Ok(InspectItemRequest {
            instance_id: ItemInstanceId::new(self.instance_id),
        })
    }
}

impl MoveItemArgs {
    /// Validates and converts raw arguments into one semantic move request.
    ///
    /// Exactly one item selector (an exact instance identifier or a semantic
    /// term) and exactly one destination (the Vault, an exact character
    /// identifier, or a character label) are required, and every bound is
    /// enforced here before any service call. The shared selector constructors
    /// validate the constructed values again, so the two layers cannot disagree
    /// on what is acceptable. A caller cannot supply a membership, credential,
    /// revision, or plan: those arguments do not exist, and the closed schema
    /// rejects them.
    pub fn into_request(self) -> Result<MoveItemRequest, McpError> {
        let item = match (self.instance_id, self.item_text) {
            (Some(instance_id), None) => {
                validate_identifier(&instance_id, "instance_id")?;
                ItemSelector::instance(ItemInstanceId::new(instance_id))
            }
            (None, Some(item_text)) => {
                ItemSelector::semantic(&item_text).map_err(McpError::from)?
            }
            // Neither selector, or both, is an ambiguous or absent target: no
            // destination is guessed and nothing reaches the service.
            (Some(_), Some(_)) | (None, None) => return Err(invalid_arguments()),
        };

        let destination = match (self.to_vault, self.character_id, self.character_label) {
            // The Vault is explicit, never a magic character name, and it is
            // exclusive with any character target.
            (true, None, None) => DestinationSelector::vault(),
            (false, Some(character_id), None) => {
                validate_identifier(&character_id, "character_id")?;
                DestinationSelector::character(CharacterId::new(character_id))
            }
            (false, None, Some(character_label)) => {
                DestinationSelector::character_label(&character_label).map_err(McpError::from)?
            }
            _ => return Err(invalid_arguments()),
        };

        Ok(MoveItemRequest { item, destination })
    }
}

/// Projects a shared character listing into its bounded MCP output.
pub fn list_characters_output(result: &CharacterListResult) -> ListCharactersOutput {
    ListCharactersOutput {
        characters: result.characters.iter().map(character_view).collect(),
        metadata: metadata_view(&result.metadata),
    }
}

fn repeated_terms(
    single: Option<&str>,
    plural: &[String],
    field: &'static str,
    kind: &'static str,
) -> Result<Option<Vec<SearchTerm>>, McpError> {
    if plural.len() + usize::from(single.is_some()) > destiny_app::MAX_NAME_TERMS {
        return Err(AppError::invalid_argument(
            kind,
            format!("at most {} {kind} names", destiny_app::MAX_NAME_TERMS),
        )
        .into());
    }
    let terms = single
        .into_iter()
        .chain(plural.iter().map(String::as_str))
        .map(|value| optional_term(Some(value), field).map(|term| term.expect("provided term")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((!terms.is_empty()).then_some(terms))
}

/// Projects a shared inventory search into its bounded MCP output.
pub fn search_inventory_output(result: &InventorySearchResult) -> SearchInventoryOutput {
    SearchInventoryOutput {
        resolved: crate::schema::ResolvedNamesView {
            perk: result.resolved.perk.clone(),
            mods: result.resolved.mods.clone(),
            perk_terms: result.resolved.perk_terms.clone(),
            mod_terms: result.resolved.mod_terms.clone(),
            archetype: result.resolved.archetype.clone(),
            stats: result.resolved.stats.clone(),
            sort: result.resolved.sort.as_ref().map(|(name, order)| {
                crate::schema::ResolvedSortView {
                    name: name.clone(),
                    order: order.clone(),
                }
            }),
        },
        candidates: result.candidates.iter().map(definition_view).collect(),
        hits: result
            .hits
            .iter()
            .map(|hit| InventorySearchHitView {
                available_perks: hit.available_perks.clone(),
                available_mods: hit.available_mods.clone(),
                definition: definition_view(&hit.definition),
                instance: instance_view(&hit.instance),
                perk_names: hit.perk_names.clone(),
                mods: hit.mod_names.clone(),
                archetype: hit.archetype.clone(),
                stats: hit
                    .stats
                    .iter()
                    .map(|stat| crate::schema::NamedStatView {
                        name: stat.name.clone(),
                        value: stat.value,
                    })
                    .collect(),
            })
            .collect(),
        metadata: metadata_view(&result.metadata),
    }
}

/// Projects a shared inspection resolution into its bounded MCP output.
///
/// [`Resolution::Missing`] is an explicit successful outcome. The shared service
/// already fails closed on an ambiguous or inconsistent projection before this
/// projection runs; a direct caller of the mapping receives the same sanitized
/// category instead of a partial read.
pub fn inspect_item_output(
    resolution: &Resolution<ItemInspection>,
) -> Result<InspectItemOutput, McpError> {
    match resolution {
        Resolution::Missing => Ok(InspectItemOutput::Missing),
        Resolution::Unique(inspection) => Ok(InspectItemOutput::Found(Box::new(FoundInspection {
            plug_details: inspection
                .plug_details
                .iter()
                .map(|plug| {
                    use crate::schema::InspectionPlugRoleView as View;
                    use destiny_app::InspectionPlugRole as Role;
                    crate::schema::InspectionPlugView {
                        hash: plug.hash.value(),
                        name: plug.name.clone(),
                        description: plug.description.clone(),
                        role: match plug.role {
                            Role::Intrinsic => View::Intrinsic,
                            Role::Perk => View::Perk,
                            Role::Mod => View::Mod,
                            Role::AvailablePerk => View::AvailablePerk,
                            Role::AvailableMod => View::AvailableMod,
                            Role::Archetype => View::Archetype,
                        },
                    }
                })
                .collect(),
            perk_names: inspection.perk_names.clone(),
            mod_names: inspection.mod_names.clone(),
            mods: inspection.mod_names.clone(),
            archetype: inspection.archetype.clone(),
            available_perks: inspection.available_perks.clone(),
            available_mods: inspection.available_mods.clone(),
            stats: inspection
                .stats
                .iter()
                .map(|stat| crate::schema::NamedStatView {
                    name: stat.name.clone(),
                    value: stat.value,
                })
                .collect(),
            definition: definition_view(&inspection.definition),
            instance: instance_view(&inspection.instance),
            metadata: metadata_view(&inspection.metadata),
        }))),
        Resolution::Ambiguous(_) => Err(McpError::new(ErrorCategory::LocalStorage)),
    }
}

/// Projects one planned and executed move into its bounded MCP output.
///
/// Every variant carries the sanitized planned effects, the refreshed final
/// location, and the bounded step evidence, so a client always sees what was
/// expected, what happened, and where the item actually is. A typed failure is
/// the stable sanitized [`McpError`] shape: no provider payload, credential,
/// membership identifier, revision, or plan identifier crosses this boundary.
///
/// The planned effects come from the plan the shared service built, and the
/// observed location comes from the outcome the shared service verified, so this
/// projection invents no target and no end state of its own.
pub fn move_item_output(plan: &MovePlan, outcome: &MoveExecutionOutcome) -> MoveItemOutput {
    let effects = plan.effects();
    match outcome {
        // The refreshed projection proves the item at the requested
        // destination, so the observed location is that proven destination.
        MoveExecutionOutcome::VerifiedSuccess(evidence) => {
            MoveItemOutput::VerifiedSuccess(MoveSuccessView {
                effects: effects_view(effects),
                observed: location_view(&effects.destination),
                steps: step_views(evidence),
            })
        }
        // The mandatory fresh preflight observed the item already at the
        // destination and dispatched nothing.
        MoveExecutionOutcome::VerifiedNoOp(evidence) => {
            MoveItemOutput::VerifiedNoOp(MoveSuccessView {
                effects: effects_view(effects),
                observed: location_view(&effects.destination),
                steps: step_views(evidence),
            })
        }
        MoveExecutionOutcome::AppliedPendingProviderRefresh(pending) => {
            MoveItemOutput::AppliedPendingProviderRefresh(crate::schema::PendingProviderRefreshView {
                effects: effects_view(effects),
                observed: location_view(pending.observed()),
                steps: step_views(pending.evidence()),
                reason: "the provider confirmed every step, but its profile still shows the pre-action state; observed location is stale/unconfirmed; do not retry, run `ghostlink sync` later to confirm".into(),
            })
        }
        MoveExecutionOutcome::IncompleteExecution(incomplete) => {
            MoveItemOutput::Incomplete(IncompleteMoveView {
                effects: effects_view(effects),
                observed: location_view(incomplete.observed()),
                partial: incomplete.is_partial(),
                steps: step_views(incomplete.evidence()),
                failure: incomplete.error().map(mutation_failure),
            })
        }
        MoveExecutionOutcome::VerificationFailed(failure) => {
            MoveItemOutput::VerificationFailed(VerificationFailureView {
                effects: effects_view(effects),
                observed: failure.observed().map(location_view),
                reason: verification_reason_view(failure.reason()),
                steps: step_views(failure.evidence()),
                failure: failure.error().map(mutation_failure),
            })
        }
    }
}

/// Maps one shared sanitized mutation failure onto its stable MCP category code.
fn mutation_failure(error: MutationError) -> McpError {
    McpError::new(error.category())
}

fn effects_view(effects: &MoveEffects) -> MoveEffectsView {
    MoveEffectsView {
        item_id: effects.item.as_str().to_owned(),
        via_vault: effects.via_vault,
        source: location_view(&effects.source),
        destination: location_view(&effects.destination),
    }
}

fn step_views(evidence: &ExecutionEvidence) -> Vec<StepEvidenceView> {
    evidence
        .steps()
        .iter()
        .filter_map(|record| match &record.step.action {
            // Keep the move_item wire contract flat: no action tag or new fields.
            StepAction::Transfer { from, to } => Some(StepEvidenceView {
                ordinal: record.step.ordinal,
                from: location_view(from),
                to: location_view(to),
                outcome: step_outcome_view(record.outcome),
            }),
            // Equip is not exposed by move_item and cannot execute in this task.
            StepAction::Equip { .. } => None,
        })
        .collect()
}

fn step_outcome_view(outcome: StepOutcome) -> StepOutcomeView {
    match outcome {
        StepOutcome::AuthenticationRefused => StepOutcomeView::AuthenticationRefused,
        StepOutcome::Unattempted => StepOutcomeView::Unattempted,
        StepOutcome::Completed => StepOutcomeView::Completed,
        StepOutcome::Rejected(rejection) => StepOutcomeView::Rejected {
            reason: rejection_view(rejection),
        },
        StepOutcome::EquipRejected(rejection) => StepOutcomeView::EquipRejected {
            reason: match rejection {
                destiny_app::EquipRejection::NotInSocialSpace => {
                    crate::schema::EquipRejectionView::NotInSocialSpace
                }
                destiny_app::EquipRejection::ExoticLimit => {
                    crate::schema::EquipRejectionView::ExoticLimit
                }
                destiny_app::EquipRejection::Unequippable => {
                    crate::schema::EquipRejectionView::Unequippable
                }
                destiny_app::EquipRejection::ItemNotFound => {
                    crate::schema::EquipRejectionView::ItemNotFound
                }
                destiny_app::EquipRejection::CharacterNotFound => {
                    crate::schema::EquipRejectionView::CharacterNotFound
                }
                destiny_app::EquipRejection::InvalidRequest => {
                    crate::schema::EquipRejectionView::InvalidRequest
                }
            },
        },
        StepOutcome::Transient => StepOutcomeView::Transient,
        StepOutcome::Uncertain => StepOutcomeView::Uncertain,
    }
}

fn rejection_view(rejection: TransferRejection) -> TransferRejectionView {
    match rejection {
        TransferRejection::NotTransferable => TransferRejectionView::NotTransferable,
        TransferRejection::Equipped => TransferRejectionView::Equipped,
        TransferRejection::DestinationFull => TransferRejectionView::DestinationFull,
        TransferRejection::InvalidRequest => TransferRejectionView::InvalidRequest,
    }
}

fn verification_reason_view(reason: VerificationFailureReason) -> VerificationFailureReasonView {
    match reason {
        VerificationFailureReason::RefreshFailed => VerificationFailureReasonView::RefreshFailed,
        VerificationFailureReason::Unresolved => VerificationFailureReasonView::Unresolved,
        VerificationFailureReason::Elsewhere => VerificationFailureReasonView::Elsewhere,
    }
}

#[cfg(test)]
#[test]
fn invalid_search_limits_explain_the_argument_and_bounds() {
    for limit in [0, 1001] {
        let error = SearchInventoryArgs {
            limit,
            ..Default::default()
        }
        .into_request()
        .unwrap_err();
        assert_eq!(error.code(), "provider_validation");
        assert_eq!(error.message(), "invalid limit: must be between 1 and 1000");
    }
}

#[cfg(test)]
#[test]
fn invalid_search_fields_have_value_free_details() {
    for (field, value) in [
        ("slot", "secret\n\u{7}sentinel"),
        ("stats", "Range>=secret-sentinel"),
        ("perk_name", "secret\u{7}sentinel"),
    ] {
        let arguments = if field == "stats" {
            serde_json::json!({field: [value]})
        } else {
            serde_json::json!({field: value})
        };
        let args: SearchInventoryArgs = serde_json::from_value(arguments).unwrap();
        let error = args.into_request().unwrap_err();
        assert_eq!(error.code(), "provider_validation");
        assert!(error.message().starts_with("invalid "));
        assert!(!error.message().contains("sentinel"));
        assert!(!error.message().chars().any(char::is_control));
        assert!(error.message().chars().count() <= 512);
    }
}

fn invalid_arguments() -> McpError {
    McpError::new(ErrorCategory::ProviderValidation)
}

/// Enforces the MCP-local transport bound on a shared opaque identifier.
///
/// The shared `ItemInstanceId` and `CharacterId` types accept any string, so the
/// MCP layer bounds them to `1..=MAX_IDENTIFIER_LENGTH` characters to keep every
/// request finite. This changes no core identifier type; it only rejects empty
/// or over-long caller input before any service call.
fn validate_identifier(value: &str, argument: &'static str) -> Result<(), McpError> {
    let length = value.chars().count();
    if value.trim().is_empty()
        || length > MAX_IDENTIFIER_LENGTH
        || value.chars().any(char::is_control)
    {
        return Err(AppError::invalid_argument(
            argument,
            format!(
                "must contain 1 to {MAX_IDENTIFIER_LENGTH} characters and no control characters"
            ),
        )
        .into());
    }
    Ok(())
}

fn optional_term(
    value: Option<&str>,
    argument: &'static str,
) -> Result<Option<SearchTerm>, McpError> {
    value
        .map(SearchTerm::new)
        .transpose()
        .map_err(|_| AppError::invalid_argument(argument, "must be non-empty, at most 256 characters, and contain no prohibited control characters").into())
}

fn character_view(character: &Character) -> CharacterView {
    CharacterView {
        id: character.id.as_str().to_owned(),
        name: character.name.clone(),
        class: character.class.clone(),
        race: character.race.clone(),
        gender: character.gender.clone(),
        light: character.light,
    }
}

fn definition_view(definition: &ItemDefinition) -> DefinitionView {
    DefinitionView {
        hash: definition.hash.value(),
        bucket_hash: definition.bucket_hash.map(|hash| hash.value()),
        name: definition.name.clone(),
        item_type: definition.item_type.clone(),
        tier: definition.tier.clone(),
        element: definition.element.clone(),
        slot: definition.slot.map(|v| v.as_str().to_owned()),
        class_type: definition.class_type.map(|v| v.as_str().to_owned()),
        tags: definition.tags.clone(),
        definition_version: definition.version.as_str().to_owned(),
    }
}

fn instance_view(instance: &ItemInstance) -> InstanceView {
    InstanceView {
        instance_id: instance.instance_id.as_str().to_owned(),
        item_hash: instance.item_hash.value(),
        location: location_view(&instance.location),
        quantity: instance.quantity,
        equipped: instance.equipped,
        locked: instance.locked,
        power: instance.power,
        perks: instance
            .perks
            .iter()
            .map(|perk| PerkView {
                perk_hash: perk.perk_hash.value(),
                active: perk.active,
                visible: perk.visible,
            })
            .collect(),
    }
}

fn location_view(location: &ItemLocation) -> LocationView {
    match location {
        ItemLocation::Vault => LocationView::Vault,
        ItemLocation::Character(character_id) => LocationView::Character {
            character_id: character_id.as_str().to_owned(),
        },
        // The opaque provider value is deliberately dropped so no provider text
        // crosses the MCP boundary.
        ItemLocation::Other(_) => LocationView::Other,
    }
}

fn metadata_view(metadata: &QueryMetadata) -> QueryMetadataView {
    QueryMetadataView {
        dataset: dataset_code(metadata.sync.dataset).to_owned(),
        source: source_code(metadata.sync.source).to_owned(),
        revision: metadata.sync.revision.value(),
        checked_at: metadata.sync.checked_at.seconds(),
        definition_version: metadata.sync.definition_version.as_str().to_owned(),
        freshness: freshness_code(metadata.freshness).to_owned(),
    }
}

fn dataset_code(dataset: Dataset) -> &'static str {
    match dataset {
        Dataset::ManifestDefinitions => "manifest_definitions",
        Dataset::ProfileInventory => "profile_inventory",
    }
}

fn source_code(source: SourceMarker) -> &'static str {
    match source {
        SourceMarker::Manifest => "manifest",
        SourceMarker::Profile => "profile",
    }
}

fn freshness_code(freshness: FreshnessStatus) -> &'static str {
    match freshness {
        FreshnessStatus::Missing => "missing",
        FreshnessStatus::Fresh => "fresh",
        FreshnessStatus::Stale => "stale",
        FreshnessStatus::InvalidSource => "invalid_source",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ComposedMutationService, InventoryReadService, ListCharactersRequest, MoveItemRequest,
        MutationToolService, QueryReadService, inspect_item_output, list_characters_output,
        search_inventory_output,
    };
    use crate::schema::{
        InspectItemArgs, InspectItemOutput, InstanceQueryArgs, LocationView, MAX_IDENTIFIER_LENGTH,
        MoveEffectsView, MoveItemArgs, MoveItemOutput, SearchInventoryArgs, StepOutcomeView,
        TransferRejectionView, VerificationFailureReasonView,
    };
    use destiny_app::test_support::{
        DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
        InMemorySearchRepository, QueuedProfileGateway, QueuedTransferGateway,
    };
    use destiny_app::{
        AppError, Character, CharacterId, DefinitionReplacementInput, DefinitionRepository,
        DefinitionSearchQuery, DefinitionVersion, DestinyMembership, ErrorCategory,
        FreshnessPolicy, InstanceSearchQuery, InventoryRepository, InventorySnapshot,
        InventorySnapshotInput, ItemDefinition, ItemHash, ItemInstance, ItemInstanceId,
        ItemLocation, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH, MAX_TAG_FILTERS,
        MembershipId, MembershipType, MutationService, PerkHash, PerkSummary, PlanLifetime,
        ProfileGateway, ProfileSyncService, QueryService, Resolution, SearchRepository,
        SourceMarker, SyncMetadata, Timestamp, TransferGateway, TransferOutcome, TransferRejection,
        TransferRequest,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    const NOW: Timestamp = Timestamp::new(10_000);
    const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };
    const FRESH_CHECKED_AT: Timestamp = Timestamp::new(9_900);
    const STALE_CHECKED_AT: Timestamp = Timestamp::new(9_000);
    const DEFINITION_VERSION: &str = "definitions-v1";
    const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

    /// A narrow test-local composite exposing one repository for all three seams.
    #[derive(Clone, Debug, Default)]
    struct CompositeRepository {
        inventory: InMemoryInventoryRepository,
        search: InMemorySearchRepository,
        definitions: InMemoryDefinitionRepository,
        calls: Rc<RefCell<Vec<&'static str>>>,
    }

    impl CompositeRepository {
        fn with_definitions(definitions: Vec<ItemDefinition>) -> Self {
            let version = DefinitionVersion::new(DEFINITION_VERSION);
            Self {
                inventory: InMemoryInventoryRepository::new(),
                search: InMemorySearchRepository::with_definitions(definitions.clone()),
                definitions: InMemoryDefinitionRepository::new(version, definitions),
                calls: Rc::new(RefCell::new(Vec::new())),
            }
        }

        fn record(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        /// Commits one valid projection and mirrors its instances into search.
        ///
        /// This uses the inner repository directly, so seeding records no
        /// read-seam call.
        fn commit(&mut self, input: InventorySnapshotInput) {
            let snapshot = self
                .inventory
                .replace_current(input)
                .expect("valid seed snapshot");
            self.search.set_instances(snapshot.items);
        }
    }

    impl InventoryRepository for CompositeRepository {
        fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
            self.record("load_current");
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
        fn name_catalog(
            &self,
            kind: &str,
        ) -> Result<destiny_app::name_matching::NameCatalog, AppError> {
            if kind == "stat" {
                Ok(self.definitions.stat_name_catalog())
            } else {
                self.search.name_catalog(kind)
            }
        }
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

    impl destiny_app::WorldDefinitionRepository for CompositeRepository {
        fn lookup_item_description(&self, hash: ItemHash) -> Result<Option<String>, AppError> {
            self.definitions.lookup_item_description(hash)
        }
        fn lookup_milestone(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::MilestoneDefinition>, AppError> {
            self.definitions.lookup_milestone(hash)
        }
        fn lookup_activity(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityDefinition>, AppError> {
            self.definitions.lookup_activity(hash)
        }
        fn lookup_activity_modifier(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityModifierDefinition>, AppError> {
            self.definitions.lookup_activity_modifier(hash)
        }
        fn lookup_objective(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ObjectiveDefinition>, AppError> {
            self.definitions.lookup_objective(hash)
        }
        fn lookup_activity_type(
            &self,
            hash: u32,
        ) -> Result<Option<destiny_app::ActivityTypeDefinition>, AppError> {
            self.definitions.lookup_activity_type(hash)
        }
        fn lookup_loadout_name(&self, hash: u32) -> Result<Option<String>, AppError> {
            self.definitions.lookup_loadout_name(hash)
        }
        fn world_definition_counts(&self) -> Result<[usize; 5], AppError> {
            self.definitions.world_definition_counts()
        }
        fn publish_manifest(
            &mut self,
            items: destiny_app::DefinitionReplacementInput,
            world: destiny_app::WorldDefinitionsAcquisition,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.publish_manifest(items, world)
        }
    }

    impl destiny_app::StatDefinitionRepository for CompositeRepository {
        fn lookup_stat(&self, hash: u32) -> Result<Option<destiny_app::StatDefinition>, AppError> {
            self.definitions.lookup_stat(hash)
        }
        fn stat_hashes_by_name(&self, name: &str) -> Result<Vec<u32>, AppError> {
            self.definitions.stat_hashes_by_name(name)
        }
        fn stat_definition_count(&self) -> Result<usize, AppError> {
            self.definitions.stat_definition_count()
        }
    }

    impl DefinitionRepository for CompositeRepository {
        fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
            self.definitions.current_definition_version()
        }

        fn lookup_definition(
            &self,
            hash: ItemHash,
        ) -> Result<Resolution<ItemDefinition>, AppError> {
            self.definitions.lookup_definition(hash)
        }

        fn current_metadata(&self) -> Result<Option<SyncMetadata>, AppError> {
            self.definitions.current_metadata()
        }

        fn replace_definitions(
            &mut self,
            replacement: DefinitionReplacementInput,
        ) -> Result<SyncMetadata, AppError> {
            self.definitions.replace_definitions(replacement)
        }
    }

    /// A read-only spy over the fake gateway the sync service owns by value.
    struct SharedGateway(Rc<RefCell<QueuedProfileGateway>>);

    impl ProfileGateway for SharedGateway {
        fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
            self.0.borrow_mut().resolve_membership()
        }

        fn load_inventory(
            &mut self,
            membership: &DestinyMembership,
        ) -> Result<InventorySnapshotInput, AppError> {
            self.0.borrow_mut().load_inventory(membership)
        }
    }

    type AppService = QueryService<SharedGateway, CompositeRepository>;
    type McpService = QueryReadService<SharedGateway, CompositeRepository, FakeClock>;

    fn gateway() -> Rc<RefCell<QueuedProfileGateway>> {
        Rc::new(RefCell::new(QueuedProfileGateway::new()))
    }

    fn app_service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        repository: CompositeRepository,
    ) -> AppService {
        QueryService::new(ProfileSyncService::new(
            SharedGateway(Rc::clone(shared)),
            repository,
        ))
    }

    fn mcp_service(queries: AppService) -> McpService {
        QueryReadService::new(queries, FakeClock::fixed(NOW), TTL)
    }

    fn membership() -> DestinyMembership {
        DestinyMembership::new(
            MembershipType::new(3),
            MembershipId::new(MEMBERSHIP_IDENTIFIER),
        )
        .expect("valid membership")
    }

    fn character(id: &str) -> Character {
        Character {
            id: CharacterId::new(id),
            date_last_played: None,
            name: Some(format!("name-{id}")),
            class: None,
            race: None,
            gender: None,
            light: None,
        }
    }

    fn definition(
        hash: i64,
        name: &str,
        item_type: Option<&str>,
        tags: Vec<&str>,
    ) -> ItemDefinition {
        ItemDefinition {
            hash: ItemHash::new(hash),
            bucket_hash: None,
            name: name.into(),
            item_type: item_type.map(str::to_string),
            tier: None,
            element: None,
            slot: None,
            class_type: None,
            plug_category: None,
            tags: tags.iter().map(|tag| (*tag).to_string()).collect(),
            version: DefinitionVersion::new(DEFINITION_VERSION),
        }
    }

    fn standard_definitions() -> Vec<ItemDefinition> {
        vec![
            definition(
                1,
                "Gjallarhorn",
                Some("Rocket Launcher"),
                vec!["exotic", "solar"],
            ),
            definition(
                2,
                "Fatebringer",
                Some("Hand Cannon"),
                vec!["legendary", "kinetic"],
            ),
            definition(
                3,
                "Vex Mythoclast",
                Some("Fusion Rifle"),
                vec!["exotic", "solar"],
            ),
        ]
    }

    fn perk(hash: u32) -> PerkSummary {
        PerkSummary {
            perk_hash: PerkHash::new(hash),
            active: true,
            visible: true,
        }
    }

    fn instance(
        instance_id: &str,
        item_hash: i64,
        location: ItemLocation,
        perks: Vec<PerkSummary>,
    ) -> ItemInstance {
        ItemInstance {
            available_plugs: Vec::new(),
            instance_id: ItemInstanceId::new(instance_id),
            item_hash: ItemHash::new(item_hash),
            location,
            quantity: 1,
            equipped: false,
            locked: false,
            power: None,
            stats: Vec::new(),
            plugs: Vec::new(),
            perks,
        }
    }

    fn snapshot_input(
        checked_at: Timestamp,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> InventorySnapshotInput {
        let membership = membership();
        InventorySnapshotInput {
            plug_set_entries: Vec::new(),
            membership_id: membership.membership_id().clone(),
            membership_type: membership.membership_type(),
            definition_version: DefinitionVersion::new(DEFINITION_VERSION),
            checked_at,
            source: SourceMarker::Profile,
            characters,
            items,
        }
    }

    fn fresh_components(
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) -> (Rc<RefCell<QueuedProfileGateway>>, AppService) {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, characters, items));
        (Rc::clone(&shared), app_service(&shared, repository))
    }

    #[test]
    fn search_attribute_filters_match_cli_contract() {
        let args: SearchInventoryArgs = serde_json::from_value(serde_json::json!({
            "element": "SoLaR", "slot": "Energy", "class_type": "Hunter", "min_power": 2000
        }))
        .expect("new arguments");
        let request = args.into_request().expect("valid filters");
        assert_eq!(
            request.definitions.element.map(|v| v.as_str()),
            Some("solar")
        );
        assert_eq!(request.definitions.slot.map(|v| v.as_str()), Some("energy"));
        assert_eq!(
            request.definitions.class_type.map(|v| v.as_str()),
            Some("hunter")
        );
        let instances = request.instances.expect("owned query");
        assert_eq!(instances.min_power, Some(2000));
        assert_eq!(instances.definitions.as_deref(), Some(&request.definitions));
        for field in ["element", "slot", "class_type"] {
            let args: SearchInventoryArgs =
                serde_json::from_value(serde_json::json!({field: "invalid"})).unwrap();
            assert!(args.into_request().is_err());
        }
        let args: SearchInventoryArgs =
            serde_json::from_value(serde_json::json!({"instances": {"min_power": 2001}})).unwrap();
        assert_eq!(
            args.into_request().unwrap().instances.unwrap().min_power,
            Some(2001)
        );
        for (outer, inner, expected) in [(2000, 2001, 2001), (2002, 2001, 2002)] {
            let args: SearchInventoryArgs = serde_json::from_value(
                serde_json::json!({"min_power": outer, "instances": {"min_power": inner}}),
            )
            .unwrap();
            assert_eq!(
                args.into_request().unwrap().instances.unwrap().min_power,
                Some(expected)
            );
        }
        let mut item = definition(1, "Solar item", None, Vec::new());
        item.element = Some("solar".into());
        item.slot = Some("energy".parse().unwrap());
        item.class_type = Some("any".parse().unwrap());
        let view = super::definition_view(&item);
        assert_eq!(view.element.as_deref(), Some("solar"));
        assert_eq!(view.slot.as_deref(), Some("energy"));
        assert_eq!(view.class_type.as_deref(), Some("any"));
    }

    fn search_args(
        text: Option<&str>,
        tags: Vec<&str>,
        item_hashes: Option<Vec<i64>>,
        limit: usize,
    ) -> SearchInventoryArgs {
        SearchInventoryArgs {
            tier: None,
            location: None,
            equipped: None,
            include_available: false,
            sort: None,
            text: text.map(str::to_owned),
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            stats: Vec::new(),
            perk_name: None,
            mod_name: None,
            archetype: None,
            item_type: None,
            tags: tags.into_iter().map(str::to_owned).collect(),
            element: None,
            slot: None,
            class_type: None,
            min_power: None,
            limit,
            instances: item_hashes.map(|hashes| InstanceQueryArgs {
                item_hashes: Some(hashes),
                perk_hash: None,
                character_id: None,
                min_power: None,
                limit,
            }),
        }
    }

    fn instance_search_args(character_id: Option<&str>) -> SearchInventoryArgs {
        SearchInventoryArgs {
            sort: None,
            tier: None,
            location: None,
            equipped: None,
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            include_available: false,
            stats: Vec::new(),
            perk_name: None,
            mod_name: None,
            archetype: None,
            element: None,
            slot: None,
            class_type: None,
            min_power: None,
            text: None,
            item_type: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: character_id.map(str::to_owned),
                min_power: None,
                limit: MAX_SEARCH_LIMIT,
            }),
        }
    }

    #[test]
    fn weekly_missing_definitions_returns_hint_without_gateway_or_account_reads() {
        struct Gateway;
        impl destiny_app::PublicMilestonesGateway for Gateway {
            fn load_public_milestones(
                &mut self,
            ) -> Result<destiny_app::PublicMilestonesSnapshot, AppError> {
                panic!("definitions gate must prevent milestones request")
            }
        }
        let (shared, queries) = fresh_components(Vec::new(), Vec::new());
        let calls = Rc::clone(&queries.repository().calls);
        let mut service = super::WeeklyReadService::new(
            destiny_app::weekly_reset::WeeklyResetService::new(
                Gateway,
                InMemoryDefinitionRepository::default(),
            ),
            mcp_service(queries),
            |_| None,
        );
        let error = service.weekly_reset().unwrap_err();
        assert_eq!(error.code(), "definitions_unavailable");
        assert_eq!(
            error.message(),
            "definitions not loaded; run ghostlink sync-manifest"
        );
        assert!(calls.borrow().is_empty());
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn weekly_output_bounds_and_sanitizes_provider_text() {
        use destiny_app::weekly_reset::{DefinitionResolution, WeeklyMilestone, WeeklyReset};
        let reset = WeeklyReset {
            definition_version: DefinitionVersion::new("v1"),
            milestones: vec![WeeklyMilestone {
                hash: 1,
                order: 0,
                definition: DefinitionResolution::Resolved {
                    name: format!("\u{1b}{}", "n".repeat(300)),
                    description: format!("\u{0}{}", "d".repeat(3000)),
                },
                start: None,
                end: None,
                activities: vec![],
            }],
        };
        let result = super::weekly_reset_output(&reset, |_| None).into_call_tool_result();
        let value = result.structured_content.unwrap();
        assert_eq!(value["kind"], "weekly_reset");
        assert_eq!(value["milestones"][0]["name"], "n".repeat(256));
        assert_eq!(value["milestones"][0]["description"], "d".repeat(2048));
        assert_eq!(value["milestones"][0]["start"], serde_json::Value::Null);
    }

    #[test]
    fn definition_bounds_are_rejected_before_any_service_call() {
        for invalid_limit in [0, MAX_SEARCH_LIMIT + 1] {
            let error = search_args(None, Vec::new(), None, invalid_limit)
                .into_request()
                .expect_err("invalid limit must be rejected");
            assert_eq!(error.code(), "provider_validation");
        }

        let too_many_tags = vec!["tag"; MAX_TAG_FILTERS + 1];
        let error = search_args(None, too_many_tags, None, MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("too many tags must be rejected");
        assert_eq!(error.code(), "provider_validation");

        let overlong = "a".repeat(MAX_SEARCH_TEXT_LENGTH + 1);
        let error = search_args(Some(&overlong), Vec::new(), None, MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("overlong text must be rejected");
        assert_eq!(error.code(), "provider_validation");
        assert!(!error.message().contains(&overlong));
    }

    #[test]
    fn instance_candidate_bounds_are_rejected_before_any_service_call() {
        let over_large: Vec<i64> = (0..=MAX_HASH_FILTERS as i64).collect();
        let error = search_args(None, Vec::new(), Some(over_large), MAX_SEARCH_LIMIT)
            .into_request()
            .expect_err("over-large hash candidate set must be rejected");
        assert_eq!(error.code(), "provider_validation");

        // An empty candidate set is valid and keeps its fail-closed meaning.
        let request = search_args(None, Vec::new(), Some(Vec::new()), MAX_SEARCH_LIMIT)
            .into_request()
            .expect("empty candidate set is valid");
        assert!(request.instances.is_some());
    }

    #[test]
    fn invalid_identifiers_are_rejected_before_any_service_call() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, Vec::new(), Vec::new()));
        let calls = Rc::clone(&repository.calls);
        let _mcp = mcp_service(app_service(&shared, repository));

        let empty_instance = InspectItemArgs {
            instance_id: String::new(),
        }
        .into_request()
        .expect_err("empty instance identifier must be rejected");
        assert_eq!(empty_instance.code(), "provider_validation");

        let overlong_instance = "i".repeat(MAX_IDENTIFIER_LENGTH + 1);
        let error = InspectItemArgs {
            instance_id: overlong_instance,
        }
        .into_request()
        .expect_err("over-long instance identifier must be rejected");
        assert_eq!(error.code(), "provider_validation");

        let empty_character = instance_search_args(Some(""))
            .into_request()
            .expect_err("empty character identifier must be rejected");
        assert_eq!(empty_character.code(), "provider_validation");

        let overlong_character = "c".repeat(MAX_IDENTIFIER_LENGTH + 1);
        let error = instance_search_args(Some(&overlong_character))
            .into_request()
            .expect_err("over-long character identifier must be rejected");
        assert_eq!(error.code(), "provider_validation");
        assert!(!error.message().contains(&overlong_character));

        // The rejected arguments never reached the composed service: no
        // repository read and no gateway call happened.
        assert!(calls.borrow().is_empty());
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn identifier_boundary_is_inclusive() {
        let boundary = "i".repeat(MAX_IDENTIFIER_LENGTH);
        let inspect = InspectItemArgs {
            instance_id: boundary.clone(),
        }
        .into_request()
        .expect("boundary identifier is valid");
        assert_eq!(inspect.instance_id.as_str(), boundary);

        let search = instance_search_args(Some(&boundary))
            .into_request()
            .expect("boundary character identifier is valid");
        assert!(search.instances.is_some());
    }

    #[test]
    fn subclass_read_delegates_selection_and_preserves_shared_failures() {
        let mut hunter = character("hunter-id");
        hunter.class = Some("Hunter".into());
        let (shared, app) = fresh_components(vec![hunter, character("warlock")], Vec::new());
        let mut mcp = mcp_service(app);
        let options = mcp.get_subclass_options(None, None).unwrap();
        assert_eq!(options.characters.len(), 2);
        let selected_options = mcp
            .get_subclass_options(
                Some(&CharacterId::new("hUnTeR")),
                Some(destiny_app::SubclassElement::Arc),
            )
            .unwrap();
        assert_eq!(
            selected_options.characters,
            vec![options.characters[0].clone()]
        );
        assert_eq!(
            mcp.get_subclass_options(Some(&CharacterId::new("missing")), None)
                .unwrap_err()
                .code(),
            "provider_validation"
        );
        let all = mcp.get_subclass(None).unwrap();
        assert_eq!(all.characters.len(), 2);
        assert!(
            all.characters
                .iter()
                .all(|character| character.subclass.is_none())
        );
        let selected = mcp.get_subclass(Some(&CharacterId::new("hUnTeR"))).unwrap();
        assert_eq!(selected.characters, vec![all.characters[0].clone()]);
        assert_eq!(
            mcp.get_subclass(Some(&CharacterId::new("missing")))
                .unwrap_err()
                .code(),
            "provider_validation"
        );
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn list_characters_parity_with_the_shared_application_result() {
        let characters = vec![character("warlock"), character("hunter")];
        let (shared, mut app) = fresh_components(characters.clone(), Vec::new());
        let app_result = app.list_characters(TTL, NOW).expect("application read");

        let (_shared, app_for_mcp) = fresh_components(characters, Vec::new());
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp
            .list_characters(&ListCharactersRequest)
            .expect("mcp read");

        assert_eq!(output, list_characters_output(&app_result));
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert_eq!(output.characters.len(), 2);
        assert_eq!(output.characters[0].id, "hunter");
        assert_eq!(output.characters[1].id, "warlock");
        assert_eq!(output.metadata.dataset, "profile_inventory");
        assert_eq!(output.metadata.source, "profile");
        assert_eq!(output.metadata.freshness, "fresh");
        assert_eq!(output.metadata.revision, 1);
    }

    #[test]
    fn search_inventory_parity_with_the_shared_application_result() {
        let items = vec![
            instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)]),
            instance("inst-b", 2, ItemLocation::Vault, vec![perk(20)]),
        ];
        let args = SearchInventoryArgs {
            tier: None,
            location: None,
            equipped: None,
            include_available: false,
            sort: None,
            text: Some("gjallar".into()),
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            stats: Vec::new(),
            perk_name: None,
            mod_name: None,
            archetype: None,
            item_type: None,
            element: None,
            slot: None,
            class_type: None,
            min_power: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: Some(vec![1, 2]),
                perk_hash: None,
                character_id: None,
                min_power: None,
                limit: MAX_SEARCH_LIMIT,
            }),
        };

        let (_shared, mut app) = fresh_components(Vec::new(), items.clone());
        let app_result = app
            .search_inventory(
                &args.clone().into_request().expect("valid arguments"),
                TTL,
                NOW,
            )
            .expect("application search");

        let (_shared, app_for_mcp) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp
            .search_inventory(&args.into_request().expect("valid arguments"))
            .expect("mcp search");

        assert_eq!(output, search_inventory_output(&app_result));
        assert_eq!(
            output
                .candidates
                .iter()
                .map(|definition| definition.hash)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            output
                .hits
                .iter()
                .map(|hit| hit.instance.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["inst-a"]
        );
        // Every instance hit keeps its exact definition identity distinct.
        for hit in &output.hits {
            assert_eq!(hit.definition.hash, hit.instance.item_hash);
        }
    }

    #[test]
    fn search_output_projects_mod_names_in_socket_order() {
        let (_shared, mut app) = fresh_components(
            Vec::new(),
            vec![instance("inst-a", 1, ItemLocation::Vault, vec![])],
        );
        let mut result = app
            .search_inventory(
                &instance_search_args(None).into_request().unwrap(),
                TTL,
                NOW,
            )
            .unwrap();
        assert_eq!(result.hits.len(), 1);
        result.hits[0].perk_names.clear();
        result.hits[0].mod_names = vec!["Harmonic Siphon".into(), "Ashes to Assets".into()];
        result.hits[0].archetype = Some("Specialist".into());
        let value = serde_json::to_value(search_inventory_output(&result)).unwrap();
        assert_eq!(
            value["hits"][0]["mods"],
            serde_json::json!(["Harmonic Siphon", "Ashes to Assets"])
        );
        assert_eq!(value["hits"][0]["archetype"], "Specialist");
        assert_eq!(value["hits"][0]["perk_names"], serde_json::json!([]));
    }

    #[test]
    fn inspect_item_parity_keeps_definition_and_instance_distinct() {
        let items = vec![instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)])];
        let request = InspectItemArgs {
            instance_id: "inst-a".into(),
        }
        .into_request()
        .expect("valid identifier");

        let (_shared, mut app) = fresh_components(Vec::new(), items.clone());
        let resolution = app
            .inspect_item(&request, TTL, NOW)
            .expect("application inspection");
        let expected = inspect_item_output(&resolution).expect("mapped inspection");

        let (_shared, app_for_mcp) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app_for_mcp);
        let output = mcp.inspect_item(&request).expect("mcp inspection");

        assert_eq!(output, expected);
        let value = serde_json::to_value(&output).unwrap();
        for field in [
            "perk_names",
            "mod_names",
            "mods",
            "available_perks",
            "available_mods",
            "stats",
        ] {
            assert_eq!(value[field], serde_json::json!([]), "{field}");
        }
        assert_eq!(value["archetype"], serde_json::Value::Null);
        match output {
            InspectItemOutput::Found(found) => {
                assert_eq!(found.definition.hash, 1);
                assert_eq!(found.instance.instance_id, "inst-a");
                assert_eq!(found.instance.item_hash, 1);
                assert_eq!(found.metadata.freshness, "fresh");
            }
            InspectItemOutput::Missing => panic!("expected a found inspection"),
        }
    }

    #[test]
    fn inspect_item_reports_missing_as_an_explicit_successful_outcome() {
        let (_shared, app) = fresh_components(Vec::new(), Vec::new());
        let mut mcp = mcp_service(app);

        let output = mcp
            .inspect_item(
                &InspectItemArgs {
                    instance_id: "absent".into(),
                }
                .into_request()
                .expect("valid identifier"),
            )
            .expect("missing is not an error");

        assert_eq!(output, InspectItemOutput::Missing);
    }

    #[test]
    fn ambiguous_instance_fails_closed_with_a_sanitized_error() {
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(FRESH_CHECKED_AT, Vec::new(), Vec::new()));
        let mut queries = app_service(&shared, repository);
        let duplicate = instance("dup", 1, ItemLocation::Vault, Vec::new());
        queries
            .repository_mut()
            .search
            .set_instances(vec![duplicate.clone(), duplicate]);
        let mut mcp = mcp_service(queries);

        let error = mcp
            .inspect_item(
                &InspectItemArgs {
                    instance_id: "dup".into(),
                }
                .into_request()
                .expect("valid identifier"),
            )
            .expect_err("ambiguous instance must fail closed");

        assert_eq!(error.code(), "local_storage");
        assert_eq!(error.message(), "local storage failed");
    }

    #[test]
    fn provider_failures_surface_only_sanitized_categories() {
        const PAYLOAD: &str = "raw-provider-payload-sentinel";
        let shared = gateway();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            STALE_CHECKED_AT,
            vec![character("warlock")],
            vec![instance(PAYLOAD, 1, ItemLocation::Vault, Vec::new())],
        ));
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderValidation)));
        let mut mcp = mcp_service(app_service(&shared, repository));

        let error = mcp
            .list_characters(&ListCharactersRequest)
            .expect_err("stale refresh failure must fail the read");

        assert_eq!(error.code(), "provider_validation");
        assert_eq!(
            error.message(),
            AppError::new(ErrorCategory::ProviderValidation).to_string()
        );

        let rendered_error = error.to_string();
        let rendered_result =
            serde_json::to_string(&error.into_call_tool_result()).expect("error result serializes");
        for rendered in [rendered_error, rendered_result] {
            assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
            assert!(
                !rendered.contains(MEMBERSHIP_IDENTIFIER),
                "leaked membership identifier: {rendered}"
            );
        }
    }

    #[test]
    fn mcp_values_never_carry_raw_provider_material() {
        const PAYLOAD: &str = "raw-provider-payload-sentinel";
        let items = vec![instance(
            "inst-a",
            1,
            ItemLocation::other(PAYLOAD),
            Vec::new(),
        )];
        let (_shared, app) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app);

        let args = SearchInventoryArgs {
            text: None,
            item_type: None,
            element: None,
            slot: None,
            class_type: None,
            min_power: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: None,
                min_power: None,
                limit: MAX_SEARCH_LIMIT,
            }),
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: Vec::new(),
            include_available: false,
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            tier: None,
            location: None,
            equipped: None,
            sort: None,
        };
        let output = mcp
            .search_inventory(&args.into_request().expect("valid arguments"))
            .expect("search succeeds");

        assert_eq!(output.hits.len(), 1);
        assert_eq!(output.hits[0].instance.location, LocationView::Other);

        let rendered = serde_json::to_string(&output).expect("output serializes");
        assert!(!rendered.contains(PAYLOAD), "leaked payload: {rendered}");
        assert!(
            !rendered.contains(MEMBERSHIP_IDENTIFIER),
            "leaked membership identifier: {rendered}"
        );
    }

    #[test]
    fn translation_is_deterministic_across_repeated_reads() {
        let items = vec![
            instance("inst-b", 2, ItemLocation::Vault, Vec::new()),
            instance("inst-a", 1, ItemLocation::Vault, Vec::new()),
        ];
        let (_shared, app) = fresh_components(Vec::new(), items);
        let mut mcp = mcp_service(app);

        let args = SearchInventoryArgs {
            text: None,
            item_type: None,
            element: None,
            slot: None,
            class_type: None,
            min_power: None,
            tags: Vec::new(),
            limit: MAX_SEARCH_LIMIT,
            instances: Some(InstanceQueryArgs {
                item_hashes: None,
                perk_hash: None,
                character_id: None,
                min_power: None,
                limit: MAX_SEARCH_LIMIT,
            }),
            perk_name: None,
            mod_name: None,
            archetype: None,
            stats: Vec::new(),
            include_available: false,
            perk_names: Vec::new(),
            mod_names: Vec::new(),
            tier: None,
            location: None,
            equipped: None,
            sort: None,
        };
        let request = args.into_request().expect("valid arguments");

        let first = mcp.search_inventory(&request).expect("first read");
        let second = mcp.search_inventory(&request).expect("second read");

        assert_eq!(first, second);
        assert_eq!(
            first
                .hits
                .iter()
                .map(|hit| hit.instance.instance_id.as_str())
                .collect::<Vec<_>>(),
            vec!["inst-a", "inst-b"]
        );
    }

    #[test]
    fn the_read_service_seam_is_object_safe_and_injectable() {
        let (_shared, app) = fresh_components(vec![character("warlock")], Vec::new());
        let mut service: Box<dyn InventoryReadService> = Box::new(mcp_service(app));

        let output = service
            .list_characters(&ListCharactersRequest)
            .expect("injected read succeeds");

        assert_eq!(output.characters.len(), 1);
        assert_eq!(output.characters[0].id, "warlock");
    }

    // ---- one-call intent-authorized mutation (DMM-002) ------------------

    /// A spy over the fake transfer port the composed service owns by value.
    struct SharedTransfer(Rc<RefCell<QueuedTransferGateway>>);

    impl TransferGateway for SharedTransfer {
        fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError> {
            self.0.borrow_mut().transfer(request)
        }
    }

    type MutationServiceUnderTest =
        MutationService<SharedGateway, CompositeRepository, DeterministicPlanIds>;
    type ComposedService = ComposedMutationService<
        SharedGateway,
        CompositeRepository,
        DeterministicPlanIds,
        FakeClock,
        SharedTransfer,
    >;

    fn transfer_port() -> Rc<RefCell<QueuedTransferGateway>> {
        Rc::new(RefCell::new(QueuedTransferGateway::new()))
    }

    fn composed_service(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        transfer: &Rc<RefCell<QueuedTransferGateway>>,
        repository: CompositeRepository,
    ) -> ComposedService {
        let mutations: MutationServiceUnderTest = MutationService::new(
            ProfileSyncService::new(SharedGateway(Rc::clone(shared)), repository),
            DeterministicPlanIds::starting_at(1),
        );
        ComposedMutationService::new(
            mutations,
            SharedTransfer(Rc::clone(transfer)),
            FakeClock::fixed(NOW),
            TTL,
            PlanLifetime::MAX,
        )
    }

    fn warlock() -> Character {
        Character {
            id: CharacterId::new("warlock"),
            date_last_played: None,
            name: Some("Warlock".to_owned()),
            class: None,
            race: None,
            gender: None,
            light: None,
        }
    }

    fn hunter() -> Character {
        Character {
            id: CharacterId::new("hunter"),
            date_last_played: None,
            name: Some("Hunter".to_owned()),
            class: None,
            race: None,
            gender: None,
            light: None,
        }
    }

    fn at_warlock() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("warlock"))
    }

    fn at_hunter() -> ItemLocation {
        ItemLocation::Character(CharacterId::new("hunter"))
    }

    /// Queues one membership resolution and one inventory acquisition.
    fn queue_projection(
        shared: &Rc<RefCell<QueuedProfileGateway>>,
        checked_at: Timestamp,
        characters: Vec<Character>,
        items: Vec<ItemInstance>,
    ) {
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Ok(snapshot_input(checked_at, characters, items)));
    }

    fn exact_move(instance_id: &str, character_id: &str) -> MoveItemRequest {
        MoveItemArgs {
            instance_id: Some(instance_id.to_owned()),
            item_text: None,
            to_vault: false,
            character_id: Some(character_id.to_owned()),
            character_label: None,
        }
        .into_request()
        .expect("valid exact arguments")
    }

    fn vault_item(instance_id: &str, item_hash: i64) -> ItemInstance {
        instance(instance_id, item_hash, ItemLocation::Vault, Vec::new())
    }

    #[test]
    fn move_item_performs_the_whole_one_call_contract_from_one_derived_membership() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        // The mandatory fresh preflight, then the mandatory post-action
        // refresh. The plan itself needs no acquisition because the seeded
        // projection is already fresh.
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        queue_projection(
            &shared,
            NOW,
            vec![warlock()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("the one call completes the move");

        let destination = LocationView::Character {
            character_id: "warlock".to_owned(),
        };
        match &output {
            MoveItemOutput::VerifiedSuccess(success) => {
                assert_eq!(
                    success.effects,
                    MoveEffectsView {
                        item_id: "inst-a".to_owned(),
                        via_vault: false,
                        source: LocationView::Vault,
                        destination: destination.clone(),
                    }
                );
                assert_eq!(success.observed, destination);
                assert_eq!(success.steps.len(), 1);
                assert_eq!(success.steps[0].ordinal, 1);
                assert_eq!(success.steps[0].from, LocationView::Vault);
                assert_eq!(success.steps[0].to, destination);
                assert_eq!(success.steps[0].outcome, StepOutcomeView::Completed);
            }
            other => panic!("expected a verified success, got {other:?}"),
        }

        // Exactly one fresh preflight, one ordered transfer, and one post-action
        // refresh, all inside the single MCP call.
        assert_eq!(shared.borrow().membership_calls(), 2);
        assert_eq!(shared.borrow().inventory_calls(), 2);
        assert_eq!(service.mutations().plan_ids().calls(), 1);

        let queue = transfer.borrow();
        assert_eq!(queue.calls(), 1);
        let issued = &queue.requests()[0];
        assert_eq!(issued.item().as_str(), "inst-a");
        assert_eq!(issued.item_hash().value(), 1);
        assert_eq!(issued.quantity(), 1);
        assert_eq!(issued.from(), &ItemLocation::Vault);
        assert_eq!(issued.to(), &at_warlock());
        // The provider identity the transfer carried was derived internally.
        assert_eq!(
            issued.membership().membership_id().as_str(),
            MEMBERSHIP_IDENTIFIER
        );
        drop(queue);

        // No membership identifier, credential, or provider text crosses MCP.
        let rendered = serde_json::to_string(&output).expect("output serializes");
        assert!(rendered.contains("verified_success"));
        assert!(!rendered.contains(MEMBERSHIP_IDENTIFIER), "{rendered}");
    }

    #[test]
    fn move_item_reports_a_verified_no_op_without_dispatching() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        // The fresh preflight is the one acquisition, and it already observes
        // the requested end state.
        queue_projection(
            &shared,
            NOW,
            vec![warlock()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("an already-satisfied move is not an error");

        match &output {
            MoveItemOutput::VerifiedNoOp(no_op) => {
                assert_eq!(
                    no_op.observed,
                    LocationView::Character {
                        character_id: "warlock".to_owned(),
                    }
                );
                assert_eq!(no_op.steps.len(), 1);
                assert_eq!(no_op.steps[0].outcome, StepOutcomeView::Unattempted);
            }
            other => panic!("expected a verified no-op, got {other:?}"),
        }

        // Zero transfer calls and zero post-action refreshes were spent.
        assert_eq!(transfer.borrow().calls(), 0);
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn move_item_reports_an_honest_incomplete_execution_with_typed_evidence() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        // The provider refused the step, so the refreshed projection still
        // places the item at the plan source, consistent with zero applied steps.
        queue_projection(&shared, NOW, vec![warlock()], vec![vault_item("inst-a", 1)]);
        transfer.borrow_mut().queue(Ok(TransferOutcome::Rejected(
            TransferRejection::DestinationFull,
        )));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("a refusal is an outcome, not a tool error");

        match &output {
            MoveItemOutput::Incomplete(incomplete) => {
                assert_eq!(incomplete.observed, LocationView::Vault);
                assert!(!incomplete.partial);
                assert!(incomplete.failure.is_none());
                assert_eq!(incomplete.steps.len(), 1);
                assert_eq!(
                    incomplete.steps[0].outcome,
                    StepOutcomeView::Rejected {
                        reason: TransferRejectionView::DestinationFull,
                    }
                );
            }
            other => panic!("expected an incomplete execution, got {other:?}"),
        }

        assert_eq!(transfer.borrow().calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 2);
    }

    #[test]
    fn move_item_reports_a_partial_via_vault_execution_honestly() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock(), hunter()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock(), hunter()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        // Step one applied, step two failed transiently, so the item is stranded
        // in the Vault, which is exactly what the refreshed projection reports.
        queue_projection(
            &shared,
            NOW,
            vec![warlock(), hunter()],
            vec![vault_item("inst-a", 1)],
        );
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));
        transfer.borrow_mut().queue(Ok(TransferOutcome::Transient));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "hunter"))
            .expect("a partial execution is an outcome");

        match &output {
            MoveItemOutput::Incomplete(incomplete) => {
                assert_eq!(incomplete.observed, LocationView::Vault);
                assert!(incomplete.partial, "one applied step is a partial write");
                assert!(incomplete.effects.via_vault);
                assert_eq!(incomplete.steps.len(), 2);
                assert_eq!(incomplete.steps[0].outcome, StepOutcomeView::Completed);
                assert_eq!(incomplete.steps[1].outcome, StepOutcomeView::Transient);
            }
            other => panic!("expected a partial incomplete execution, got {other:?}"),
        }

        let queue = transfer.borrow();
        assert_eq!(queue.calls(), 2);
        assert_eq!(queue.requests()[0].to(), &ItemLocation::Vault);
        assert_eq!(queue.requests()[1].from(), &ItemLocation::Vault);
        assert_eq!(queue.requests()[1].to(), &at_hunter());
    }

    #[test]
    fn move_item_reports_an_uncertain_step_as_uncertain_rather_than_a_success() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        // The step effect could not be determined, and the refreshed projection
        // still shows the item at its source, so the honest shape is incomplete.
        queue_projection(&shared, NOW, vec![warlock()], vec![vault_item("inst-a", 1)]);
        transfer.borrow_mut().queue(Ok(TransferOutcome::Uncertain));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("an uncertain effect is an outcome, not a tool error");

        match &output {
            MoveItemOutput::Incomplete(incomplete) => {
                assert_eq!(incomplete.observed, LocationView::Vault);
                assert!(!incomplete.partial);
                assert_eq!(incomplete.steps[0].outcome, StepOutcomeView::Uncertain);
            }
            other => panic!("an uncertain step must never be reported as success: {other:?}"),
        }
    }

    #[test]
    fn move_item_reports_a_changed_preflight_projection_as_a_typed_stale_failure() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock(), hunter()],
            vec![vault_item("inst-a", 1)],
        ));
        // The mandatory fresh preflight observes a changed projection: the same
        // item now sits on another character, so the plan no longer binds.
        queue_projection(
            &shared,
            NOW,
            vec![warlock(), hunter()],
            vec![instance("inst-a", 1, at_hunter(), Vec::new())],
        );

        let mut service = composed_service(&shared, &transfer, repository);
        let error = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect_err("a changed projection must fail closed before dispatch");

        assert_eq!(error.code(), "stale_state");
        // Nothing reached the provider and no post-action refresh was spent.
        assert_eq!(transfer.borrow().calls(), 0);
        assert_eq!(shared.borrow().membership_calls(), 1);
        assert_eq!(shared.borrow().inventory_calls(), 1);
    }

    #[test]
    fn move_item_reports_a_contradicted_projection_as_a_typed_verification_failure() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock(), hunter()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock(), hunter()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        // A changed profile still contradicts the reported completion.
        let mut changed_character = warlock();
        changed_character.name = Some("Changed character".into());
        queue_projection(
            &shared,
            NOW,
            vec![changed_character, hunter()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "hunter"))
            .expect("a contradiction is an outcome, not a tool error");

        match &output {
            MoveItemOutput::VerificationFailed(failure) => {
                assert!(matches!(
                    failure.reason,
                    VerificationFailureReasonView::Elsewhere
                ));
                assert_eq!(
                    failure.observed,
                    Some(LocationView::Character {
                        character_id: "warlock".to_owned(),
                    })
                );
                assert!(failure.failure.is_none());
                assert_eq!(failure.steps.len(), 2);
            }
            other => panic!("expected a verification failure, got {other:?}"),
        }
    }

    #[test]
    fn move_item_pending_provider_refresh_preserves_stale_observation_and_steps() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        for checked_at in [FRESH_CHECKED_AT, NOW] {
            queue_projection(
                &shared,
                checked_at,
                vec![warlock()],
                vec![vault_item("inst-a", 1)],
            );
        }
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));
        let mut service = composed_service(&shared, &transfer, repository);
        let output = service.move_item(&exact_move("inst-a", "warlock")).unwrap();
        let MoveItemOutput::AppliedPendingProviderRefresh(pending) = &output else {
            panic!("expected pending, got {output:?}");
        };
        assert_eq!(pending.observed, LocationView::Vault);
        assert_eq!(pending.steps.len(), 1);
        assert_eq!(pending.steps[0].outcome, StepOutcomeView::Completed);
        assert!(pending.reason.contains("stale/unconfirmed"));
        assert!(pending.reason.contains("ghostlink sync"));
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["status"], "applied_pending_provider_refresh");
        assert_eq!(output.into_call_tool_result().is_error, Some(true));
    }

    #[test]
    fn move_item_reports_a_refresh_failure_as_a_typed_sanitized_failure() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        // The mandatory post-action refresh fails, so no observation is possible.
        shared.borrow_mut().queue_membership(Ok(membership()));
        shared
            .borrow_mut()
            .queue_inventory(Err(AppError::new(ErrorCategory::ProviderUnavailable)));
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));

        let mut service = composed_service(&shared, &transfer, repository);
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("a refresh failure keeps its bounded evidence");

        match &output {
            MoveItemOutput::VerificationFailed(failure) => {
                assert!(matches!(
                    failure.reason,
                    VerificationFailureReasonView::RefreshFailed
                ));
                assert!(failure.observed.is_none());
                let typed = failure.failure.as_ref().expect("a typed refresh failure");
                assert_eq!(typed.code(), "provider_unavailable");
                assert_eq!(failure.steps.len(), 1);
                assert_eq!(failure.steps[0].outcome, StepOutcomeView::Completed);
            }
            other => panic!("expected a verification failure, got {other:?}"),
        }

        let rendered = serde_json::to_string(&output).expect("output serializes");
        assert!(!rendered.contains(MEMBERSHIP_IDENTIFIER), "{rendered}");
    }

    #[test]
    fn move_item_keeps_missing_ambiguous_and_unsupported_failures_typed_and_remote_free() {
        // A missing target and an unsupported source/destination pair both fail
        // closed before any provider mutation.
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        let calls = Rc::clone(&repository.calls);
        let mut service = composed_service(&shared, &transfer, repository);

        let missing = service
            .move_item(&exact_move("absent", "warlock"))
            .expect_err("a missing instance must fail closed");
        assert_eq!(missing.code(), "stale_state");

        let unsupported = service
            .move_item(
                &MoveItemArgs {
                    instance_id: Some("inst-a".to_owned()),
                    item_text: None,
                    to_vault: true,
                    character_id: None,
                    character_label: None,
                }
                .into_request()
                .expect("valid arguments"),
            )
            .expect_err("a vault-to-vault move must fail closed");
        assert_eq!(unsupported.code(), "unsupported_operation");

        // An ambiguous exact lookup is only reachable through a
        // contract-violating projection, and it fails closed instead of guessing.
        let duplicate = vault_item("dup", 1);
        service
            .mutations_mut()
            .repository_mut()
            .search
            .set_instances(vec![duplicate.clone(), duplicate]);
        let ambiguous = service
            .move_item(&exact_move("dup", "warlock"))
            .expect_err("an ambiguous instance must fail closed");
        assert_eq!(ambiguous.code(), "ambiguous_match");

        // The initial fresh projection means no acquisition was needed, and no
        // failure above reached the provider.
        assert_eq!(transfer.borrow().calls(), 0);
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
        assert!(
            calls.borrow().iter().all(|call| *call != "replace_current"),
            "no failed request may publish a projection: {:?}",
            calls.borrow()
        );
    }

    #[test]
    fn move_item_resolves_semantic_and_label_selectors_without_guessing() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        queue_projection(
            &shared,
            NOW,
            vec![warlock()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));

        let mut service = composed_service(&shared, &transfer, repository);
        let request = MoveItemArgs {
            instance_id: None,
            item_text: Some("gjallar".to_owned()),
            to_vault: false,
            character_id: None,
            character_label: Some("Warlock".to_owned()),
        }
        .into_request()
        .expect("valid semantic arguments");

        let output = service
            .move_item(&request)
            .expect("the semantic term and label resolve uniquely");

        match &output {
            MoveItemOutput::VerifiedSuccess(success) => {
                assert_eq!(success.effects.item_id, "inst-a");
            }
            other => panic!("expected a verified success, got {other:?}"),
        }
        assert_eq!(transfer.borrow().requests()[0].to(), &at_warlock());
    }

    #[test]
    fn move_item_rejects_absent_ambiguous_or_unbounded_selectors_before_any_call() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            Vec::new(),
        ));
        let calls = Rc::clone(&repository.calls);
        let mut service = composed_service(&shared, &transfer, repository);

        let overlong = "i".repeat(MAX_IDENTIFIER_LENGTH + 1);
        let cases = vec![
            // Neither item selector, and both item selectors.
            MoveItemArgs {
                instance_id: None,
                item_text: None,
                to_vault: true,
                character_id: None,
                character_label: None,
            },
            MoveItemArgs {
                instance_id: Some("inst-a".to_owned()),
                item_text: Some("gjallar".to_owned()),
                to_vault: true,
                character_id: None,
                character_label: None,
            },
            // No destination, two destinations, and the Vault plus a character.
            MoveItemArgs {
                instance_id: Some("inst-a".to_owned()),
                item_text: None,
                to_vault: false,
                character_id: None,
                character_label: None,
            },
            MoveItemArgs {
                instance_id: Some("inst-a".to_owned()),
                item_text: None,
                to_vault: false,
                character_id: Some("warlock".to_owned()),
                character_label: Some("Warlock".to_owned()),
            },
            MoveItemArgs {
                instance_id: Some("inst-a".to_owned()),
                item_text: None,
                to_vault: true,
                character_id: Some("warlock".to_owned()),
                character_label: None,
            },
            // Empty, over-long, and control-bearing semantic values.
            MoveItemArgs {
                instance_id: None,
                item_text: Some(String::new()),
                to_vault: true,
                character_id: None,
                character_label: None,
            },
            MoveItemArgs {
                instance_id: Some(overlong.clone()),
                item_text: None,
                to_vault: true,
                character_id: None,
                character_label: None,
            },
            MoveItemArgs {
                instance_id: Some("inst-a".to_owned()),
                item_text: None,
                to_vault: false,
                character_id: None,
                character_label: Some("Warlock\u{7}".to_owned()),
            },
        ];

        for case in cases {
            let error = case
                .into_request()
                .expect_err("an absent or ambiguous selector must be rejected");
            assert_eq!(error.code(), "provider_validation");
            assert!(!error.message().contains(&overlong));
        }

        // A rejected argument never reached the composed service.
        assert!(calls.borrow().is_empty());
        assert_eq!(transfer.borrow().calls(), 0);
        assert_eq!(shared.borrow().membership_calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);

        // The same wired service does reach the shared seams for a valid
        // request, so the empty record above is a real argument rejection and
        // not a dead harness. A missing item still fails closed with its typed
        // category and spends no provider call.
        let missing = service
            .move_item(&exact_move("absent", "warlock"))
            .expect_err("a missing instance must fail closed");
        assert_eq!(missing.code(), "stale_state");
        assert!(
            calls.borrow().contains(&"lookup_instance"),
            "the service must reach the shared repository: {:?}",
            calls.borrow()
        );
        assert_eq!(transfer.borrow().calls(), 0);
        assert_eq!(shared.borrow().inventory_calls(), 0);
    }

    #[test]
    fn the_mutation_seam_is_object_safe_and_injectable() {
        let shared = gateway();
        let transfer = transfer_port();
        let mut repository = CompositeRepository::with_definitions(standard_definitions());
        repository.commit(snapshot_input(
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        ));
        queue_projection(
            &shared,
            FRESH_CHECKED_AT,
            vec![warlock()],
            vec![vault_item("inst-a", 1)],
        );
        queue_projection(
            &shared,
            NOW,
            vec![warlock()],
            vec![instance("inst-a", 1, at_warlock(), Vec::new())],
        );
        transfer
            .borrow_mut()
            .queue(Ok(TransferOutcome::Transferred));

        let mut service: Box<dyn MutationToolService> =
            Box::new(composed_service(&shared, &transfer, repository));
        let output = service
            .move_item(&exact_move("inst-a", "warlock"))
            .expect("injected mutation succeeds");

        assert!(matches!(output, MoveItemOutput::VerifiedSuccess(_)));
    }
}
