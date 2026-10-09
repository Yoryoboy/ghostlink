//! Consolidated cross-boundary leak journey for DHS-005.
//!
//! One coherent sentinel set — `destiny_app::test_support::leak_sentinels` — is
//! driven through the durable, process, and serialized boundaries so a value
//! proven absent at one boundary is the exact value probed at the others. The
//! MCP stdio child-process suite probes the same set.
//!
//! What is real here, not faked:
//!
//! - the real [`BungieProfileGateway`] decodes a crafted provider body;
//! - the real [`SqliteStore`] persists the committed projection to an on-disk
//!   SQLite file and its WAL/shm/journal sidecars;
//! - the real [`QueryService`] answers the reads; and
//! - the real CLI runner renders the exact bytes the `ghostlink` process
//!   writes with [`destiny_cli::run`].
//!
//! The only injected seams are the two the production code already takes by
//! dependency injection: a scripted [`ProfileTransport`], so the journey never
//! touches the network and never needs a live credential, and a [`Clock`] the
//! test advances explicitly, so freshness is deterministic.
//!
//! # Positive controls
//!
//! A clean scan or stream is only meaningful when the surface was genuinely
//! exercised. Two non-secret controls prove that:
//!
//! - [`leak_sentinels::CONTROL`] is published as a definition name and rendered
//!   by the CLI, so its presence proves the CLI streams carry real content;
//! - the membership identifier is committed in the clear, so its presence in the
//!   raw database bytes proves the disk scan read committed rows.
//!
//! # CLI process boundary
//!
//! The `ghostlink` process cannot be driven to the SQLite success path
//! offline: its production composition loads the OS-keyring credential before it
//! opens the store, so without a live credential it exits before any read. The
//! success-path streams are therefore proven through the CLI runner that produces
//! those exact bytes, and the real child process is exercised for the sanitized
//! failure and rejected-request paths, which never touch the network.
//!
//! [`BungieProfileGateway`]: destiny_adapters::profile_gateway::BungieProfileGateway
//! [`SqliteStore`]: destiny_adapters::sqlite::SqliteStore
//! [`QueryService`]: destiny_app::QueryService
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
    AccessCredential, AccessCredentialMetadata, AppError, Clock, CredentialMaterial,
    DefinitionReplacementInput, DefinitionRepository, DefinitionSearchQuery, DefinitionVersion,
    DestinyMembership, ErrorCategory, FreshnessPolicy, ItemDefinition, ItemHash, MAX_SEARCH_LIMIT,
    ManifestDefinitions, MembershipId, MembershipType, SearchTerm, SourceMarker, Timestamp,
};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The effective Cross Save primary the journey resolves to.
const MEMBERSHIP_TYPE: u32 = 3;
const MEMBERSHIP_ID: &str = "4611686018484639825";

/// The Manifest version shared by the published definition and the gateway.
const DEFINITION_VERSION: &str = "hardening-leak-definitions-v1";

/// A substring of [`leak_sentinels::CONTROL`] that a definition search matches.
const CONTROL_TERM: &str = "hardening-control";

const NOW: Timestamp = Timestamp::new(1_724_000_000);
const CREDENTIAL_EXPIRES_AT: u64 = 4_000_000_000;
const TTL: FreshnessPolicy = FreshnessPolicy { ttl_seconds: 300 };

/// The complete profile fixture decoded by the real adapter.
const FIXTURE: &[u8] = include_bytes!("fixtures/profile-complete.json");

/// The membership discovery body the gateway resolves the primary from.
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

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// A private temporary database path plus cleanup of its SQLite sidecars.
///
/// The guard mirrors the convention used by the SQLite repository tests and the
/// query journey: one unique file per test derived from the process id and an
/// atomic counter, with the database and its `-wal`/`-shm`/`-journal` sidecars
/// removed on drop.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-hardening-leak-{}-{sequence}.db",
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

/// A private temporary directory removed when the guard drops.
struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-hardening-{label}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create the temporary directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// One request the real gateway handed to the scripted transport.
///
/// The credential newtypes deliberately expose no value accessor outside the
/// adapter module, so the trust boundary records the observed presence of the
/// API key and bearer rather than their bytes. The shared sentinels are the
/// only credentials the composed gateway is ever built with (see [`api_key`]
/// and [`credential`]), so a credential-bearing request is a sentinel-bearing
/// one: recording presence is what turns the leak scans from inferred ingress
/// into observed ingress.
struct RecordedRequest {
    url: String,
    has_api_key: bool,
    has_bearer: bool,
}

