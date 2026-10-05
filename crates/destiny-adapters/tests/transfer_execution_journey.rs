//! Composed, offline end-to-end item-transfer execution journey.
//!
//! This integration test drives the real composed stack instead of a stand-in
//! for it: the real [`BungieProfileGateway`] decodes the deterministic provider
//! payloads, the real [`SqliteStore`] persists the committed projection to an
//! on-disk SQLite file, the real [`QueryService`] answers list/search/inspect
//! through the real [`ProfileSyncService`] freshness gates, the real
//! [`MutationService`] composes the same freshness gates with the real core
//! mutation/execution policy, and the real [`BungieTransferGateway`] maps each
//! ordered step onto the exact documented provider POST and classifies the
//! provider envelope. Nothing at those layers is faked.
//!
//! The only fakes are the outermost seams the production code already takes by
//! dependency injection:
//!
//! - a scripted [`ProfileTransport`] that answers the membership endpoint and
//!   generates every profile body from a mutable scripted world, so the journey
//!   never touches the network and every refresh honestly reflects the current
//!   scripted state instead of a pre-baked body;
//! - a scripted [`TransferTransport`] that applies the requested move to the same
//!   scripted world before answering with a real provider envelope, so a second
//!   step sees the first step's effect and the post-action refresh can observe the
//!   moved item;
//! - a [`Clock`] whose `now` the test controls explicitly; and
//! - the deterministic plan identifier generator the application already ships.
//!
//! Every assertion is on public API behavior: the journey never reaches into
//! production internals, never substitutes a fake repository or gateway, and
//! never weakens an assertion to make it pass.
//!
//! Scenarios covered:
//!
//! 1. the named "The Last Word to Warlock" move: the real read service lists the
//!    characters and resolves the concrete Vault instance, the real planner binds
//!    that revision, the mandatory fresh preflight passes, one direct
//!    Vault-to-character transfer applies, and the forced `PostAction` refresh
//!    advances the committed revision so both the executor and an independent
//!    read-service re-inspection verify the destination, including the stale
//!    cache refresh that spends exactly one more gateway call;
//! 2. a character-to-character move posts exactly two ordered Vault-mediated
//!    transfers and verifies the final destination;
//! 3. a refused second step strands the item in the Vault and reports honest
//!    partial evidence with no rollback claim;
//! 4. a changed fresh preflight rejects before any transfer is dispatched;
//! 5. an uncertain step whose effect is confirmed by the post-action projection
//!    is a verified success;
//! 6. a post-action projection that contradicts the reported progress is a
//!    verification failure;
//! 7. a failed post-action refresh is a verification failure that preserves the
//!    bounded evidence;
//! 8. an already-satisfied fresh preflight is a verified no-op with zero
//!    transfers and no post-action refresh; and
//! 9. credentials and raw provider payloads never reach public errors, evidence,
//!    or the on-disk database.
//!
//! [`BungieProfileGateway`]: destiny_adapters::profile_gateway::BungieProfileGateway
//! [`BungieTransferGateway`]: destiny_adapters::transfer::BungieTransferGateway
//! [`SqliteStore`]: destiny_adapters::sqlite::SqliteStore
//! [`QueryService`]: destiny_app::QueryService
//! [`MutationService`]: destiny_app::MutationService
//! [`ProfileSyncService`]: destiny_app::ProfileSyncService
//! [`ProfileTransport`]: destiny_adapters::profile_gateway::ProfileTransport
//! [`TransferTransport`]: destiny_adapters::transfer::TransferTransport
//! [`Clock`]: destiny_core::Clock

