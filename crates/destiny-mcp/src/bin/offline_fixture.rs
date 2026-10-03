//! Test-only offline stdio fixture for the child-process integration test.
//!
//! This binary is compiled only with the non-default `offline-fixtures` feature
//! and is never reachable from the product `destiny-mcp` binary: it declares no
//! flag, environment branch, or module that the product binary can select. It
//! serves the same [`ReadToolServer`] over the same stdio runtime, but composes
//! the read and mutation seams with deterministic in-memory application test
//! doubles instead of adapters, so the transport and one-call mutation contract
//! can be exercised with no credential, network, or database.
//!
//! # One shared world
//!
//! One in-memory [`FixtureWorld`] is the source of truth. The profile gateway
//! reads it on every forced refresh, and the transfer gateway applies each
//! accepted transfer to it. The mutation service therefore plans against a
//! seeded projection, performs a real fresh preflight and post-action refresh
//! against the world, and verifies the destination only after the transfer moved
//! the item. Reads are served from a separate seeded projection.
//!
//! # Scenarios
//!
//! The initial world and the mutation seed are selected by the
//! `DESTINY_FIXTURE_SCENARIO` environment variable so a test can spawn a fresh
//! process per contract. The default `success` scenario seeds the world and the
//! mutation projection identically; `already-satisfied` and `preflight-change`
//! place one designated item differently in the world than in the seed, and
//! `partial` refuses the second step of the one two-step move.
//!
//! The fixture data embeds a secret sentinel inside an opaque provider location
//! value. The MCP projection drops that text, so the integration test can assert
//! the sentinel never reaches stdout or stderr.

use std::sync::{Arc, Mutex};

use destiny_app::test_support::leak_sentinels;
use destiny_app::test_support::{
    DeterministicPlanIds, FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository,
    InMemorySearchRepository,
};
use destiny_app::{
    AppError, Character, CharacterId, DefinitionRepository, DefinitionSearchQuery,
    DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy, InventoryRepository,
    InventorySnapshot, InventorySnapshotInput, ItemDefinition, ItemHash, ItemInstance,
    ItemInstanceId, ItemLocation, MembershipId, MembershipType, MutationService, PerkHash,
    PerkSummary, PlanLifetime, ProfileGateway, ProfileSyncService, QueryService, Resolution,
    SearchRepository, SourceMarker, Timestamp, TransferGateway, TransferOutcome, TransferRejection,
    TransferRequest,
};
use destiny_mcp::runtime::serve_stdio;
use destiny_mcp::{ComposedMutationService, QueryReadService, ReadToolServer};

/// The scenario selector read once at process start.
const SCENARIO_ENV: &str = "DESTINY_FIXTURE_SCENARIO";

const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";
const DEFINITION_VERSION: &str = "fixture-definitions-v1";
const NOW: Timestamp = Timestamp::new(10_000);
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// The deterministic offline contract this fixture process serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    /// The world and the mutation seed agree: a Vault-to-character move succeeds.
    Success,
    /// The world already holds the designated item at the requested destination,
    /// so the fresh preflight reports a verified no-op without dispatching.
    AlreadySatisfied,
    /// The world moved the designated item away from its planned source before
    /// the fresh preflight, so the plan is rejected as stale without dispatching.
    PreflightChange,
    /// The second, Vault-to-character step of a two-step character-to-character
    /// move is refused, honestly stranding the item in the Vault.
    Partial,
}

impl Scenario {
    /// Reads the scenario from the environment, defaulting to a clean success.
    fn from_env() -> Self {
        match std::env::var(SCENARIO_ENV).ok().as_deref() {
            Some("already-satisfied") => Self::AlreadySatisfied,
            Some("preflight-change") => Self::PreflightChange,
            Some("partial") => Self::Partial,
            _ => Self::Success,
        }
    }

    /// The projection the mutation service plans against.
    ///
    /// Every scenario seeds the designated item at its planned source; only the
    /// [`FixtureWorld`] diverges, which is exactly what the fresh preflight
    /// observes.
    fn seed_items(self) -> Vec<ItemInstance> {
        base_items()
    }

