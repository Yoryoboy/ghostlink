//! Composed, offline end-to-end profile synchronization journey.
//!
//! This integration test drives the real composed system instead of a stand-in
//! for it: the real [`BungieProfileGateway`] decodes the provider payloads, the
//! real [`SqliteStore`] persists the committed projection to an on-disk SQLite
//! database, and the real [`ProfileSyncService`] makes the freshness,
//! provenance, and revision decisions. Nothing at those layers is faked.
//!
//! The only fakes are the two outermost seams the production code already takes
//! by dependency injection:
//!
//! - a scripted [`ProfileTransport`] that returns crafted Bungie responses and
//!   records the requests the gateway builds, so the journey never touches the
//!   network and never needs live credentials; and
//! - a [`Clock`] whose `now` the test advances explicitly, so `checked_at`
//!   stamping and the freshness TTL are deterministic instead of wall-clock
//!   dependent.
//!
//! Every assertion is on public API behavior: the journey never reaches into
//! production internals, never substitutes a fake gateway or repository, and
//! never weakens an assertion to make it pass. When the composed behavior
//! differs from the contract, the test fails rather than bending the contract.
//!
//! Scenarios covered:
//!
//! 1. cold publish through SQLite;
//! 2. persistence across a real reopen of the same database file;
//! 3. ordinary fresh skip that performs no transport request;
//! 4. identical refresh that retains the revision and refreshes `checked_at`;
//! 5. materially changed content that advances the revision exactly once;
//! 6. partial, private, or malformed provider responses that preserve the prior
//!    projection; and
//! 7. credential and raw-payload secrecy in errors, disk, and the projection.
//!
//! [`BungieProfileGateway`]: destiny_adapters::profile_gateway::BungieProfileGateway
//! [`SqliteStore`]: destiny_adapters::sqlite::SqliteStore
//! [`ProfileSyncService`]: destiny_app::ProfileSyncService
//! [`ProfileTransport`]: destiny_adapters::profile_gateway::ProfileTransport
//! [`Clock`]: destiny_core::Clock

use destiny_adapters::profile_gateway::{
    BungieProfileGateway, MEMBERSHIP_ENDPOINT, ProfileApiKey, ProfileHttpRequest,
    ProfileHttpResponse, ProfileTransport, ProfileTransportError, profile_endpoint,
};
use destiny_adapters::sqlite::SqliteStore;
use destiny_app::ProfileSyncService;
use destiny_core::ports::{ReadRetry, ReadRetryPolicy, RetryAfterHeader, RetryDelay};
use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial,
    DefinitionVersion, DestinyMembership, ErrorCategory, FreshnessPolicy, InventoryRepository,
    InventorySnapshot, MembershipId, MembershipType, ProfileSyncReason, SourceMarker, Timestamp,
};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The effective Cross Save primary the whole journey resolves to. It matches
/// the membership the `profile-complete.json` fixture is consistent with.
const MEMBERSHIP_TYPE: u32 = 3;
const MEMBERSHIP_ID: &str = "4611686018484639825";

/// The definition version the gateway attaches to every normalized candidate.
const DEFINITION_VERSION: &str = "journey-definitions-v1";

/// Deterministic application time. The gateway stamps `checked_at` from the
/// injected clock; the freshness policy below is evaluated against the explicit
/// `now` the test passes to `sync`.
const NOW: Timestamp = Timestamp::new(1_724_000_000);
const CREDENTIAL_EXPIRES_AT: u64 = 4_000_000_000;
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// Distinctive sentinels for the two secrets and one raw-payload marker. None
/// of them may ever reach an error, the on-disk database, or the committed
/// projection.
const API_KEY_SENTINEL: &str = "sentinel-journey-api-key";
const BEARER_SENTINEL: &str = "sentinel-journey-bearer-token";
const PAYLOAD_SENTINEL: &str = "sentinel-journey-raw-payload";

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
/// The guard mirrors the convention already used by the SQLite repository
/// tests: one unique file per test derived from the process id and an atomic
/// counter, with the database and its `-wal`/`-shm`/`-journal` sidecars removed
/// on drop.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-profile-journey-{}-{sequence}.db",
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

/// The retry pacing seam: it records the exact bounded waits without sleeping,
/// so a retry scenario is deterministic and never depends on ambient time.
struct NoDelay(Arc<Mutex<Vec<Duration>>>);

impl RetryDelay for NoDelay {
    fn wait(&mut self, delay: Duration) {
        self.0.lock().expect("delay log lock").push(delay);
    }
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
/// advance time between syncs while the gateway keeps the same clock value.
struct TestClock(Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(self.0.get()))
    }
}