use destiny_adapters::profile_gateway::{
    BungieProfileGateway, MEMBERSHIP_ENDPOINT, ProfileApiKey, ProfileHttpRequest,
    ProfileHttpResponse, ProfileTransport, ProfileTransportError, profile_endpoint,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_adapters::transfer::{
    BungieTransferGateway, TRANSFER_ENDPOINT, TransferApiKey, TransferHttpRequest,
    TransferHttpResponse, TransferTransport, TransferTransportError,
};
use destiny_app::test_support::DeterministicPlanIds;
use destiny_app::{
    MoveExecutionOutcome, MutationError, MutationService, ProfileSyncService, QueryService,
    SearchInventoryRequest, VerificationFailureReason,
};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    Dataset, DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery,
    DefinitionVersion, DestinationSelector, DestinyMembership, ErrorCategory, FreshnessPolicy,
    FreshnessStatus, InspectItemRequest, InstanceSearchQuery, InventoryRepository,
    InventorySnapshot, ItemDefinition, ItemHash, ItemHashFilter, ItemInstanceId, ItemLocation,
    ItemSelector, MAX_SEARCH_LIMIT, ManifestDefinitions, MembershipId, MembershipType,
    MoveIntentAuthorization, MoveRequest, PlanLifetime, ProfileSyncReason, Resolution, SearchTerm,
    SourceMarker, StepOutcome, SyncRevision, Timestamp, TransferRejection,
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
const DEFINITION_VERSION: &str = "transfer-journey-definitions-v1";

/// The two fixture characters. The Warlock class code (`2`) belongs to
/// [`WARLOCK_ID`] and the Hunter class code (`1`) belongs to [`HUNTER_ID`].
const WARLOCK_ID: &str = "900000000000000001";
const HUNTER_ID: &str = "900000000000000002";

/// The Vault-held "The Last Word" instance and the Hunter-held "Crota's Sword"
/// instance the fixture normalizes to.
const LAST_WORD_INSTANCE: &str = "700000000000000001";
const LAST_WORD_HASH: i64 = 1001;
const CROTA_INSTANCE: &str = "700000000000000005";

/// Deterministic application time. `checked_at` is excluded from revision
/// identity, so the same clock value is used across preflight and post-action.
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const CREDENTIAL_EXPIRES_AT: u64 = 4_000_000_000;
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// Distinctive sentinels for the two secrets and one raw-payload marker. None of
/// them may ever reach a public error, the bounded evidence, or the on-disk
/// bytes.
const API_KEY_SENTINEL: &str = "sentinel-transfer-journey-api-key";
const BEARER_SENTINEL: &str = "sentinel-transfer-journey-bearer-token";
const PAYLOAD_SENTINEL: &str = "sentinel-transfer-journey-raw-payload";

/// The complete profile fixture the scripted world is seeded from.
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

/// The exact success envelope the real transfer adapter classifies as applied.
const TRANSFER_SUCCESS: &str = "{\"ErrorCode\":1,\"ErrorStatus\":\"Success\",\"Response\":{}}";

fn warlock_location() -> ItemLocation {
    ItemLocation::Character(CharacterId::new(WARLOCK_ID))
}

fn hunter_location() -> ItemLocation {
    ItemLocation::Character(CharacterId::new(HUNTER_ID))
}

fn expected_membership() -> DestinyMembership {
    DestinyMembership::new(
        MembershipType::new(MEMBERSHIP_TYPE),
        MembershipId::new(MEMBERSHIP_ID),
    )
    .expect("fixture membership is canonical")
}

/// Asserts that every recorded profile request is a read-only membership or
/// profile GET, so the read path never contacted another remote endpoint.
fn assert_read_only_profile_requests(urls: &[String]) {
    let profile = profile_endpoint(&expected_membership());
    for url in urls {
        assert!(
            url == MEMBERSHIP_ENDPOINT || url == &profile,
            "unexpected remote endpoint: {url}"
        );
    }
}

/// One entry of the scripted provider transfer behaviour.
///
/// A step is applied to the scripted world exactly when its declared effect
/// landed, so a later step and the post-action refresh observe the first step's
/// effect. The returned response is a real provider envelope (or transport
/// failure) that the real [`BungieTransferGateway`] classifies; nothing here
/// bypasses the adapter's mapping.
#[derive(Clone, Copy, Debug)]
enum TransferAction {
    /// The provider applies the move and answers with the success envelope.
    Applied,
    /// Received HTTP authentication refusal: attempted, never applied.
    AuthenticationRefused(u16),
    /// Independent game action reaches the target while this call is refused.
    AuthenticationRefusedConcurrent,
    /// The provider applies the move, then the next profile refresh fails at the
    /// transport, modelling a lost post-action observation.
    AppliedThenPostActionFails,
    /// The provider answers with the success envelope but the scripted state does
    /// not move, modelling affinity or cache lag.
    ReportWithoutApplying,
    /// Confirmed transfer, but a concurrent action places the item elsewhere.
    Contradiction,
    /// The provider deterministically refuses with the mapped transfer status.
    Refused(TransferRejection),
    /// A dispatched request returns `5xx`; `lands` declares whether it applied.
    ServerError { lands: bool },
    /// A success-shaped envelope that carries the raw-payload sentinel in fields
    /// the adapter must never surface; the classification is uncertain.
    SentinelEnvelope,
}

impl TransferAction {
    /// True when this action moves the scripted item to its requested endpoint.
    const fn lands(self) -> bool {
        match self {
            Self::Applied
            | Self::AppliedThenPostActionFails
            | Self::AuthenticationRefusedConcurrent => true,
            Self::ReportWithoutApplying
            | Self::Contradiction
            | Self::Refused(_)
            | Self::SentinelEnvelope
            | Self::AuthenticationRefused(_) => false,
            Self::ServerError { lands } => lands,
        }
    }
}

/// One profile response override, consumed by the next profile-endpoint request.
#[derive(Clone, Copy, Debug)]
enum ProfileFault {
    /// The profile refresh fails at the transport.
    Unavailable,
    /// The profile refresh returns a raw-payload sentinel envelope that the real
    /// decoder rejects.
    Sentinel,
}

/// The scripted provider world: a mutable Bungie profile body whose item
/// locations are the source of truth for every generated profile response.
///
/// Items are re-placed exactly as the profile decoder expects: a Vault item is a
/// profile-inventory entry carrying `location: 2`, while a character item is a
/// character-inventory entry with no explicit location. Moving an item therefore
/// changes the normalized projection and advances the committed revision, which
/// is what makes preflight invalidation and post-action verification honest.
struct World {
    value: Value,
}

impl World {
    fn new() -> Self {
        Self {
            value: serde_json::from_slice(FIXTURE).expect("fixture is JSON"),
        }
    }

    fn profile_body(&self) -> Vec<u8> {
        serde_json::to_vec(&self.value).expect("scripted world serializes")
    }

    fn profile_items(&mut self) -> &mut Vec<Value> {
        self.value
            .pointer_mut("/Response/profileInventory/data/items")
            .and_then(Value::as_array_mut)
            .expect("profile inventory items")
    }

    fn character_items(&mut self, character_id: &str) -> &mut Vec<Value> {
        self.value
            .get_mut("Response")
            .and_then(|response| response.get_mut("characterInventories"))
            .and_then(|component| component.get_mut("data"))
            .and_then(|data| data.get_mut(character_id))
            .and_then(|group| group.get_mut("items"))
            .and_then(Value::as_array_mut)
            .expect("character inventory items")
    }

    fn character_ids(&self) -> Vec<String> {
        self.value
            .pointer("/Response/characterInventories/data")
            .and_then(Value::as_object)
            .map(|data| data.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Removes one tracked item from wherever it currently lives.
    fn take_item(&mut self, instance_id: &str) -> Value {
        let index = self.profile_items().iter().position(|item| {
            item.get("itemInstanceId").and_then(Value::as_str) == Some(instance_id)
        });
        if let Some(index) = index {
            return self.profile_items().remove(index);
        }
        for character_id in self.character_ids() {
            let index = self.character_items(&character_id).iter().position(|item| {
                item.get("itemInstanceId").and_then(Value::as_str) == Some(instance_id)
            });
            if let Some(index) = index {
                return self.character_items(&character_id).remove(index);
            }
        }
        panic!("scripted world does not track item {instance_id}");
    }

    fn move_to_vault(&mut self, instance_id: &str) {
        let mut item = self.take_item(instance_id);
        if let Some(object) = item.as_object_mut() {
            object.insert("location".to_owned(), json!(2));
        }
        self.profile_items().push(item);
    }

    fn move_to_character(&mut self, instance_id: &str, character_id: &str) {
        let mut item = self.take_item(instance_id);
        if let Some(object) = item.as_object_mut() {
            object.remove("location");
        }
        self.character_items(character_id).push(item);
    }

    /// Applies the exact direction the real adapter encoded on the wire.
    fn apply_transfer_body(&mut self, body: &str) {
        let value: Value = serde_json::from_str(body).expect("transfer body is JSON");
        let item_id = value
            .get("itemId")
            .and_then(Value::as_str)
            .expect("itemId is a string")
            .to_owned();
        if value
            .get("transferToVault")
            .and_then(Value::as_bool)
            .expect("transferToVault is a boolean")
        {
            self.move_to_vault(&item_id);
        } else {
            let character_id = value
                .get("characterId")
                .and_then(Value::as_str)
                .expect("characterId is a string")
                .to_owned();
            self.move_to_character(&item_id, &character_id);
        }
    }
}

/// One transfer POST recorded off the real adapter's request.
#[derive(Clone, Debug)]
struct RecordedTransfer {
    url: String,
    body: String,
}

#[derive(Default)]
struct TransferScript {
    actions: VecDeque<TransferAction>,
    recorded: Vec<RecordedTransfer>,
}

/// The scripted profile transport backing the real gateway.
struct SharedProfileTransport {
    world: Rc<RefCell<World>>,
    requests: Rc<RefCell<Vec<String>>>,
    authorized: Rc<RefCell<Vec<bool>>>,
    faults: Rc<RefCell<VecDeque<ProfileFault>>>,
}

impl ProfileTransport for SharedProfileTransport {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError> {
        let url = request.url().to_owned();
        self.requests.borrow_mut().push(url.clone());
        // Record only whether the injected fixture authorization was attached;
        // the secret values are never captured or rendered.
        self.authorized
            .borrow_mut()
            .push(request.api_key().is_some() && request.bearer().is_some());
        if url == MEMBERSHIP_ENDPOINT {
            return Ok(profile_json(MEMBERSHIP_BODY.as_bytes().to_vec()));
        }
        // Any non-membership request is the profile endpoint the gateway built.
        let fault = self.faults.borrow_mut().pop_front();
        match fault {
            Some(ProfileFault::Unavailable) => Err(ProfileTransportError::Unavailable),
            Some(ProfileFault::Sentinel) => Ok(profile_json(sentinel_profile_body())),
            None => {
                let body = self.world.borrow().profile_body();
                Ok(profile_json(body))
            }
        }
    }
}

/// The scripted transfer transport backing the real gateway.
struct SharedTransferTransport {
    world: Rc<RefCell<World>>,
    faults: Rc<RefCell<VecDeque<ProfileFault>>>,
    script: Rc<RefCell<TransferScript>>,
}

impl TransferTransport for SharedTransferTransport {
    fn post(
        &mut self,
        request: &TransferHttpRequest<'_>,
    ) -> Result<TransferHttpResponse, TransferTransportError> {
        let action = self
            .script
            .borrow_mut()
            .actions
            .pop_front()
            .expect("the executor dispatched more transfer steps than the journey scripted");
        self.script.borrow_mut().recorded.push(RecordedTransfer {
            url: request.url().to_owned(),
            body: request.body().to_owned(),
        });
        if action.lands() {
            self.world.borrow_mut().apply_transfer_body(request.body());
        }
        match action {
            TransferAction::Applied => Ok(transfer_json(TRANSFER_SUCCESS.to_owned())),
            TransferAction::AppliedThenPostActionFails => {
                self.faults
                    .borrow_mut()
                    .push_back(ProfileFault::Unavailable);
                Ok(transfer_json(TRANSFER_SUCCESS.to_owned()))
            }
            TransferAction::AuthenticationRefused(status) => Ok(plain_response(status)),
            TransferAction::AuthenticationRefusedConcurrent => Ok(plain_response(403)),
            TransferAction::ReportWithoutApplying => Ok(transfer_json(TRANSFER_SUCCESS.to_owned())),
            TransferAction::Contradiction => {
                self.world
                    .borrow_mut()
                    .move_to_character(LAST_WORD_INSTANCE, HUNTER_ID);
                Ok(transfer_json(TRANSFER_SUCCESS.to_owned()))
            }
            TransferAction::Refused(TransferRejection::InvalidRequest) => Ok(plain_response(400)),
            TransferAction::Refused(rejection) => Ok(transfer_json(refusal_envelope(rejection))),
            TransferAction::ServerError { .. } => Ok(plain_response(500)),
            TransferAction::SentinelEnvelope => Ok(transfer_json(sentinel_transfer_body())),
        }
    }
}

/// The explicit test clock. `Clock::now` takes `&self`, so the shared handle can
/// advance time while the gateways keep the same clock value.
struct TestClock(Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(self.0.get()))
    }
}

type ProfileAdapter = BungieProfileGateway<SharedProfileTransport, TestClock>;
type TransferAdapter = BungieTransferGateway<SharedTransferTransport, TestClock>;
type ReadService = QueryService<ProfileAdapter, SqliteStore>;
type Service = MutationService<ProfileAdapter, SqliteStore, DeterministicPlanIds>;

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-transfer-journey-{}-{sequence}.db",
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

/// The composed journey harness: one temporary database, one scripted world here,
/// and one controllable clock. `service()` and `transfer_adapter()` compose fresh
/// real adapters over the same shared seams, so a dropped store can be reopened
/// without losing the recorded history.
struct Journey {
    database: TempDatabase,
    clock: Rc<Cell<u64>>,
    world: Rc<RefCell<World>>,
    profile_requests: Rc<RefCell<Vec<String>>>,
    profile_authorized: Rc<RefCell<Vec<bool>>>,
    profile_faults: Rc<RefCell<VecDeque<ProfileFault>>>,
    script: Rc<RefCell<TransferScript>>,
}

impl Journey {
    fn new() -> Self {
        Self {
            database: TempDatabase::new(),
            clock: Rc::new(Cell::new(NOW.seconds())),
            world: Rc::new(RefCell::new(World::new())),
            profile_requests: Rc::new(RefCell::new(Vec::new())),
            profile_authorized: Rc::new(RefCell::new(Vec::new())),
            profile_faults: Rc::new(RefCell::new(VecDeque::new())),
            script: Rc::new(RefCell::new(TransferScript::default())),
        }
    }

    /// Advances the injected clock the gateways read.
    fn at(&self, time: Timestamp) {
        self.clock.set(time.seconds());
    }

    /// Composes the real mutation service over a fresh real profile gateway and
    /// a fresh real SQLite store.
    fn service(&self) -> Service {
        let store = SqliteStore::open(self.database.path()).expect("open file store");
        MutationService::new(
            ProfileSyncService::new(self.profile_gateway(), store),
            DeterministicPlanIds::starting_at(1),
        )
    }

    /// Composes the real read application service over the same durable SQLite
    /// path the mutation service uses, so search and inspection observe exactly
    /// the committed projection a mutation acts on.
    fn read_service(&self) -> ReadService {
        let store = SqliteStore::open(self.database.path()).expect("open file store");
        QueryService::new(ProfileSyncService::new(self.profile_gateway(), store))
    }

    /// The real profile gateway over the shared world, clock, and fault script.
    fn profile_gateway(&self) -> ProfileAdapter {
        BungieProfileGateway::new(
            SharedProfileTransport {
                world: Rc::clone(&self.world),
                requests: Rc::clone(&self.profile_requests),
                authorized: Rc::clone(&self.profile_authorized),
                faults: Rc::clone(&self.profile_faults),
            },
            TestClock(Rc::clone(&self.clock)),
            destiny_core::StaticCredentialSource::new(credential()),
            ProfileApiKey::new(API_KEY_SENTINEL).expect("valid API key"),
            DefinitionVersion::new(DEFINITION_VERSION),
        )
        .expect("valid profile gateway configuration")
    }

    /// Composes the real Bungie transfer adapter over the scripted transport.
    fn transfer_adapter(&self) -> TransferAdapter {
        BungieTransferGateway::new(
            SharedTransferTransport {
                world: Rc::clone(&self.world),
                faults: Rc::clone(&self.profile_faults),
                script: Rc::clone(&self.script),
            },
            TestClock(Rc::clone(&self.clock)),
            destiny_core::StaticCredentialSource::new(credential()),
            TransferApiKey::new(API_KEY_SENTINEL).expect("valid transfer API key"),
        )
        .expect("valid transfer gateway configuration")
    }

    fn queue_transfer(&self, action: TransferAction) {
        self.script.borrow_mut().actions.push_back(action);
    }

    fn queue_profile_fault(&self, fault: ProfileFault) {
        self.profile_faults.borrow_mut().push_back(fault);
    }

    fn move_to_character(&self, instance_id: &str, character_id: &str) {
        self.world
            .borrow_mut()
            .move_to_character(instance_id, character_id);
    }

    fn transfer_calls(&self) -> usize {
        self.script.borrow().recorded.len()
    }

    fn recorded_transfers(&self) -> Vec<RecordedTransfer> {
        self.script.borrow().recorded.clone()
    }

    fn request_count(&self) -> usize {
        self.profile_requests.borrow().len()
    }

    /// The recorded profile/membership request URLs, in dispatch order.
    fn profile_urls(&self) -> Vec<String> {
        self.profile_requests.borrow().clone()
    }

    /// True when every recorded request carried the injected fixture
    /// authorization, proving the read path never reached a provider without a
    /// credential and never needed a live or OS-keyring one.
    fn all_profile_requests_authorized(&self) -> bool {
        self.profile_authorized
            .borrow()
            .iter()
            .all(|authorized| *authorized)
    }
}

fn credential() -> AccessCredential {
    AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(CREDENTIAL_EXPIRES_AT),
        },
        material: CredentialMaterial::from_bytes(BEARER_SENTINEL.as_bytes().to_vec()),
        refresh_grant: None,
    }
}