    /// The current provider state the world gateway reads back.
    fn world_items(self) -> Vec<ItemInstance> {
        let mut items = base_items();
        match self {
            Self::AlreadySatisfied => set_location(&mut items, "inst-d", at_character("warlock")),
            Self::PreflightChange => set_location(&mut items, "inst-c", at_character("zavala")),
            Self::Success | Self::Partial => {}
        }
        items
    }
}

#[tokio::main]
async fn main() {
    let scenario = Scenario::from_env();
    if let Err(error) = serve_stdio(build_server(scenario)).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Builds the deterministic offline read-plus-mutation server for the child.
fn build_server(scenario: Scenario) -> ReadToolServer {
    let world = Arc::new(Mutex::new(FixtureWorld {
        characters: characters(),
        items: scenario.world_items(),
    }));

    // Reads are served from a projection seeded from the world's initial state.
    let read_repository = {
        let world = world.lock().expect("fixture world lock");
        FixtureRepository::seeded(&world.characters, &world.items)
    };
    let read_service = QueryReadService::new(
        QueryService::new(ProfileSyncService::new(
            WorldGateway {
                membership: membership(),
                world: Arc::clone(&world),
            },
            read_repository,
        )),
        FakeClock::fixed(NOW),
        TTL,
    );

    // Mutation plans against the seed projection and refreshes from the world.
    let mutations = MutationService::new(
        ProfileSyncService::new(
            WorldGateway {
                membership: membership(),
                world: Arc::clone(&world),
            },
            FixtureRepository::seeded(&characters(), &scenario.seed_items()),
        ),
        DeterministicPlanIds::starting_at(1),
    );
    let mutation_service = ComposedMutationService::new(
        mutations,
        WorldTransferGateway {
            world,
            refuse_vault_route: scenario == Scenario::Partial,
        },
        FakeClock::fixed(NOW),
        TTL,
        PlanLifetime::MAX,
    );

    let read_service = destiny_mcp::service::PersonalWeeklyReadService::new(
        destiny_app::WeeklyPersonalService::new(
            PublicGateway,
            PersonalGateway,
            weekly_repository(),
        ),
        read_service,
    );
    let read_service = destiny_mcp::service::WeeklyReadService::new(
        destiny_app::weekly_reset::WeeklyResetService::new(PublicGateway, weekly_repository()),
        read_service,
        |t| destiny_adapters::format_reset_timestamp(t, false),
    );
    ReadToolServer::with_mutation(Box::new(read_service), Box::new(mutation_service))
}

struct PersonalGateway;
impl destiny_app::WeeklyProfileGateway for PersonalGateway {
    fn load_character_weekly(
        &mut self,
        _: &DestinyMembership,
        character: &CharacterId,
    ) -> Result<destiny_app::CharacterWeeklySnapshot, AppError> {
        if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("definitions-unavailable") {
            panic!("definitions must gate provider requests");
        }
        Ok(destiny_app::CharacterWeeklySnapshot {
            character_id: character.clone(),
            milestones: vec![],
            available_activities: (10..=16)
                .map(|hash| destiny_app::AvailableActivity {
                    activity_hash: hash,
                    focused: false,
                    completed_ever: false,
                    recommended_light: Some(if hash == 10 { 400 } else { 300 }),
                    difficulty_tier: None,
                    modifier_hashes: vec![],
                    challenges: vec![destiny_app::ChallengeProgress {
                        objective_hash: 5,
                        progress: 0,
                        completion_value: 1,
                        complete: false,
                    }],
                    visible_reward_item_hashes: vec![if hash == 13 { 2 } else { 1 }],
                })
                .collect(),
        })
    }
}
struct PublicGateway;

impl destiny_app::PublicMilestonesGateway for PublicGateway {
    fn load_public_milestones(
        &mut self,
    ) -> Result<destiny_app::PublicMilestonesSnapshot, AppError> {
        if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("definitions-unavailable") {
            panic!("definitions must gate provider requests");
        }
        Ok(destiny_app::PublicMilestonesSnapshot {
            milestones: vec![destiny_app::PublicMilestone {
                milestone_hash: 1,
                order: 0,
                start: Some(Timestamp::new(0)),
                end: None,
                activities: vec![destiny_app::PublicMilestoneActivity {
                    activity_hash: 2,
                    modifier_hashes: vec![99, 3],
                    challenge_objective_hashes: vec![],
                }],
            }],
        })
    }
}

fn weekly_repository() -> InMemoryDefinitionRepository {
    use destiny_app::{
        ActivityDefinition, ActivityModifierDefinition, DefinitionReplacementInput,
        ManifestDefinitions, MilestoneDefinition, WorldDefinitionRepository, WorldDefinitionTable,
        WorldDefinitionsAcquisition,
    };
    let mut repository = InMemoryDefinitionRepository::default();
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("definitions-unavailable") {
        return repository;
    }
    let version = DefinitionVersion::new(DEFINITION_VERSION);
    repository
        .publish_manifest(
            DefinitionReplacementInput {
                definitions: ManifestDefinitions {
                    version: version.clone(),
                    definitions: fixture_definitions(),
                },
                checked_at: NOW,
                source: SourceMarker::Manifest,
            },
            WorldDefinitionsAcquisition {
                stats: WorldDefinitionTable {
                    definitions: vec![
                        destiny_app::StatDefinition::new(1, "Weapons".into()).unwrap(),
                    ],
                    skipped_count: 0,
                },
                objectives: WorldDefinitionTable {
                    definitions: vec![
                        destiny_app::ObjectiveDefinition::new(5, "".into(), 1).unwrap(),
                    ],
                    skipped_count: 0,
                },
                activity_types: WorldDefinitionTable {
                    definitions: vec![
                        destiny_app::ActivityTypeDefinition::new(4, "Raid".into(), "".into())
                            .unwrap(),
                        destiny_app::ActivityTypeDefinition::new(6, "Mission".into(), "".into())
                            .unwrap(),
                        destiny_app::ActivityTypeDefinition::new(
                            7,
                            "The Crucible".into(),
                            "".into(),
                        )
                        .unwrap(),
                        destiny_app::ActivityTypeDefinition::new(
                            8,
                            "Vanguard Op".into(),
                            "".into(),
                        )
                        .unwrap(),
                    ],
                    skipped_count: 0,
                },
                version,
                milestones: WorldDefinitionTable {
                    definitions: vec![
                        MilestoneDefinition::new(
                            1,
                            "Weekly raid".into(),
                            "Public rotation".into(),
                            0,
                        )
                        .unwrap(),
                    ],
                    skipped_count: 0,
                },
                activities: WorldDefinitionTable {
                    definitions: vec![
                        ActivityDefinition::new(2, "Raid".into(), "Activity".into(), 0, 0).unwrap(),
                        ActivityDefinition::new(10, "The Disgraced".into(), "".into(), 6, 0)
                            .unwrap(),
                        ActivityDefinition::new(11, "Control: Matchmade".into(), "".into(), 7, 0)
                            .unwrap(),
                        ActivityDefinition::new(12, "Quickplay: Matchmade".into(), "".into(), 8, 0)
                            .unwrap(),
                        ActivityDefinition::new(13, "Quickplay: Customize".into(), "".into(), 8, 0)
                            .unwrap(),
                        ActivityDefinition::new(
                            14,
                            "Override: Europa: Matchmade".into(),
                            "".into(),
                            6,
                            0,
                        )
                        .unwrap(),
                        ActivityDefinition::new(
                            15,
                            "Override: Europa: Customize".into(),
                            "".into(),
                            6,
                            0,
                        )
                        .unwrap(),
                        ActivityDefinition::new(
                            16,
                            "Expedition: Cosmodrome: Private".into(),
                            "".into(),
                            6,
                            0,
                        )
                        .unwrap(),
                    ],
                    skipped_count: 0,
                },
                modifiers: WorldDefinitionTable {
                    definitions: vec![
                        ActivityModifierDefinition::new(3, "Modifier".into(), "{var:42}".into())
                            .unwrap(),
                    ],
                    skipped_count: 0,
                },
            },
        )
        .unwrap();
    repository
}

/// The mutable provider world shared by the profile and transfer gateways.
#[derive(Clone, Debug)]
struct FixtureWorld {
    characters: Vec<Character>,
    items: Vec<ItemInstance>,
}

impl FixtureWorld {
    /// One complete candidate for the current world state.
    fn snapshot_input(&self) -> InventorySnapshotInput {
        snapshot_input(&self.characters, &self.items)
    }

    /// Applies one accepted transfer to the concrete item's location.
    fn apply(&mut self, item: &ItemInstanceId, to: &ItemLocation) {
        if let Some(instance) = self
            .items
            .iter_mut()
            .find(|instance| &instance.instance_id == item)
        {
            instance.location = to.clone();
        }
    }
}

/// The profile gateway backed by the shared in-memory world.
#[derive(Clone)]
struct WorldGateway {
    membership: DestinyMembership,
    world: Arc<Mutex<FixtureWorld>>,
}

impl ProfileGateway for WorldGateway {
    fn resolve_membership(&mut self) -> Result<DestinyMembership, AppError> {
        assert_ne!(
            std::env::var(SCENARIO_ENV).ok().as_deref(),
            Some("definitions-unavailable"),
            "definitions gate must prevent membership requests"
        );
        Ok(self.membership.clone())
    }

    fn load_inventory(
        &mut self,
        membership: &DestinyMembership,
    ) -> Result<InventorySnapshotInput, AppError> {
        assert_ne!(
            std::env::var(SCENARIO_ENV).ok().as_deref(),
            Some("definitions-unavailable"),
            "definitions gate must prevent inventory requests"
        );
        if membership != &self.membership {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        let world = self
            .world
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        Ok(world.snapshot_input())
    }
}

/// The transfer gateway that applies each accepted step to the shared world.
#[derive(Clone)]
struct WorldTransferGateway {
    world: Arc<Mutex<FixtureWorld>>,
    /// When set, refuses every Vault-to-character step, which strands a
    /// two-step move honestly in the Vault.
    refuse_vault_route: bool,
}

impl TransferGateway for WorldTransferGateway {
    fn transfer(&mut self, request: &TransferRequest) -> Result<TransferOutcome, AppError> {
        if self.refuse_vault_route && *request.from() == ItemLocation::Vault {
            return Ok(TransferOutcome::Rejected(
                TransferRejection::NotTransferable,
            ));
        }
        let mut world = self
            .world
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        world.apply(request.item(), request.to());
        Ok(TransferOutcome::Transferred)
    }
}

/// One composite repository exposing the three read seams over in-memory fakes.
#[derive(Clone, Debug, Default)]
struct FixtureRepository {
    inventory: InMemoryInventoryRepository,
    search: InMemorySearchRepository,
    definitions: InMemoryDefinitionRepository,
}

impl FixtureRepository {
    /// Seeds one committed projection for the supplied characters and items.
    fn seeded(characters: &[Character], items: &[ItemInstance]) -> Self {
        let version = DefinitionVersion::new(DEFINITION_VERSION);
        let definitions = fixture_definitions();
        let mut repository = Self {
            inventory: InMemoryInventoryRepository::new(),
            search: InMemorySearchRepository::with_definitions(definitions.clone()),
            definitions: InMemoryDefinitionRepository::new(version, definitions),
        };
        let snapshot = repository
            .inventory
            .replace_current(snapshot_input(characters, items))
            .expect("fixture projection is valid");
        repository.search.set_instances(snapshot.items);
        repository
    }
}

impl InventoryRepository for FixtureRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        self.inventory.load_current()
    }

    fn replace_current(
        &mut self,
        input: InventorySnapshotInput,
    ) -> Result<InventorySnapshot, AppError> {
        let snapshot = self.inventory.replace_current(input)?;
        self.search.set_instances(snapshot.items.clone());
        Ok(snapshot)
    }
}

