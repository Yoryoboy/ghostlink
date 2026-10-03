//! Cross-cutting DSF-006 verification harness for the SQLite adapter.
//!
//! The per-repository suites already prove contract behavior on a single
//! connection. These tests close the four remaining WU-005 evidence gaps:
//!
//! - two file-backed connections contending for the same write lock, where the
//!   loser waits a bounded test-only timeout, fails with a sanitized
//!   local-storage error, and cannot disturb the committed snapshot;
//! - read visibility while one connection holds uncommitted raw writes to the
//!   anchor, the metadata row, and the child rows, in write-ahead logging mode;
//! - a combined runtime harness that publishes both datasets, closes, reopens,
//!   mutates, closes, and reopens again, proving durable coexistence and exact
//!   revision advancement;
//! - secrecy of the on-disk bytes and of the schema itself: a sentinel that was
//!   never passed to any API must be absent, and no BLOB or generic payload
//!   store may exist.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars. The only waiting is
//! the bounded lock timeout one test lowers on its own second connection; no
//! test sleeps and no test changes the production connection policy.

use super::{BUSY_TIMEOUT_MILLIS, SqliteStore};
use destiny_core::{
    AppError, Character, CharacterId, Dataset, DefinitionReplacementInput, DefinitionRepository,
    DefinitionVersion, ErrorCategory, InventoryRepository, InventorySnapshotInput, ItemDefinition,
    ItemHash, ItemInstance, ItemInstanceId, ItemLocation, ManifestDefinitions, MembershipId,
    MembershipType, Resolution, SourceMarker, SyncRevision, Timestamp,
};
use rusqlite::TransactionBehavior;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// The lock wait used on the contending test connection only.
///
/// It is short so the suite never pays the production five-second interval, and
/// it is asserted separately that the store's own policy remains
/// [`BUSY_TIMEOUT_MILLIS`].
const TEST_BUSY_TIMEOUT_MILLIS: u64 = 150;

/// A value never handed to any store API, so no durable artifact may contain it.
const SECRET_SENTINEL: &str = "sentinel-token-ApiKey-auth-code-callback-raw-payload-7f21c9";

/// A non-secret display value seeded on purpose as a positive scan control.
const DISPLAY_CONTROL: &str = "Verification Alpha Rifle";

/// Vocabulary that must never appear in a table or index name or its SQL, so
/// the projection cannot become a credential or raw-payload store.
const PROHIBITED_SCHEMA_VOCABULARY: [&str; 15] = [
    "token",
    "secret",
    "credential",
    "password",
    "api_key",
    "apikey",
    "authorization",
    "oauth",
    "code",
    "callback",
    "refresh",
    "payload",
    "blob",
    "json",
    "raw",
];

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-verification-{}-{sequence}.db",
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

fn storage_error() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

fn definition(hash: i64, name: &str, version: &str) -> ItemDefinition {
    ItemDefinition {
        hash: ItemHash::new(hash),
        bucket_hash: None,
        name: name.to_owned(),
        item_type: Some("Auto Rifle".into()),
        tier: Some("Legendary".into()),
        element: Some("Void".into()),
        slot: None,
        class_type: None,
        plug_category: None,
        tags: vec!["weapon".into()],
        version: DefinitionVersion::new(version),
    }
}

fn replacement(
    version: &str,
    definitions: Vec<ItemDefinition>,
    checked_at: u64,
) -> DefinitionReplacementInput {
    DefinitionReplacementInput {
        definitions: ManifestDefinitions {
            version: DefinitionVersion::new(version),
            definitions,
        },
        checked_at: Timestamp::new(checked_at),
        source: SourceMarker::Manifest,
    }
}

/// One structurally valid profile with a single character and a referenced item.
fn profile(
    membership_id: &str,
    character_id: &str,
    instance_id: &str,
    checked_at: u64,
) -> InventorySnapshotInput {
    InventorySnapshotInput {
        membership_id: MembershipId::new(membership_id),
        membership_type: MembershipType::new(3),
        definition_version: DefinitionVersion::new("defs-v1"),
        checked_at: Timestamp::new(checked_at),
        source: SourceMarker::Profile,
        characters: vec![Character {
            id: CharacterId::new(character_id),
            name: Some("Nova".into()),
            class: Some("Warlock".into()),
            race: Some("Awoken".into()),
            gender: Some("Female".into()),
            light: Some(1810),
        }],
        items: vec![ItemInstance {
            instance_id: ItemInstanceId::new(instance_id),
            item_hash: ItemHash::new(-42),
            location: ItemLocation::Character(CharacterId::new(character_id)),
            quantity: 1,
            equipped: false,
            locked: true,
            power: Some(1810),
            plugs: Vec::new(),
            perks: Vec::new(),
        }],
    }
}