type Gateway = BungieProfileGateway<SharedTransport, TestClock>;
type Service = ProfileSyncService<Gateway, SqliteStore>;

/// The composed journey harness: one temporary database, one scripted transport,
/// and one controllable clock. `service()` composes a fresh real gateway and a
/// fresh real SQLite store over those shared seams, so a test can drop the store
/// and reopen it without losing the recorded request history.
struct Journey {
    database: TempDatabase,
    transport: Rc<RefCell<ScriptedTransport>>,
    clock: Rc<Cell<u64>>,
    delays: Arc<Mutex<Vec<Duration>>>,
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
            delays: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Advances the injected clock the gateway stamps `checked_at` from.
    fn at(&self, time: Timestamp) {
        self.clock.set(time.seconds());
    }

    /// Composes the real gateway and the real SQLite store over the shared
    /// transport and clock, with an injected bounded retry policy whose pacing
    /// is recorded rather than slept.
    fn service(&self) -> Service {
        let gateway = BungieProfileGateway::new(
            SharedTransport(Rc::clone(&self.transport)),
            TestClock(Rc::clone(&self.clock)),
            destiny_core::StaticCredentialSource::new(credential()),
            api_key(),
            DefinitionVersion::new(DEFINITION_VERSION),
        )
        .expect("valid gateway configuration")
        .with_read_retry(ReadRetry::new(
            ReadRetryPolicy::new(3, Duration::from_millis(10), Duration::from_secs(5)),
            NoDelay(Arc::clone(&self.delays)),
        ));
        let store = SqliteStore::open(self.database.path()).expect("open file store");
        ProfileSyncService::new(gateway, store)
    }

    /// Queues one bare status response, used to script a transient failure.
    fn queue_status(&self, status: u16) {
        self.transport
            .borrow_mut()
            .responses
            .push_back(Ok(status_response(status)));
    }

    /// Queues one `429` carrying the given classified `Retry-After` hint.
    fn queue_rate_limit(&self, retry_after: RetryAfterHeader) {
        self.transport
            .borrow_mut()
            .responses
            .push_back(Ok(status_response(429).with_retry_after(retry_after)));
    }

    /// The exact bounded waits the gateway performed, in order.
    fn waits(&self) -> Vec<Duration> {
        self.delays.lock().expect("delay log lock").clone()
    }

