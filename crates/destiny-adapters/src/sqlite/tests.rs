//! Focused storage, policy, migration, and schema tests for `sqlite`.
//!
//! Every test is offline, credential-free, and deterministic. File-backed tests
//! use a private temporary path derived from the process id and an atomic
//! counter, and always remove the database and its sidecars.

use super::{BUSY_TIMEOUT_MILLIS, LATEST_SCHEMA_VERSION, SqliteStore, migrations, storage_failure};
use destiny_core::{AppError, ErrorCategory};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const EXPECTED_TABLE_NAMES: [&str; 15] = [
    "active_profile",
    "activity_definitions",
    "activity_modifier_definitions",
    "activity_type_definitions",
    "characters",
    "definition_tags",
    "definitions",
    "item_perks",
    "item_plugs",
    "item_stats",
    "items",
    "milestone_definitions",
    "objective_definitions",
    "stat_definitions",
    "sync_metadata",
];

const EXPECTED_INDEX_NAMES: [&str; 2] = ["items_definition_hash_idx", "items_location_idx"];

/// Expected `(table, column, declared type)` triples in declaration order.
///
/// A flat triple keeps the fixture free of nested slice references; the test
/// regroups it per table when it compares `PRAGMA table_info` output.
const EXPECTED_COLUMNS: [(&str, &str, &str); 74] = [
    ("stat_definitions", "hash", "INTEGER"),
    ("stat_definitions", "name", "TEXT"),
    ("stat_definitions", "definition_version", "TEXT"),
    ("objective_definitions", "hash", "INTEGER"),
    ("objective_definitions", "progress_description", "TEXT"),
    ("objective_definitions", "completion_value", "INTEGER"),
    ("objective_definitions", "definition_version", "TEXT"),
    ("activity_type_definitions", "hash", "INTEGER"),
    ("activity_type_definitions", "name", "TEXT"),
    ("activity_type_definitions", "description", "TEXT"),
    ("activity_type_definitions", "definition_version", "TEXT"),
    ("activity_definitions", "hash", "INTEGER"),
    ("activity_definitions", "name", "TEXT"),
    ("activity_definitions", "description", "TEXT"),
    ("activity_definitions", "activity_type_hash", "INTEGER"),
    ("activity_definitions", "light_level", "INTEGER"),
    ("activity_definitions", "definition_version", "TEXT"),
    ("activity_modifier_definitions", "hash", "INTEGER"),
    ("activity_modifier_definitions", "name", "TEXT"),
    ("activity_modifier_definitions", "description", "TEXT"),
    (
        "activity_modifier_definitions",
        "definition_version",
        "TEXT",
    ),
    ("milestone_definitions", "hash", "INTEGER"),
    ("milestone_definitions", "name", "TEXT"),
    ("milestone_definitions", "description", "TEXT"),
    ("milestone_definitions", "milestone_type", "INTEGER"),
    ("milestone_definitions", "definition_version", "TEXT"),
    ("active_profile", "singleton", "INTEGER"),
    ("active_profile", "membership_id", "TEXT"),
    ("active_profile", "membership_type", "TEXT"),
    ("characters", "character_id", "TEXT"),
    ("characters", "name", "TEXT"),
    ("characters", "class_name", "TEXT"),
    ("characters", "race", "TEXT"),
    ("characters", "gender", "TEXT"),
    ("characters", "light", "INTEGER"),
    ("definition_tags", "definition_hash", "INTEGER"),
    ("definition_tags", "ordinal", "INTEGER"),
    ("definition_tags", "tag", "TEXT"),
    ("definitions", "hash", "INTEGER"),
    ("definitions", "bucket_hash", "INTEGER"),
    ("definitions", "name", "TEXT"),
    ("definitions", "item_type", "TEXT"),
    ("definitions", "tier", "TEXT"),
    ("definitions", "element", "TEXT"),
    ("definitions", "definition_version", "TEXT"),
    ("definitions", "damage_type", "TEXT"),
    ("definitions", "slot", "TEXT"),
    ("definitions", "class_type", "TEXT"),
    ("definitions", "plug_category", "TEXT"),
    ("item_stats", "instance_id", "TEXT"),
    ("item_stats", "stat_hash", "INTEGER"),
    ("item_stats", "value", "INTEGER"),
    ("item_plugs", "instance_id", "TEXT"),
    ("item_plugs", "ordinal", "INTEGER"),
    ("item_plugs", "plug_hash", "INTEGER"),
    ("item_perks", "instance_id", "TEXT"),
    ("item_perks", "ordinal", "INTEGER"),
    ("item_perks", "perk_hash", "INTEGER"),
    ("item_perks", "active", "INTEGER"),
    ("item_perks", "visible", "INTEGER"),
    ("items", "instance_id", "TEXT"),
    ("items", "item_hash", "INTEGER"),
    ("items", "location_kind", "TEXT"),
    ("items", "location_character_id", "TEXT"),
    ("items", "location_other", "TEXT"),
    ("items", "quantity", "INTEGER"),
    ("items", "equipped", "INTEGER"),
    ("items", "locked", "INTEGER"),
    ("items", "power", "INTEGER"),
    ("sync_metadata", "dataset", "TEXT"),
    ("sync_metadata", "checked_at", "TEXT"),
    ("sync_metadata", "source", "TEXT"),
    ("sync_metadata", "revision", "TEXT"),
    ("sync_metadata", "definition_version", "TEXT"),
];