/// The scripted transport: canned responses plus the recorded requests.
struct ScriptedTransport {
    responses: VecDeque<Result<ProfileHttpResponse, ProfileTransportError>>,
    recorded_requests: Vec<RecordedRequest>,
}

/// The owning-side view of the scripted transport.
struct SharedTransport(Rc<RefCell<ScriptedTransport>>);

impl ProfileTransport for SharedTransport {
    fn get(
        &mut self,
        request: &ProfileHttpRequest<'_>,
    ) -> Result<ProfileHttpResponse, ProfileTransportError> {
        let mut inner = self.0.borrow_mut();
        inner.recorded_requests.push(RecordedRequest {
            url: request.url().to_owned(),
            has_api_key: request.api_key().is_some(),
            has_bearer: request.bearer().is_some(),
        });
        inner
            .responses
            .pop_front()
            .unwrap_or(Err(ProfileTransportError::Unavailable))
    }
}

/// The explicit test clock; `Clock::now` takes `&self`, so time can advance
/// between reads while the gateway keeps the same handle.
struct TestClock(Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now(&self) -> Result<Timestamp, AppError> {
        Ok(Timestamp::new(self.0.get()))
    }
}

type Gateway = BungieProfileGateway<SharedTransport, TestClock>;
type Service = QueryService<Gateway, SqliteStore>;

/// One composed journey: a temporary database, a scripted transport, and a
/// controllable clock.
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
                recorded_requests: Vec::new(),
            })),
            clock: Rc::new(Cell::new(NOW.seconds())),
        }
    }

    fn at(&self, time: Timestamp) {
        self.clock.set(time.seconds());
    }

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

    fn queue_membership(&self) {
        self.queue_json(MEMBERSHIP_BODY.as_bytes().to_vec());
    }

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

    fn recorded_urls(&self) -> Vec<String> {
        self.transport
            .borrow()
            .recorded_requests
            .iter()
            .map(|request| request.url.clone())
            .collect()
    }

    /// Asserts every request the real gateway handed to the transport carried
    /// the shared API-key and bearer credentials.
    ///
    /// A request without a credential would make the cross-boundary leak scan
    /// vacuous, because the secret would never have been in play. The secret
    /// bytes remain unreadable here by design, so this observes attachment
    /// rather than value equality.
    fn assert_transport_received_credential_requests(&self) {
        let transport = self.transport.borrow();
        assert!(
            !transport.recorded_requests.is_empty(),
            "the transport must have received at least one request"
        );
        for request in &transport.recorded_requests {
            assert!(
                request.has_api_key,
                "the transport received a request without the shared API key: {}",
                request.url
            );
            assert!(
                request.has_bearer,
                "the transport received a request without the shared bearer: {}",
                request.url
            );
        }
    }
}

fn api_key() -> ProfileApiKey {
    ProfileApiKey::new(leak_sentinels::API_KEY).expect("valid API key")
}

fn credential() -> AccessCredential {
    AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(CREDENTIAL_EXPIRES_AT),
        },
        material: CredentialMaterial::from_bytes(leak_sentinels::BEARER_TOKEN.as_bytes().to_vec()),
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

/// The single published definition, whose name is the non-secret control.
fn control_definition() -> ItemDefinition {
    ItemDefinition {
        does_postmaster_pull_have_side_effects: None,
        is_instance_item: None,
        max_stack_size: None,
        stack_unique_label: None,
        hash: ItemHash::new(1001),
        bucket_hash: None,
        name: leak_sentinels::CONTROL.to_owned(),
        item_type: Some("Rocket Launcher".to_owned()),
        tier: None,
        element: None,
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec!["control".to_owned()],
        version: DefinitionVersion::new(DEFINITION_VERSION),
    }
}

fn publish_control_definition(service: &mut Service) {
    service
        .repository_mut()
        .replace_definitions(DefinitionReplacementInput {
            definitions: ManifestDefinitions {
                version: DefinitionVersion::new(DEFINITION_VERSION),
                definitions: vec![control_definition()],
            },
            checked_at: NOW,
            source: SourceMarker::Manifest,
        })
        .expect("publish the control definition set");
}