    /// Queues the Cross Save membership discovery body.
    fn queue_membership(&self) {
        self.queue_json(membership_body());
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

fn membership_body() -> Vec<u8> {
    MEMBERSHIP_BODY.as_bytes().to_vec()
}

/// A bare status response with an empty body, used to script a transient
/// provider failure without a decoded payload.
fn status_response(status: u16) -> ProfileHttpResponse {
    ProfileHttpResponse::new(
        status,
        Some(0),
        Some("application/json".to_owned()),
        Box::new(Cursor::new(Vec::new())),
    )
}

fn expected_membership() -> DestinyMembership {
    DestinyMembership::new(
        MembershipType::new(MEMBERSHIP_TYPE),
        MembershipId::new(MEMBERSHIP_ID),
    )
    .expect("fixture membership is canonical")
}

/// The committed projection read back through the real repository.
fn committed(service: &Service) -> InventorySnapshot {
    service
        .repository()
        .load_current()
        .expect("repository read")
        .expect("committed projection")
}

/// The vaulted gear instance used by the existing quantity-change journey.
const CHANGED_INSTANCE: &str = "700000000000000001";

#[test]
fn ordinary_minted_guard_survives_effects_reopen_and_keeps_stale_freshness() {
    use destiny_core::{InventoryEffect, ProfileSyncOutcome};
    let journey = Journey::new();
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    value["Response"]["responseMintedTimestamp"] = json!("2026-10-08T21:09:13.123Z");
    value["Response"]["characterLoadouts"] = json!({"data":{"1":{"loadouts":[{"nameHash":10,"colorHash":20,"iconHash":30,"items":[{"itemInstanceId":"456","plugItemHashes":[42]}]}]}}});
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    let mut service = journey.service();
    assert!(
        service
            .sync(ProfileSyncReason::Ordinary, TTL, NOW)
            .unwrap()
            .refreshed()
    );
    let minted = service
        .repository()
        .profile_minted_at()
        .unwrap()
        .expect("gateway forwarded minted evidence");
    let saved = service.repository().saved_loadouts().unwrap().unwrap();
    let effected = service
        .repository_mut()
        .apply_acknowledged_effects(
            &expected_membership(),
            &[InventoryEffect::SetLocked {
                item: CHANGED_INSTANCE.into(),
                locked: true,
            }],
        )
        .unwrap();
    drop(service);
    let mut service = journey.service();
    assert_eq!(
        service.repository().saved_loadouts().unwrap(),
        Some(saved.clone())
    );
    value["Response"]["characterLoadouts"] = json!({"data":{}});
    let stale_now = Timestamp::new(NOW.seconds() + 301);
    journey.at(stale_now);
    for time in [
        "2026-10-08T21:09:13.122999999Z",
        "2026-10-08T23:09:13.123+02:00",
    ] {
        value["Response"]["responseMintedTimestamp"] = json!(time);
        journey.queue_membership();
        journey.queue_profile(&serde_json::to_vec(&value).unwrap());
        assert!(matches!(
            service
                .sync(ProfileSyncReason::Ordinary, TTL, stale_now)
                .unwrap(),
            ProfileSyncOutcome::DiscardedNotNewer { .. }
        ));
        assert_eq!(committed(&service), effected);
        assert_eq!(
            service.repository().saved_loadouts().unwrap(),
            Some(saved.clone())
        );
        assert_eq!(
            service.repository().profile_minted_at().unwrap(),
            Some(minted)
        );
    }
    value["Response"]["responseMintedTimestamp"] = json!("2026-10-08T21:09:13.123000001Z");
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    assert!(
        service
            .sync(ProfileSyncReason::Ordinary, TTL, stale_now)
            .unwrap()
            .refreshed()
    );
    assert!(service.repository().profile_minted_at().unwrap().unwrap() > minted);
    assert_eq!(committed(&service).metadata.checked_at, stale_now);
}

/// Actual decoder -> injected transport gateway -> sync -> real in-memory SQLite.
/// Runtime buckets are synthetic, not live-account or capacity evidence.
#[test]
fn published_observations_follow_exact_sqlite_return_not_revision_identity() {
    use destiny_app::ProfileSyncWithObservationsOutcome;
    let journey = Journey::new();
    let gateway = BungieProfileGateway::new(
        SharedTransport(Rc::clone(&journey.transport)),
        TestClock(Rc::clone(&journey.clock)),
        destiny_core::StaticCredentialSource::new(credential()),
        api_key(),
        DefinitionVersion::new(DEFINITION_VERSION),
    )
    .unwrap();
    let mut service = ProfileSyncService::new(gateway, SqliteStore::open_in_memory().unwrap());
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    for row in value["Response"]["profileInventory"]["data"]["items"]
        .as_array_mut()
        .unwrap()
    {
        if row["itemInstanceId"].is_string() {
            row["bucketHash"] = json!(5100);
        }
    }
    for component in ["characterInventories", "characterEquipment"] {
        for owner in value["Response"][component]["data"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            for row in owner["items"].as_array_mut().unwrap() {
                if row["itemInstanceId"].is_string() {
                    row["bucketHash"] = json!(5200);
                }
            }
        }
    }
    for (offset, bucket, quantity, revision) in [(0, 5100, 1, 1), (1, 5101, 1, 1), (2, 5101, 2, 2)]
    {
        let now = Timestamp::new(NOW.seconds() + offset);
        journey.at(now);
        value["Response"]["profileInventory"]["data"]["items"][0]["bucketHash"] = json!(bucket);
        value["Response"]["profileInventory"]["data"]["items"][0]["quantity"] = json!(quantity);
        journey.queue_membership();
        journey.queue_profile(&serde_json::to_vec(&value).unwrap());
        let ProfileSyncWithObservationsOutcome::Refreshed(pair) = service
            .sync_with_observations(ProfileSyncReason::Preflight, TTL, now)
            .unwrap()
        else {
            panic!("forced publication required")
        };
        let snapshot = pair.snapshot();
        assert_eq!(
            snapshot,
            &service.repository().load_current().unwrap().unwrap()
        );
        assert_eq!(
            snapshot.metadata.membership_id,
            *expected_membership().membership_id()
        );
        assert_eq!(
            snapshot.metadata.membership_type,
            expected_membership().membership_type()
        );
        assert_eq!(
            snapshot.metadata.definition_version,
            DefinitionVersion::new(DEFINITION_VERSION)
        );
        assert_eq!(snapshot.metadata.source, SourceMarker::Profile);
        assert_eq!(snapshot.metadata.checked_at, now);
        assert_eq!(snapshot.metadata.revision.value(), revision);
        let observations = pair.observations().expect("complete runtime buckets");
        assert_eq!(observations.len(), snapshot.items.len());
        for item in &snapshot.items {
            let row = observations
                .iter()
                .find(|row| row.instance_id() == &item.instance_id)
                .unwrap();
            assert_eq!(row.location(), &item.location);
        }
        assert_eq!(
            observations
                .iter()
                .find(|row| row.instance_id().as_str() == CHANGED_INSTANCE)
                .unwrap()
                .bucket_hash()
                .value(),
            bucket
        );
        assert_eq!(journey.request_count(), 2 * (offset as usize + 1));
    }
    let before = service.repository().load_current().unwrap().unwrap();
    assert!(matches!(
        service
            .sync_with_observations(ProfileSyncReason::Ordinary, TTL, before.metadata.checked_at)
            .unwrap(),
        ProfileSyncWithObservationsOutcome::SkippedFresh { .. }
    ));
    assert_eq!(journey.request_count(), 6);
    // Invalid instance component join fails before SQLite publication, preserving
    // the real prior projection. This is not a rollback promise for bad returns.
    value["Response"]["itemComponents"]["instances"]["data"]
        .as_object_mut()
        .unwrap()
        .remove(CHANGED_INSTANCE);
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    assert_eq!(
        service
            .sync_with_observations(ProfileSyncReason::PostAction, TTL, NOW)
            .unwrap_err()
            .category(),
        ErrorCategory::ProviderValidation
    );
    assert_eq!(
        service.repository().load_current().unwrap().unwrap(),
        before
    );
    assert_eq!(journey.request_count(), 8);
}

/// Synthetic bucket hashes exercise provenance only, not provider bucket contracts.
#[test]
fn profile_stack_multiset_and_postmaster_survive_sqlite_sync() {
    use destiny_core::{CharacterId, ItemLocation, StackContainer};
    let journey = Journey::new();
    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    let rows =
        &mut value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"];
    rows[0]["location"] = json!(4);
    rows[1]["location"] = json!(4);
    let duplicate = rows[1].clone();
    rows.as_array_mut().unwrap().push(duplicate);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut service = journey.service();
    journey.queue_membership();
    journey.queue_profile(&bytes);
    service.sync(ProfileSyncReason::Ordinary, TTL, NOW).unwrap();
    let first = committed(&service);
    let owner = CharacterId::new("900000000000000001");
    assert_eq!(first.stacks.len(), 4);
    assert_eq!(
        first
            .stacks
            .iter()
            .filter(|row| row.container() == &StackContainer::Postmaster(owner.clone()))
            .count(),
        2
    );
    assert_eq!(
        first
            .items
            .iter()
            .find(|item| item.instance_id.as_str() == "700000000000000004")
            .unwrap()
            .location,
        ItemLocation::Postmaster(owner)
    );
    drop(service);
    let mut service = journey.service();
    assert_eq!(committed(&service), first);

    value["Response"]["profileInventory"]["data"]["items"]
        .as_array_mut()
        .unwrap()
        .reverse();
    value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"]
        .as_array_mut()
        .unwrap()
        .reverse();
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    let reordered = service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
    assert_eq!(reordered.revision(), first.metadata.revision);
    value["Response"]["characterInventories"]["data"]["900000000000000001"]["items"][0]["quantity"] =
        json!(49);
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    let changed = service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
    assert_eq!(
        changed.revision().value(),
        first.metadata.revision.value() + 1
    );
    assert_eq!(committed(&service).stacks.len(), 4);
}

/// Synthetic component wrappers exercise durable availability, never live coverage.
#[test]
fn profile_currency_coverage_balances_and_atomic_rejection_survive_reopen() {
    use destiny_core::{ProfileCurrenciesCoverage as Coverage, StackContainer};
    let journey = Journey::new();
    let mut service = journey.service();
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service.sync(ProfileSyncReason::Ordinary, TTL, NOW).unwrap();
    let legacy = committed(&service);
    assert_eq!(legacy.profile_currencies_coverage, Coverage::Unavailable);
    assert!(
        journey.recorded_urls()[1]
            .ends_with("?components=102,103,200,201,205,206,300,302,304,305,310")
    );
    drop(service);
    let mut service = journey.service();
    assert_eq!(committed(&service), legacy);

    let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
    value["Response"]["profileCurrencies"] = json!({"privacy":2,"data":{"items":[]}});
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
    let empty = committed(&service);
    assert_eq!(empty.profile_currencies_coverage, Coverage::Available);
    assert_eq!(
        empty.metadata.revision.value(),
        legacy.metadata.revision.value() + 1
    );
    assert_eq!(empty.stacks, legacy.stacks);

    let overlap = value["Response"]["profileInventory"]["data"]["items"][2].clone();
    let mut zero = overlap.clone();
    zero["quantity"] = json!(0);
    value["Response"]["profileCurrencies"]["data"]["items"] =
        json!([overlap.clone(), overlap, zero]);
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
    let balances = committed(&service);
    assert_eq!(
        balances.metadata.revision.value(),
        empty.metadata.revision.value() + 1
    );
    assert_eq!(balances.stacks.len(), legacy.stacks.len() + 3);
    assert_eq!(
        balances
            .stacks
            .iter()
            .filter(|s| s.container() == &StackContainer::ProfileCurrencies)
            .count(),
        3
    );
    assert_eq!(
        balances
            .stacks
            .iter()
            .filter(|s| s.container() == &StackContainer::ProfileInventory)
            .count(),
        2
    );
    drop(service);
    let mut service = journey.service();
    assert_eq!(committed(&service), balances);

    value["Response"]["profileCurrencies"]["data"]["items"]
        .as_array_mut()
        .unwrap()
        .reverse();
    journey.queue_membership();
    journey.queue_profile(&serde_json::to_vec(&value).unwrap());
    service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
    assert_eq!(
        committed(&service).metadata.revision,
        balances.metadata.revision
    );
    for (field, invalid) in [
        ("quantity", json!(-1)),
        ("quantity", json!(2147483648u64)),
        ("itemHash", json!(4294967296u64)),
        ("bucketHash", Value::Null),
        ("itemInstanceId", json!("0")),
    ] {
        let mut bad = value.clone();
        bad["Response"]["profileInventory"]["data"]["items"][0]["quantity"] = json!(2);
        bad["Response"]["profileCurrencies"]["data"]["items"][0][field] = invalid;
        journey.queue_membership();
        journey.queue_profile(&serde_json::to_vec(&bad).unwrap());
        assert_eq!(
            service
                .sync(ProfileSyncReason::Forced, TTL, NOW)
                .unwrap_err()
                .category(),
            ErrorCategory::ProviderValidation
        );
        assert_eq!(committed(&service), balances);
    }
    for wrapper in [
        Value::Null,
        json!({"disabled":true}),
        json!({"privacy":2,"data":null}),
    ] {
        value["Response"]["profileCurrencies"] = wrapper;
        journey.queue_membership();
        journey.queue_profile(&serde_json::to_vec(&value).unwrap());
        service.sync(ProfileSyncReason::Forced, TTL, NOW).unwrap();
        assert_eq!(
            committed(&service).profile_currencies_coverage,
            Coverage::Unavailable
        );
        assert_eq!(committed(&service).stacks, legacy.stacks);
    }
    let unavailable = committed(&service);
    drop(service);
    assert_eq!(committed(&journey.service()), unavailable);
}

#[test]
fn profile_invalid_stack_rows_never_publish_partial_sqlite_content() {
    let journey = Journey::new();
    let mut service = journey.service();
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service.sync(ProfileSyncReason::Ordinary, TTL, NOW).unwrap();
    let before = committed(&service);
    for (field, invalid) in [
        ("bucketHash", Value::Null),
        ("bucketHash", json!(-1)),
        ("bucketHash", json!(4294967296u64)),
        ("itemHash", json!(-1)),
        ("itemHash", json!(4294967296u64)),
        ("quantity", json!(-1)),
        ("quantity", json!(0)),
        ("quantity", json!(2147483648u64)),
        ("quantity", json!(1.5)),
        ("location", json!(4)),
    ] {
        let mut value: Value = serde_json::from_slice(FIXTURE).unwrap();
        // A valid gear change before the invalid stack must not leak through.
        value["Response"]["profileInventory"]["data"]["items"][0]["quantity"] = json!(2);
        value["Response"]["profileInventory"]["data"]["items"][2][field] = invalid;
        journey.queue_membership();
        journey.queue_profile(&serde_json::to_vec(&value).unwrap());
        let error = service
            .sync(ProfileSyncReason::Forced, TTL, NOW)
            .unwrap_err();
        assert_eq!(error.category(), ErrorCategory::ProviderValidation);
        assert_eq!(committed(&service), before);
    }
    drop(service);
    assert_eq!(committed(&journey.service()), before);
}

/// A materially changed fixture: the vaulted item [`CHANGED_INSTANCE`] carries a
/// different quantity, which is part of revision identity.
fn changed_profile() -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).expect("fixture is JSON");
    value["Response"]["profileInventory"]["data"]["items"][0]["quantity"] = json!(2);
    serde_json::to_vec(&value).expect("mutated fixture serializes")
}