fn pragma_i64(store: &SqliteStore, pragma: &str) -> i64 {
    store
        .connection()
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .expect("read integer pragma")
}

fn metadata_rows(store: &SqliteStore) -> i64 {
    store
        .connection()
        .query_row("SELECT count(*) FROM sync_metadata", [], |row| row.get(0))
        .expect("count metadata rows")
}

fn schema_objects(store: &SqliteStore) -> Vec<(String, String, String)> {
    let mut statement = store
        .connection()
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_schema \
             WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
        )
        .expect("prepare schema query");
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("run schema query")
        .map(|row| row.expect("read schema row"))
        .collect()
}

fn table_names(store: &SqliteStore) -> Vec<String> {
    let mut statement = store
        .connection()
        .prepare(
            "SELECT name FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .expect("prepare table query");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run table query")
        .map(|row| row.expect("read table row"))
        .collect()
}

fn column_types(store: &SqliteStore, table: &str) -> Vec<(String, String)> {
    let mut statement = store
        .connection()
        .prepare(&format!(
            "SELECT name, type FROM pragma_table_info('{table}') ORDER BY cid"
        ))
        .expect("prepare column query");
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("run column query")
        .map(|row| row.expect("read column row"))
        .collect()
}

/// Reads the database file plus any sidecar that still exists, as raw bytes.
fn read_database_and_sidecars(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let candidate = PathBuf::from(sidecar);
        if let Ok(contents) = std::fs::read(&candidate) {
            files.push((candidate.display().to_string(), contents));
        }
    }
    files
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[test]
fn second_connection_write_waits_then_fails_sanitized_without_mutating() {
    let temp = TempDatabase::new();
    let path_text = temp.path().display().to_string();

    let mut first = SqliteStore::open(temp.path()).expect("open the first connection");
    let seeded = first
        .replace_current(profile("membership-1", "character-1", "instance-1", 100))
        .expect("seed the committed projection");

    let mut second = SqliteStore::open(temp.path()).expect("open the second connection");
    second
        .connection()
        .busy_timeout(Duration::from_millis(TEST_BUSY_TIMEOUT_MILLIS))
        .expect("shorten only the second connection's lock wait");

    // The production policy is untouched: the constant, the first connection,
    // and the second connection's override are all asserted directly.
    assert_eq!(BUSY_TIMEOUT_MILLIS, 5_000);
    assert_eq!(
        pragma_i64(&first, "busy_timeout"),
        i64::from(BUSY_TIMEOUT_MILLIS)
    );
    assert_eq!(
        pragma_i64(&second, "busy_timeout"),
        i64::try_from(TEST_BUSY_TIMEOUT_MILLIS).expect("test timeout fits in i64")
    );

    let transaction = first
        .connection_mut()
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("hold an immediate write transaction");
    transaction
        .execute(
            "UPDATE active_profile SET membership_id = 'held-open' WHERE singleton = 1",
            [],
        )
        .expect("stage an uncommitted write");

    let started = Instant::now();
    let error = second
        .replace_current(profile("membership-1", "character-2", "instance-2", 300))
        .expect_err("a locked database must reject the second writer");
    let elapsed = started.elapsed();
    println!("second-connection lock wait observed {elapsed:?}");

    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(error.to_string(), storage_error().to_string());
    assert!(
        elapsed >= Duration::from_millis(TEST_BUSY_TIMEOUT_MILLIS * 2 / 3),
        "the second writer did not honor most of its lock timeout: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "the second writer ignored its shortened timeout: {elapsed:?}"
    );

    let display = error.to_string();
    let debug = format!("{error:?}");
    for leaked in [
        path_text.as_str(),
        "membership-1",
        "character-2",
        "instance-2",
        "active_profile",
        "sync_metadata",
        "UPDATE",
        "INSERT",
        "sqlite",
    ] {
        assert!(
            !display.contains(leaked),
            "display leaked {leaked}: {display}"
        );
        assert!(!debug.contains(leaked), "debug leaked {leaked}: {debug}");
    }

    drop(transaction);

    assert_eq!(
        second.load_current().expect("read the prior projection"),
        Some(seeded),
        "the failed writer must leave the committed snapshot untouched"
    );
}

