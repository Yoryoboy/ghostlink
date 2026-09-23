//! Composed, offline end-to-end mutation-planning journey.
//!
//! This integration test drives the real composed stack instead of a stand-in
//! for it: the real [`BungieProfileGateway`] decodes the deterministic provider
//! fixture, the real [`SqliteStore`] persists the committed profile and the
//! published Manifest definitions to an on-disk SQLite file, and the real
//! [`MutationService`] composes the real [`ProfileSyncService`] freshness gate,
//! the shared core mutation policy, and the durable [`SearchRepository`]
//! resolution path. Nothing at those layers is faked: definitions are published
//! through the real [`DefinitionRepository::replace_definitions`] contract, the
//! profile is committed through the real repository revision logic, and the plan
//! is built by the production planner.
//!
//! The only fakes are the two outermost seams the production code already takes
//! by dependency injection:
//!
//! - a scripted [`ProfileTransport`] that returns crafted Bungie responses and
//!   records the URLs the gateway builds, so the journey never touches the
//!   network and never needs live credentials; and
//! - a [`Clock`] whose `now` the test advances explicitly, so `checked_at`
//!   stamping and the freshness TTL are deterministic instead of wall-clock
//!   dependent.
//!
//! The plan identifier generator is also injected, because the production
//! service takes it as a seam; the journey uses the deterministic generator the
//! application already ships for tests.
//!
//! Every assertion is on public API behavior: the journey never reaches into
//! production internals, never substitutes a fake repository or gateway, and
//! never weakens an assertion to make it pass.
//!
//! Scenarios covered:
//!
//! 1. the semantic request "The Last Word to Warlock" resolves one concrete
//!    Vault item and one Warlock character and plans a single direct step;
//! 2. a semantic character-to-character request produces a two-step plan that
//!    routes through the Vault exactly once;
//! 3. `prepare_move` returns a usable plan from one explicit-intent call with no
//!    approval artifact or second call;
//! 4. planning is read-only on a fresh projection while preflight forces a
//!    refresh, and an identical refresh keeps the plan valid;
//! 5. a refreshed state change advances the revision and invalidates the plan;
//! 6. absent or ambiguous intent fails closed before any network call; and
//! 7. planning and preflight never touch a remote mutation boundary: every
//!    recorded request is a read-only membership or profile GET.
//!
//! [`BungieProfileGateway`]: destiny_adapters::profile_gateway::BungieProfileGateway
//! [`SqliteStore`]: destiny_adapters::sqlite::SqliteStore
//! [`MutationService`]: destiny_app::MutationService
//! [`ProfileSyncService`]: destiny_app::ProfileSyncService
//! [`SearchRepository`]: destiny_core::SearchRepository
//! [`DefinitionRepository::replace_definitions`]: destiny_core::DefinitionRepository::replace_definitions
//! [`ProfileTransport`]: destiny_adapters::profile_gateway::ProfileTransport
//! [`Clock`]: destiny_core::Clock

use destiny_adapters::profile_gateway::{
    BungieProfileGateway, MEMBERSHIP_ENDPOINT, ProfileApiKey, ProfileHttpRequest,
    ProfileHttpResponse, ProfileTransport, ProfileTransportError, profile_endpoint,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::test_support::DeterministicPlanIds;
use destiny_app::{MutationError, MutationService, ProfileSyncService};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    DefinitionReplacementInput, DefinitionRepository, DefinitionVersion, DestinationSelector,
    DestinyMembership, ErrorCategory, FreshnessPolicy, InventoryRepository, InventorySnapshot,
    ItemDefinition, ItemHash, ItemLocation, ItemSelector, ManifestDefinitions, MembershipId,
    MembershipType, MoveIntentAuthorization, MoveRequest, PlanLifetime, ProfileSyncReason,
    SourceMarker, SyncRevision, Timestamp,
};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The effective Cross Save primary the whole journey resolves to. It matches
/// the membership the `profile-complete.json` fixture is consistent with.
const MEMBERSHIP_TYPE: u32 = 3;
const MEMBERSHIP_ID: &str = "4611686018484639825";

/// The Manifest version shared by the published definitions and the gateway that
/// stamps the synchronized profile.
const DEFINITION_VERSION: &str = "mutation-journey-definitions-v1";

/// The two fixture characters. The Warlock class code (`2`) belongs to
/// [`WARLOCK_ID`] and the Hunter class code (`1`) belongs to [`HUNTER_ID`].
const WARLOCK_ID: &str = "900000000000000001";
const HUNTER_ID: &str = "900000000000000002";

