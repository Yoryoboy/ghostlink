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
//! 9. the durable schema's refusal to mis-source a profile projection;
//! 10. the real CLI runner rendering list/search/inspect in both human and
//!     JSON modes, including fresh reuse, a stale refresh, explicit multiple
//!     candidates, a typed `item_not_found`, and secret-free streams; and
//! 11. a decoded provider failure reaching the process boundary as a sanitized
//!     typed outcome on stderr only; and
//! 12. the real CLI JSON reads and the real MCP read projections, driven over
//!     the same composed service and committed SQLite state, agreeing on every
//!     shared semantic field while pinning the documented MCP-only additions and
//!     the dropped `Other` location text.
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
use destiny_app::test_support::leak_sentinels;
use destiny_app::{ProfileSyncService, QueryService, SearchInventoryRequest};
use destiny_cli::{ProcessOutput, ReadFreshness};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CharacterId, Clock, CredentialMaterial,
    Dataset, DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery,
    DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy, FreshnessStatus,
    InspectItemRequest, InstanceSearchQuery, InventoryRepository, InventorySearchResult,
    InventorySnapshot, ItemDefinition, ItemHash, ItemHashFilter, ItemInspection, ItemInstanceId,
    MAX_SEARCH_LIMIT, ManifestDefinitions, MembershipId, MembershipType, PerkHash, Resolution,
    SearchTerm, SourceMarker, SyncRevision, Timestamp,
};
use destiny_mcp::{
    InspectItemArgs, InventoryReadService, ListCharactersArgs, QueryReadService,
    SearchInventoryArgs,
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
/// They come from the shared leak set so this journey probes the exact values the
/// MCP and consolidated hardening harnesses probe.
const API_KEY_SENTINEL: &str = leak_sentinels::API_KEY;
const BEARER_SENTINEL: &str = leak_sentinels::BEARER_TOKEN;
const PAYLOAD_SENTINEL: &str = leak_sentinels::RAW_PAYLOAD;

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

/// An instance identifier the fixture never carries, used by the typed
/// missing-inspection boundary comparison.
const INSTANCE_MISSING: &str = "700000000000000099";

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
            destiny_core::StaticCredentialSource::new(credential()),
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
        refresh_grant: None,
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
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.to_owned(),
        item_type: item_type.map(str::to_owned),
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
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
        tier: None,
        text: text.map(term),
        item_type: item_type.map(term),
        tags: tags.iter().map(|tag| term(tag)).collect(),
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
        offset: 0,
        sort: None,
        location: None,
        equipped: None,
        include_available: false,
        item_hashes,
        perk_hash: perk_hash.map(PerkHash::new),
        perk_name: None,
        mod_name: None,
        archetype: None,
        stats: Vec::new(),
        character_id: character.map(CharacterId::new),
        definitions: None,
        min_power: None,
        limit,
    }
}