fn profile_json(body: Vec<u8>) -> ProfileHttpResponse {
    let length = body.len() as u64;
    ProfileHttpResponse::new(
        200,
        Some(length),
        Some("application/json".to_owned()),
        Box::new(Cursor::new(body)),
    )
}

fn transfer_json(body: String) -> TransferHttpResponse {
    let length = body.len() as u64;
    TransferHttpResponse::new(
        200,
        Some(length),
        Some("application/json".to_owned()),
        Box::new(Cursor::new(body.into_bytes())),
    )
}

fn plain_response(status: u16) -> TransferHttpResponse {
    TransferHttpResponse::new(status, None, None, Box::new(Cursor::new(Vec::new())))
}

fn refusal_envelope(rejection: TransferRejection) -> String {
    let status = match rejection {
        TransferRejection::NotTransferable => "NotTransferrable",
        TransferRejection::Equipped => "ItemIsEquipped",
        TransferRejection::DestinationFull => "NoRoomInDestination",
        // The `InvalidRequest` refusal is carried by an HTTP status instead.
        TransferRejection::InvalidRequest => "InvalidRequest",
    };
    format!("{{\"ErrorCode\":5,\"ErrorStatus\":\"{status}\",\"Response\":{{}}}}")
}

fn sentinel_transfer_body() -> String {
    format!(
        "{{\"ErrorCode\":9,\"ErrorStatus\":\"unknown-transfer-status\",\
         \"Message\":\"{PAYLOAD_SENTINEL}\",\"Response\":{{}}}}"
    )
}