/// A profile response with a non-success `ErrorCode` and no usable payload.
fn non_success_profile() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "ErrorCode": 5,
        "ErrorStatus": "InvalidRequest",
        "Message": "provider-supplied text",
        "Response": {}
    }))
    .expect("payload serializes")
}

/// A profile response missing a required component wrapper.
fn missing_component_profile() -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).expect("fixture is JSON");
    value["Response"]
        .as_object_mut()
        .expect("response object")
        .remove("profileInventory");
    serde_json::to_vec(&value).expect("payload serializes")
}

/// A profile response whose character component is privacy-denied: private
/// and, unlike an owner-authorized response, without `data`.
fn private_component_profile() -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(FIXTURE).expect("fixture is JSON");
    let characters = &mut value["Response"]["characters"];
    characters["privacy"] = json!(2);
    characters
        .as_object_mut()
        .expect("characters")
        .remove("data");
    serde_json::to_vec(&value).expect("payload serializes")
}

/// A failing profile response carrying the raw-payload sentinel in a provider
/// message field, which must never reach an error or disk.
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

/// Scenario 1: an empty store plus an ordinary sync over a missing projection
/// publishes revision 1 through the real SQLite repository, and the committed
/// metadata carries the profile provenance, the gateway-stamped `checked_at`,
/// and the constructed definition version.
#[test]
fn cold_publish_commits_the_first_revision_through_sqlite() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);

    let outcome = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");
    assert!(outcome.refreshed());
    assert_eq!(outcome.revision().value(), 1);

    let snapshot = committed(&service);
    assert_eq!(snapshot.metadata.revision.value(), 1);
    assert_eq!(snapshot.metadata.source, SourceMarker::Profile);
    assert_eq!(snapshot.metadata.checked_at, NOW);
    assert_eq!(
        snapshot.metadata.definition_version,
        DefinitionVersion::new(DEFINITION_VERSION)
    );
    assert_eq!(
        snapshot.metadata.membership_id.as_str(),
        expected_membership().membership_id().as_str()
    );
    assert_eq!(
        snapshot.metadata.membership_type,
        expected_membership().membership_type()
    );
    assert_eq!(outcome.metadata(), &snapshot.metadata);

    // The normalized projection reached the repository: the fixture's retained
    // characters and items are all present.
    assert_eq!(snapshot.characters.len(), 2);
    assert_eq!(snapshot.items.len(), 6);

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