fn request(
    definitions: DefinitionSearchQuery,
    instances: Option<InstanceSearchQuery>,
) -> SearchInventoryRequest {
    SearchInventoryRequest::new(definitions, instances)
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

/// The freshness inputs the CLI forwards verbatim to the shared service.
fn cli_freshness(now: Timestamp) -> ReadFreshness {
    ReadFreshness { policy: TTL, now }
}

/// Builds one process argument vector with the binary name first.
fn cli_argv(arguments: &[&str]) -> Vec<String> {
    let mut argv = vec!["ghostlink".to_string()];
    argv.extend(arguments.iter().map(|argument| (*argument).to_string()));
    argv
}

/// Drives the real CLI runner over the composed service, exactly as the binary
/// process boundary does, and returns the fully rendered process result.
#[test]
fn absent_manifest_errors_in_cli_and_mcp_but_loaded_no_match_is_empty() {
    let journey = Journey::new();
    let mut service = journey.service();
    let hint = "definitions not loaded; run ghostlink sync-manifest";
    for command in [
        vec!["search-inventory"],
        vec!["inspect-item", INSTANCE_MISSING],
    ] {
        let output = cli(&mut service, &command, NOW);
        assert_eq!(output.exit_code, 33);
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, format!("error: {hint}\n"));
        assert_no_secrets(&output.stderr);
    }
    let output = cli(&mut service, &["--json", "search-inventory"], NOW);
    assert_eq!(
        output.stderr,
        format!("{{\"error\":{{\"code\":\"definitions_unavailable\",\"message\":\"{hint}\"}}}}\n")
    );
    let mut reads = QueryReadService::new(service, TestClock(Rc::clone(&journey.clock)), TTL);
    let args: SearchInventoryArgs = serde_json::from_value(json!({})).unwrap();
    let error = reads
        .search_inventory(&args.into_request().unwrap())
        .unwrap_err();
    assert_eq!(error.code(), "definitions_unavailable");
    assert_eq!(error.to_string(), hint);
    assert_no_secrets(&format!("{error:?} {error}"));
    let tool_error = serde_json::to_value(error.into_call_tool_result()).unwrap();
    assert_eq!(tool_error["isError"], true);
    assert_eq!(
        tool_error["structuredContent"]["code"],
        "definitions_unavailable"
    );
    assert_eq!(tool_error["structuredContent"]["message"], hint);
    assert!(journey.transport.borrow().recorded_urls.is_empty());
    let mut service = journey.service();
    seed(&journey, &mut service);
    let output = cli_json(
        &mut service,
        &["--json", "search-inventory", "--text", "no such item"],
    );
    assert_eq!(output["candidates"], json!([]));
    assert_eq!(output["hits"], json!([]));
}

fn cli(service: &mut Service, arguments: &[&str], now: Timestamp) -> ProcessOutput {
    destiny_cli::run(service, cli_argv(arguments), cli_freshness(now))
}

/// Asserts that no credential, raw payload marker, or membership identifier
/// reached a rendered stream.
fn assert_no_secrets(rendered: &str) {
    for sentinel in [
        API_KEY_SENTINEL,
        BEARER_SENTINEL,
        PAYLOAD_SENTINEL,
        MEMBERSHIP_ID,
    ] {
        assert!(
            !rendered.contains(sentinel),
            "rendered output leaked {sentinel}"
        );
    }
}

/// Drives the real CLI runner in JSON mode and returns its single parsed object.
///
/// A successful read writes exactly one JSON object to stdout and nothing to
/// stderr, and no rendered byte may carry a credential, raw provider payload, or
/// membership identifier.
fn cli_json(service: &mut Service, arguments: &[&str]) -> Value {
    let output = cli(service, arguments, NOW);
    assert_eq!(output.exit_code, 0, "CLI read must succeed");
    assert!(
        output.stderr.is_empty(),
        "a successful CLI read writes no stderr"
    );
    let value: Value = serde_json::from_str(output.stdout.trim()).expect("one JSON object");
    assert_no_secrets(&value.to_string());
    value
}

/// The real MCP read seam: the [`InventoryReadService`] implementation the stdio
/// server calls, composed over the same shared query service instance, clock,
/// and freshness policy the CLI runner reads through.
type McpReads = QueryReadService<Gateway, SqliteStore, TestClock>;

/// Runs one `list_characters` read through the real MCP read seam.
fn mcp_read_list(reads: &mut McpReads) -> Value {
    let request = ListCharactersArgs {}.into_request();
    let value = serde_json::to_value(
        reads
            .list_characters(&request)
            .expect("MCP character list succeeds"),
    )
    .expect("MCP character list serializes to JSON");
    assert_no_secrets(&value.to_string());
    value
}