impl SearchRepository for FixtureRepository {
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
        self.search.search_definitions(query)
    }

    fn search_instances(
        &self,
        query: &destiny_app::InstanceSearchQuery,
    ) -> Result<Vec<ItemInstance>, AppError> {
        self.search.search_instances(query)
    }

    fn lookup_instance(
        &self,
        instance_id: &ItemInstanceId,
    ) -> Result<Resolution<ItemInstance>, AppError> {
        self.search.lookup_instance(instance_id)
    }
}

impl destiny_app::StatDefinitionRepository for FixtureRepository {
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

impl DefinitionRepository for FixtureRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.definitions.current_definition_version()
    }

    fn lookup_definition(&self, hash: ItemHash) -> Result<Resolution<ItemDefinition>, AppError> {
        self.definitions.lookup_definition(hash)
    }

    fn current_metadata(&self) -> Result<Option<destiny_app::SyncMetadata>, AppError> {
        self.definitions.current_metadata()
    }

    fn replace_definitions(
        &mut self,
        replacement: destiny_app::DefinitionReplacementInput,
    ) -> Result<destiny_app::SyncMetadata, AppError> {
        self.definitions.replace_definitions(replacement)
    }
}

/// The fixture membership every gateway resolves.
fn membership() -> DestinyMembership {
    DestinyMembership::new(
        MembershipType::new(3),
        MembershipId::new(MEMBERSHIP_IDENTIFIER),
    )
    .expect("fixture membership is valid")
}