fn sentinel_profile_body() -> Vec<u8> {
    format!(
        "{{\"ErrorCode\":7,\"ErrorStatus\":\"PrivacyRestriction\",\
         \"Message\":\"{PAYLOAD_SENTINEL}\",\"Response\":{{}}}}"
    )
    .into_bytes()
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

/// The published definition set. It makes "The Last Word" and "Crota's Sword"
/// unique semantic targets so resolution can pair a definition with its concrete
/// instance.
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

fn term(value: &str) -> SearchTerm {
    SearchTerm::new(value).expect("valid search term")
}

fn definition_query(text: Option<&str>, limit: usize) -> DefinitionSearchQuery {
    DefinitionSearchQuery {
        tier: None,
        text: text.map(term),
        item_type: None,
        tags: Vec::new(),
        element: None,
        slot: None,
        class_type: None,
        limit,
    }
}

fn instance_query(item_hashes: ItemHashFilter, limit: usize) -> InstanceSearchQuery {
    InstanceSearchQuery {
        sort: None,
        location: None,
        equipped: None,
        include_available: false,
        item_hashes,
        perk_hash: None,
        perk_name: None,
        mod_name: None,
        archetype: None,
        stats: Vec::new(),
        character_id: None,
        definitions: None,
        min_power: None,
        limit,
    }
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
    service
        .sync_mut()
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold refresh commits the projection");
    assert_eq!(journey.request_count(), 2, "membership + profile");
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

fn body_json(recorded: &RecordedTransfer) -> Value {
    serde_json::from_str(&recorded.body).expect("transfer body is JSON")
}

/// Scenario 1: the named "The Last Word to Warlock" move composed end to end.
///
/// The real read service resolves the concrete Vault instance by definition name
/// and by instance identity over the same durable SQLite path the mutation path
/// uses; the planner binds that exact revision and the named destination; the
/// mandatory fresh preflight passes; one direct Vault-to-Warlock transfer
/// applies; the forced `PostAction` refresh commits the destination and advances
/// the revision; an independent read-service re-inspection verifies the Warlock
/// placement from the fresh cache without a gateway call; and a stale
/// re-inspection then spends exactly one ordinary refresh and still observes the
/// Warlock placement at the retained revision.
#[test]
fn direct_move_is_verified_after_a_post_action_refresh() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut read = journey.read_service();
    let mut transfer = journey.transfer_adapter();

    // The read service lists the committed characters with repository-owned
    // metadata, resolved from the projected Cross Save primary.
    let characters = read
        .list_characters(TTL, NOW)
        .expect("cold query reads the committed projection");
    let ids: Vec<&str> = characters
        .characters
        .iter()
        .map(|character| character.id.as_str())
        .collect();
    assert_eq!(ids, [WARLOCK_ID, HUNTER_ID]);
    assert_eq!(characters.metadata.sync.dataset, Dataset::ProfileInventory);
    assert_eq!(characters.metadata.sync.source, SourceMarker::Profile);
    assert_eq!(characters.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(characters.metadata.sync.checked_at, NOW);
    assert_eq!(characters.metadata.freshness, FreshnessStatus::Fresh);

    // Definition search and concrete instance search pair the named definition
    // with the one Vault-held instance, and inspection loses neither identity.
    let search = read
        .search_inventory(
            &SearchInventoryRequest {
                definitions: definition_query(Some("the last word"), MAX_SEARCH_LIMIT),
                instances: Some(instance_query(
                    ItemHashFilter::AnyOf(vec![ItemHash::new(LAST_WORD_HASH)]),
                    MAX_SEARCH_LIMIT,
                )),
            },
            TTL,
            NOW,
        )
        .expect("search resolves the named item");
    assert_eq!(search.candidates.len(), 1);
    assert_eq!(search.candidates[0].name, "The Last Word");
    assert_eq!(search.hits.len(), 1);
    assert_eq!(
        search.hits[0].instance.instance_id.as_str(),
        LAST_WORD_INSTANCE
    );
    assert_eq!(search.hits[0].instance.location, ItemLocation::Vault);
    assert_eq!(
        search.hits[0].definition.hash,
        search.hits[0].instance.item_hash
    );

    let resolution = read
        .inspect_item(
            &InspectItemRequest {
                instance_id: ItemInstanceId::new(LAST_WORD_INSTANCE),
            },
            TTL,
            NOW,
        )
        .expect("inspection resolves the instance");
    let inspection = match resolution {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected one concrete instance, got {other:?}"),
    };
    assert_eq!(inspection.definition.hash, ItemHash::new(LAST_WORD_HASH));
    assert_eq!(inspection.definition.name, "The Last Word");
    assert_eq!(inspection.instance.location, ItemLocation::Vault);
    assert_eq!(inspection.metadata.sync.revision, SyncRevision::new(1));

    // Every read stayed on the allowlisted membership/profile GETs, and every
    // one carried the injected fixture authorization instead of a live one.
    assert_read_only_profile_requests(&journey.profile_urls());
    assert!(journey.all_profile_requests_authorized());

    // The planner resolves the named item and the Warlock itself and binds the
    // committed revision into a single direct step.
    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("direct plan");
    assert_eq!(plan.plan_id.as_str(), "plan-1");
    assert_eq!(plan.membership.as_str(), MEMBERSHIP_ID);
    assert_eq!(plan.item.as_str(), LAST_WORD_INSTANCE);
    assert_eq!(plan.source, ItemLocation::Vault);
    assert_eq!(plan.destination, warlock_location());
    assert_eq!(plan.revision, SyncRevision::new(1));
    assert_eq!(plan.step_count(), 1);
    assert!(!plan.effects().via_vault);
    let destiny_core::StepAction::Transfer { from, to } = &plan.steps()[0].action else {
        panic!("expected transfer");
    };
    assert_eq!(from, &ItemLocation::Vault);
    assert_eq!(to, &warlock_location());

    // The mandatory fresh preflight forces one refresh even though the read
    // projection was fresh, and the unchanged state keeps the bound revision.
    service
        .preflight_move(&request, &plan, TTL, NOW)
        .expect("the plan survives the mandatory fresh preflight");
    assert_eq!(committed(&service).metadata.revision, SyncRevision::new(1));
    assert_eq!(
        journey.request_count(),
        4,
        "seed plus the explicit fresh preflight"
    );

    journey.queue_transfer(TransferAction::Applied);

    // Execution runs its own mandatory fresh preflight, dispatches exactly one
    // ordered transfer, then forces the post-action refresh.
    let post_action_now = Timestamp::new(NOW.seconds() + 2);
    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            post_action_now,
        )
        .expect("verified outcome");
    let evidence = match outcome {
        MoveExecutionOutcome::VerifiedSuccess(evidence) => evidence,
        other => panic!("expected verified success, got {other:?}"),
    };
    assert_eq!(evidence.completed_count(), 1);
    assert_eq!(evidence.attempted_count(), 1);
    assert_eq!(evidence.unattempted_count(), 0);
    assert_eq!(evidence.steps()[0].outcome, StepOutcome::Completed);
    assert_eq!(service.plan_ids().calls(), 1);

    // The post-action refresh committed the moved item at the destination, which
    // is what advances the revision from the preflight observation.
    let snapshot = committed(&service);
    assert_eq!(
        location_of(&snapshot, LAST_WORD_INSTANCE),
        warlock_location()
    );
    assert_eq!(snapshot.metadata.revision, SyncRevision::new(2));
    let requests_after_execution = journey.request_count();
    assert_eq!(
        requests_after_execution, 8,
        "seed, explicit preflight, execution preflight, post-action"
    );

    // A fresh re-inspection is served from the committed projection without
    // spending a gateway call, and still places the item at the Warlock.
    let re_read = read
        .inspect_item(
            &InspectItemRequest {
                instance_id: ItemInstanceId::new(LAST_WORD_INSTANCE),
            },
            TTL,
            post_action_now,
        )
        .expect("fresh re-inspection succeeds");
    let verified = match re_read {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected one concrete instance, got {other:?}"),
    };
    assert_eq!(verified.instance.location, warlock_location());
    assert_eq!(verified.metadata.sync.revision, SyncRevision::new(2));
    assert_eq!(verified.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(journey.request_count(), requests_after_execution);

    // Once the cached observation is stale, the read path refreshes exactly once
    // and still reads the Warlock at the retained revision 2.
    let stale_now = Timestamp::new(post_action_now.seconds() + TTL.ttl_seconds + 1);
    journey.at(stale_now);
    let stale = read
        .inspect_item(
            &InspectItemRequest {
                instance_id: ItemInstanceId::new(LAST_WORD_INSTANCE),
            },
            TTL,
            stale_now,
        )
        .expect("stale re-inspection refreshes once");
    let stale_inspection = match stale {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected one concrete instance, got {other:?}"),
    };
    assert_eq!(stale_inspection.instance.location, warlock_location());
    assert_eq!(
        stale_inspection.metadata.sync.revision,
        SyncRevision::new(2)
    );
    assert_eq!(stale_inspection.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(
        journey.request_count(),
        requests_after_execution + 2,
        "one stale ordinary refresh"
    );

    // Exactly one ordered POST, with the Vault-to-character direction the
    // executor derived from the projection.
    assert_eq!(journey.transfer_calls(), 1);
    let recorded = journey.recorded_transfers();
    assert_eq!(recorded[0].url, TRANSFER_ENDPOINT);
    let body = body_json(&recorded[0]);
    assert_eq!(
        body.get("transferToVault").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        body.get("characterId").and_then(Value::as_str),
        Some(WARLOCK_ID)
    );
    assert_eq!(
        body.get("itemId").and_then(Value::as_str),
        Some(LAST_WORD_INSTANCE)
    );
    assert_read_only_profile_requests(&journey.profile_urls());
    assert!(journey.all_profile_requests_authorized());
}

/// Scenario 2: a character-to-character move posts exactly two ordered
/// Vault-mediated transfers, and the post-action refresh verifies the item at
/// the final character.
#[test]
fn character_to_character_move_posts_exactly_two_ordered_transfers() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("Crota's Sword"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("mediated plan");
    assert_eq!(plan.step_count(), 2);
    assert!(plan.effects().via_vault);
    assert_eq!(plan.source, hunter_location());
    assert_eq!(plan.destination, warlock_location());

    // Step 1: Hunter -> Vault applies. Step 2: Vault -> Warlock applies.
    journey.queue_transfer(TransferAction::Applied);
    journey.queue_transfer(TransferAction::Applied);

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("verified outcome");
    let evidence = match outcome {
        MoveExecutionOutcome::VerifiedSuccess(evidence) => evidence,
        other => panic!("expected verified success, got {other:?}"),
    };
    assert_eq!(evidence.completed_count(), 2);
    assert_eq!(evidence.steps()[0].outcome, StepOutcome::Completed);
    assert_eq!(evidence.steps()[1].outcome, StepOutcome::Completed);

    let snapshot = committed(&service);
    assert_eq!(location_of(&snapshot, CROTA_INSTANCE), warlock_location());
    assert_eq!(snapshot.metadata.revision, SyncRevision::new(2));

    // Exactly two ordered POSTs through the Vault, in plan order.
    assert_eq!(journey.transfer_calls(), 2);
    let recorded = journey.recorded_transfers();
    let first = body_json(&recorded[0]);
    assert_eq!(recorded[0].url, TRANSFER_ENDPOINT);
    assert_eq!(
        first.get("transferToVault").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        first.get("characterId").and_then(Value::as_str),
        Some(HUNTER_ID)
    );
    assert_eq!(
        first.get("itemId").and_then(Value::as_str),
        Some(CROTA_INSTANCE)
    );
    let second = body_json(&recorded[1]);
    assert_eq!(recorded[1].url, TRANSFER_ENDPOINT);
    assert_eq!(
        second.get("transferToVault").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        second.get("characterId").and_then(Value::as_str),
        Some(WARLOCK_ID)
    );
    assert_eq!(
        second.get("itemId").and_then(Value::as_str),
        Some(CROTA_INSTANCE)
    );
}

/// Scenario 3: a refused second step strands the item in the Vault and reports
/// honest partial evidence: the bounded record proves step one applied, the
/// post-action projection places the item at the Vault, and no rollback is
/// claimed.
#[test]
fn rc003_move_auth_concurrent_target_remains_non_success() {
    let journey = Journey::new();
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();
    let request = move_request(semantic("Crota's Sword"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .unwrap();
    journey.queue_transfer(TransferAction::Applied);
    journey.queue_transfer(TransferAction::AuthenticationRefusedConcurrent);
    let before = journey.request_count();
    let MoveExecutionOutcome::IncompleteExecution(incomplete) = service
        .execute_move_verified(&mut transfer, &request, &plan, TTL, NOW, NOW)
        .unwrap()
    else {
        panic!("authentication refusal cannot become verified success");
    };
    assert_eq!(
        incomplete.error(),
        Some(MutationError::Application(
            ErrorCategory::ProviderAuthentication
        ))
    );
    assert_eq!(incomplete.observed(), &warlock_location());
    assert_eq!(incomplete.evidence().completed_count(), 1);
    assert_eq!(incomplete.evidence().attempted_count(), 2);
    assert_eq!(
        incomplete.stop_outcome(),
        Some(StepOutcome::AuthenticationRefused)
    );
    assert_eq!(journey.transfer_calls(), 2);
    assert_eq!(journey.request_count() - before, 4);
}

#[test]
fn rc003_move_auth_retains_progress_stops_and_refreshes() {
    for status in [401, 403] {
        for prior_completed in [false, true] {
            let journey = Journey::new();
            let mut service = journey.service();
            seed(&journey, &mut service);
            let mut transfer = journey.transfer_adapter();
            let request = move_request(semantic("Crota's Sword"), warlock_label());
            let plan = service
                .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
                .unwrap();
            if prior_completed {
                journey.queue_transfer(TransferAction::Applied);
            }
            journey.queue_transfer(TransferAction::AuthenticationRefused(status));
            let before = journey.request_count();
            let MoveExecutionOutcome::IncompleteExecution(incomplete) = service
                .execute_move_verified(&mut transfer, &request, &plan, TTL, NOW, NOW)
                .unwrap()
            else {
                panic!("auth refusal not applied");
            };
            let completed = usize::from(prior_completed);
            assert_eq!(incomplete.evidence().attempted_count(), completed + 1);
            assert_eq!(incomplete.evidence().completed_count(), completed);
            assert_eq!(incomplete.evidence().unattempted_count(), 1 - completed);
            assert_eq!(
                incomplete.stop_outcome(),
                Some(StepOutcome::AuthenticationRefused)
            );
            assert_eq!(
                incomplete.error(),
                Some(MutationError::Application(
                    ErrorCategory::ProviderAuthentication
                ))
            );
            assert_eq!(journey.transfer_calls(), completed + 1);
            assert_eq!(journey.request_count() - before, 4);
            let expected = if prior_completed {
                ItemLocation::Vault
            } else {
                hunter_location()
            };
            assert_eq!(incomplete.observed(), &expected);
        }
    }
}

#[test]
fn refused_second_step_strands_the_item_in_the_vault() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("Crota's Sword"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("mediated plan");

    journey.queue_transfer(TransferAction::Applied);
    journey.queue_transfer(TransferAction::Refused(TransferRejection::DestinationFull));

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("classified outcome");
    let incomplete = match outcome {
        MoveExecutionOutcome::IncompleteExecution(incomplete) => incomplete,
        other => panic!("expected an incomplete execution, got {other:?}"),
    };
    assert!(incomplete.is_partial());
    assert_eq!(incomplete.error(), None);
    assert_eq!(incomplete.observed(), &ItemLocation::Vault);
    assert_eq!(
        incomplete.stop_outcome(),
        Some(StepOutcome::Rejected(TransferRejection::DestinationFull))
    );
    assert_eq!(incomplete.evidence().completed_count(), 1);
    assert_eq!(incomplete.evidence().attempted_count(), 2);

    // The item is honestly stranded in the Vault, and step one remains applied.
    assert_eq!(
        location_of(&committed(&service), CROTA_INSTANCE),
        ItemLocation::Vault
    );
    assert_eq!(journey.transfer_calls(), 2);
}

/// Scenario 4: a plan whose freshly refreshed state changed is rejected before
/// any transfer is dispatched, so a stale plan can never mutate the provider.
#[test]
fn changed_fresh_preflight_rejects_before_any_transfer() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");
    assert_eq!(plan.revision, SyncRevision::new(1));

    // The vaulted item left the Vault after the plan was built, which is a
    // content change and advances the committed revision.
    journey.move_to_character(LAST_WORD_INSTANCE, HUNTER_ID);

    let failure = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect_err("changed state invalidates the plan");
    assert_eq!(failure.error(), MutationError::StaleRevision);
    assert!(!failure.requires_post_action_refresh());
    assert_eq!(failure.evidence().attempted_count(), 0);

    // No transfer reached the provider, and no post-action refresh was spent.
    assert_eq!(journey.transfer_calls(), 0);
    assert_eq!(journey.request_count(), 2 + 2);
    assert_eq!(committed(&service).metadata.revision, SyncRevision::new(2));
}

/// Scenario 5: a step whose effect the provider left uncertain is confirmed by
/// the post-action projection, so the verified outcome is a success rather than
/// a guess.
#[test]
fn uncertain_step_confirmed_by_the_post_action_projection_is_a_success() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");

    // A dispatched mutation returning a server error cannot prove
    // non-application; the scripted world declares it landed.
    journey.queue_transfer(TransferAction::ServerError { lands: true });

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("verified outcome");
    let evidence = match outcome {
        MoveExecutionOutcome::VerifiedSuccess(evidence) => evidence,
        other => panic!("expected verified success, got {other:?}"),
    };
    assert_eq!(evidence.steps()[0].outcome, StepOutcome::Uncertain);
    assert_eq!(evidence.completed_count(), 0);
    assert_eq!(
        location_of(&committed(&service), LAST_WORD_INSTANCE),
        warlock_location()
    );
}

/// Scenario 6: unchanged provider content cannot verify reported completion.
#[test]
fn unchanged_post_action_projection_is_pending_provider_refresh() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");

    // The provider reported success but the scripted state never moved, so the
    // refreshed projection places the item at the source instead of the
    // success the step reported.
    journey.queue_transfer(TransferAction::ReportWithoutApplying);

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("classified outcome");
    let failure = match outcome {
        MoveExecutionOutcome::AppliedPendingProviderRefresh(pending) => pending,
        other => panic!("expected pending provider refresh, got {other:?}"),
    };
    assert_eq!(failure.observed(), &ItemLocation::Vault);
    assert_eq!(failure.evidence().completed_count(), 1);
    assert_eq!(
        failure.evidence().steps()[0].outcome,
        StepOutcome::Completed
    );

    assert_eq!(
        location_of(&committed(&service), LAST_WORD_INSTANCE),
        ItemLocation::Vault
    );
    assert_eq!(committed(&service).metadata.revision, SyncRevision::new(1));
}

#[test]
fn changed_post_action_revision_with_wrong_location_is_elsewhere() {
    let journey = Journey::new();
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();
    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .unwrap();
    journey.queue_transfer(TransferAction::Contradiction);
    let MoveExecutionOutcome::VerificationFailed(failure) = service
        .execute_move_verified(&mut transfer, &request, &plan, TTL, NOW, NOW)
        .unwrap()
    else {
        panic!("changed revision cannot qualify as stale provider refresh");
    };
    assert_eq!(failure.reason(), VerificationFailureReason::Elsewhere);
    assert_eq!(failure.observed(), Some(&hunter_location()));
    assert_eq!(failure.evidence().completed_count(), 1);
    assert_eq!(
        failure.evidence().steps()[0].outcome,
        StepOutcome::Completed
    );
    let snapshot = committed(&service);
    assert_eq!(snapshot.metadata.revision, SyncRevision::new(2));
    assert_eq!(
        location_of(&snapshot, LAST_WORD_INSTANCE),
        hunter_location()
    );
    assert_eq!(journey.request_count(), 6);
    let recorded = journey.recorded_transfers();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].url, TRANSFER_ENDPOINT);
    assert_eq!(
        body_json(&recorded[0]),
        json!({
            "itemReferenceHash":LAST_WORD_HASH,"stackSize":1,"transferToVault":false,
            "itemId":LAST_WORD_INSTANCE,"characterId":WARLOCK_ID,"membershipType":MEMBERSHIP_TYPE
        })
    );
    drop(service);
    assert_eq!(
        journey
            .service()
            .repository()
            .load_current()
            .unwrap()
            .unwrap(),
        snapshot
    );
}