/// Publishes the control definition and synchronizes the projection with one cold
/// query, leaving two recorded gateway requests behind.
fn seed(journey: &Journey, service: &mut Service) {
    publish_control_definition(service);
    journey.queue_membership();
    journey.queue_profile(FIXTURE);
    service
        .list_characters(TTL, NOW)
        .expect("cold query synchronizes the projection");
}

/// A failing profile response whose provider message carries the raw-payload
/// sentinel, which must never reach an error, disk, or a query answer.
fn sentinel_profile() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "ErrorCode": 7,
        "ErrorStatus": "PrivacyRestriction",
        "Message": leak_sentinels::RAW_PAYLOAD,
        "Response": {}
    }))
    .expect("payload serializes")
}

/// Concatenates the raw bytes of the database file and every present sidecar,
/// so the disk scan covers whichever journal mode SQLite chose.
///
/// An absent sidecar is expected and contributes no bytes. A path that exists
/// but cannot be read -- a present-but-unreadable database or sidecar -- aborts
/// the scan rather than silently dropping those bytes, so a clean scan can
/// never be an unread scan. The main database must exist: a missing database is
/// a broken harness, not an empty one.
fn database_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let candidate = PathBuf::from(sidecar);
        match std::fs::read(&candidate) {
            Ok(part) => bytes.extend_from_slice(&part),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                assert!(
                    !suffix.is_empty(),
                    "the durable database {} is missing: {error}",
                    candidate.display()
                );
            }
            Err(error) => panic!(
                "the durable database or sidecar {} could not be read: {error}",
                candidate.display()
            ),
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

/// Asserts no shared secret canary appears in a rendered string.
fn assert_no_secret(rendered: &str, boundary: &str) {
    for secret in leak_sentinels::SECRETS {
        assert!(
            !rendered.contains(secret),
            "{boundary} leaked a shared secret: {rendered}"
        );
    }
}

fn cli_argv(arguments: &[&str]) -> Vec<String> {
    let mut argv = vec!["ghostlink".to_string()];
    argv.extend(arguments.iter().map(|argument| (*argument).to_string()));
    argv
}

/// Drives the real CLI runner over the composed service, exactly as the binary
/// process boundary does, and returns the fully rendered process result.
fn cli(service: &mut Service, arguments: &[&str], now: Timestamp) -> ProcessOutput {
    destiny_cli::run(
        service,
        cli_argv(arguments),
        ReadFreshness { policy: TTL, now },
    )
}

/// The definition search request the CLI forwards verbatim.
fn control_search() -> SearchInventoryRequest {
    SearchInventoryRequest {
        query: None,
        definitions: DefinitionSearchQuery {
            tier: None,
            text: Some(SearchTerm::new(CONTROL_TERM).expect("valid search term")),
            item_type: None,
            tags: Vec::new(),
            element: None,
            slot: None,
            class_type: None,
            limit: MAX_SEARCH_LIMIT,
        },
        instances: None,
    }
}

/// Locates the built `ghostlink` binary beside the test executable.
fn cli_binary() -> Option<PathBuf> {
    let mut directory = std::env::current_exe().ok()?;
    directory.pop(); // the `deps` directory
    if directory.file_name().and_then(|name| name.to_str()) == Some("deps") {
        directory.pop();
    }
    let candidate = directory.join(format!("ghostlink{}", std::env::consts::EXE_SUFFIX));
    candidate.is_file().then_some(candidate)
}

/// Runs the real `ghostlink` child process with a cleared environment.
///
/// The environment is cleared so no ambient `DBUS_SESSION_BUS_ADDRESS` or
/// inherited credential reaches the keyring seam; only `PATH` and `HOME` are
/// restored, plus the exact configuration the caller supplies.
fn cli_process(
    binary: &Path,
    working_dir: &Path,
    arguments: &[&str],
    environment: &[(&str, &str)],
) -> std::process::Output {
    let mut command = std::process::Command::new(binary);
    command
        .args(arguments)
        .current_dir(working_dir)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", working_dir);
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().expect("the CLI process runs")
}

/// The disk scan is fail-closed: a database or sidecar path that exists but
/// cannot be read aborts the scan instead of silently contributing no bytes,
/// while an absent sidecar is still allowed.
#[test]
#[should_panic(expected = "could not be read")]
fn the_disk_scan_fails_on_a_present_unreadable_database() {
    let directory = TempDirectory::new("unreadable-database");
    // A directory stands in for a present-but-unreadable database file: it
    // exists at the scanned path but cannot be read as a byte stream, which is
    // not `NotFound`, so the scan must abort rather than report a clean read.
    let unreadable = directory.path().join("database.db");
    std::fs::create_dir(&unreadable).expect("create the stand-in database directory");
    let _ = database_bytes(&unreadable);
}

