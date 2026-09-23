//! Composed, offline end-to-end query journey over the real durable projection.
//!
//! This integration test drives the real composed stack instead of a stand-in
//! for it: the real [`BungieProfileGateway`] decodes the provider fixture, the
//! real [`SqliteStore`] persists the committed profile **and** the published
//! Manifest definitions to an on-disk SQLite file, and the real
//! [`QueryService`] runs `list_characters`, `search_inventory`, and
//! `inspect_item` through the real [`ProfileSyncService`] freshness policy and
//! the durable [`SearchRepository`] implementation. Nothing at those layers is
//! faked: definitions are published through the real
//! [`DefinitionRepository::replace_definitions`] contract and the profile is
//! committed through the real repository revision logic.
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
//! Every assertion is on public API behavior: the journey never reaches into
//! production internals, never substitutes a fake repository or gateway, and
//! never weakens an assertion to make it pass.
//!
//! Scenarios covered:
//!
//! 1. a cold query publishes definitions and synchronizes the projection through
//!    SQLite before listing characters;
//! 2. name/type/tag definition search, including bounded deterministic
//!    ambiguous candidates;
//! 3. exact perk-hash, character-location, and definition-hash instance
//!    filtering with each hit paired to its exact definition;
//! 4. exact inspection that keeps `ItemHash` and `ItemInstanceId` distinct, plus
//!    an explicit missing inspection;
//! 5. an ordinary fresh query that performs no gateway request, and a stale
//!    query that refreshes exactly once while an unchanged refresh retains the
//!    revision and refreshes `checked_at`;
//! 6. a materially changed refresh that advances the revision exactly once and
//!    changes observable query results;
//! 7. a close/reopen of the same SQLite file that preserves durable list, search,
//!    and inspect results with no gateway call;
//! 8. a provider failure that preserves the prior projection and leaks no
//!    credential, raw-payload, or membership sentinel into errors or disk; and
//! 9. the durable schema's refusal to mis-source a profile projection.
//!
//! [`BungieProfileGateway`]: destiny_adapters::profile_gateway::BungieProfileGateway
//! [`SqliteStore`]: destiny_adapters::sqlite::SqliteStore
//! [`QueryService`]: destiny_app::QueryService
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
use destiny_app::{ProfileSyncService, QueryService, SearchInventoryRequest};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    Dataset, DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery,
    DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy, FreshnessStatus,
    InspectItemRequest, InstanceSearchQuery, InventoryRepository, InventorySearchResult,
    InventorySnapshot, ItemDefinition, ItemHash, ItemHashFilter, ItemInspection, ItemInstanceId,
    MAX_SEARCH_LIMIT, ManifestDefinitions, MembershipId, MembershipType, PerkHash, Resolution,
    SearchTerm, SourceMarker, SyncRevision, Timestamp,
};
use rusqlite::Connection;
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

/// The Manifest version shared by the published definitions and the gateway
/// that stamps the synchronized profile.
const DEFINITION_VERSION: &str = "query-journey-definitions-v1";

/// Deterministic application time. The gateway stamps `checked_at` from the
/// injected clock; the freshness policy below is evaluated against the explicit
/// `now` the test passes to each query.
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const CREDENTIAL_EXPIRES_AT: u64 = 4_000_000_000;
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// Distinctive sentinels for the two secrets and one raw-payload marker. None of
/// them may ever reach an error, the committed projection, or the on-disk bytes.
const API_KEY_SENTINEL: &str = "sentinel-query-api-key";
const BEARER_SENTINEL: &str = "sentinel-query-bearer-token";
const PAYLOAD_SENTINEL: &str = "sentinel-query-raw-payload";

/// The complete profile fixture decoded by the real adapter.
const FIXTURE: &[u8] = include_bytes!("fixtures/profile-complete.json");

/// The two committed characters, in their canonical identifier order.
const CHARACTER_ONE: &str = "900000000000000001";
const CHARACTER_TWO: &str = "900000000000000002";

/// The retained instanced items the fixture normalizes to. The vaulted item is
/// the one the "changed profile" scenario mutates.
const INSTANCE_VAULT: &str = "700000000000000001";
const INSTANCE_OTHER: &str = "700000000000000003";
const INSTANCE_CHAR_ONE: &str = "700000000000000004";
const INSTANCE_CHAR_TWO: &str = "700000000000000005";
const INSTANCE_CHAR_ONE_EQUIPPED: &str = "700000000000000006";
const INSTANCE_CHAR_TWO_EQUIPPED: &str = "700000000000000007";