/// Vocabulary that must never appear in the schema, so the projection can never
/// become a secret, credential, or raw-payload store.
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

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

/// A private temporary database path plus cleanup of its SQLite sidecars.
struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new() -> Self {
        let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "destiny-sqlite-{}-{sequence}.db",
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

fn strings<const N: usize>(values: [&str; N]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn expected_columns(table: &str) -> Vec<(String, String)> {
    EXPECTED_COLUMNS
        .iter()
        .filter(|(name, _, _)| *name == table)
        .map(|(_, column, column_type)| ((*column).to_owned(), (*column_type).to_owned()))
        .collect()
}

fn table_names(store: &SqliteStore) -> Vec<String> {
    query_names(
        store,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
}

fn index_names(store: &SqliteStore) -> Vec<String> {
    query_names(
        store,
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
}

fn query_names(store: &SqliteStore, sql: &str) -> Vec<String> {
    let mut statement = store.connection().prepare(sql).expect("prepare name query");
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("run name query")
        .map(|row| row.expect("read name row"))
        .collect()
}

fn column_signature(store: &SqliteStore, table: &str) -> Vec<(String, String)> {
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

fn pragma_i64(store: &SqliteStore, pragma: &str) -> i64 {
    store
        .connection()
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .expect("read integer pragma")
}

fn pragma_text(store: &SqliteStore, pragma: &str) -> String {
    store
        .connection()
        .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
        .expect("read text pragma")
}

fn expect_rejected(store: &SqliteStore, sql: &str) {
    assert!(
        store.connection().execute_batch(sql).is_err(),
        "schema accepted an invalid statement: {sql}"
    );
}

fn table_sql(store: &SqliteStore, table: &str) -> String {
    store
        .connection()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .expect("read table SQL")
}

/// Counts every row in one table.
fn row_count(store: &SqliteStore, table: &str) -> i64 {
    store
        .connection()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
}

/// Writes one complete version-1 profile projection with raw SQL.
///
/// A DPS-002A migration test cannot seed version-1 rows through the repository
/// while the schema is still at version 1, because the repository writes the
/// version-2 `item_perks` table. This helper therefore inserts the exact
/// version-1 rows the version-1 schema accepts, so an up or down migration can
/// be checked for row preservation without depending on repository code.
fn seed_v1_projection(store: &SqliteStore) {
    store
        .connection()
        .execute_batch(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (1, '4611686018484639825', '3');\
             INSERT INTO characters (character_id, name, class_name, race, gender, light) \
             VALUES ('hunter-1', 'Nova', 'Hunter', 'Human', 'Female', 1810);\
             INSERT INTO items \
             (instance_id, item_hash, location_kind, location_character_id, location_other, \
              quantity, equipped, locked, power) \
             VALUES ('item-1', 2522695, 'character', 'hunter-1', NULL, 1, 1, 0, 1810);\
             INSERT INTO sync_metadata \
             (dataset, checked_at, source, revision, definition_version) \
             VALUES ('profile_inventory', '100', 'profile', '1', 'defs-v1');",
        )
        .expect("seed a version-1 profile projection with raw SQL");
}

/// Inserts one canonical `sync_metadata` pair and asserts it round-trips exactly.
fn accept_sync_metadata(store: &SqliteStore, checked_at: &str, revision: &str) {
    store
        .connection()
        .execute_batch("DELETE FROM sync_metadata")
        .expect("clear sync metadata");
    let sql = format!(
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('manifest_definitions', '{checked_at}', 'manifest', '{revision}', 'v')"
    );
    store
        .connection()
        .execute_batch(&sql)
        .expect("accept a canonical in-range value");
    let stored: (String, String) = store
        .connection()
        .query_row(
            "SELECT checked_at, revision FROM sync_metadata WHERE dataset = 'manifest_definitions'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read stored metadata");
    assert_eq!(stored, (checked_at.to_owned(), revision.to_owned()));
}

/// Asserts one `sync_metadata` pair is rejected by the canonical decimal checks.
fn reject_sync_metadata(store: &SqliteStore, checked_at: &str, revision: &str) {
    store
        .connection()
        .execute_batch("DELETE FROM sync_metadata")
        .expect("clear sync metadata");
    let sql = format!(
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('manifest_definitions', '{checked_at}', 'manifest', '{revision}', 'v')"
    );
    expect_rejected(store, &sql);
}

fn accept_membership_type(store: &SqliteStore, value: &str) {
    store
        .connection()
        .execute_batch("DELETE FROM active_profile")
        .expect("clear active profile");
    let sql = format!(
        "INSERT INTO active_profile (singleton, membership_id, membership_type) \
         VALUES (1, 'm', '{value}')"
    );
    store
        .connection()
        .execute_batch(&sql)
        .expect("accept an in-range membership type");
    let stored: String = store
        .connection()
        .query_row("SELECT membership_type FROM active_profile", [], |row| {
            row.get(0)
        })
        .expect("read stored membership type");
    assert_eq!(stored, value);
}

fn reject_membership_type(store: &SqliteStore, value: &str) {
    store
        .connection()
        .execute_batch("DELETE FROM active_profile")
        .expect("clear active profile");
    let sql = format!(
        "INSERT INTO active_profile (singleton, membership_id, membership_type) \
         VALUES (1, 'm', '{value}')"
    );
    expect_rejected(store, &sql);
}

#[test]
fn empty_database_migrates_to_latest_version_with_exact_schema() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    assert_eq!(LATEST_SCHEMA_VERSION, 9);
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(table_names(&store), strings(EXPECTED_TABLE_NAMES));
    assert_eq!(index_names(&store), strings(EXPECTED_INDEX_NAMES));

    for table in EXPECTED_TABLE_NAMES {
        assert_eq!(
            expected_columns(table),
            column_signature(&store, table),
            "unexpected columns for {table}"
        );
        assert!(
            table_sql(&store, table).contains("STRICT"),
            "{table} must be a STRICT table"
        );
    }
}

#[test]
fn reopening_and_rerunning_the_migration_is_idempotent() {
    let temp = TempDatabase::new();

    {
        let store = SqliteStore::open(temp.path()).expect("open file store");
        assert_eq!(
            store.schema_version().expect("read schema version"),
            LATEST_SCHEMA_VERSION
        );
        store
            .connection()
            .execute_batch(
                "INSERT INTO sync_metadata \
                 (dataset, checked_at, source, revision, definition_version) \
                 VALUES ('manifest_definitions', '18446744073709551615', 'manifest', '1', 'manifest-2026-09-21')",
            )
            .expect("seed committed metadata");
    }

    let mut store = SqliteStore::open(temp.path()).expect("reopen file store");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    store
        .migrate_to(LATEST_SCHEMA_VERSION)
        .expect("re-running the migration is a no-op");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(table_names(&store), strings(EXPECTED_TABLE_NAMES));

    let committed: i64 = store
        .connection()
        .query_row("SELECT count(*) FROM sync_metadata", [], |row| row.get(0))
        .expect("count committed metadata");
    assert_eq!(committed, 1);
}

#[test]
fn future_version_and_invalid_target_fail_closed_with_sanitized_errors() {
    let temp = TempDatabase::new();
    let path_text = temp.path().display().to_string();
    {
        let connection = Connection::open(temp.path()).expect("create future database");
        connection
            .pragma_update(None, "user_version", i64::from(LATEST_SCHEMA_VERSION + 1))
            .expect("write future user_version");
    }

    let error = SqliteStore::open(temp.path()).expect_err("refuse a newer database");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(
        error.to_string(),
        AppError::new(ErrorCategory::LocalStorage).to_string()
    );
    for leaked in ["user_version", "sqlite", "2", &path_text] {
        assert!(
            !error.to_string().contains(leaked),
            "error leaked {leaked}: {error}"
        );
    }

    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    let error = store
        .migrate_to(LATEST_SCHEMA_VERSION + 1)
        .expect_err("refuse an unsupported target");
    assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
    assert_eq!(
        error.to_string(),
        AppError::new(ErrorCategory::UnsupportedOperation).to_string()
    );
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(table_names(&store), strings(EXPECTED_TABLE_NAMES));
}

#[test]
fn downgrade_then_upgrade_is_reversible_without_stale_tables() {
    let temp = TempDatabase::new();

    {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store
            .migrate_to(0)
            .expect("apply the explicit down migration");
        assert_eq!(store.schema_version().expect("read schema version"), 0);
        assert!(table_names(&store).is_empty());
        assert!(index_names(&store).is_empty());

        store.migrate_to(LATEST_SCHEMA_VERSION).expect("reapply up");
        assert_eq!(
            store.schema_version().expect("read schema version"),
            LATEST_SCHEMA_VERSION
        );
        assert_eq!(table_names(&store), strings(EXPECTED_TABLE_NAMES));
        assert_eq!(index_names(&store), strings(EXPECTED_INDEX_NAMES));
    }

    let store = SqliteStore::open(temp.path()).expect("reopen after upgrade");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        LATEST_SCHEMA_VERSION
    );
    assert_eq!(table_names(&store), strings(EXPECTED_TABLE_NAMES));
}

#[test]
fn v2_up_migration_preserves_version_one_rows_and_starts_with_no_perks() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store.migrate_to(1).expect("downgrade to version 1");
    assert_eq!(store.schema_version().expect("read schema version"), 1);
    assert!(!table_names(&store).contains(&"item_perks".to_owned()));
    seed_v1_projection(&store);

    store.migrate_to(2).expect("apply the version-2 up step");

    assert_eq!(store.schema_version().expect("read schema version"), 2);
    assert!(table_names(&store).contains(&"item_perks".to_owned()));
    // Every version-1 row survives the up step, and the new child table starts
    // empty because the up step only creates it.
    assert_eq!(row_count(&store, "active_profile"), 1);
    assert_eq!(row_count(&store, "characters"), 1);
    assert_eq!(row_count(&store, "items"), 1);
    assert_eq!(row_count(&store, "sync_metadata"), 1);
    assert_eq!(row_count(&store, "item_perks"), 0);
    let membership: String = store
        .connection()
        .query_row(
            "SELECT membership_id FROM active_profile WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read the preserved anchor");
    assert_eq!(membership, "4611686018484639825");
    let revision: String = store
        .connection()
        .query_row(
            "SELECT revision FROM sync_metadata WHERE dataset = 'profile_inventory'",
            [],
            |row| row.get(0),
        )
        .expect("read the preserved metadata");
    assert_eq!(revision, "1");
}

#[test]
fn v2_down_drops_only_item_perks_then_reapply_up_preserves_v1_rows() {
    let temp = TempDatabase::new();
    {
        let mut store = SqliteStore::open(temp.path()).expect("open file store");
        store.migrate_to(1).expect("downgrade to version 1");
        seed_v1_projection(&store);
        store.migrate_to(2).expect("upgrade to version 2");
        store
            .connection()
            .execute(
                "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
                 VALUES ('item-1', 0, 42, 1, 0), ('item-1', 1, 42, 0, 1)",
                [],
            )
            .expect("seed two perk rows");
        assert_eq!(row_count(&store, "item_perks"), 2);

        // The down step drops exactly the perk child table.
        store.migrate_to(1).expect("apply the version-2 down step");
        assert_eq!(store.schema_version().expect("read schema version"), 1);
        assert!(!table_names(&store).contains(&"item_perks".to_owned()));
        assert_eq!(row_count(&store, "active_profile"), 1);
        assert_eq!(row_count(&store, "characters"), 1);
        assert_eq!(row_count(&store, "items"), 1);
        assert_eq!(row_count(&store, "sync_metadata"), 1);

        // Reapplying the up step succeeds and the version-1 rows are still there.
        store.migrate_to(2).expect("reapply the version-2 up step");
        assert_eq!(store.schema_version().expect("read schema version"), 2);
        assert!(table_names(&store).contains(&"item_perks".to_owned()));
        assert_eq!(row_count(&store, "item_perks"), 0);
        assert_eq!(row_count(&store, "items"), 1);
        assert_eq!(row_count(&store, "active_profile"), 1);
    }

    // The entire v1 -> v2 -> v1 -> v2 cycle stays durable across a reopen.
    let store = SqliteStore::open(temp.path()).expect("reopen after the cycle");
    assert_eq!(store.schema_version().expect("read schema version"), 9);
    assert_eq!(row_count(&store, "items"), 1);
    assert_eq!(row_count(&store, "item_perks"), 0);
}

#[test]
fn v7_plugs_migration_reverses_without_touching_profile_or_perks() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    store.migrate_to(6).unwrap();
    seed_v1_projection(&store);
    store
        .connection()
        .execute("INSERT INTO item_perks VALUES ('item-1',0,42,1,1)", [])
        .unwrap();
    store.migrate_to(7).unwrap();
    assert_eq!(row_count(&store, "item_plugs"), 0);
    store
        .connection()
        .execute("INSERT INTO item_plugs VALUES ('item-1',0,42)", [])
        .unwrap();
    store.migrate_to(6).unwrap();
    assert!(!table_names(&store).contains(&"item_plugs".to_owned()));
    for table in [
        "items",
        "characters",
        "active_profile",
        "item_perks",
        "sync_metadata",
    ] {
        assert_eq!(row_count(&store, table), 1);
    }
    store.migrate_to(7).unwrap();
    assert_eq!(row_count(&store, "item_plugs"), 0);
}

#[test]
fn item_plugs_schema_cascades_and_rejects_invalid_rows() {
    let store = SqliteStore::open_in_memory().unwrap();
    seed_v1_projection(&store);
    store
        .connection()
        .execute("INSERT INTO item_plugs VALUES ('item-1',0,42)", [])
        .unwrap();
    for sql in [
        "INSERT INTO item_plugs VALUES ('item-1',0,1)",
        "INSERT INTO item_plugs VALUES ('item-1',1,-1)",
        "INSERT INTO item_plugs VALUES ('item-1',1,4294967296)",
        "INSERT INTO item_plugs VALUES ('item-1',-1,1)",
        "INSERT INTO item_plugs VALUES ('item-1',32,1)",
        "INSERT INTO item_plugs VALUES ('missing',0,1)",
        "INSERT INTO item_plugs VALUES ('item-1',1,1.5)",
    ] {
        expect_rejected(&store, sql);
    }
    store.connection().execute("DELETE FROM items", []).unwrap();
    assert_eq!(row_count(&store, "item_plugs"), 0);
}

#[test]
fn item_perks_schema_cascades_and_rejects_invalid_rows() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch(
            "INSERT INTO characters (character_id, name) VALUES ('hunter-1', 'Nova');\
             INSERT INTO items \
             (instance_id, item_hash, location_kind, quantity, equipped, locked) \
             VALUES ('item-1', 1, 'vault', 1, 0, 0);\
             INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
             VALUES ('item-1', 0, 1, 1, 1);",
        )
        .expect("seed an item and one perk");

    // Out-of-range hashes, non-boolean flags, a negative ordinal, and a
    // duplicated ordinal are all rejected by the schema itself.
    for statement in [
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', 1, 4294967296, 0, 0)",
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', 1, -1, 0, 0)",
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', 1, 1, 2, 0)",
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', 1, 1, 0, -1)",
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', -1, 1, 0, 0)",
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) VALUES ('item-1', 0, 1, 0, 0)",
    ] {
        expect_rejected(&store, statement);
    }

    // A dangling instance reference is rejected while foreign keys are on.
    expect_rejected(
        &store,
        "INSERT INTO item_perks (instance_id, ordinal, perk_hash, active, visible) \
         VALUES ('missing-item', 0, 1, 0, 0)",
    );

    // Deleting the item cascades to its perks.
    store
        .connection()
        .execute("DELETE FROM items WHERE instance_id = 'item-1'", [])
        .expect("delete the item");
    assert_eq!(
        row_count(&store, "item_perks"),
        0,
        "deleting an item must cascade to its perk rows"
    );
}