/// The durable boundary and the CLI runner share one sentinel set: the
/// secret-bearing configuration never reaches SQLite bytes or a rendered stream,
/// while the non-secret controls prove both surfaces were exercised.
#[test]
fn durable_bytes_and_cli_streams_share_one_sentinel_set() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    // The real gateway resolved the Cross Save primary and read the profile.
    assert_eq!(
        journey.recorded_urls(),
        vec![
            MEMBERSHIP_ENDPOINT.to_owned(),
            profile_endpoint(&expected_membership()),
        ]
    );

    // Observed ingress, not inferred: every request the gateway built carried
    // the shared sentinel-bearing API key and bearer, so the leak scan below is
    // anchored to a transport that genuinely received the configured secrets.
    journey.assert_transport_received_credential_requests();

    // Positive control: the committed projection and the control definition are
    // genuinely on disk, so the negative secret scan is not an empty-file scan.
    let bytes = database_bytes(journey.database.path());
    assert!(
        contains_bytes(&bytes, leak_sentinels::CONTROL.as_bytes()),
        "the disk scan must cover the committed control definition"
    );
    assert!(
        contains_bytes(&bytes, MEMBERSHIP_ID.as_bytes()),
        "the disk scan must cover the committed profile metadata"
    );
    for secret in leak_sentinels::SECRETS {
        assert!(
            !contains_bytes(&bytes, secret.as_bytes()),
            "the on-disk database and sidecars leaked {secret}"
        );
    }

    // The real CLI renders the control definition in both modes; neither stream
    // carries a shared secret.
    let human = cli(
        &mut service,
        &["search-inventory", "--text", CONTROL_TERM],
        NOW,
    );
    assert_eq!(human.exit_code, 0);
    assert!(human.stderr.is_empty(), "success must not write to stderr");
    assert!(
        human.stdout.contains(leak_sentinels::CONTROL),
        "the CLI stdout must carry the non-secret control: {}",
        human.stdout
    );
    assert_no_secret(&human.stdout, "CLI stdout");
    assert_no_secret(&human.stderr, "CLI stderr");

    let json = cli(
        &mut service,
        ["--json", "search-inventory", "--text", CONTROL_TERM].as_slice(),
        NOW,
    );
    assert_eq!(json.exit_code, 0);
    assert!(json.stderr.is_empty());
    let value: Value = serde_json::from_str(json.stdout.trim()).expect("one JSON object");
    assert_eq!(value["kind"].as_str(), Some("inventory_search"));
    assert_eq!(
        value["candidates"][0]["name"].as_str(),
        Some(leak_sentinels::CONTROL)
    );
    assert_no_secret(&json.stdout, "CLI JSON stdout");
}

/// Serialized error shapes — the shared `AppError` and the CLI's human and JSON
/// failures — never carry a shared secret, even when the provider payload does.
#[test]
fn serialized_error_shapes_never_carry_a_shared_sentinel() {
    let journey = Journey::new();
    journey.at(NOW);
    let mut service = journey.service();
    seed(&journey, &mut service);

    let failure_now = Timestamp::new(NOW.seconds() + TTL.ttl_seconds + 1);
    journey.at(failure_now);
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let error = service
        .search_inventory(&control_search(), TTL, failure_now)
        .expect_err("a non-success profile must fail the query");
    assert_eq!(error.category(), ErrorCategory::ProviderValidation);
    assert_no_secret(&error.to_string(), "AppError Display");
    assert_no_secret(&format!("{error:?}"), "AppError Debug");

    // The CLI JSON failure is a sanitized typed outcome on stderr only.
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let json = cli(
        &mut service,
        ["--json", "search-inventory", "--text", CONTROL_TERM].as_slice(),
        failure_now,
    );
    assert_eq!(json.exit_code, 2);
    assert!(json.stdout.is_empty(), "a failure must not write to stdout");
    let value: Value = serde_json::from_str(json.stderr.trim()).expect("one JSON error object");
    assert_eq!(value["error"]["code"].as_str(), Some("invalid_request"));
    assert_no_secret(&json.stderr, "CLI JSON error");

    // The same decoded failure renders as one sanitized human line.
    journey.queue_membership();
    journey.queue_profile(&sentinel_profile());
    let human = cli(
        &mut service,
        ["search-inventory", "--text", CONTROL_TERM].as_slice(),
        failure_now,
    );
    assert_eq!(human.exit_code, 2);
    assert!(human.stdout.is_empty());
    assert!(human.stderr.starts_with("error: "));
    assert_no_secret(&human.stderr, "CLI human error");

    // The prior projection is preserved: the failure changed nothing on disk.
    let bytes = database_bytes(journey.database.path());
    assert!(contains_bytes(&bytes, MEMBERSHIP_ID.as_bytes()));
    for secret in leak_sentinels::SECRETS {
        assert!(
            !contains_bytes(&bytes, secret.as_bytes()),
            "a failed refresh persisted {secret} to disk"
        );
    }
}