/// The Vault-held "The Last Word" instance and the Hunter-held "Crota's Sword"
/// instance the fixture normalizes to.
const LAST_WORD_INSTANCE: &str = "700000000000000001";
const CROTA_INSTANCE: &str = "700000000000000005";

/// Deterministic application time. The gateway stamps `checked_at` from the
/// injected clock; the freshness policy below is evaluated against the explicit
/// `now` each call passes.
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const CREDENTIAL_EXPIRES_AT: u64 = 4_000_000_000;
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// Distinctive sentinels for the two secrets the gateway holds. None of them may
/// ever reach an error or the on-disk bytes.
const API_KEY_SENTINEL: &str = "sentinel-mutation-api-key";
const BEARER_SENTINEL: &str = "sentinel-mutation-bearer-token";

/// The complete profile fixture decoded by the real adapter.
const FIXTURE: &[u8] = include_bytes!("fixtures/profile-complete.json");

/// The membership discovery body: an established Cross Save relationship whose
/// primary is [`MEMBERSHIP_ID`] and which carries one overridden platform
/// record. It satisfies the strict selection rules in
/// `decode_effective_membership`.
const MEMBERSHIP_BODY: &str = r#"{
  "ErrorCode": 1,
  "ErrorStatus": "Success",
  "Message": "Ok",
  "Response": {
    "primaryMembershipId": "4611686018484639825",
    "destinyMemberships": [
      {
        "membershipType": 3,
        "membershipId": "4611686018484639825",
        "crossSaveOverride": 3,
        "applicableMembershipTypes": [3, 1],
        "isPublic": true
      },
      {
        "membershipType": 1,
        "membershipId": "4611686018484639000",
        "crossSaveOverride": 3,
        "applicableMembershipTypes": [],
        "isPublic": true
      }
    ]
  }
}"#;

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A private temporary database path plus cleanup of its SQLite sidecars.
///
/// The guard mirrors the convention already used by the profile and query
/// journeys: one unique file per test derived from the process id and an atomic
/// counter, with the database and its `-wal`/`-shm`/`-journal` sidecars removed
/// on drop.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-mutation-journey-{}-{sequence}.db",
            std::process::id()
        ));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let mut sidecar = self.path.clone().into_os_string();
            sidecar.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(sidecar));
        }
    }
}

/// The scripted transport: a queue of canned responses plus the recorded URLs
/// of the requests the real gateway built.
struct ScriptedTransport {
    responses: VecDeque<Result<ProfileHttpResponse, ProfileTransportError>>,
    recorded_urls: Vec<String>,
}

/// The owning-side view of the scripted transport. The gateway takes ownership
/// of this handle by value; the harness keeps its own `Rc` so it can inspect the
/// recorded requests and queue further responses after construction.
struct SharedTransport(Rc<RefCell<ScriptedTransport>>);

impl ProfileTransport for SharedTransport {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError> {
        let mut inner = self.0.borrow_mut();
        inner.recorded_urls.push(request.url().to_owned());
        inner
            .responses
            .pop_front()
            .unwrap_or(Err(ProfileTransportError::Unavailable))
    }
}

/// The explicit test clock. `Clock::now` takes `&self`, so the shared handle can
/// advance time between calls while the gateway keeps the same clock value.
struct TestClock(Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(self.0.get()))
    }
}

type Gateway = BungieProfileGateway<SharedTransport, TestClock>;
type Service = MutationService<Gateway, SqliteStore, DeterministicPlanIds>;

/// The composed journey harness: one temporary database, one scripted transport,
/// and one controllable clock. `service()` composes a fresh real gateway and a
/// fresh real SQLite store over those shared seams, so a test can drop the store
/// and reopen it without losing the recorded request history.
struct Journey {
    database: TempDatabase,
    transport: Rc<RefCell<ScriptedTransport>>,
    clock: Rc<Cell<u64>>,
}

impl Journey {
    fn new() -> Self {
        Self {
            database: TempDatabase::new(),
            transport: Rc::new(RefCell::new(ScriptedTransport {
                responses: VecDeque::new(),
                recorded_urls: Vec::new(),
            })),
            clock: Rc::new(Cell::new(NOW.seconds())),
        }
    }

    /// Advances the injected clock the gateway stamps `checked_at` from.
    fn at(&self, time: Timestamp) {
        self.clock.set(time.seconds());
    }

    /// Composes the real mutation service over a fresh real gateway and store.
    fn service(&self) -> Service {
        let gateway = BungieProfileGateway::new(
            SharedTransport(Rc::clone(&self.transport)),
            TestClock(Rc::clone(&self.clock)),
            credential(),
            api_key(),
            DefinitionVersion::new(DEFINITION_VERSION),
        )
        .expect("valid gateway configuration");
        let store = SqliteStore::open(self.database.path()).expect("open file store");
        MutationService::new(
            ProfileSyncService::new(gateway, store),
            DeterministicPlanIds::starting_at(1),
        )
    }