#[test]
fn failed_migration_step_rolls_back_schema_and_version_together() {
    let mut store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .migrate_to(0)
        .expect("apply the explicit down migration");
    assert_eq!(store.schema_version().expect("read schema version"), 0);

    // The first statement succeeds, the second fails, so the whole step must be
    // undone: no probe table and no version bump.
    let failing = "CREATE TABLE rollback_probe (value INTEGER NOT NULL);\n\
                   INSERT INTO absent_table (value) VALUES (1);";
    let error = migrations::apply_up_for_test(store.connection_mut(), 1, failing)
        .expect_err("a failing step must not commit");
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(
        error.to_string(),
        AppError::new(ErrorCategory::LocalStorage).to_string()
    );
    assert_eq!(store.schema_version().expect("read schema version"), 0);
    assert!(table_names(&store).is_empty());

    // The same seam commits schema and version together when the SQL succeeds.
    migrations::apply_up_for_test(
        store.connection_mut(),
        1,
        "CREATE TABLE commit_probe (value INTEGER);",
    )
    .expect("a successful step commits");
    assert_eq!(
        store.schema_version().expect("read schema version"),
        1,
        "committed steps must advance user_version"
    );
    assert!(table_names(&store).contains(&"commit_probe".to_owned()));
}

#[test]
fn file_connection_policy_is_applied() {
    let temp = TempDatabase::new();
    let store = SqliteStore::open(temp.path()).expect("open file store");

    assert_eq!(pragma_i64(&store, "foreign_keys"), 1);
    assert_eq!(
        pragma_i64(&store, "busy_timeout"),
        i64::from(BUSY_TIMEOUT_MILLIS)
    );
    assert_eq!(pragma_i64(&store, "trusted_schema"), 0);
    assert_eq!(pragma_text(&store, "journal_mode"), "wal");
    assert_eq!(pragma_i64(&store, "synchronous"), 1);
}