/// One complete normalized candidate for the supplied characters and items.
fn snapshot_input(characters: &[Character], items: &[ItemInstance]) -> InventorySnapshotInput {
    let membership = membership();
    InventorySnapshotInput {
        membership_id: membership.membership_id().clone(),
        membership_type: membership.membership_type(),
        definition_version: DefinitionVersion::new(DEFINITION_VERSION),
        checked_at: NOW,
        source: SourceMarker::Profile,
        characters: characters.to_vec(),
        items: items.to_vec(),
    }
}

fn characters() -> Vec<Character> {
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("empty-characters") {
        return vec![];
    }
    let mut characters = vec![character("warlock"), character("zavala")];
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("ambiguous-character") {
        characters[0].class = Some("Titan".into());
    }
    characters
}

fn base_items() -> Vec<ItemInstance> {
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("empty-characters") {
        return vec![];
    }
    vec![
        instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)]),
        instance(
            "inst-b",
            2,
            ItemLocation::other(opaque_provider_location()),
            Vec::new(),
        ),
        instance("inst-c", 3, ItemLocation::Vault, Vec::new()),
        instance("inst-d", 4, ItemLocation::Vault, Vec::new()),
        instance("inst-e", 5, at_character("zavala"), Vec::new()),
    ]
}

/// One opaque provider location value carrying every shared secret canary.
///
/// The projection renders this as [`LocationView::Other`] without its text, so
/// no byte of it may reach stdout or stderr.
fn opaque_provider_location() -> String {
    format!(
        "{}|{}|{}",
        leak_sentinels::RAW_PAYLOAD,
        leak_sentinels::API_KEY,
        leak_sentinels::BEARER_TOKEN
    )
}