/// Scenario 2: reopening the same on-disk database with a fresh store reads back
/// an equal projection, proving normalization reached durable storage rather
/// than an in-memory cache.
#[test]
fn persistence_across_reopen_reads_back_an_equal_projection() {
    let journey = Journey::new();
    journey.at(NOW);

    let published = {
        let mut service = journey.service();
        journey.queue_membership();
        journey.queue_profile(FIXTURE);
        service
            .sync(ProfileSyncReason::Ordinary, TTL, NOW)
            .expect("cold publish succeeds");
        committed(&service)
    };

    // Drop the first store and gateway, then reopen the same file.
    let reopened = journey.service();
    let reloaded = reopened
        .repository()
        .load_current()
        .expect("repository read")
        .expect("reopened projection");

    assert_eq!(reloaded, published);
    assert_eq!(reloaded.metadata, published.metadata);
    assert_eq!(reloaded.characters, published.characters);
    assert_eq!(reloaded.items, published.items);
    assert_eq!(
        reloaded
            .items
            .iter()
            .find(|item| item.instance_id.as_str() == CHANGED_INSTANCE)
            .expect("vault item")
            .perks,
        published
            .items
            .iter()
            .find(|item| item.instance_id.as_str() == CHANGED_INSTANCE)
            .expect("vault item")
            .perks
    );
}