#[test]
fn in_memory_connection_policy_is_applied_without_wal() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    assert_eq!(pragma_i64(&store, "foreign_keys"), 1);
    assert_eq!(
        pragma_i64(&store, "busy_timeout"),
        i64::from(BUSY_TIMEOUT_MILLIS)
    );
    assert_eq!(pragma_i64(&store, "trusted_schema"), 0);
    // In-memory databases cannot use WAL, so no WAL semantics are required here.
    assert_ne!(pragma_text(&store, "journal_mode"), "wal");
    assert_eq!(pragma_text(&store, "journal_mode"), "memory");
}

#[test]
fn foreign_keys_are_enforced() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch(
            "INSERT INTO characters (character_id, name, class_name, race, gender, light) \
             VALUES ('hunter-1', 'Nova', 'Hunter', 'Human', 'Female', 1810);\
             INSERT INTO items \
             (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) \
             VALUES ('item-1', 2522695, 'character', 'hunter-1', NULL, 1, 1, 0, 1810);",
        )
        .expect("seed a character and a referenced item");

    expect_rejected(
        &store,
        "INSERT INTO items \
         (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) \
         VALUES ('item-x', 1, 'character', 'missing-character', NULL, 1, 0, 0, NULL)",
    );
    // A referenced character cannot be deleted while an item points at it.
    expect_rejected(
        &store,
        "DELETE FROM characters WHERE character_id = 'hunter-1'",
    );
}