#[test]
fn uncommitted_raw_writes_stay_invisible_until_rollback() {
    let temp = TempDatabase::new();

    let mut writer = SqliteStore::open(temp.path()).expect("open the writer connection");
    let seeded = writer
        .replace_current(profile("membership-1", "character-1", "instance-1", 100))
        .expect("seed the committed projection");

    let reader = SqliteStore::open(temp.path()).expect("open the reader connection");
    assert_eq!(
        reader
            .load_current()
            .expect("read the committed projection"),
        Some(seeded.clone())
    );

    let transaction = writer
        .connection_mut()
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("hold an immediate write transaction");
    transaction
        .execute(
            "UPDATE active_profile SET membership_id = 'uncommitted-member' WHERE singleton = 1",
            [],
        )
        .expect("stage an uncommitted anchor change");
    transaction
        .execute(
            "UPDATE sync_metadata SET checked_at = '18446744073709551615', revision = '42' \
             WHERE dataset = 'profile_inventory'",
            [],
        )
        .expect("stage an uncommitted metadata change");
    transaction
        .execute(
            "INSERT INTO characters (character_id, name) VALUES ('uncommitted-character', 'Phantom')",
            [],
        )
        .expect("stage a new child row without violating the foreign keys");

    // The reader sees only the prior complete projection, never a partial or new
    // combination of the uncommitted rows.
    let reader_revision: String = reader
        .connection()
        .query_row(
            "SELECT revision FROM sync_metadata WHERE dataset = 'profile_inventory'",
            [],
            |row| row.get(0),
        )
        .expect("read the committed revision");
    assert_eq!(reader_revision, "1");
    let reader_anchor: String = reader
        .connection()
        .query_row(
            "SELECT membership_id FROM active_profile WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read the committed anchor");
    assert_eq!(reader_anchor, "membership-1");
    let reader_characters: i64 = reader
        .connection()
        .query_row("SELECT count(*) FROM characters", [], |row| row.get(0))
        .expect("count committed characters");
    assert_eq!(reader_characters as usize, seeded.characters.len());
    assert_eq!(
        reader
            .load_current()
            .expect("read during the open transaction"),
        Some(seeded.clone())
    );

    drop(transaction);

    assert_eq!(
        reader.load_current().expect("read after the rollback"),
        Some(seeded.clone()),
        "rolling back the uncommitted writes must leave the prior projection"
    );
    assert_eq!(
        writer
            .load_current()
            .expect("read from the rolled-back writer"),
        Some(seeded)
    );
}