/// The exact perk hashes the fixture associates with the equipped item.
const PERK_ACTIVE: u32 = 5001;
const PERK_INACTIVE: u32 = 5002;

/// The membership discovery body: an established Cross Save relationship whose
/// primary is [`MEMBERSHIP_ID`] and which carries one overridden platform
/// record. It satisfies the strict selection rules in `decode_effective_membership`.
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
/// The guard mirrors the convention already used by the SQLite repository tests
/// and the profile journey: one unique file per test derived from the process id
/// and an atomic counter, with the database and its `-wal`/`-shm`/`-journal`
/// sidecars removed on drop.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-query-journey-{}-{sequence}.db",
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
/// advance time between queries while the gateway keeps the same clock value.
struct TestClock(Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(self.0.get()))
    }
}

type Gateway = BungieProfileGateway<SharedTransport, TestClock>;
type Service = QueryService<Gateway, SqliteStore>;

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

    /// Composes the real gateway and the real SQLite store over the shared
    /// transport and clock.
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
        QueryService::new(ProfileSyncService::new(gateway, store))
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
fn definition(hash: i64, name: &str, item_type: Option<&str>, tags: &[&str]) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.to_owned(),
        item_type: item_type.map(str::to_owned),
        tier: None,
        element: None,
        tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        version: DefinitionVersion::new(DEFINITION_VERSION),
    }
}

/// The published definition set. It covers every item hash the fixture
/// normalizes to, so instance search can always pair a concrete instance with
/// its exact definition, and adds shared name/type/tag values that exercise
/// bounded deterministic candidate matching.
fn definitions() -> Vec<ItemDefinition> {
    vec![
        definition(
            1001,
            "Gjallarhorn",
            Some("Rocket Launcher"),
            &["exotic", "solar"],
        ),
        definition(
            1003,
            "Gjallarswift",
            Some("Rocket Launcher"),
            &["legendary"],
        ),
        definition(
            2001,
            "Sword of Crota",
            Some("Sword"),
            &["legendary", "void"],
        ),
        definition(
            2003,
            "Crota's Sword",
            Some("Sword"),
            &["legendary", "solar"],
        ),
        definition(
            3001,
            "Vex Mythoclast",
            Some("Fusion Rifle"),
            &["exotic", "solar"],
        ),
        definition(3002, "Ghost Shell", Some("Ghost Shell"), &["exotic"]),
    ]
}

/// Publishes the complete definition set through the real repository contract
/// and returns the committed Manifest metadata it reports.
fn publish_definitions(service: &mut Service) -> destiny_core::SyncMetadata {
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
        .expect("publish the definition set")
}

fn term(value: &str) -> SearchTerm {
    SearchTerm::new(value).expect("valid search term")
}

fn definition_query(
    text: Option<&str>,
    item_type: Option<&str>,
    tags: &[&str],
    limit: usize,
) -> DefinitionSearchQuery {
    DefinitionSearchQuery {
        text: text.map(term),
        item_type: item_type.map(term),
        tags: tags.iter().map(|tag| term(tag)).collect(),
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
        character_id: character.map(CharacterId::new),
        limit,
    }
}

fn request(
    definitions: DefinitionSearchQuery,
    instances: Option<InstanceSearchQuery>,
) -> SearchInventoryRequest {
    SearchInventoryRequest {
        definitions,
        instances,
    }
}

/// Runs one successful search through the composed service at `now`.
fn search(
    service: &mut Service,
    request: &SearchInventoryRequest,
    now: Timestamp,
) -> InventorySearchResult {
    service
        .search_inventory(request, TTL, now)
        .expect("search succeeds")
}

/// Runs one successful inspection through the composed service at `now`.
fn inspect(service: &mut Service, instance_id: &str, now: Timestamp) -> Resolution<ItemInspection> {
    service
        .inspect_item(
            &InspectItemRequest {
                instance_id: ItemInstanceId::new(instance_id),
            },
            TTL,
            now,
        )
        .expect("inspection succeeds")
}

/// The committed projection read back through the real repository.
fn committed(service: &Service) -> InventorySnapshot {
    service
        .repository()
        .load_current()
        .expect("repository read")
        .expect("committed projection")
}

