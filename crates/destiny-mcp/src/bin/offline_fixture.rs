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
//! `equip-provider-stale` and `move-provider-stale` confirm every step without
//! mutating the world, retaining the pre-dispatch projection. In contrast,
//! `equip-contradiction` applies the transfer but leaves the item unequipped,
//! producing changed content that cannot verify the requested equip.
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
        refresh_failed: false,
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
    let mut manifest = destiny_app::test_support::QueuedManifestGateway::new();
    for _ in 0..2 {
        manifest.queue_capacity_evidence(Ok(destiny_app::EquipmentCapacityEvidence::new(
            fixture_version(),
            destiny_adapters::equipment_capacity_policy::PINNED_EQUIPMENT_BUCKET_PATH,
        )
        .unwrap()));
    }
    let pull = destiny_mcp::service::InstancedPullToolService::new(
        WorldGateway {
            membership: membership(),
            world: Arc::clone(&world),
        },
        destiny_app::InstancedPullService::new(
            ProfileSyncService::new(
                WorldGateway {
                    membership: membership(),
                    world: Arc::clone(&world),
                },
                FixtureRepository::seeded(&characters(), &scenario.seed_items()),
            ),
            InMemoryDefinitionRepository::new(fixture_version(), fixture_definitions()),
            manifest,
            destiny_adapters::pinned_equipment_capacity_policy(),
            FakeClock::fixed(NOW),
            DeterministicPlanIds::starting_at(1),
        ),
        WorldPullGateway {
            world: Arc::clone(&world),
        },
        FixturePullDelay,
        TTL,
    );
    let mutation_service = ComposedMutationService::new(
        mutations,
        WorldTransferGateway {
            world: Arc::clone(&world),
            refuse_vault_route: scenario == Scenario::Partial,
        },
        FakeClock::fixed(NOW),
        TTL,
        PlanLifetime::MAX,
    )
    .with_equip(WorldEquipGateway {
        world: Arc::clone(&world),
    })
    .with_lock(WorldLockGateway { world })
    .with_pull(pull);

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
    let read_service = destiny_mcp::service::ArtifactReadService::new(
        FixtureArtifact,
        read_service,
        FakeClock::fixed(NOW),
        TTL,
    );
    let read_service = destiny_mcp::service::LoadoutsReadService::new(
        FixtureLoadouts,
        read_service,
        FakeClock::fixed(NOW),
        TTL,
    );
    ReadToolServer::with_mutation(Box::new(read_service), Box::new(mutation_service))
}

struct FixtureLoadouts;
impl destiny_app::LoadoutOperations for FixtureLoadouts {
    fn loadouts(
        &mut self,
        selector: Option<&destiny_app::CharacterId>,
        policy: FreshnessPolicy,
        now: destiny_app::Timestamp,
    ) -> Result<destiny_app::LoadoutsResult, AppError> {
        assert_eq!(policy, TTL);
        assert_eq!(now, NOW);
        if selector.is_some_and(|id| id.as_str() == "missing") {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(destiny_app::LoadoutsResult { characters: vec![] })
    }
}

struct FixtureArtifact;
impl destiny_app::ArtifactOperations for FixtureArtifact {
    fn artifact(
        &mut self,
        selector: Option<&destiny_app::CharacterId>,
        policy: FreshnessPolicy,
        now: destiny_app::Timestamp,
    ) -> Result<destiny_app::ArtifactResult, AppError> {
        assert_eq!(policy, TTL);
        assert_eq!(now, NOW);
        if selector.is_some_and(|id| id.as_str() == "missing") {
            return Err(AppError::new(ErrorCategory::ProviderValidation));
        }
        Ok(destiny_app::ArtifactResult {
            points: None,
            characters: vec![],
        })
    }
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
    let version = fixture_version();
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
                item_descriptions: Vec::new(),
                loadout_names: WorldDefinitionTable {
                    definitions: vec![],
                    skipped_count: 0,
                },
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
    refresh_failed: bool,
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