#[test]
fn combined_file_store_keeps_both_datasets_across_close_and_reopen() {
    let temp = TempDatabase::new();

    let alpha_v1 = definition(10, "Alpha", "defs-v1");
    let beta_v1 = definition(20, "Beta", "defs-v1");
    let (manifest_v1, profile_v1) = {
        let mut store = SqliteStore::open(temp.path()).expect("open the file store");
        let manifest = store
            .replace_definitions(replacement(
                "defs-v1",
                vec![alpha_v1.clone(), beta_v1.clone()],
                100,
            ))
            .expect("publish the definition set");
        let snapshot = store
            .replace_current(profile("membership-1", "character-1", "instance-1", 100))
            .expect("publish the profile snapshot");
        (manifest, snapshot)
    };

    assert_eq!(manifest_v1.dataset, Dataset::ManifestDefinitions);
    assert_eq!(manifest_v1.revision, SyncRevision::new(1));
    assert_eq!(profile_v1.metadata.source, SourceMarker::Profile);
    assert_eq!(profile_v1.metadata.revision, SyncRevision::new(1));

    let mut store = SqliteStore::open(temp.path()).expect("reopen the file store");
    assert_eq!(
        store
            .current_metadata()
            .expect("read the manifest metadata"),
        Some(manifest_v1.clone())
    );
    assert_eq!(
        store
            .current_definition_version()
            .expect("read the definition version"),
        DefinitionVersion::new("defs-v1")
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(10))
            .expect("look up Alpha"),
        Resolution::Unique(alpha_v1.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(20))
            .expect("look up Beta"),
        Resolution::Unique(beta_v1.clone())
    );
    assert_eq!(
        store.load_current().expect("read the profile snapshot"),
        Some(profile_v1.clone())
    );
    assert_eq!(metadata_rows(&store), 2, "both datasets must coexist");

    let alpha_v2 = definition(10, "Alpha Prime", "defs-v2");
    let gamma_v2 = definition(30, "Gamma", "defs-v2");
    let manifest_v2 = store
        .replace_definitions(replacement(
            "defs-v2",
            vec![alpha_v2.clone(), gamma_v2.clone()],
            200,
        ))
        .expect("publish a changed definition set");
    let profile_v2 = store
        .replace_current(profile("membership-2", "character-2", "instance-2", 200))
        .expect("publish a changed profile snapshot");
    assert_eq!(manifest_v2.revision, SyncRevision::new(2));
    assert_eq!(profile_v2.metadata.revision, SyncRevision::new(2));
    drop(store);

    let store = SqliteStore::open(temp.path()).expect("reopen after the changes");
    assert_eq!(
        store
            .current_metadata()
            .expect("read the advanced manifest metadata"),
        Some(manifest_v2.clone())
    );
    assert_eq!(
        store
            .load_current()
            .expect("read the advanced profile snapshot"),
        Some(profile_v2.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(10))
            .expect("look up Alpha Prime"),
        Resolution::Unique(alpha_v2.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(30))
            .expect("look up Gamma"),
        Resolution::Unique(gamma_v2.clone())
    );
    assert_eq!(
        store
            .lookup_definition(ItemHash::new(20))
            .expect("look up the removed Beta"),
        Resolution::Missing
    );
    assert_eq!(manifest_v2.revision, SyncRevision::new(2));
    assert_eq!(
        metadata_rows(&store),
        2,
        "both datasets must still coexist after the reopen"
    );
}

#[test]
fn reopened_bytes_and_schema_hold_no_secret_or_payload_store() {
    let temp = TempDatabase::new();

    {
        let mut store = SqliteStore::open(temp.path()).expect("open the file store");
        store
            .replace_definitions(replacement(
                "defs-v1",
                vec![definition(10, DISPLAY_CONTROL, "defs-v1")],
                100,
            ))
            .expect("publish the definition set");
        store
            .replace_current(profile("membership-1", "character-1", "instance-1", 100))
            .expect("publish the profile snapshot");
        let busy: i64 = store
            .connection()
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
            .expect("checkpoint the write-ahead log");
        assert_eq!(busy, 0, "the checkpoint must not be blocked");
    }

    let files = read_database_and_sidecars(temp.path());
    assert!(!files.is_empty(), "the database file must exist on disk");
    let all_bytes: Vec<u8> = files
        .iter()
        .flat_map(|(_, contents)| contents.iter().copied())
        .collect();
    // Positive control: the scan sees real committed bytes, so an absent
    // sentinel is meaningful rather than a scan of nothing.
    assert!(
        contains_bytes(&all_bytes, DISPLAY_CONTROL.as_bytes()),
        "the byte scan did not see the committed display control"
    );
    for (name, contents) in &files {
        assert!(
            !contains_bytes(contents, SECRET_SENTINEL.as_bytes()),
            "{name} leaked the never-passed sentinel"
        );
    }

    let store = SqliteStore::open(temp.path()).expect("reopen for schema inspection");
    for (kind, name, sql) in schema_objects(&store) {
        let haystack = format!("{name} {sql}").to_lowercase();
        for prohibited in PROHIBITED_SCHEMA_VOCABULARY {
            assert!(
                !haystack.contains(prohibited),
                "{kind} {name} contains prohibited vocabulary {prohibited}"
            );
        }
        assert!(
            !sql.to_lowercase().contains("blob"),
            "{name} declares a blob payload store"
        );
    }
    for table in table_names(&store) {
        for (column, column_type) in column_types(&store, &table) {
            assert_ne!(
                column_type, "BLOB",
                "{table}.{column} stores a generic blob payload"
            );
            assert!(
                column_type == "TEXT" || column_type == "INTEGER",
                "{table}.{column} has an unexpected type {column_type}"
            );
        }
    }
}