fn at_character(id: &str) -> ItemLocation {
    ItemLocation::Character(CharacterId::new(id))
}

fn set_location(items: &mut [ItemInstance], instance_id: &str, location: ItemLocation) {
    if let Some(item) = items
        .iter_mut()
        .find(|item| item.instance_id.as_str() == instance_id)
    {
        item.location = location;
    }
}

fn fixture_definitions() -> Vec<ItemDefinition> {
    vec![
        definition(1, "Gjallarhorn"),
        definition(2, "Fatebringer"),
        definition(3, "Vex Mythoclast"),
        definition(4, "Thorn"),
        definition(5, "The Last Word"),
    ]
}

fn character(id: &str) -> Character {
    Character {
        id: CharacterId::new(id),
        // The shared non-secret control crosses to the client in the readable
        // name, which proves the MCP stdout stream carries real content.
        name: Some(format!("name-{id}-{}", leak_sentinels::CONTROL)),
        class: Some(if id == "warlock" { "Warlock" } else { "Titan" }.into()),
        race: None,
        gender: None,
        light: Some(if id == "warlock" { 2000 } else { 1900 }),
    }
}

fn definition(hash: i64, name: &str) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.to_owned(),
        item_type: None,
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: Vec::new(),
        version: DefinitionVersion::new(DEFINITION_VERSION),
    }
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