    /// Queues the Cross Save membership discovery body.
    fn queue_membership(&self) {
        self.queue_json(MEMBERSHIP_BODY.as_bytes().to_vec());
    }

    /// Queues one `200 application/json` profile body with an exact
    /// `content_length`, as the hardened adapter requires.
    fn queue_profile(&self, body: &[u8]) {
        self.queue_json(body.to_vec());
    }

    fn queue_json(&self, body: Vec<u8>) {
        self.transport
            .borrow_mut()
            .responses
            .push_back(Ok(ProfileHttpResponse::new(
                200,
                Some(body.len() as u64),
                Some("application/json".to_owned()),
                Box::new(Cursor::new(body)),
            )));
    }

    fn request_count(&self) -> usize {
        self.transport.borrow().recorded_urls.len()
    }

    fn recorded_urls(&self) -> Vec<String> {
        self.transport.borrow().recorded_urls.clone()
    }
}

fn api_key() -> ProfileApiKey {
    ProfileApiKey::new(API_KEY_SENTINEL).expect("valid API key")
}

fn credential() -> AccessCredential {
    AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(CREDENTIAL_EXPIRES_AT),
        },
        material: CredentialMaterial::from_bytes(BEARER_SENTINEL.as_bytes().to_vec()),
    }
}

fn expected_membership() -> DestinyMembership {
    DestinyMembership::new(
        MembershipType::new(MEMBERSHIP_TYPE),
        MembershipId::new(MEMBERSHIP_ID),
    )
    .expect("fixture membership is canonical")
}

/// A deterministic normalized Manifest definition.
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

/// The published definition set. It covers every retained fixture item hash, so
/// semantic item resolution can pair the matched definition with its concrete
/// instance, and it makes "The Last Word" and "Crota's Sword" unique semantic
/// targets.
fn definitions() -> Vec<ItemDefinition> {
    vec![
        definition(1001, "The Last Word"),
        definition(1002, "Gjallarhorn"),
        definition(1003, "Gjallarswift"),
        definition(2001, "Sword of Crota"),
        definition(2003, "Crota's Sword"),
        definition(3001, "Vex Mythoclast"),
        definition(3002, "Ghost Shell"),
    ]
}

/// Publishes the complete definition set through the real repository contract.
fn publish_definitions(service: &mut Service) {
    service
        .repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new(DEFINITION_VERSION),
                definitions: definitions(),
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .expect("publish the definition set");
}

fn semantic(term: &str) -> ItemSelector {
    ItemSelector::semantic(term).expect("valid semantic term")
}

fn warlock_label() -> DestinationSelector {
    DestinationSelector::character_label("Warlock").expect("valid destination label")
}

fn move_request(item: ItemSelector, destination: DestinationSelector) -> MoveRequest {
    MoveRequest {
        membership: MembershipId::new(MEMBERSHIP_ID),
        item,
        destination,
        intent: MoveIntentAuthorization::Explicit,
    }
}

/// Publishes definitions and synchronizes the projection with one ordinary
/// refresh, leaving a fresh committed revision behind.
fn seed(journey: &Journey, service: &mut Service) {
    publish_definitions(service);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync_mut()
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold refresh commits the projection");
}

/// The committed projection read back through the real repository.
fn committed(service: &Service) -> InventorySnapshot {
    service
        .repository()
        .load_current()
        .expect("repository read")
        .expect("committed projection")
}

fn location_of(snapshot: &InventorySnapshot, instance_id: &str) -> ItemLocation {
    snapshot
        .items
        .iter()
        .find(|item| item.instance_id.as_str() == instance_id)
        .expect("committed instance")
        .location
        .clone()
}

fn character_location(character_id: &str) -> ItemLocation {
    ItemLocation::Character(CharacterId::new(character_id))
}

/// Asserts that every recorded request is a read-only membership or profile GET,
/// so no remote mutation boundary was ever contacted.
fn assert_read_only_requests(urls: &[String]) {
    let profile = profile_endpoint(&expected_membership());
    for url in urls {
        assert!(
            url == MEMBERSHIP_ENDPOINT || url == &profile,
            "unexpected remote endpoint: {url}"
        );
    }
}