/// Runs one `search_inventory` read through the real MCP read seam, converting
/// the MCP argument schema exactly as the tool boundary does.
fn mcp_read_search(reads: &mut McpReads, arguments: Value) -> Value {
    let args: SearchInventoryArgs =
        serde_json::from_value(arguments).expect("MCP search arguments deserialize");
    let request = args
        .into_request()
        .expect("MCP search arguments are valid and bounded");
    let value = serde_json::to_value(
        reads
            .search_inventory(&request)
            .expect("MCP search succeeds"),
    )
    .expect("MCP search serializes to JSON");
    assert_no_secrets(&value.to_string());
    value
}

/// Runs one `inspect_item` read through the real MCP read seam, converting the
/// MCP argument schema exactly as the tool boundary does.
fn mcp_read_inspect(reads: &mut McpReads, instance_id: &str) -> Value {
    let args: InspectItemArgs = serde_json::from_value(json!({ "instance_id": instance_id }))
        .expect("MCP inspect arguments deserialize");
    let request = args
        .into_request()
        .expect("MCP inspect arguments are valid");
    let value = serde_json::to_value(
        reads
            .inspect_item(&request)
            .expect("MCP inspection succeeds"),
    )
    .expect("MCP inspection serializes to JSON");
    assert_no_secrets(&value.to_string());
    value
}

/// Reduces one CLI or MCP JSON payload to the fields both boundaries share.
///
/// The only removals are the documented boundary differences, each bound to its
/// exact view by shape: a definition's `definition_version`, metadata's `dataset`
/// and `definition_version`, a perk's `perk_hash`, and the `Other` location's
/// provider `value`. Every other key, at every level, survives, so an unexpected
/// addition, drop, or rename on either side makes the comparison fail.
fn shared(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !is_documented_difference(object, key))
                .map(|(key, nested)| (key.clone(), shared(nested)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(shared).collect()),
        scalar => scalar.clone(),
    }
}

/// Whether `key` is one of the documented differences for this object's exact
/// view. The view is identified by its shape, never by an arbitrary key name, so
/// only the intended field at the intended view is ever removed.
fn is_documented_difference(object: &serde_json::Map<String, Value>, key: &str) -> bool {
    let definition = ["hash", "name", "tags"]
        .iter()
        .all(|field| object.contains_key(*field));
    let metadata = object.contains_key("checked_at") && object.contains_key("source");
    let perk = object.contains_key("active") && object.contains_key("visible");
    let other = object.get("kind").and_then(Value::as_str) == Some("other") && object.len() == 2;
    (key == "definition_version" && (definition || metadata))
        || (key == "dataset" && metadata)
        || (key == "perk_hash" && perk)
        || (key == "value" && other)
}

/// Finds one instance hit by its exact identifier in a search payload.
fn hit_of<'a>(hits: &'a Value, instance_id: &str) -> &'a Value {
    hits.as_array()
        .expect("hits is an array")
        .iter()
        .find(|hit| hit["instance"]["instance_id"].as_str() == Some(instance_id))
        .unwrap_or_else(|| panic!("the search payload carries a hit for {instance_id}"))
}