#[test]
fn check_constraints_reject_invalid_rows() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch(
            "INSERT INTO definitions \
             (hash, bucket_hash, name, item_type, tier, element, definition_version) \
             VALUES (2522695, 1498876634, 'Suros Regime', 'Auto Rifle', 'Exotic', NULL, 'manifest-2026-09-21');",
        )
        .expect("seed a valid definition");

    // Required and bounded display strings.
    expect_rejected(
        &store,
        "INSERT INTO definitions (hash, name, definition_version) VALUES (1, '', 'v')",
    );
    expect_rejected(
        &store,
        "INSERT INTO definitions (hash, name, definition_version) VALUES (2, 'ok', '')",
    );

    // Tag order is bounded at 32 entries and tags are non-empty.
    expect_rejected(
        &store,
        "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (2522695, 32, 'overflow')",
    );
    expect_rejected(
        &store,
        "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (2522695, 2, '')",
    );

    // One active profile only, with a canonical unsigned membership type.
    store
        .connection()
        .execute_batch(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (1, '4611686018484639825', '3');",
        )
        .expect("seed the active profile");
    expect_rejected(
        &store,
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (2, 'x', '3')",
    );
    expect_rejected(
        &store,
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (1, 'x', '4294967296')",
    );
    expect_rejected(
        &store,
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (1, 'x', '03')",
    );

    // Character bounds.
    expect_rejected(
        &store,
        "INSERT INTO characters (character_id, light) VALUES ('c', 70000)",
    );

    // Item location consistency, quantity, and boolean flags.
    let base = "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ";
    expect_rejected(
        &store,
        &format!("{base}('i1', 1, 'vault', NULL, 'extra', 1, 0, 0, NULL)"),
    );
    expect_rejected(
        &store,
        &format!("{base}('i2', 1, 'character', NULL, NULL, 1, 0, 0, NULL)"),
    );
    expect_rejected(
        &store,
        &format!("{base}('i3', 1, 'other', NULL, NULL, 1, 0, 0, NULL)"),
    );
    expect_rejected(
        &store,
        &format!("{base}('i4', 1, 'vault', NULL, NULL, -1, 0, 0, NULL)"),
    );
    expect_rejected(
        &store,
        &format!("{base}('i5', 1, 'vault', NULL, NULL, 1, 2, 0, NULL)"),
    );
    expect_rejected(
        &store,
        &format!("{base}('i6', 1, 'vault', NULL, NULL, 1, 0, 0, 65536)"),
    );

    // Sync metadata: fixed dataset/source pairing, canonical unsigned text, and
    // the full u64 range without silent truncation.
    store
        .connection()
        .execute_batch(
            "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
             VALUES ('manifest_definitions', '18446744073709551615', 'manifest', '1', 'manifest-2026-09-21');",
        )
        .expect("accept the maximum u64 checked_at");
    expect_rejected(
        &store,
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('profile_inventory', '1', 'manifest', '1', 'v')",
    );
    expect_rejected(
        &store,
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('profile_inventory', '18446744073709551616', 'profile', '1', 'v')",
    );
    expect_rejected(
        &store,
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('profile_inventory', '01', 'profile', '1', 'v')",
    );
    expect_rejected(
        &store,
        "INSERT INTO sync_metadata (dataset, checked_at, source, revision, definition_version) \
         VALUES ('manifest_definitions', '1', 'manifest', '1', '')",
    );
}