/// Scenario 7: a failed post-action refresh is itself a verification failure and
/// still preserves the bounded evidence of the dispatched step.
#[test]
fn failed_post_action_refresh_is_a_verification_failure() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");

    journey.queue_transfer(TransferAction::AppliedThenPostActionFails);

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("classified outcome");
    let failure = match outcome {
        MoveExecutionOutcome::VerificationFailed(failure) => failure,
        other => panic!("expected a verification failure, got {other:?}"),
    };
    assert_eq!(failure.reason(), VerificationFailureReason::RefreshFailed);
    assert_eq!(failure.observed(), None);
    assert_eq!(
        failure.error(),
        Some(MutationError::Application(
            ErrorCategory::ProviderUnavailable
        ))
    );
    assert_eq!(failure.evidence().completed_count(), 1);
}

/// Scenario 8: an already-satisfied fresh preflight is a verified no-op with zero
/// transfers and no post-action refresh, because the provider already holds the
/// item at the requested destination.
#[test]
fn already_satisfied_preflight_is_a_verified_no_op() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");

    // The item already reached the requested destination before execution.
    journey.move_to_character(LAST_WORD_INSTANCE, WARLOCK_ID);

    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("verified outcome");
    let evidence = match outcome {
        MoveExecutionOutcome::VerifiedNoOp(evidence) => evidence,
        other => panic!("expected a verified no-op, got {other:?}"),
    };
    assert_eq!(evidence.attempted_count(), 0);
    assert_eq!(evidence.unattempted_count(), 1);

    assert_eq!(journey.transfer_calls(), 0);
    // Seed plus the forced preflight only: a verified no-op spends no post-action
    // refresh.
    assert_eq!(journey.request_count(), 2 + 2);
    assert_eq!(
        location_of(&committed(&service), LAST_WORD_INSTANCE),
        warlock_location()
    );
}