/// Asserts the real CLI and MCP answers agree on every shared section they both
/// carry, after the bounded documented-difference reduction above.
fn assert_shared(cli: &Value, mcp: &Value, label: &str) {
    for section in [
        "characters",
        "candidates",
        "hits",
        "definition",
        "instance",
        "metadata",
    ] {
        if cli.get(section).is_some() {
            assert_eq!(
                shared(&mcp[section]),
                shared(&cli[section]),
                "{label}: {section}"
            );
        }
    }
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
    assert_eq!(hit_instances(&by_name), [INSTANCE_VAULT, INSTANCE_OTHER]);

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

#[test]
fn type_filter_constrains_owned_hits_through_service_cli_and_reopen() {
    let journey = Journey::new();
    let mut service = journey.service();
    seed(&journey, &mut service);
    let filtered = request(definition_query(None, Some("sword"), &[], 1), None);
    let result = search(&mut service, &filtered, NOW);
    assert_eq!(candidate_hashes(&result), [2001]);
    assert_eq!(hit_instances(&result), [INSTANCE_CHAR_ONE]);
    let output = cli_json(
        &mut service,
        &["--json", "search-inventory", "--detail", "--type", "Sword"],
    );
    let hits = output["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .all(|hit| hit["definition"]["item_type"] == "Sword")
    );
    let mut power_query = instance_query(ItemHashFilter::Any, None, None, MAX_SEARCH_LIMIT);
    power_query.min_power = Some(u16::MAX);
    assert!(
        search(
            &mut service,
            &request(
                definition_query(None, None, &[], MAX_SEARCH_LIMIT),
                Some(power_query)
            ),
            NOW
        )
        .hits
        .is_empty()
    );
    drop(service);
    let mut reopened = journey.service();
    assert_eq!(search(&mut reopened, &filtered, NOW), result);
    assert_eq!(journey.request_count(), 2);
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
        ..
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

/// CLI journey: the real CLI runner renders the committed character projection
/// in both human and JSON modes, reuses the fresh projection without a gateway
/// call, and leaks no credential, payload, or membership sentinel.
#[test]
fn cli_list_characters_renders_human_and_json_and_reuses_the_fresh_projection() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let requests_before = journey.request_count();

    let human = cli(&mut service, &["list-characters"], NOW);
    assert_eq!(human.exit_code, 0);
    assert!(human.stderr.is_empty(), "success must not write to stderr");
    assert!(human.stdout.starts_with("characters: 2\n"));
    assert!(human.stdout.contains("id: 900000000000000001"));
    assert!(human.stdout.contains("class: Warlock"));
    assert!(human.stdout.contains("id: 900000000000000002"));
    assert!(human.stdout.ends_with(&format!(
        "revision: 1\nchecked_at: {}\nfreshness: fresh\n",
        NOW.seconds()
    )));
    assert!(
        human
            .stdout
            .chars()
            .all(|character| character == '\n' || !character.is_control()),
        "human output must not carry raw control characters"
    );
    assert_no_secrets(&human.stdout);

    let json = cli(&mut service, &["--json", "list-characters"], NOW);
    assert_eq!(json.exit_code, 0);
    assert!(json.stderr.is_empty());
    let value: Value = serde_json::from_str(json.stdout.trim()).expect("one JSON object");
    assert_eq!(value["kind"].as_str(), Some("character_list"));
    assert_eq!(value["metadata"]["revision"].as_i64(), Some(1));
    assert_eq!(value["metadata"]["source"].as_str(), Some("profile"));
    assert_eq!(value["metadata"]["freshness"].as_str(), Some("fresh"));
    assert_eq!(value["characters"].as_array().expect("characters").len(), 2);
    assert_no_secrets(&json.stdout);

    // Both reads reused the fresh projection: the gateway never ran again.
    assert_eq!(journey.request_count(), requests_before);
}

/// CLI journey: the real CLI search reports bounded, deterministic definition
/// candidates and pairs instance hits with their exact definition, never
/// collapsing multiple candidates into one guessed match.
#[test]
fn cli_search_inventory_reports_bounded_candidates_and_paired_hits() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let human = cli(
        &mut service,
        &["search-inventory", "--text", "gjallar"],
        NOW,
    );
    assert_eq!(human.exit_code, 0);
    assert!(human.stderr.is_empty());
    assert!(human.stdout.starts_with("candidates: 2\n"));
    assert!(human.stdout.contains("hash: 1001; name: Gjallarhorn"));
    assert!(human.stdout.contains("hash: 1003; name: Gjallarswift"));
    assert!(human.stdout.contains("hits: 2\n"));
    assert_no_secrets(&human.stdout);

    // Two "sword" definitions stay explicit candidates with matching owned hits.
    let ambiguous = cli(
        &mut service,
        &["--json", "search-inventory", "--text", "sword"],
        NOW,
    );
    assert_eq!(ambiguous.exit_code, 0);
    let value: Value = serde_json::from_str(ambiguous.stdout.trim()).expect("one JSON object");
    assert_eq!(value["kind"].as_str(), Some("inventory_search"));
    let hashes: Vec<i64> = value["candidates"]
        .as_array()
        .expect("candidates")
        .iter()
        .map(|candidate| candidate["hash"].as_i64().expect("hash"))
        .collect();
    assert_eq!(hashes, [2001, 2003]);
    assert_eq!(value["hits"].as_array().expect("hits").len(), 2);

    // The validated shared limit bounds the candidate list.
    let bounded = cli(
        &mut service,
        &["search-inventory", "--text", "sword", "--limit", "1"],
        NOW,
    );
    assert!(bounded.stdout.starts_with("candidates: 1\n"));
    assert!(bounded.stdout.contains("hash: 2001; name: Sword of Crota"));
    assert!(!bounded.stdout.contains("hash: 2003"));

    // An instance filter pairs the concrete instance with its exact definition.
    let paired = cli(
        &mut service,
        &[
            "--json",
            "search-inventory",
            "--detail",
            "--text",
            "gjallar",
            "--item-hash",
            "1001",
        ],
        NOW,
    );
    assert_eq!(paired.exit_code, 0);
    let value: Value = serde_json::from_str(paired.stdout.trim()).expect("one JSON object");
    let hits = value["hits"].as_array().expect("hits");
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0]["instance"]["instance_id"].as_str(),
        Some(INSTANCE_VAULT)
    );
    assert_eq!(hits[0]["instance"]["item_hash"].as_i64(), Some(1001));
    assert_eq!(hits[0]["definition"]["hash"].as_i64(), Some(1001));
    assert_no_secrets(&paired.stdout);
}