#[test]
fn canonical_unsigned_text_rejects_oversized_and_malformed_values() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    // Exact boundaries are accepted and round-trip unchanged.
    accept_sync_metadata(&store, "0", "0");
    accept_sync_metadata(&store, "18446744073709551615", "18446744073709551615");
    accept_sync_metadata(&store, "10000000000000000000", "1");

    // Empty, non-digit, signed, whitespace, and leading-zero values are not
    // canonical decimal text.
    for value in ["", "12a", "a12", "01", "0x1", "-1", " 1", "1 "] {
        reject_sync_metadata(&store, value, "1");
        reject_sync_metadata(&store, "1", value);
    }

    // Over-length values with lexically low prefixes must not slip past the
    // value ceiling, and the exact maximum plus one must be refused.
    for value in [
        "100000000000000000000",
        "18446744073709551616",
        "99999999999999999999999",
    ] {
        reject_sync_metadata(&store, value, "1");
        reject_sync_metadata(&store, "1", value);
    }
}

#[test]
fn canonical_membership_type_rejects_oversized_and_malformed_values() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    accept_membership_type(&store, "0");
    accept_membership_type(&store, "4294967295");

    for value in ["", "12a", "a12", "03", "0x1", "-1", " 1", "1 "] {
        reject_membership_type(&store, value);
    }

    // Eleven digits with a lexically low prefix, and the exact maximum plus one.
    for value in ["10000000000", "4294967296", "99999999999"] {
        reject_membership_type(&store, value);
    }
}