/// A materially changed fixture: the vaulted "The Last Word" instance carries a
/// different quantity, which is part of revision identity and therefore advances
/// the committed revision.
fn changed_profile() -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).expect("fixture is JSON");
    let items = value
        .get_mut("Response")
        .and_then(|response| response.get_mut("profileInventory"))
        .and_then(|component| component.get_mut("data"))
        .and_then(|data| data.get_mut("items"))
        .and_then(Value::as_array_mut)
        .expect("fixture carries a profileInventory items array");
    let vaulted = items
        .iter_mut()
        .find(|item| item["itemInstanceId"].as_str() == Some(LAST_WORD_INSTANCE))
        .expect("fixture carries The Last Word by its stable instance id");
    vaulted["quantity"] = json!(2);
    serde_json::to_vec(&value).expect("mutated fixture serializes")
}

/// Concatenates the raw bytes of the database file and every sidecar, so the
/// disk scan covers whichever journal mode SQLite chose.
fn database_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        if let Ok(part) = std::fs::read(PathBuf::from(sidecar)) {
            bytes.extend_from_slice(&part);
        }
    }
    bytes
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Scenario 1: the semantic request "The Last Word to Warlock" resolves one
/// concrete Vault instance and the one Warlock character and builds a single
/// direct revision-bound step, without mutating anything.
#[test]
fn semantic_last_word_plans_a_direct_vault_to_warlock_move() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    publish_definitions(&mut service);

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let plan = service
        .plan_move(
            &move_request(semantic("The Last Word"), warlock_label()),
            PlanLifetime::MAX,
            TTL,
            NOW,
        )
        .expect("semantic plan");

    assert_eq!(plan.plan_id.as_str(), "plan-1");
    assert_eq!(plan.membership.as_str(), MEMBERSHIP_ID);
    assert_eq!(plan.item.as_str(), LAST_WORD_INSTANCE);
    assert_eq!(plan.source, ItemLocation::Vault);
    assert_eq!(plan.destination, character_location(WARLOCK_ID));
    assert_eq!(plan.revision, SyncRevision::new(1));
    assert_eq!(plan.created_at(), NOW);
    assert_eq!(
        plan.expires_at(),
        Timestamp::new(NOW.seconds() + PlanLifetime::MAX.ttl_seconds())
    );
    assert_eq!(plan.step_count(), 1);
    assert_eq!(plan.steps()[0].ordinal, 1);
    assert_eq!(plan.steps()[0].from, ItemLocation::Vault);
    assert_eq!(plan.steps()[0].to, character_location(WARLOCK_ID));
    assert!(!plan.effects().via_vault);

    // Planning contacted only the read-only provider endpoints.
    assert_read_only_requests(&journey.recorded_urls());

    // Planning is read-only: the item stays in the Vault and the revision holds.
    let snapshot = committed(&service);
    assert_eq!(snapshot.metadata.revision, SyncRevision::new(1));
    assert_eq!(
        location_of(&snapshot, LAST_WORD_INSTANCE),
        ItemLocation::Vault
    );
}

/// Scenario 2: a semantic character-to-character request produces the exact
/// two-step plan that routes through the Vault exactly once, and leaves the item
/// where it started.
#[test]
fn semantic_character_to_character_move_is_vault_mediated() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    publish_definitions(&mut service);

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let plan = service
        .plan_move(
            &move_request(semantic("Crota's Sword"), warlock_label()),
            PlanLifetime::MAX,
            TTL,
            NOW,
        )
        .expect("semantic mediated plan");

    assert_eq!(plan.item.as_str(), CROTA_INSTANCE);
    assert_eq!(plan.source, character_location(HUNTER_ID));
    assert_eq!(plan.destination, character_location(WARLOCK_ID));
    assert_eq!(plan.step_count(), 2);
    assert_eq!(plan.steps()[0].from, character_location(HUNTER_ID));
    assert_eq!(plan.steps()[0].to, ItemLocation::Vault);
    assert_eq!(plan.steps()[1].from, ItemLocation::Vault);
    assert_eq!(plan.steps()[1].to, character_location(WARLOCK_ID));
    assert!(plan.effects().via_vault);

    assert_read_only_requests(&journey.recorded_urls());
    let snapshot = committed(&service);
    assert_eq!(
        location_of(&snapshot, CROTA_INSTANCE),
        character_location(HUNTER_ID)
    );
}