/// CLI journey: the real CLI inspection keeps definition and instance identity
/// distinct, renders both modes, and reports a missing instance as the typed
/// `item_not_found`/32 outcome with an empty stdout.
#[test]
fn sqlite_continuation_and_exact_name_inspection() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let first = cli_json(
        &mut service,
        &[
            "--json",
            "search-inventory",
            "--text",
            "sword",
            "--limit",
            "1",
        ],
    );
    assert_eq!(first["next_offset"], 1);
    let last = cli_json(
        &mut service,
        &[
            "--json",
            "search-inventory",
            "--text",
            "sword",
            "--limit",
            "1",
            "--offset",
            "1",
        ],
    );
    assert!(last.get("next_offset").is_none());
    assert_ne!(
        first["hits"][0]["instance_id"],
        last["hits"][0]["instance_id"]
    );
    assert_eq!(first["candidates"], last["candidates"]);
    let unique = cli_json(
        &mut service,
        &["--json", "inspect-item", "--name", "  GJALLARHORN  "],
    );
    assert_eq!(unique["instance"]["instance_id"], INSTANCE_VAULT);
    for name in ["gjallar", "Unknown"] {
        let missing = cli(
            &mut service,
            &["--json", "inspect-item", "--name", name],
            NOW,
        );
        assert_eq!(missing.exit_code, 32);
    }
    let mut same_names = definitions();
    same_names
        .iter_mut()
        .find(|definition| definition.hash == ItemHash::new(2003))
        .unwrap()
        .name = "Sword of Crota".into();
    service
        .repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new(DEFINITION_VERSION),
                definitions: same_names,
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .unwrap();
    let ambiguous = cli(
        &mut service,
        &["--json", "inspect-item", "--name", "Sword of Crota"],
        NOW,
    );
    let error: Value = serde_json::from_str(&ambiguous.stderr).unwrap();
    assert_eq!(error["error"]["code"], "ambiguous_match");
    let candidates = error["error"]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(
        |candidate| candidate["instance_id"].is_string() && candidate.get("instance").is_none()
    ));
    assert_no_secrets(&ambiguous.stderr);
}