#[test]
fn integer_columns_reject_real_values() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch(
            "INSERT INTO definitions (hash, name, definition_version) VALUES (10, 'n', 'v');",
        )
        .expect("seed a definition");

    for statement in [
        "INSERT INTO definitions (hash, name, definition_version) VALUES (2.5, 'n', 'v')",
        "INSERT INTO definitions (hash, bucket_hash, name, definition_version) VALUES (11, 2.5, 'n', 'v')",
        "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (10, 0.5, 't')",
        "INSERT INTO characters (character_id, light) VALUES ('real-light', 1810.5)",
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (1.5, 'm', '3')",
        "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ('r1', 2.5, 'vault', NULL, NULL, 1, 0, 0, NULL)",
        "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ('r2', 1, 'vault', NULL, NULL, 2.5, 0, 0, NULL)",
        "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ('r3', 1, 'vault', NULL, NULL, 1, 0.5, 0, NULL)",
        "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ('r4', 1, 'vault', NULL, NULL, 1, 0, 0.5, NULL)",
        "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, location_other, quantity, equipped, locked, power) VALUES ('r5', 1, 'vault', NULL, NULL, 1, 0, 0, 2.5)",
    ] {
        expect_rejected(&store, statement);
    }
}

#[test]
fn integer_columns_only_coerce_losslessly_and_never_change_the_value() {
    // The one coercion SQLite performs despite STRICT and `typeof` guards: an
    // integral REAL and a numeric text value convert to the integer they
    // already represent. It is asserted here instead of hidden. The typed Rust
    // API only binds Rust integer types, so this path is unreachable from
    // repository code, and the stored value is always an in-range integer, so
    // no domain invariant is weakened.
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    store
        .connection()
        .execute_batch(
            "INSERT INTO items (instance_id, item_hash, location_kind, quantity, equipped, locked) \
             VALUES ('real', 1.0, 'vault', 5.0, 0.0, 1.0);\
             INSERT INTO items (instance_id, item_hash, location_kind, quantity, equipped, locked) \
             VALUES ('text', '1', 'vault', '5', '0', '1');",
        )
        .expect("lossless integer coercion is accepted");

    let mut statement = store
        .connection()
        .prepare(
            "SELECT typeof(item_hash), typeof(quantity), typeof(equipped), typeof(locked) \
             FROM items ORDER BY instance_id",
        )
        .expect("prepare type query");
    let types: Vec<(String, String, String, String)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("run type query")
        .map(|row| row.expect("read type row"))
        .collect();

    assert_eq!(types.len(), 2);
    let integer = (
        "integer".to_owned(),
        "integer".to_owned(),
        "integer".to_owned(),
        "integer".to_owned(),
    );
    assert!(types.iter().all(|row| row == &integer));
}