/// Scenario 3: one explicit-intent `prepare_move` call returns a usable plan, and
/// the returned plan is the only artifact the operation produces.
///
/// Explicit intent is the whole authorization fact: the move request carries only
/// the membership, validated selectors, and the intent fact, so there is no
/// preview token, confirmation value, approval ceremony, or second call for an
/// adapter to satisfy.
#[test]
fn explicit_intent_completes_in_one_prepare_call_without_an_approval_artifact() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    publish_definitions(&mut service);

    // One ordinary planning refresh and one forced preflight refresh.
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);

    let plan = service
        .prepare_move(
            &move_request(semantic("The Last Word"), warlock_label()),
            PlanLifetime::MAX,
            TTL,
            NOW,
        )
        .expect("single explicit-intent call returns a plan");

    assert_eq!(plan.item.as_str(), LAST_WORD_INSTANCE);
    assert_eq!(plan.destination, character_location(WARLOCK_ID));
    assert_eq!(plan.revision, SyncRevision::new(1));
    // Exactly one plan identifier was spent for the single call.
    assert_eq!(service.plan_ids().calls(), 1);
    // The ordinary plan gate and the mandatory preflight gate each refreshed
    // once, and both used only read-only endpoints.
    assert_eq!(journey.request_count(), 4);
    assert_read_only_requests(&journey.recorded_urls());
}

/// Scenario 4: planning against a fresh projection is a pure read that performs
/// no transport request, while preflight forces one refresh regardless of
/// freshness; an identical refresh keeps the plan valid on the same revision.
#[test]
fn fresh_planning_is_read_only_and_preflight_forces_a_refresh() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    assert_eq!(journey.request_count(), 2);

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan over the fresh projection");

    // The projection is fresh, so the ordinary gate skipped acquisition.
    assert_eq!(journey.request_count(), 2);
    assert_eq!(plan.revision, SyncRevision::new(1));

    // Preflight refreshes even though the projection is fresh.
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .preflight_move(&request, &plan, TTL, NOW)
        .expect("valid preflight");

    assert_eq!(journey.request_count(), 4);
    assert_read_only_requests(&journey.recorded_urls());
    // Identical refreshed content retains the revision, so the plan still holds.
    assert_eq!(committed(&service).metadata.revision, SyncRevision::new(1));
}

/// Scenario 5: a refresh whose committed state changed advances the revision and
/// invalidates the plan with a specific typed outcome instead of executing it.
#[test]
fn preflight_invalidates_a_plan_when_refreshed_state_changes() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");
    assert_eq!(plan.revision, SyncRevision::new(1));

    // The vaulted item's quantity changed, which is part of revision identity.
    let changed_at = Timestamp::new(NOW.seconds() + 10);
    journey.at(changed_at);
    journey.queue_membership();
    journey.queue_profile(&changed_profile());

    let error = service
        .preflight_move(&request, &plan, TTL, changed_at)
        .expect_err("changed state invalidates the plan");

    assert_eq!(error, MutationError::StaleRevision);
    assert_eq!(error.category(), ErrorCategory::StaleState);
    assert_eq!(committed(&service).metadata.revision, SyncRevision::new(2));
}

/// Scenario 6: absent or ambiguous intent fails closed before any network call or
/// local publication, so omitting intent can never reach the provider.
#[test]
fn prepare_move_rejects_absent_or_ambiguous_intent_before_any_network_call() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    let mut absent = move_request(semantic("The Last Word"), warlock_label());
    absent.intent = MoveIntentAuthorization::Absent;
    assert_eq!(
        service
            .prepare_move(&absent, PlanLifetime::MAX, TTL, NOW)
            .expect_err("absent intent"),
        MutationError::MissingIntent
    );

    let mut ambiguous = move_request(semantic("The Last Word"), warlock_label());
    ambiguous.intent = MoveIntentAuthorization::Ambiguous;
    assert_eq!(
        service
            .prepare_move(&ambiguous, PlanLifetime::MAX, TTL, NOW)
            .expect_err("ambiguous intent"),
        MutationError::AmbiguousIntent
    );

    assert_eq!(journey.request_count(), 0);
    assert!(
        service
            .repository()
            .load_current()
            .expect("repository read")
            .is_none()
    );
    assert_eq!(service.plan_ids().calls(), 0);
}

/// Scenario 7: neither planning nor preflight leaks a credential to disk, and the
/// on-disk database only holds the committed projection.
#[test]
fn planning_never_persists_credentials() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .preflight_move(&request, &plan, TTL, NOW)
        .expect("valid preflight");

    let bytes = database_bytes(journey.database.path());
    assert!(
        contains_bytes(&bytes, MEMBERSHIP_ID.as_bytes()),
        "the disk scan must cover committed rows"
    );
    for sentinel in [API_KEY_SENTINEL, BEARER_SENTINEL] {
        assert!(
            !contains_bytes(&bytes, sentinel.as_bytes()),
            "on-disk database leaked {sentinel}"
        );
    }
}