/// The real `ghostlink` child process sanitizes both streams, including the
/// JSON failure shape, and never echoes secret-bearing configuration.
///
/// The success path is out of reach offline (the production composition loads an
/// OS-keyring credential before it opens the store), so this runs the real
/// process only for the sanitized failure and rejected-request paths. An
/// unopenable database path guarantees the process fails before any read, so the
/// check can never reach the network even on a machine holding a credential.
///
/// The check requires the built `ghostlink` binary and fails loudly when it is
/// absent rather than skipping: a silently skipped process check would report a
/// clean subprocess leak scan without ever running the process. Build the
/// binary with `cargo build -p ghostlink` before a focused
/// `cargo test -p destiny-adapters` run.
#[test]
fn the_real_cli_process_streams_carry_no_shared_sentinel() {
    let binary = cli_binary().expect(
        "the real `ghostlink` subprocess leak check requires the built binary; \
         run `cargo build -p ghostlink` before `cargo test -p destiny-adapters`",
    );
    let scratch = TempDirectory::new("cli");
    // The store creates missing parent directories, so the parent must be a
    // regular file: no directory can ever be created there, the open fails
    // before any read, and the manifest auto-sync can never start.
    let blocking_file = scratch.path().join("missing");
    std::fs::write(&blocking_file, b"").expect("the blocking file is written");
    let blocked = blocking_file.join(format!("{}.db", leak_sentinels::BEARER_TOKEN));
    let environment = [
        ("BUNGIE_API_KEY", leak_sentinels::API_KEY),
        (
            "DESTINY_DATABASE_PATH",
            blocked.to_str().expect("temporary path is utf-8"),
        ),
        ("DESTINY_DEFINITION_VERSION", DEFINITION_VERSION),
        ("DESTINY_FRESHNESS_TTL_SECONDS", "300"),
    ];

    // A valid request with no usable credential store fails closed, and the
    // secret-bearing configuration never reaches either stream.
    let failure = cli_process(
        &binary,
        scratch.path(),
        ["list-characters"].as_slice(),
        environment.as_slice(),
    );
    assert_ne!(
        failure.status.code(),
        Some(0),
        "the failure path must not report success"
    );
    assert!(
        failure.stdout.is_empty(),
        "a failure writes to stderr only: {:?}",
        String::from_utf8_lossy(&failure.stdout)
    );
    let stderr = String::from_utf8_lossy(&failure.stderr);
    assert!(
        stderr.starts_with("error: "),
        "the failure must be one sanitized line: {stderr}"
    );
    assert_no_secret(&stderr, "CLI process stderr");

    // A rejected request renders the sanitized JSON error shape and never echoes
    // the sentinel-bearing rejected value.
    let rejected_arguments = [
        "--json",
        "search-inventory",
        "--text",
        leak_sentinels::API_KEY,
        "--limit",
        "0",
    ];
    let rejected = cli_process(
        &binary,
        scratch.path(),
        rejected_arguments.as_slice(),
        environment.as_slice(),
    );
    assert_eq!(rejected.status.code(), Some(2));
    assert!(rejected.stdout.is_empty());
    let body = String::from_utf8_lossy(&rejected.stderr);
    let value: Value = serde_json::from_str(body.trim()).expect("one JSON error object");
    assert_eq!(value["error"]["code"].as_str(), Some("invalid_request"));
    assert_no_secret(&body, "CLI process JSON error");
}