/// Scenario 3: a second ordinary sync inside the freshness TTL returns
/// `SkippedFresh` with unchanged metadata and performs no transport request.
#[test]
fn ordinary_fresh_skip_does_not_touch_the_transport() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");
    let before = journey.request_count();
    let published = committed(&service);

    let skipped = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("fresh skip succeeds");
    assert!(!skipped.refreshed());
    assert_eq!(skipped.metadata(), &published.metadata);
    assert_eq!(committed(&service), published);
    assert_eq!(
        journey.request_count(),
        before,
        "a fresh skip must not perform a transport request"
    );

    // A repeated identical ordinary request is another no-op skip.
    let repeat = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("second fresh skip succeeds");
    assert!(!repeat.refreshed());
    assert_eq!(repeat.metadata(), skipped.metadata());
    assert_eq!(journey.request_count(), before);
}

/// Scenario 4: a forced sync and, separately, an ordinary sync past the TTL both
/// yield byte-identical normalized content; the revision is retained while the
/// committed `checked_at` refreshes.
#[test]
fn identical_refresh_retains_revision_and_refreshes_checked_at() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let first = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");
    assert_eq!(first.revision().value(), 1);
    assert_eq!(committed(&service).metadata.checked_at, NOW);

    // Forced refresh of identical content at a later instant.
    let forced_now = Timestamp::new(NOW.seconds() + 100);
    journey.at(forced_now);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let forced = service
        .sync(ProfileSyncReason::Forced, TTL, forced_now)
        .expect("forced refresh succeeds");
    assert!(forced.refreshed());
    assert_eq!(forced.revision().value(), 1);
    assert_eq!(committed(&service).metadata.checked_at, forced_now);

    // Ordinary refresh one second past the TTL of the refreshed timestamp.
    let stale_now = Timestamp::new(forced_now.seconds() + TTL.ttl_seconds + 1);
    journey.at(stale_now);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let stale = service
        .sync(ProfileSyncReason::Ordinary, TTL, stale_now)
        .expect("stale refresh succeeds");
    assert!(stale.refreshed());
    assert_eq!(stale.revision().value(), 1);
    assert_eq!(committed(&service).metadata.checked_at, stale_now);
}