#[test]
fn integer_columns_enforce_exact_bounds() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");
    let item = |values: &str| {
        format!(
            "INSERT INTO items (instance_id, item_hash, location_kind, location_character_id, \
             location_other, quantity, equipped, locked, power) VALUES ({values})"
        )
    };

    // Accepted lower and upper boundaries, including a signed 64-bit hash.
    store
        .connection()
        .execute_batch(&item(
            "'lo', -9223372036854775807, 'vault', NULL, NULL, 0, 0, 0, 0",
        ))
        .expect("accept lower integer bounds");
    store
        .connection()
        .execute_batch(&item(
            "'hi', 2522695, 'vault', NULL, NULL, 4294967295, 1, 1, 65535",
        ))
        .expect("accept upper integer bounds");

    for values in [
        "'x1', 1, 'vault', NULL, NULL, -1, 0, 0, NULL",
        "'x2', 1, 'vault', NULL, NULL, 4294967296, 0, 0, NULL",
        "'x3', 1, 'vault', NULL, NULL, 1, 2, 0, NULL",
        "'x4', 1, 'vault', NULL, NULL, 1, -1, 0, NULL",
        "'x5', 1, 'vault', NULL, NULL, 1, 0, 2, NULL",
        "'x6', 1, 'vault', NULL, NULL, 1, 0, 0, -1",
        "'x7', 1, 'vault', NULL, NULL, 1, 0, 0, 65536",
    ] {
        expect_rejected(&store, &item(values));
    }

    // Character light bounds.
    store
        .connection()
        .execute_batch(
            "INSERT INTO characters (character_id, light) VALUES ('lo', 0), ('hi', 65535);",
        )
        .expect("accept light bounds");
    expect_rejected(
        &store,
        "INSERT INTO characters (character_id, light) VALUES ('neg', -1)",
    );
    expect_rejected(
        &store,
        "INSERT INTO characters (character_id, light) VALUES ('over', 65536)",
    );

    // Tag ordinal bounds.
    store
        .connection()
        .execute_batch(
            "INSERT INTO definitions (hash, name, definition_version) VALUES (20, 'n', 'v');\
             INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (20, 0, 'a'), (20, 31, 'b');",
        )
        .expect("accept ordinal bounds");
    expect_rejected(
        &store,
        "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (20, -1, 'c')",
    );
    expect_rejected(
        &store,
        "INSERT INTO definition_tags (definition_hash, ordinal, tag) VALUES (20, 32, 'd')",
    );

    // The active-profile singleton is exactly one.
    store
        .connection()
        .execute_batch(
            "INSERT INTO active_profile (singleton, membership_id, membership_type) \
             VALUES (1, 'm', '3');",
        )
        .expect("accept the singleton one");
    expect_rejected(
        &store,
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (0, 'm', '3')",
    );
    expect_rejected(
        &store,
        "INSERT INTO active_profile (singleton, membership_id, membership_type) VALUES (2, 'm', '3')",
    );
}

#[test]
fn schema_contains_no_secret_or_payload_vocabulary() {
    let store = SqliteStore::open_in_memory().expect("open in-memory store");

    let mut statement = store
        .connection()
        .prepare("SELECT name, sql FROM sqlite_master WHERE type IN ('table', 'index')")
        .expect("prepare schema query");
    let objects: Vec<(String, String)> = statement
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let sql: Option<String> = row.get(1)?;
            Ok((name, sql.unwrap_or_default()))
        })
        .expect("run schema query")
        .map(|row| row.expect("read schema row"))
        .collect();
    assert!(!objects.is_empty());

    for (name, sql) in objects {
        let haystack = format!("{name} {sql}").to_lowercase();
        for prohibited in PROHIBITED_SCHEMA_VOCABULARY {
            assert!(
                !haystack.contains(prohibited),
                "{name} contains prohibited vocabulary {prohibited}"
            );
        }
    }

    // Every stored value is text or an integer, so no generic blob or JSON
    // payload store exists.
    for table in EXPECTED_TABLE_NAMES {
        for (column, column_type) in column_signature(&store, table) {
            assert!(
                column_type == "TEXT" || column_type == "INTEGER",
                "{table}.{column} has unexpected type {column_type}"
            );
        }
    }
}

#[test]
fn storage_failure_helper_is_sanitized() {
    let error = storage_failure();
    assert_eq!(error.category(), ErrorCategory::LocalStorage);
    assert_eq!(
        error.to_string(),
        AppError::new(ErrorCategory::LocalStorage).to_string()
    );
}

/// Migration-layer guard: the migration module is a pure schema and version
/// authority and must stay free of repository behavior. Repository
/// implementations live in sibling modules, so this guard inspects only
/// `migrations.rs`. The negative assertion is expressed against its own authored
/// source because it cannot be written as a trait-impl check without
/// specialization or a new dependency.
#[test]
fn migrations_module_remains_schema_only() {
    let source = include_str!("migrations.rs");
    for marker in [
        "impl DefinitionRepository",
        "impl InventoryRepository",
        "DefinitionRepository for",
        "InventoryRepository for",
        "fn replace_definitions",
        "fn replace_current",
        "fn lookup_definition",
        "fn load_current",
    ] {
        assert!(
            !source.contains(marker),
            "unexpected repository surface in the migration layer: {marker}"
        );
    }
}