fn candidate_hashes(result: &InventorySearchResult) -> Vec<i64> {
    result
        .candidates
        .iter()
        .map(|definition| definition.hash.value())
        .collect()
}

fn hit_instances(result: &InventorySearchResult) -> Vec<&str> {
    result
        .hits
        .iter()
        .map(|hit| hit.instance.instance_id.as_str())
        .collect()
}

/// Publishes definitions and synchronizes the projection with one cold query,
/// leaving two recorded gateway requests behind.
fn seed(journey: &Journey, service: &mut Service) {
    publish_definitions(service);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .list_characters(TTL, NOW)
        .expect("cold query synchronizes the projection");
}

/// A materially changed fixture: the vaulted item [`INSTANCE_VAULT`] carries a
/// different quantity, which is part of revision identity.
fn changed_profile() -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).expect("fixture is JSON");
    let items = value["Response"]["profileInventory"]["data"]["items"]
        .as_array_mut()
        .expect("fixture carries a profileInventory items array");
    let vaulted = items
        .iter_mut()
        .find(|item| item["itemInstanceId"].as_str() == Some(INSTANCE_VAULT))
        .expect("fixture carries the vaulted item by its stable instance id");
    vaulted["quantity"] = json!(2);
    serde_json::to_vec(&value).expect("mutated fixture serializes")
}

/// A failing profile response carrying the raw-payload sentinel in a provider
/// message field, which must never reach an error, disk, or a query answer.
fn sentinel_profile() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "ErrorCode": 7,
        "ErrorStatus": "PrivacyRestriction",
        "Message": PAYLOAD_SENTINEL,
        "Response": {}
    }))
    .expect("payload serializes")
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

/// Scenario 1: an empty store plus one ordinary `list_characters` query
/// publishes the definition set through the real `DefinitionRepository`,
/// synchronizes the profile through the real `ProfileSyncService`, and answers
/// from the committed SQLite projection with repository-owned metadata.
#[test]
fn list_characters_reads_the_committed_projection_with_freshness() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    let manifest = publish_definitions(&mut service);
    assert_eq!(manifest.dataset, Dataset::ManifestDefinitions);
    assert_eq!(manifest.source, SourceMarker::Manifest);
    assert_eq!(manifest.revision, SyncRevision::new(1));

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let result = service
        .list_characters(TTL, NOW)
        .expect("cold list synchronizes the projection");

    let ids: Vec<&str> = result
        .characters
        .iter()
        .map(|character| character.id.as_str())
        .collect();
    assert_eq!(ids, [CHARACTER_ONE, CHARACTER_TWO]);
    assert_eq!(result.metadata.sync.dataset, Dataset::ProfileInventory);
    assert_eq!(result.metadata.sync.source, SourceMarker::Profile);
    assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(result.metadata.sync.checked_at, NOW);
    assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);

    // The Manifest definition revision and the profile revision stay separate.
    assert_eq!(
        service
            .repository()
            .current_metadata()
            .expect("definition metadata read")
            .expect("committed definitions")
            .dataset,
        Dataset::ManifestDefinitions
    );

    // The real gateway resolved the Cross Save primary and then requested the
    // exact profile endpoint for that resolved membership.
    assert_eq!(
        journey.recorded_urls(),
        vec![
            MEMBERSHIP_ENDPOINT.to_owned(),
            profile_endpoint(&expected_membership()),
        ]
    );
}