#[test]
fn cli_inspect_item_renders_exact_identity_and_a_typed_missing_item() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let json = cli(
        &mut service,
        &["--json", "inspect-item", INSTANCE_VAULT],
        NOW,
    );
    assert_eq!(json.exit_code, 0);
    assert!(json.stderr.is_empty());
    let value: Value = serde_json::from_str(json.stdout.trim()).expect("one JSON object");
    assert_eq!(value["kind"].as_str(), Some("item_inspection"));
    assert_eq!(value["definition"]["hash"].as_i64(), Some(1001));
    assert_eq!(
        value["instance"]["instance_id"].as_str(),
        Some(INSTANCE_VAULT)
    );
    assert_eq!(value["instance"]["item_hash"].as_i64(), Some(1001));
    assert_eq!(value["metadata"]["freshness"].as_str(), Some("fresh"));
    assert_no_secrets(&json.stdout);

    let human = cli(&mut service, &["inspect-item", INSTANCE_VAULT], NOW);
    assert_eq!(human.exit_code, 0);
    assert!(human.stderr.is_empty());
    assert!(
        human
            .stdout
            .starts_with(&format!("instance: {INSTANCE_VAULT}\n"))
    );
    assert!(human.stdout.contains("  item_hash: 1001\n"));
    assert!(human.stdout.contains("  location: vault\n"));

    let missing = cli(&mut service, &["inspect-item", "700000000000000099"], NOW);
    assert_eq!(missing.exit_code, 32);
    assert!(
        missing.stdout.is_empty(),
        "a failure must not write to stdout"
    );
    assert_eq!(
        missing.stderr,
        "error: the requested item instance was not found\n"
    );
}

/// CLI journey: a stale read refreshes the projection exactly once through the
/// shared freshness policy and reports the advanced committed metadata.
#[test]
fn cli_stale_read_refreshes_once_and_reports_the_advanced_metadata() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let after_seed = journey.request_count();

    let stale_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(stale_now);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);

    let output = cli(&mut service, &["list-characters"], stale_now);
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    assert_eq!(journey.request_count(), after_seed + 2);
    assert!(output.stdout.ends_with(&format!(
        "revision: 1\nchecked_at: {}\nfreshness: fresh\n",
        stale_now.seconds()
    )));
}

/// CLI journey: a decoded provider failure reaches the process boundary as a
/// sanitized typed outcome on stderr only, with no raw payload, credential, or
/// membership sentinel on either stream.
#[test]
fn cli_provider_failure_is_typed_and_sanitized_on_stderr_only() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let failure_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(failure_now);
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());

    let json = cli(
        &mut service,
        &["--json", "search-inventory", "--text", "gjallar"],
        failure_now,
    );
    assert_eq!(json.exit_code, 2);
    assert!(json.stdout.is_empty(), "a failure must not write to stdout");
    let value: Value = serde_json::from_str(json.stderr.trim()).expect("one JSON error object");
    assert_eq!(value["error"]["code"].as_str(), Some("invalid_request"));
    assert_no_secrets(&json.stderr);

    // The same decoded failure renders as one sanitized human line.
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let human = cli(
        &mut service,
        &["search-inventory", "--text", "gjallar"],
        failure_now,
    );
    assert_eq!(human.exit_code, 2);
    assert!(human.stdout.is_empty());
    assert!(human.stderr.starts_with("error: "));
    assert_no_secrets(&human.stderr);
}