    fn load_inventory_with_observations_scoped(
        &mut self,
        membership: &DestinyMembership,
        _reason: destiny_app::ProfileSyncReason,
    ) -> Result<destiny_app::ProfileAcquisition, AppError> {
        let input = self.load_inventory(membership)?;
        let rows = input
            .items
            .iter()
            .map(|item| {
                destiny_app::InstancedBucketObservation::new(
                    item.instance_id.clone(),
                    destiny_app::BucketHash::new(
                        if matches!(item.location, ItemLocation::Postmaster(_)) {
                            215593132
                        } else {
                            1498876634
                        },
                    ),
                    item.location.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        destiny_app::ProfileAcquisition::new(input, Some(rows))
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
        if let Some(category) = match std::env::var(SCENARIO_ENV).ok().as_deref() {
            Some("postmaster-refresh-refused") => Some(ErrorCategory::ProviderUnavailable),
            Some("postmaster-auth-refused") => Some(ErrorCategory::ProviderAuthentication),
            _ => None,
        } {
            return Err(AppError::new(category));
        }
        if world.refresh_failed {
            return Err(AppError::new(
                if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("equip-auth-refresh-failure")
                {
                    ErrorCategory::ProviderAuthentication
                } else {
                    ErrorCategory::ProviderUnavailable
                },
            ));
        }
        Ok(world.snapshot_input())
    }
}

struct FixturePullDelay;
impl destiny_app::RetryDelay for FixturePullDelay {
    fn wait(&mut self, duration: std::time::Duration) {
        assert_eq!(duration, std::time::Duration::from_secs(2));
    }
}
struct WorldPullGateway {
    world: Arc<Mutex<FixtureWorld>>,
}
impl destiny_app::InstancedPullGateway for WorldPullGateway {
    fn pull(
        &mut self,
        request: &destiny_app::test_support::InstancedPullRequest,
    ) -> Result<destiny_app::InstancedPullOutcome, AppError> {
        let scenario = std::env::var(SCENARIO_ENV).unwrap_or_default();
        if scenario == "pull-gateway-error" {
            return Err(AppError::new(ErrorCategory::ProviderUnavailable));
        }
        if scenario != "pull-pending" && scenario != "pull-uncertain" {
            self.world.lock().expect("fixture world").apply(
                request.item(),
                &ItemLocation::Character(request.character().clone()),
            );
        }
        Ok(match scenario.as_str() {
            "pull-refused-arrival" => destiny_app::InstancedPullOutcome::Rejected(
                destiny_app::InstancedPullRejection::InvalidRequest,
            ),
            "pull-uncertain" => destiny_app::InstancedPullOutcome::Uncertain,
            _ => destiny_app::InstancedPullOutcome::Acknowledged,
        })
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
        if !matches!(
            std::env::var(SCENARIO_ENV).ok().as_deref(),
            Some("equip-provider-stale" | "move-provider-stale")
        ) {
            world.apply(request.item(), request.to());
        }
        Ok(TransferOutcome::Transferred)
    }
}

/// Offline equip port: mutates only the fixture world, never the provider.
struct WorldEquipGateway {
    world: Arc<Mutex<FixtureWorld>>,
}
impl destiny_app::EquipGateway for WorldEquipGateway {
    fn equip(
        &mut self,
        request: &destiny_app::EquipItemRequest,
    ) -> Result<destiny_app::EquipOutcome, AppError> {
        let scenario = std::env::var(SCENARIO_ENV).unwrap_or_default();
        if scenario == "equip-partial" {
            return Ok(destiny_app::EquipOutcome::Rejected(
                destiny_app::EquipRejection::NotInSocialSpace,
            ));
        }
        let mut world = self
            .world
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        if scenario.starts_with("equip-auth") {
            world.refresh_failed = matches!(
                scenario.as_str(),
                "equip-auth-refresh-failure" | "equip-auth-unavailable-refresh"
            );
            if scenario == "equip-auth-concurrent" {
                for item in &mut world.items {
                    if item.instance_id == *request.item() {
                        item.location = ItemLocation::Character(request.character().clone());
                        item.equipped = true;
                    }
                }
            }
            return Ok(destiny_app::EquipOutcome::AuthenticationRefused);
        }
        if !matches!(
            scenario.as_str(),
            "equip-uncertain-no-effect" | "equip-contradiction" | "equip-provider-stale"
        ) {
            for item in &mut world.items {
                if item.instance_id == *request.item() {
                    item.location = ItemLocation::Character(request.character().clone());
                    item.equipped = true;
                } else if item.location == ItemLocation::Character(request.character().clone())
                    && item.item_hash == ItemHash::new(100)
                {
                    item.equipped = false;
                }
            }
        }
        if scenario == "equip-refresh-failure" {
            world.refresh_failed = true;
        }
        Ok(if scenario.starts_with("equip-uncertain") {
            destiny_app::EquipOutcome::Uncertain
        } else {
            destiny_app::EquipOutcome::Equipped
        })
    }
}

/// Offline lock port: one bounded attempt against the shared provider world.
struct WorldLockGateway {
    world: Arc<Mutex<FixtureWorld>>,
}
impl destiny_app::LockGateway for WorldLockGateway {
    fn set_lock_state(
        &mut self,
        request: &destiny_app::SetLockStateRequest,
    ) -> Result<destiny_app::LockOutcome, AppError> {
        let scenario = std::env::var(SCENARIO_ENV).unwrap_or_default();
        if scenario == "lock-refused" {
            return Ok(destiny_app::LockOutcome::Rejected(
                destiny_app::LockRejection::NotInSocialSpace,
            ));
        }
        let mut world = self
            .world
            .lock()
            .map_err(|_| AppError::new(ErrorCategory::LocalStorage))?;
        if scenario == "lock-refresh-failure" {
            world.refresh_failed = true;
        }
        if !matches!(
            scenario.as_str(),
            "lock-provider-stale" | "lock-uncertain-no-effect"
        ) {
            for item in &mut world.items {
                if item.instance_id == *request.item() {
                    item.locked = request.locked();
                }
            }
        }
        Ok(if scenario.starts_with("lock-uncertain") {
            destiny_app::LockOutcome::Uncertain
        } else {
            destiny_app::LockOutcome::Applied
        })
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
        let version = fixture_version();
        let definitions = fixture_definitions();
        let mut repository = Self {
            inventory: InMemoryInventoryRepository::new(),
            search: InMemorySearchRepository::with_definitions(definitions.clone()),
            definitions: InMemoryDefinitionRepository::new(version, definitions),
        };
        let mut input = snapshot_input(characters, items);
        if postmaster_scenario() {
            input.checked_at = Timestamp::new(1);
        }
        let snapshot = repository
            .inventory
            .replace_current(input)
            .expect("fixture projection is valid");
        repository.search.set_instances(snapshot.items);
        repository
    }
}

impl InventoryRepository for FixtureRepository {
    fn load_current(&self) -> Result<Option<InventorySnapshot>, AppError> {
        if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("postmaster-local-storage") {
            return Err(AppError::new(ErrorCategory::LocalStorage));
        }
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

impl destiny_app::WorldDefinitionRepository for FixtureRepository {
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
    ) -> Result<destiny_app::SyncMetadata, AppError> {
        self.definitions.publish_manifest(items, world)
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
        profile_currencies_coverage: match std::env::var(SCENARIO_ENV).ok().as_deref() {
            Some("postmaster-available") => destiny_app::ProfileCurrenciesCoverage::Available,
            Some("postmaster-unavailable") => destiny_app::ProfileCurrenciesCoverage::Unavailable,
            _ => Default::default(),
        },
        stacks: if postmaster_scenario() {
            [
                (900, 215593132, 7),
                (900, 215593132, 7),
                (901, 1469714392, 3),
            ]
            .into_iter()
            .map(|(hash, bucket, quantity)| {
                destiny_app::StackObservation::new(
                    ItemHash::new(hash),
                    destiny_app::BucketHash::new(bucket),
                    quantity,
                    destiny_app::StackContainer::Postmaster(CharacterId::new("warlock")),
                )
                .expect("synthetic Postmaster stack")
            })
            .collect()
        } else {
            vec![]
        },
        plug_set_entries: Vec::new(),
        membership_id: membership.membership_id().clone(),
        membership_type: membership.membership_type(),
        definition_version: fixture_version(),
        checked_at: NOW,
        source: SourceMarker::Profile,
        characters: characters.to_vec(),
        items: items.to_vec(),
    }
}

fn characters() -> Vec<Character> {
    if pull_scenario() {
        return vec![character("2")];
    }
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("empty-characters") {
        return vec![];
    }
    let mut characters = vec![character("warlock"), character("zavala")];
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("ambiguous-character") {
        characters[0].class = Some("Titan".into());
    }
    characters
}

fn postmaster_scenario() -> bool {
    std::env::var(SCENARIO_ENV)
        .ok()
        .is_some_and(|s| s.starts_with("postmaster-"))
}

fn pull_scenario() -> bool {
    std::env::var(SCENARIO_ENV)
        .ok()
        .is_some_and(|s| s.starts_with("pull-"))
}
fn fixture_version() -> DefinitionVersion {
    DefinitionVersion::new(if pull_scenario() {
        destiny_adapters::equipment_capacity_policy::PINNED_EQUIPMENT_CAPACITY_VERSION
    } else {
        DEFINITION_VERSION
    })
}
fn base_items() -> Vec<ItemInstance> {
    if pull_scenario() {
        return vec![instance(
            "123",
            1,
            ItemLocation::Postmaster(CharacterId::new("2")),
            vec![],
        )];
    }
    if postmaster_scenario() {
        return vec![instance(
            "postmaster-gear",
            1,
            ItemLocation::Postmaster(CharacterId::new("warlock")),
            vec![],
        )];
    }
    if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("empty-characters") {
        return vec![];
    }
    if std::env::var(SCENARIO_ENV)
        .ok()
        .is_some_and(|s| s.starts_with("equip-"))
    {
        let mut candidate = instance("123", 100, ItemLocation::Vault, Vec::new());
        if std::env::var(SCENARIO_ENV).ok().as_deref() == Some("equip-noop") {
            candidate.location = at_character("warlock");
            candidate.equipped = true;
        }
        let mut ghost = instance("124", 101, at_character("warlock"), Vec::new());
        ghost.equipped = true;
        let mut subclass = instance("125", 102, at_character("warlock"), Vec::new());
        subclass.equipped = true;
        return vec![candidate, ghost, subclass];
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
    if pull_scenario() {
        let mut item = definition(1, "PRIVATE_PULL_NAME");
        item.does_postmaster_pull_have_side_effects = Some(false);
        item.is_instance_item = Some(true);
        item.bucket_hash = Some(destiny_app::BucketHash::new(1498876634));
        return vec![item];
    }
    if postmaster_scenario() {
        return vec![
            definition(1, "Gjallarhorn"),
            definition(900, "Synthetic Material"),
        ];
    }
    if std::env::var(SCENARIO_ENV)
        .ok()
        .is_some_and(|s| s.starts_with("equip-"))
    {
        let mut equipment = definition(100, "Ace of Spades");
        equipment.bucket_hash = Some(destiny_app::BucketHash::new(1498876634));
        equipment.slot = Some(destiny_app::ItemSlot::Kinetic);
        equipment.tier = Some("Exotic".into());
        equipment.class_type = Some("any".parse().unwrap());
        let mut ghost = definition(101, "Ghost");
        ghost.bucket_hash = Some(destiny_app::BucketHash::new(4023194814));
        let mut subclass = definition(102, "Subclass");
        subclass.bucket_hash = Some(destiny_app::BucketHash::new(3284755031));
        return vec![equipment, ghost, subclass];
    }
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
        date_last_played: None,
    }
}

fn definition(hash: i64, name: &str) -> ItemDefinition {
    ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
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
        version: fixture_version(),
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