/// Scenario 2: definition search matches normalized name text, exact item type,
/// and tag conjunction, and a broad display-name term returns bounded
/// deterministic candidates instead of a single guessed hit.
#[test]
fn definition_search_matches_name_type_and_tags_with_bounded_candidates() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let by_name = search(
        &mut service,
        &request(
            definition_query(Some("  GJALLAR "), None, &[], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert_eq!(candidate_hashes(&by_name), [1001, 1003]);
    assert!(by_name.hits.is_empty());

    let by_type = search(
        &mut service,
        &request(
            definition_query(None, Some("rocket launcher"), &[], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert_eq!(candidate_hashes(&by_type), [1001, 1003]);

    let by_tags = search(
        &mut service,
        &request(
            definition_query(None, None, &["EXOTIC", " solar "], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert_eq!(candidate_hashes(&by_tags), [1001, 3001]);

    // "sword" matches two definitions; both are explicit candidates, and the
    // bounded limit truncates the hash-ordered list deterministically.
    let ambiguous = search(
        &mut service,
        &request(
            definition_query(Some("sword"), None, &[], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert_eq!(candidate_hashes(&ambiguous), [2001, 2003]);
    let bounded = search(
        &mut service,
        &request(definition_query(Some("sword"), None, &[], 1), None),
        NOW,
    );
    assert_eq!(candidate_hashes(&bounded), [2001]);

    let no_match = search(
        &mut service,
        &request(
            definition_query(Some("nonexistent"), None, &[], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert!(no_match.candidates.is_empty());
}

/// Scenario 3: instance search filters exactly by perk hash, character location,
/// and definition hash, pairs every hit with the exact definition its
/// `item_hash` references, and keeps an empty candidate set explicit.
#[test]
fn instance_search_filters_exactly_and_pairs_each_hit_with_its_definition() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    // Every retained instance resolves to a published definition.
    let all = search(
        &mut service,
        &request(
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::Any,
                None,
                None,
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert_eq!(
        hit_instances(&all),
        [
            INSTANCE_VAULT,
            INSTANCE_OTHER,
            INSTANCE_CHAR_ONE,
            INSTANCE_CHAR_TWO,
            INSTANCE_CHAR_ONE_EQUIPPED,
            INSTANCE_CHAR_TWO_EQUIPPED,
        ]
    );
    for hit in &all.hits {
        assert_eq!(hit.definition.hash, hit.instance.item_hash);
    }

    // The active and the inactive/invisible perk both match their exact hash.
    for perk in [PERK_ACTIVE, PERK_INACTIVE] {
        let by_perk = search(
            &mut service,
            &request(
                definition_query(None, None, &[], MAX_SEARCH_LIMIT),
                Some(instance_query(
                    ItemHashFilter::Any,
                    Some(perk),
                    None,
                    MAX_SEARCH_LIMIT,
                )),
            ),
            NOW,
        );
        assert_eq!(hit_instances(&by_perk), [INSTANCE_CHAR_ONE_EQUIPPED]);
        assert_eq!(by_perk.hits[0].definition.hash, ItemHash::new(3001));
    }

    // A character filter includes only that character's owned instances and
    // truncates the instance-identifier-ordered list to the limit.
    let char_one = search(
        &mut service,
        &request(
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::Any,
                None,
                Some(CHARACTER_ONE),
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert_eq!(
        hit_instances(&char_one),
        [INSTANCE_CHAR_ONE, INSTANCE_CHAR_ONE_EQUIPPED]
    );
    let bounded = search(
        &mut service,
        &request(
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::Any,
                None,
                Some(CHARACTER_ONE),
                1,
            )),
        ),
        NOW,
    );
    assert_eq!(hit_instances(&bounded), [INSTANCE_CHAR_ONE]);

    // The definition search composes with the instance search: the matched
    // definitions drive an exact definition-hash instance filter.
    let composed = search(
        &mut service,
        &request(
            definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(1001), ItemHash::new(1003)]),
                None,
                None,
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert_eq!(candidate_hashes(&composed), [1001, 1003]);
    assert_eq!(hit_instances(&composed), [INSTANCE_VAULT, INSTANCE_OTHER]);

    // An empty definition-hash candidate set is an explicit no-match.
    let empty = search(
        &mut service,
        &request(
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::AnyOf(Vec::new()),
                None,
                None,
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert!(empty.hits.is_empty());

    // A perk hash no instance carries is an explicit no-match.
    let absent = search(
        &mut service,
        &request(
            definition_query(None, None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::Any,
                Some(9999),
                None,
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert!(absent.hits.is_empty());
}

/// Scenario 4: exact inspection reports the Manifest definition and the concrete
/// instance in separate fields, keeps the two identity types distinct, and
/// reports an unknown instance as an explicit missing result.
#[test]
fn inspect_item_keeps_definition_and_instance_identity_distinct() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let unique = inspect(&mut service, INSTANCE_VAULT, NOW);
    let ItemInspection {
        definition,
        instance,
        metadata,
    } = match unique {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected a unique inspection, got {other:?}"),
    };

    assert_eq!(definition.hash, ItemHash::new(1001));
    assert_eq!(instance.instance_id, ItemInstanceId::new(INSTANCE_VAULT));
    assert_eq!(definition.hash, instance.item_hash);
    assert_eq!(metadata.sync.source, SourceMarker::Profile);
    assert_eq!(metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(metadata.freshness, FreshnessStatus::Fresh);

    // The instance identifier is not a definition hash: the concrete runtime id
    // and the Manifest hash are numerically different, so the two can never be
    // confused even when both are rendered as text.
    let instance_identifier: i64 = instance
        .instance_id
        .as_str()
        .parse()
        .expect("fixture instance id is decimal");
    assert_ne!(instance_identifier, definition.hash.value());

    // A missing instance is explicit, not a guessed or empty inspection.
    assert_eq!(
        inspect(&mut service, "700000000000000099", NOW),
        Resolution::Missing
    );
}

/// Scenario 5: an ordinary query over a fresh projection performs no gateway
/// request, while a query past the TTL refreshes exactly once; an unchanged
/// refresh retains the revision and refreshes `checked_at`.
#[test]
fn fresh_query_skips_the_gateway_and_a_stale_query_refreshes_once() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let after_seed = journey.request_count();
    assert_eq!(after_seed, 2);

    // Fresh: no gateway call, unchanged metadata.
    let fresh = search(
        &mut service,
        &request(
            definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
            None,
        ),
        NOW,
    );
    assert_eq!(journey.request_count(), after_seed);
    assert_eq!(fresh.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(fresh.metadata.sync.checked_at, NOW);
    assert_eq!(fresh.metadata.freshness, FreshnessStatus::Fresh);

    // Stale: one ordinary refresh, unchanged content retains the revision while
    // `checked_at` advances to the explicit `now`.
    let stale_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(stale_now);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let stale = search(
        &mut service,
        &request(
            definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
            None,
        ),
        stale_now,
    );
    assert_eq!(journey.request_count(), after_seed + 2);
    assert_eq!(stale.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(stale.metadata.sync.checked_at, stale_now);
    assert_eq!(stale.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(candidate_hashes(&stale), [1001, 1003]);
}

/// Scenario 6: a refresh whose profile content changed materially advances the
/// revision exactly once, and the new content is observable through a query;
/// an unchanged refresh of the same content then retains that revision.
#[test]
fn changed_refresh_advances_the_revision_once_and_changes_results() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let before = match inspect(&mut service, INSTANCE_VAULT, NOW) {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected a unique inspection, got {other:?}"),
    };
    assert_eq!(before.instance.quantity, 1);
    assert_eq!(before.metadata.sync.revision, SyncRevision::new(1));

    let changed_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(changed_now);
    journey.queue_membership();
    journey.queue_profile(&changed_profile());
    let changed = match inspect(&mut service, INSTANCE_VAULT, changed_now) {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected a unique inspection, got {other:?}"),
    };
    assert_eq!(changed.metadata.sync.revision, SyncRevision::new(2));
    assert_eq!(changed.instance.quantity, 2);

    // Re-synchronizing the now-unchanged content retains revision 2.
    let later = Timestamp::new(changed_now.seconds() + TTL.ttl_seconds + 1);
    journey.at(later);
    journey.queue_membership();
    journey.queue_profile(&changed_profile());
    let retained = match inspect(&mut service, INSTANCE_VAULT, later) {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected a unique inspection, got {other:?}"),
    };
    assert_eq!(retained.metadata.sync.revision, SyncRevision::new(2));
    assert_eq!(retained.instance.quantity, 2);
}

/// Scenario 7: closing and reopening the same on-disk database with a fresh
/// store preserves the definition and profile projections, so the same list,
/// search, and inspect answers are durable and need no gateway call.
#[test]
fn reopening_the_same_file_preserves_list_search_and_inspect_results() {
    let journey = Journey::new();
    journey.at(NOW);

    let (published, search_result, inspection) = {
        let mut service = journey.service();
        seed(&journey, &mut service);
        let list = service.list_characters(TTL, NOW).expect("list succeeds");
        let search_result = search(
            &mut service,
            &request(
                definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
                Some(instance_query(
                    ItemHashFilter::AnyOf(vec![ItemHash::new(1001)]),
                    None,
                    None,
                    MAX_SEARCH_LIMIT,
                )),
            ),
            NOW,
        );
        let inspection = match inspect(&mut service, INSTANCE_VAULT, NOW) {
            Resolution::Unique(inspection) => inspection,
            other => panic!("expected a unique inspection, got {other:?}"),
        };
        (list, search_result, inspection)
    };

    // Drop the first store and gateway, then reopen the same file.
    let requests_before = journey.request_count();
    let mut reopened = journey.service();

    let list = reopened
        .list_characters(TTL, NOW)
        .expect("reopened list succeeds");
    assert_eq!(list, published);

    let search_result_again = search(
        &mut reopened,
        &request(
            definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
            Some(instance_query(
                ItemHashFilter::AnyOf(vec![ItemHash::new(1001)]),
                None,
                None,
                MAX_SEARCH_LIMIT,
            )),
        ),
        NOW,
    );
    assert_eq!(search_result_again, search_result);

    let inspection_again = match inspect(&mut reopened, INSTANCE_VAULT, NOW) {
        Resolution::Unique(inspection) => inspection,
        other => panic!("expected a unique inspection, got {other:?}"),
    };
    assert_eq!(inspection_again, inspection);

    // The reopened answers are durable, not a refresh: no gateway request ran.
    assert_eq!(journey.request_count(), requests_before);
}

/// Scenario 8: a failing refresh preserves the prior projection, and neither the
/// sanitized error nor the on-disk database bytes carry a credential, a raw
/// payload marker, or a provider message.
#[test]
fn failed_refresh_preserves_the_projection_without_leaking_secrets() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let prior = committed(&service);
    assert_eq!(prior.metadata.revision.value(), 1);
    assert_eq!(prior.metadata.checked_at, NOW);

    let failure_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(failure_now);
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let error = service
        .search_inventory(
            &request(
                definition_query(Some("gjallar"), None, &[], MAX_SEARCH_LIMIT),
                None,
            ),
            TTL,
            failure_now,
        )
        .expect_err("a non-success profile must fail the query");
    assert_eq!(error.category(), ErrorCategory::ProviderValidation);

    let sentinels = [API_KEY_SENTINEL, BEARER_SENTINEL, PAYLOAD_SENTINEL];
    for sentinel in sentinels {
        assert!(
            !error.to_string().contains(sentinel),
            "error Display leaked {sentinel}"
        );
        assert!(
            !format!("{error:?}").contains(sentinel),
            "error Debug leaked {sentinel}"
        );
    }

    // The prior projection is readably unchanged at the same revision.
    let after = committed(&service);
    assert_eq!(after, prior);
    assert_eq!(after.metadata.revision.value(), 1);
    assert_eq!(after.metadata.checked_at, NOW);

    let rendered = format!("{after:?}");
    for sentinel in sentinels {
        assert!(
            !rendered.contains(sentinel),
            "committed projection leaked {sentinel}"
        );
    }

    // Scan the database and every sidecar. The membership identifier is stored
    // in the clear, so its presence proves the scan actually read committed data
    // rather than an empty file.
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

/// Scenario 9: the durable schema itself forbids a mis-sourced profile
/// projection, so a tampered row cannot reach the freshness gate. This is
/// stronger than a refresh: the mis-sourced row is rejected, and the readable
/// projection keeps answering as `Profile` without a gateway call.
///
/// The equivalent "an invalid-source projection refreshes instead of answering"
/// behavior belongs to an in-memory repository and is proven at that fake seam in
/// `destiny-app`; a file-backed projection can never hold a mis-sourced row.
#[test]
fn the_durable_schema_refuses_a_mis_sourced_profile_projection() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let requests_before = journey.request_count();

    // The schema CHECK pins `profile_inventory` to the `profile` source, so the
    // tampering write is refused rather than committed.
    {
        let raw = Connection::open(journey.database.path()).expect("open raw connection");
        let tampered = raw.execute(
            "UPDATE sync_metadata SET source = 'manifest' WHERE dataset = 'profile_inventory'",
            [],
        );
        assert!(
            tampered.is_err(),
            "the schema must refuse a mis-sourced profile row"
        );
    }

    // The projection still answers as `Profile` with the prior metadata, and no
    // refresh was attempted.
    let result = service
        .list_characters(TTL, NOW)
        .expect("the projection still answers");
    assert_eq!(result.metadata.sync.source, SourceMarker::Profile);
    assert_eq!(result.metadata.sync.revision, SyncRevision::new(1));
    assert_eq!(result.metadata.freshness, FreshnessStatus::Fresh);
    assert_eq!(journey.request_count(), requests_before);
}