/// CLI/MCP read parity: the real CLI JSON reads and the real MCP read
/// projections are driven over the *same* composed service, clock, and committed
/// SQLite fixture state, then compared on their shared semantic fields.
///
/// Parity is never asserted as whole-payload JSON identity: the MCP projection
/// deliberately adds sanitized metadata (`dataset`, `definition_version`) and the
/// exact perk hash, and it deliberately drops the provider-derived text behind
/// `LocationView::Other`. `assert_shared` excludes exactly those fields at exactly
/// those views, and each removed field is pinned separately below.
///
/// The CLI read is read-only; the MCP mutation surface stays the separate
/// one-call authorization journey proven elsewhere. Nothing here touches the
/// network, a live account, the OS keyring, or a persisted secret.
#[test]
fn cli_and_mcp_reads_share_semantics_with_pinned_key_output() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);
    let after_seed = journey.request_count();
    // The real MCP read seam the stdio server calls, wrapped around the same
    // composed service, clock, and policy the CLI reads through.
    let mut mcp = McpReads::new(service, TestClock(Rc::clone(&journey.clock)), TTL);

    // Every read is fresh on the one committed revision, so both boundaries share
    // this metadata; the MCP side adds exactly the dataset code and Manifest
    // version that the CLI never renders.
    let metadata = json!({
        "checked_at": NOW.seconds(),
        "freshness": "fresh",
        "revision": 1,
        "source": "profile",
    });
    let mut mcp_metadata = metadata.clone();
    mcp_metadata["dataset"] = json!("profile_inventory");
    mcp_metadata["definition_version"] = json!(DEFINITION_VERSION);

    // ---- list_characters: the exact two-character view, identical on both
    // boundaries because a character carries no MCP-only field. ----
    let cli_list = cli_json(mcp.queries_mut(), &["--json", "list-characters"]);
    let mcp_list = mcp_read_list(&mut mcp);
    assert_shared(&cli_list, &mcp_list, "list");
    assert_eq!(cli_list["metadata"], metadata);
    assert_eq!(mcp_list["metadata"], mcp_metadata);
    let characters = json!([
        {"class": "Warlock", "gender": "Male", "id": CHARACTER_ONE, "light": 1810, "name": null, "race": "Awoken"},
        {"class": "Hunter", "gender": "Female", "id": CHARACTER_TWO, "light": 1795, "name": null, "race": "Human"},
    ]);
    assert_eq!(cli_list["characters"], characters);
    assert_eq!(mcp_list["characters"], characters);

    // ---- search_inventory: one bounded read covers the vaulted item, the
    // `Other`-located item, and the equipped item carrying perks and power. ----
    let cli_search = cli_json(
        mcp.queries_mut(),
        &[
            "--json",
            "search-inventory",
            "--detail",
            "--item-hash",
            "1001",
            "--item-hash",
            "1003",
            "--item-hash",
            "3001",
        ],
    );
    let mcp_search = mcp_read_search(
        &mut mcp,
        json!({"detail": true, "instances": {"item_hashes": [1001, 1003, 3001]}}),
    );
    assert_shared(&cli_search, &mcp_search, "search");
    assert_eq!(cli_search["metadata"], metadata);
    assert_eq!(mcp_search["metadata"], mcp_metadata);
    // The MCP definition view carries the Manifest version the CLI never renders.
    assert_eq!(
        mcp_search["candidates"][0]["definition_version"],
        DEFINITION_VERSION
    );

    // The vaulted hit is pinned exactly. Its instance carries no MCP-only field,
    // so both boundaries agree byte for byte on it, which proves the MCP instance
    // adds nothing outside the documented differences.
    let cli_vault = hit_of(&cli_search["hits"], INSTANCE_VAULT);
    let vault_instance = json!({
        "equipped": false, "instance_id": INSTANCE_VAULT, "item_hash": 1001,
        "location": {"kind": "vault"}, "locked": true, "perks": [], "power": 1800, "quantity": 1,
    });
    assert_eq!(cli_vault["instance"], vault_instance);
    assert_eq!(
        hit_of(&mcp_search["hits"], INSTANCE_VAULT)["instance"],
        vault_instance
    );
    assert_eq!(
        cli_vault["definition"],
        json!({
            "bucket_hash": null, "element": null, "hash": 1001, "item_type": "Rocket Launcher",
            "name": "Gjallarhorn", "tags": ["exotic", "solar"], "tier": null,
        })
    );

    // The `Other`-located hit carries the one CLI-only field: the provider
    // location text. The MCP `Other` view keeps the kind and drops the text, so
    // no provider string crosses the MCP boundary.
    assert_eq!(
        hit_of(&cli_search["hits"], INSTANCE_OTHER)["instance"]["location"],
        json!({"kind": "other", "value": "bungie-profile-location:3"})
    );
    assert_eq!(
        hit_of(&mcp_search["hits"], INSTANCE_OTHER)["instance"]["location"],
        json!({"kind": "other"})
    );
    assert!(
        !mcp_search.to_string().contains("bungie-profile-location"),
        "the MCP projection must not carry provider Other location text"
    );

    // The equipped hit pins the perk views: the CLI keeps the sanitized
    // active/visible pair, the MCP view keeps the exact perk hash beside it.
    assert_eq!(
        hit_of(&cli_search["hits"], INSTANCE_CHAR_ONE_EQUIPPED)["instance"]["perks"],
        json!([
            {"active": true, "visible": true},
            {"active": false, "visible": false},
            {"active": false, "visible": true},
        ])
    );
    assert_eq!(
        hit_of(&mcp_search["hits"], INSTANCE_CHAR_ONE_EQUIPPED)["instance"]["perks"],
        json!([
            {"active": true, "perk_hash": 5001, "visible": true},
            {"active": false, "perk_hash": 5002, "visible": false},
            {"active": false, "perk_hash": 5001, "visible": true},
        ])
    );

    // ---- inspect_item: exact identity on both boundaries ----
    let cli_inspect = cli_json(
        mcp.queries_mut(),
        &["--json", "inspect-item", INSTANCE_CHAR_ONE_EQUIPPED],
    );
    let mcp_inspect = mcp_read_inspect(&mut mcp, INSTANCE_CHAR_ONE_EQUIPPED);
    assert_shared(&cli_inspect, &mcp_inspect, "inspect");
    assert_eq!(cli_inspect["metadata"], metadata);
    assert_eq!(mcp_inspect["metadata"], mcp_metadata);
    assert_eq!(mcp_inspect["status"], "found");
    // Definition identity and instance identity stay distinct on both sides, and
    // the MCP definition carries the pinned Manifest version.
    assert_eq!(cli_inspect["definition"]["hash"], 3001);
    assert_eq!(cli_inspect["instance"]["item_hash"], 3001);
    assert_eq!(
        mcp_inspect["definition"]["definition_version"],
        DEFINITION_VERSION
    );

    // ---- inspect_item missing: the one intentional representation difference.
    // The CLI returns a typed `item_not_found` process outcome on stderr with no
    // success object; the MCP read returns the explicit `{"status":"missing"}`
    // variant of the same shared `Resolution::Missing`. ----
    let mcp_missing = mcp_read_inspect(&mut mcp, INSTANCE_MISSING);
    assert_eq!(mcp_missing, json!({"status": "missing"}));
    let cli_missing = cli(
        mcp.queries_mut(),
        &["--json", "inspect-item", INSTANCE_MISSING],
        NOW,
    );
    assert_eq!(cli_missing.exit_code, 32);
    assert!(
        cli_missing.stdout.is_empty(),
        "a missing instance renders no CLI success object"
    );
    let cli_missing_error: Value =
        serde_json::from_str(cli_missing.stderr.trim()).expect("one JSON error object");
    assert_eq!(cli_missing_error["error"]["code"], "item_not_found");
    assert_eq!(
        cli_missing_error["error"]["message"],
        "the requested item instance was not found"
    );
    assert_no_secrets(&cli_missing.stderr);

    // Neither boundary refreshed the projection or touched the scripted gateway.
    assert_eq!(journey.request_count(), after_seed);
}
