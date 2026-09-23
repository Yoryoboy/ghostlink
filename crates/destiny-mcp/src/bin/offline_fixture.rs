//! Test-only offline stdio fixture for the child-process integration test.
//!
//! This binary is compiled only with the non-default `offline-fixtures` feature
//! and is never reachable from the product `destiny-mcp` binary: it declares no
//! flag, environment branch, or module that the product binary can select. It
//! serves the same [`ReadToolServer`] over the same stdio runtime, but composes
//! the read seam with deterministic in-memory application test doubles instead
//! of adapters, so the transport contract can be exercised with no credential,
//! network, or database.
//!
//! The fixture data embeds a secret sentinel inside an opaque provider location
//! value. The MCP projection drops that text, so the integration test can assert
//! the sentinel never reaches stdout or stderr.

use destiny_app::test_support::{
    FakeClock, InMemoryDefinitionRepository, InMemoryInventoryRepository, InMemorySearchRepository,
    QueuedProfileGateway,
};
use destiny_app::{
    AppError, Character, CharacterId, DefinitionRepository, DefinitionVersion, DestinyMembership,
    FreshnessPolicy, InventoryRepository, InventorySnapshot, InventorySnapshotInput,
    ItemDefinition, ItemHash, ItemInstance, ItemInstanceId, ItemLocation, MembershipId,
    MembershipType, PerkHash, PerkSummary, ProfileSyncService, QueryService, SearchRepository,
    SourceMarker, Timestamp,
};
use destiny_mcp::runtime::serve_stdio;
use destiny_mcp::{InventoryReadService, QueryReadService, ReadToolServer};

/// The sentinel embedded in opaque provider text; the projection must drop it.
pub const SENTINEL: &str = "offline-fixture-secret-sentinel";

const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";
const DEFINITION_VERSION: &str = "fixture-definitions-v1";
const NOW: Timestamp = Timestamp::new(10_000);
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

#[tokio::main]
async fn main() {
    if let Err(error) = serve_stdio(build_read_server()).await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

/// Builds the deterministic offline read server for the child process.
fn build_read_server() -> ReadToolServer {
    let gateway = QueuedProfileGateway::new();
    let sync = ProfileSyncService::new(gateway, seed_repository());
    let queries = QueryService::new(sync);
    let service = QueryReadService::new(queries, FakeClock::fixed(NOW), TTL);
    let boxed: Box<dyn InventoryReadService + Send> = Box::new(service);
    ReadToolServer::new(boxed)
}

/// One composite repository exposing the three read seams over in-memory fakes.
#[derive(Clone, Debug, Default)]
struct FixtureRepository {
    inventory: InMemoryInventoryRepository,
    search: InMemorySearchRepository,
    definitions: InMemoryDefinitionRepository,
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
    fn search_definitions(
        &self,
        query: &destiny_app::DefinitionSearchQuery,
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
    ) -> Result<destiny_app::Resolution<ItemInstance>, AppError> {
        self.search.lookup_instance(instance_id)
    }
}

impl DefinitionRepository for FixtureRepository {
    fn current_definition_version(&self) -> Result<DefinitionVersion, AppError> {
        self.definitions.current_definition_version()
    }

    fn lookup_definition(
        &self,
        hash: ItemHash,
    ) -> Result<destiny_app::Resolution<ItemDefinition>, AppError> {
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

/// Seeds one fresh projection whose opaque provider text carries the sentinel.
fn seed_repository() -> FixtureRepository {
    let version = DefinitionVersion::new(DEFINITION_VERSION);
    let definitions = vec![definition(1, "Gjallarhorn"), definition(2, "Fatebringer")];
    let mut repository = FixtureRepository {
        inventory: InMemoryInventoryRepository::new(),
        search: InMemorySearchRepository::with_definitions(definitions.clone()),
        definitions: InMemoryDefinitionRepository::new(version, definitions),
    };

    let membership = DestinyMembership::new(
        MembershipType::new(3),
        MembershipId::new(MEMBERSHIP_IDENTIFIER),
    )
    .expect("fixture membership is valid");
    let input = InventorySnapshotInput {
        membership_id: membership.membership_id().clone(),
        membership_type: membership.membership_type(),
        definition_version: DefinitionVersion::new(DEFINITION_VERSION),
        checked_at: NOW,
        source: SourceMarker::Profile,
        characters: vec![character("warlock")],
        items: vec![
            instance("inst-a", 1, ItemLocation::Vault, vec![perk(10)]),
            instance("inst-b", 2, ItemLocation::other(SENTINEL), Vec::new()),
        ],
    };
    let snapshot = repository
        .inventory
        .replace_current(input)
        .expect("fixture projection is valid");
    repository.search.set_instances(snapshot.items);
    repository
}

fn character(id: &str) -> Character {
    Character {
        id: CharacterId::new(id),
        name: Some(format!("name-{id}")),
        class: None,
        race: None,
        gender: None,
        light: None,
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
        perks,
    }
}