/// Scenario 5: a refresh whose profile content changed materially advances the
/// revision exactly once, and the reopened database reflects the new content.
#[test]
fn materially_changed_profile_advances_the_revision_once() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");
    assert_eq!(committed(&service).metadata.revision.value(), 1);

    let changed_now = Timestamp::new(NOW.seconds() + 1_000);
    journey.at(changed_now);
    journey.queue_membership();
    journey.queue_profile(&changed_profile());
    let changed = service
        .sync(ProfileSyncReason::Forced, TTL, changed_now)
        .expect("changed refresh succeeds");
    assert_eq!(changed.revision().value(), 2);

    // A further refresh of the same changed content retains the revision, so the
    // material change advanced it exactly once.
    journey.queue_membership();
    journey.queue_profile(&changed_profile());
    let unchanged = service
        .sync(ProfileSyncReason::Forced, TTL, changed_now)
        .expect("identical refresh succeeds");
    assert_eq!(unchanged.revision().value(), 2);

    // The reopened on-disk database reflects the new content.
    let reopened = journey.service();
    let reloaded = reopened
        .repository()
        .load_current()
        .expect("repository read")
        .expect("reopened projection");
    assert_eq!(reloaded.metadata.revision.value(), 2);
    let item = reloaded
        .items
        .iter()
        .find(|item| item.instance_id.as_str() == CHANGED_INSTANCE)
        .expect("changed item");
    assert_eq!(item.quantity, 2);
}

/// Scenario 6: a private, partial, or malformed profile response makes `sync`
/// return a sanitized error and leaves the previous projection readably
/// unchanged at the same revision.
#[test]
fn failed_refresh_preserves_the_committed_projection() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");
    let prior = committed(&service);
    assert_eq!(prior.metadata.revision.value(), 1);

    let cases: [(&str, Vec<u8>); 3] = [
        ("non-success error code", non_success_profile()),
        ("missing required component", missing_component_profile()),
        ("privacy-denied component", private_component_profile()),
    ];

    for (label, body) in cases {
        journey.queue_membership();
        journey.queue_profile(&body);
        let error = service
            .sync(ProfileSyncReason::Forced, TTL, NOW)
            .expect_err(label);

        assert_eq!(
            error.category(),
            ErrorCategory::ProviderValidation,
            "{label}"
        );
        let after = committed(&service);
        assert_eq!(after, prior, "{label} must preserve the prior projection");
        assert_eq!(after.metadata.revision.value(), 1, "{label}");
    }
}

/// Scenario 7: the API key, the bearer token, and a raw-payload marker never
/// appear in error renderings, the committed projection, or the raw bytes of the
/// on-disk database file.
#[test]
fn secrets_and_payload_markers_never_reach_errors_or_disk() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("cold publish succeeds");

    // A failing refresh whose raw payload carries the marker sentinel.
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let error = service
        .sync(ProfileSyncReason::Forced, TTL, NOW)
        .expect_err("a non-success profile must fail");
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

    let snapshot = committed(&service);
    let rendered = format!("{snapshot:?}");
    for sentinel in sentinels {
        assert!(
            !rendered.contains(sentinel),
            "committed projection leaked {sentinel}"
        );
    }

    // Scan the database and every sidecar. The membership identifier is stored
    // in the clear, so its presence proves the scan actually read committed
    // data rather than an empty file.
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