/// Scenario 9: the API key, the bearer token, and a raw-payload marker never
/// appear in a public error, the bounded evidence, or the raw bytes of the
/// on-disk database file, whether the failure comes from a refresh or from a
/// transfer envelope the adapter could not classify.
#[test]
fn secrets_and_payload_markers_never_reach_public_evidence_or_disk() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let mut transfer = journey.transfer_adapter();

    // A profile refresh that returns a raw-payload sentinel envelope must fail
    // sanitized, before any transfer is dispatched.
    let request = move_request(semantic("The Last Word"), warlock_label());
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("plan");
    journey.queue_profile_fault(ProfileFault::Sentinel);
    let refresh_failure = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect_err("a sentinel profile must fail sanitized");
    assert_eq!(
        refresh_failure.error(),
        MutationError::Application(ErrorCategory::ProviderValidation)
    );

    // A transfer envelope whose unclassified fields carry the same marker still
    // produces only a bounded step outcome.
    let plan = service
        .plan_move(&request, PlanLifetime::MAX, TTL, NOW)
        .expect("re-plan against the preserved projection");
    journey.queue_transfer(TransferAction::SentinelEnvelope);
    let outcome = service
        .execute_move_verified(
            &mut transfer,
            &request,
            &plan,
            TTL,
            Timestamp::new(NOW.seconds() + 1),
            Timestamp::new(NOW.seconds() + 2),
        )
        .expect("classified outcome");
    // The unknown provider envelope must classify as an uncertain step: the
    // post-action projection still places the item in the Vault, which is
    // consistent with zero reported progress, so the honest result is an
    // incomplete execution with no applied step. Asserting that exact variant
    // means a misclassified transfer (which would report a completed step and
    // land in a different, later `Elsewhere` result) fails this test.
    match &outcome {
        MoveExecutionOutcome::IncompleteExecution(incomplete) => {
            assert!(!incomplete.is_partial());
            assert_eq!(incomplete.observed(), &ItemLocation::Vault);
            assert_eq!(incomplete.stop_outcome(), Some(StepOutcome::Uncertain));
            assert_eq!(incomplete.evidence().completed_count(), 0);
            assert_eq!(incomplete.evidence().attempted_count(), 1);
        }
        other => panic!("expected an honest incomplete execution, got {other:?}"),
    }

    let sentinels = [API_KEY_SENTINEL, BEARER_SENTINEL, PAYLOAD_SENTINEL];
    let rendered = format!("{outcome:?}");
    for sentinel in sentinels {
        assert!(
            !refreshed_renderings(&refresh_failure)
                .iter()
                .any(|text| text.contains(sentinel)),
            "the refresh failure leaked {sentinel}"
        );
        assert!(
            !rendered.contains(sentinel),
            "the transfer outcome leaked {sentinel}"
        );
    }

    // Scan the database and every sidecar. The membership identifier is stored in
    // the clear, so its presence proves the scan read committed data.
    let bytes = database_bytes(journey.database.path());
    assert!(
        contains_bytes(&bytes, MEMBERSHIP_ID.as_bytes()),
        "the disk scan must cover committed rows"
    );
    for sentinel in sentinels {
        assert!(
            !contains_bytes(&bytes, sentinel.as_bytes()),
            "on-disk database leaked {sentinel}"
        );
    }
}

/// Every sanitized public rendering of one execution failure.
fn refreshed_renderings(failure: &destiny_app::ExecutionFailure) -> Vec<String> {
    vec![
        failure.error().to_string(),
        format!("{:?}", failure.error()),
        format!("{:?}", failure.evidence()),
        format!("{failure:?}"),
    ]
}