/// Scenario 8: an ordinary sync repeats a transient provider-unavailable
/// membership read under the bounded policy, then publishes through SQLite. The
/// real gateway and the real sync service make this decision; only the retry
/// pacing is injected so the test never sleeps.
#[test]
fn ordinary_sync_retries_a_transient_membership_failure() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();

    journey.queue_status(503);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);

    let outcome = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("the retried membership read succeeds");
    assert!(outcome.refreshed());
    assert_eq!(outcome.revision().value(), 1);

    let urls = journey.recorded_urls();
    assert_eq!(urls.len(), 3, "one transient repeat then both reads");
    assert_eq!(urls[0], MEMBERSHIP_ENDPOINT);
    assert_eq!(urls[1], MEMBERSHIP_ENDPOINT);
    assert_eq!(urls[2], profile_endpoint(&expected_membership()));
    assert_eq!(journey.waits(), vec![Duration::from_millis(10)]);
}

/// Scenario 9: a preflight or post-action mutation observation reports a
/// provider-unavailable membership read after exactly one attempt and never
/// mutates the repository. This is the explicit counterpart to scenario 8.
#[test]
fn mutation_observation_does_not_retry_a_transient_membership_failure() {
    for reason in [ProfileSyncReason::Preflight, ProfileSyncReason::PostAction] {
        let journey = Journey::new();
        journey.at(NOW);
        let mut service = journey.service();

        journey.queue_status(503);

        let error = service
            .sync(reason, TTL, NOW)
            .expect_err("a mutation observation is one-shot for unavailability");
        assert_eq!(
            error.category(),
            ErrorCategory::ProviderUnavailable,
            "{reason:?}"
        );
        assert_eq!(journey.request_count(), 1, "{reason:?}");
        assert!(journey.waits().is_empty(), "{reason:?}");
        assert!(
            service
                .repository()
                .load_current()
                .expect("repository read")
                .is_none(),
            "{reason:?} must not publish a projection"
        );
    }
}

/// Scenario 10: a preflight or post-action mutation observation still repeats
/// the explicit provider rate-limit refusal, so a throttled observation is not
/// treated as an unavailable one.
#[test]
fn mutation_observation_still_retries_a_rate_limit() {
    for reason in [ProfileSyncReason::Preflight, ProfileSyncReason::PostAction] {
        let journey = Journey::new();
        journey.at(NOW);
        let mut service = journey.service();

        journey.queue_rate_limit(RetryAfterHeader::Absent);
        journey.queue_membership();
        journey.queue_profile(FIXTURE);

        let outcome = service
            .sync(reason, TTL, NOW)
            .expect("a rate-limited observation succeeds after one repeat");
        assert!(outcome.refreshed(), "{reason:?}");
        assert_eq!(journey.request_count(), 3, "{reason:?}");
        assert_eq!(
            journey.waits(),
            vec![Duration::from_millis(10)],
            "{reason:?}"
        );
    }
}

/// Scenario 11: an ordinary sync interprets a provider `Retry-After` against the
/// injected clock and the bounded policy. A date within the ceiling is honored
/// exactly; a date beyond it, a malformed instruction, and a delta beyond the
/// ceiling are surfaced as the sanitized rate limit without any repeat.
#[test]
fn ordinary_sync_bounds_retry_after_instructions() {
    // A date two seconds ahead is within the five-second policy ceiling.
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    journey.queue_rate_limit(RetryAfterHeader::HttpDate(NOW.seconds() + 2));
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect("a bounded date hint is honored");
    assert_eq!(journey.waits(), vec![Duration::from_secs(2)]);

    // A date beyond the ceiling must not be silently shortened, so the rate
    // limit is surfaced after one attempt.
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    journey.queue_rate_limit(RetryAfterHeader::HttpDate(NOW.seconds() + 600));
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let error = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect_err("a date beyond the ceiling is not retried");
    assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
    assert_eq!(journey.request_count(), 1);
    assert!(journey.waits().is_empty());

    // A delta in seconds beyond the ceiling behaves the same way.
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    journey.queue_rate_limit(RetryAfterHeader::DeltaSeconds(600));
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let error = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect_err("a delta beyond the ceiling is not retried");
    assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
    assert_eq!(journey.request_count(), 1);
    assert!(journey.waits().is_empty());

    // A malformed instruction is not guessed into a wait.
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    journey.queue_rate_limit(RetryAfterHeader::Invalid);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    let error = service
        .sync(ProfileSyncReason::Ordinary, TTL, NOW)
        .expect_err("a malformed instruction is not retried");
    assert_eq!(error.category(), ErrorCategory::ProviderRateLimit);
    assert_eq!(journey.request_count(), 1);
    assert!(journey.waits().is_empty());
}
