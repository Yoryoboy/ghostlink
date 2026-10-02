//! Explicit, monotonic, reversible schema migrations.
//!
//! Migration descriptors are versioned, ordered, and carry both directions of
//! SQL. The runner never builds SQL from caller input: it selects descriptors by
//! version and executes their literal statements inside one transaction per
//! step. That same transaction records `PRAGMA user_version`, so a step either
//! leaves the schema and its version advanced together or leaves both untouched.
//!
//! The version-1 schema is a normalized, secret-free projection of provider
//! domains. Every table is `STRICT`, so SQLite enforces the declared column
//! types, and integer columns additionally carry `typeof(...) = 'integer'`
//! guards and explicit range checks. Signed `i64` hashes are stored as the
//! native signed `INTEGER` type, and values that are wider than `i64` semantics
//! in the Rust domain (`u64` timestamps and revisions, `u32` membership type)
//! are stored as canonical decimal `TEXT` with `CHECK` constraints: digits only,
//! no leading zero, an explicit maximum length, and, at that maximum length, an
//! explicit value ceiling. Later repository modules parse and range-check those
//! values; no unsigned value wider than SQLite's signed integer capacity is
//! written as `INTEGER`, while bounded `u32` values use exact range checks, so
//! nothing can truncate silently.
//!
//! The version-2 schema adds the normalized, secret-free `item_perks` child
//! projection. It is `STRICT` like every other table, is keyed by item instance
//! plus a zero-based ordinal, references `items(instance_id)` with
//! `ON DELETE CASCADE`, and constrains its perk hash to the exact unsigned
//! `u32` range and its two state flags to the exact integers `0`/`1`. The up step
//! only creates that table, so every version-1 row survives unchanged; the down
//! step drops only that table, so a downgrade removes perk summaries without
//! touching the version-1 profile projection. Ordinal uniqueness is supplied by
//! the composite primary key, and the repository rejects non-contiguous ordinals
//! when it reads them back.
//!
//! One coercion boundary is inherent to SQLite and documented rather than
//! hidden: under `STRICT`, a fractional `REAL` or a `BLOB` is rejected for an
//! `INTEGER` column, while an integral `REAL` (such as `1.0`) and a numeric text
//! value (such as `'5'`) are losslessly converted to the integer they already
//! represent. The typed Rust API only binds Rust integer types, so those
//! coercions are unreachable from repository code, and the stored value is
//! always an in-range integer.

use super::storage_failure;
use destiny_core::{AppError, ErrorCategory};
use rusqlite::Connection;

/// Highest schema version defined by this build.
pub(crate) const LATEST_VERSION: u32 = 3;

/// One explicit migration step with an up and a down direction.
struct Migration {
    version: u32,
    up: &'static str,
    down: &'static str,
}

/// Ordered, monotonic migration descriptors. Versions must be contiguous from 1.
const MIGRATIONS: [Migration; 3] = [
    Migration {
        version: 1,
        up: SCHEMA_V1_UP,
        down: SCHEMA_V1_DOWN,
    },
    Migration {
        version: 2,
        up: SCHEMA_V2_UP,
        down: SCHEMA_V2_DOWN,
    },
    Migration {
        version: 3,
        up: SCHEMA_V3_UP,
        down: SCHEMA_V3_DOWN,
    },
];

/// Version-1 schema: normalized sync metadata, definitions, one active profile,
/// characters, and item instances.
///
/// Deliberate properties:
///
/// - `sync_metadata` holds exactly the two datasets the MVP owns, and a `CHECK`
///   pins each dataset to its single legal source marker.
/// - `definitions.definition_version` and `sync_metadata.definition_version`
///   preserve the exact Manifest version string; display strings are stored
///   verbatim and only bounded in length.
/// - `definition_tags` preserves tag order through `ordinal` and bounds the
///   count at 32 by construction (`ordinal BETWEEN 0 AND 31`).
/// - `active_profile` is a singleton row, so only one MVP account can be active.
/// - `items.location_*` are a typed, constrained location: exactly one
///   representation is populated for each `location_kind`, and character
///   locations reference a stored character.
/// - Every table is `STRICT`; integer columns additionally require
///   `typeof(column) = 'integer'` and their exact range, so fractional `REAL`
///   and `BLOB` values cannot be stored in an integer domain.
/// - Canonical decimal columns carry an explicit maximum length (20 for `u64`
///   fields, 10 for the `u32` membership type) before the lexicographic value
///   ceiling, so an over-long value with a lexically low prefix cannot pass.
/// - No credential, token, API-key, authorization-code, callback, or raw-payload
///   column exists, and there is no generic blob or JSON store.
/// - Plain indexes exist only for deterministic joins and reads. No full-text or
///   search index is created here; search belongs to a later slice.
const SCHEMA_V1_UP: &str = r#"
CREATE TABLE sync_metadata (
    dataset TEXT NOT NULL PRIMARY KEY
        CHECK (dataset IN ('manifest_definitions', 'profile_inventory')),
    checked_at TEXT NOT NULL
        CHECK (
            checked_at GLOB '[0-9]*'
            AND checked_at NOT GLOB '*[^0-9]*'
            AND (checked_at = '0' OR checked_at GLOB '[1-9]*')
            AND length(checked_at) <= 20
            AND (length(checked_at) < 20 OR checked_at <= '18446744073709551615')
        ),
    source TEXT NOT NULL
        CHECK (source IN ('manifest', 'profile')),
    revision TEXT NOT NULL
        CHECK (
            revision GLOB '[0-9]*'
            AND revision NOT GLOB '*[^0-9]*'
            AND (revision = '0' OR revision GLOB '[1-9]*')
            AND length(revision) <= 20
            AND (length(revision) < 20 OR revision <= '18446744073709551615')
        ),
    definition_version TEXT NOT NULL
        CHECK (length(definition_version) BETWEEN 1 AND 128),
    CHECK (
        (dataset = 'manifest_definitions' AND source = 'manifest')
        OR (dataset = 'profile_inventory' AND source = 'profile')
    )
) STRICT;

CREATE TABLE definitions (
    hash INTEGER PRIMARY KEY
        CHECK (typeof(hash) = 'integer'),
    bucket_hash INTEGER
        CHECK (bucket_hash IS NULL OR typeof(bucket_hash) = 'integer'),
    name TEXT NOT NULL
        CHECK (length(name) BETWEEN 1 AND 256),
    item_type TEXT
        CHECK (item_type IS NULL OR length(item_type) BETWEEN 1 AND 256),
    tier TEXT
        CHECK (tier IS NULL OR length(tier) BETWEEN 1 AND 256),
    element TEXT
        CHECK (element IS NULL OR length(element) BETWEEN 1 AND 256),
    definition_version TEXT NOT NULL
        CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;

CREATE TABLE definition_tags (
    definition_hash INTEGER NOT NULL
        REFERENCES definitions(hash) ON DELETE CASCADE
        CHECK (typeof(definition_hash) = 'integer'),
    ordinal INTEGER NOT NULL
        CHECK (typeof(ordinal) = 'integer' AND ordinal BETWEEN 0 AND 31),
    tag TEXT NOT NULL
        CHECK (length(tag) BETWEEN 1 AND 64),
    PRIMARY KEY (definition_hash, ordinal),
    UNIQUE (definition_hash, tag)
) STRICT;

CREATE TABLE active_profile (
    singleton INTEGER NOT NULL PRIMARY KEY
        CHECK (typeof(singleton) = 'integer' AND singleton = 1),
    membership_id TEXT NOT NULL
        CHECK (length(membership_id) BETWEEN 1 AND 128),
    membership_type TEXT NOT NULL
        CHECK (
            membership_type GLOB '[0-9]*'
            AND membership_type NOT GLOB '*[^0-9]*'
            AND (membership_type = '0' OR membership_type GLOB '[1-9]*')
            AND length(membership_type) <= 10
            AND (length(membership_type) < 10 OR membership_type <= '4294967295')
        )
) STRICT;

CREATE TABLE characters (
    character_id TEXT NOT NULL PRIMARY KEY
        CHECK (length(character_id) BETWEEN 1 AND 128),
    name TEXT
        CHECK (name IS NULL OR length(name) BETWEEN 1 AND 256),
    class_name TEXT
        CHECK (class_name IS NULL OR length(class_name) BETWEEN 1 AND 256),
    race TEXT
        CHECK (race IS NULL OR length(race) BETWEEN 1 AND 256),
    gender TEXT
        CHECK (gender IS NULL OR length(gender) BETWEEN 1 AND 256),
    light INTEGER
        CHECK (light IS NULL OR (typeof(light) = 'integer' AND light BETWEEN 0 AND 65535))
) STRICT;

CREATE TABLE items (
    instance_id TEXT NOT NULL PRIMARY KEY
        CHECK (length(instance_id) BETWEEN 1 AND 128),
    item_hash INTEGER NOT NULL
        CHECK (typeof(item_hash) = 'integer'),
    location_kind TEXT NOT NULL
        CHECK (location_kind IN ('vault', 'character', 'other')),
    location_character_id TEXT
        REFERENCES characters(character_id) ON DELETE RESTRICT,
    location_other TEXT
        CHECK (location_other IS NULL OR length(location_other) BETWEEN 1 AND 256),
    quantity INTEGER NOT NULL
        CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 0 AND 4294967295),
    equipped INTEGER NOT NULL
        CHECK (typeof(equipped) = 'integer' AND equipped IN (0, 1)),
    locked INTEGER NOT NULL
        CHECK (typeof(locked) = 'integer' AND locked IN (0, 1)),
    power INTEGER
        CHECK (power IS NULL OR (typeof(power) = 'integer' AND power BETWEEN 0 AND 65535)),
    CHECK (
        (location_kind = 'vault' AND location_character_id IS NULL AND location_other IS NULL)
        OR (location_kind = 'character' AND location_character_id IS NOT NULL AND location_other IS NULL)
        OR (location_kind = 'other' AND location_character_id IS NULL AND location_other IS NOT NULL)
    )
) STRICT;

CREATE INDEX items_definition_hash_idx ON items(item_hash);
CREATE INDEX items_location_idx ON items(location_kind, location_character_id);
"#;

/// Exact reverse of [`SCHEMA_V1_UP`]. Children are dropped before the parents
/// they reference, so foreign-key enforcement stays satisfied while unwinding.
const SCHEMA_V1_DOWN: &str = r#"
DROP TABLE IF EXISTS definition_tags;
DROP TABLE IF EXISTS definitions;
DROP TABLE IF EXISTS items;
DROP TABLE IF EXISTS characters;
DROP TABLE IF EXISTS active_profile;
DROP TABLE IF EXISTS sync_metadata;
"#;

/// Version-2 schema: one normalized, secret-free child projection of item perk
/// summaries.
///
/// Deliberate properties:
///
/// - `item_perks` is keyed by `(instance_id, ordinal)`, so an item's ordered
///   perk list can never contain two rows at the same ordinal, and the ordinal
///   is the sole deterministic ordering key on read.
/// - `instance_id` references `items(instance_id)` with `ON DELETE CASCADE`, so
///   removing an item removes exactly its perk summaries and no orphan can be
///   committed while foreign-key enforcement is on.
/// - `ordinal` is a checked non-negative integer. The repository additionally
///   requires a stored item's ordinals to run contiguously from zero, so a gap,
///   a negative value, or a trailing extra row fails closed on read.
/// - `perk_hash` is stored as the native signed `INTEGER` type but constrained to
///   the exact unsigned `u32` range `0..=4294967295`, matching [`PerkHash`].
/// - `active` and `visible` are constrained to the exact integers `0`/`1`, so no
///   other value can be committed as a boolean.
/// - The table is `STRICT` and integer columns additionally require
///   `typeof(column) = 'integer'`, so a fractional `REAL` or a `BLOB` cannot be
///   stored in an integer domain.
/// - No display text, raw payload, socket, or credential column exists, and the
///   perk hash is an opaque reference rather than a rendered string.
///
/// [`PerkHash`]: destiny_core::PerkHash
const SCHEMA_V2_UP: &str = r#"
CREATE TABLE item_perks (
    instance_id TEXT NOT NULL
        REFERENCES items(instance_id) ON DELETE CASCADE
        CHECK (length(instance_id) BETWEEN 1 AND 128),
    ordinal INTEGER NOT NULL
        CHECK (typeof(ordinal) = 'integer' AND ordinal >= 0),
    perk_hash INTEGER NOT NULL
        CHECK (typeof(perk_hash) = 'integer' AND perk_hash BETWEEN 0 AND 4294967295),
    active INTEGER NOT NULL
        CHECK (typeof(active) = 'integer' AND active IN (0, 1)),
    visible INTEGER NOT NULL
        CHECK (typeof(visible) = 'integer' AND visible IN (0, 1)),
    PRIMARY KEY (instance_id, ordinal)
) STRICT;
"#;

/// Exact reverse of [`SCHEMA_V2_UP`].
///
/// It drops only the perk child table, so a version-2 database can return to the
/// complete version-1 profile projection with every version-1 row intact.
const SCHEMA_V2_DOWN: &str = r#"
DROP TABLE IF EXISTS item_perks;
"#;

/// All four Manifest tables share the existing manifest_definitions metadata
/// row: one version and one transaction own the entire projection. No metadata
/// rebuild is needed, and dropping only these tables restores v2 exactly.
const SCHEMA_V3_UP: &str = r#"
CREATE TABLE milestone_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    description TEXT NOT NULL CHECK (length(description) <= 2048),
    milestone_type INTEGER NOT NULL CHECK (typeof(milestone_type) = 'integer' AND milestone_type BETWEEN 0 AND 4294967295),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
CREATE TABLE activity_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    description TEXT NOT NULL CHECK (length(description) <= 2048),
    activity_type_hash INTEGER NOT NULL CHECK (typeof(activity_type_hash) = 'integer' AND activity_type_hash BETWEEN 0 AND 4294967295),
    light_level INTEGER NOT NULL CHECK (typeof(light_level) = 'integer' AND light_level BETWEEN -2147483648 AND 2147483647),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
CREATE TABLE activity_modifier_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    description TEXT NOT NULL CHECK (length(description) <= 2048),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
"#;

const SCHEMA_V3_DOWN: &str = r#"
DROP TABLE IF EXISTS activity_modifier_definitions;
DROP TABLE IF EXISTS activity_definitions;
DROP TABLE IF EXISTS milestone_definitions;
"#;

/// Reads the recorded schema version from `PRAGMA user_version`.
///
/// SQLite stores that value as a signed 32-bit integer; a negative or otherwise
/// unrepresentable value is treated as corrupt local state, not as version 0.
pub(crate) fn read_user_version(connection: &Connection) -> Result<u32, AppError> {
    let raw: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| storage_failure())?;
    u32::try_from(raw).map_err(|_| storage_failure())
}

/// Migrates `connection` to `target`, rejecting any unsupported target or state.
///
/// Only targets in `0..=LATEST_VERSION` are accepted. A stored version above
/// `LATEST_VERSION` is a newer database than this build understands and fails
/// closed. Each applied step is atomic on its own, and steps run in ascending
/// order for upgrades and descending order for downgrades.
pub(crate) fn migrate_to(connection: &mut Connection, target: u32) -> Result<(), AppError> {
    if target > LATEST_VERSION {
        return Err(AppError::new(ErrorCategory::UnsupportedOperation));
    }
    let current = read_user_version(connection)?;
    if current > LATEST_VERSION {
        return Err(storage_failure());
    }
    if current < target {
        for migration in MIGRATIONS
            .iter()
            .filter(|migration| migration.version > current && migration.version <= target)
        {
            apply_step(connection, migration.version, migration.up, true)?;
        }
    } else {
        for migration in MIGRATIONS
            .iter()
            .rev()
            .filter(|migration| migration.version <= current && migration.version > target)
        {
            apply_step(connection, migration.version, migration.down, false)?;
        }
    }
    Ok(())
}

/// Executes one migration step and its version update in a single transaction.
///
/// `forward` selects the recorded version: an applied up step records its own
/// version, and an applied down step records the version it returns to.
fn apply_step(
    connection: &mut Connection,
    version: u32,
    statements: &str,
    forward: bool,
) -> Result<(), AppError> {
    let transaction = connection.transaction().map_err(|_| storage_failure())?;
    transaction
        .execute_batch(statements)
        .map_err(|_| storage_failure())?;
    let recorded = if forward { version } else { version - 1 };
    transaction
        .pragma_update(None, "user_version", i64::from(recorded))
        .map_err(|_| storage_failure())?;
    transaction.commit().map_err(|_| storage_failure())
}

/// Private seam that lets tests drive one explicit up step from arbitrary SQL.
///
/// It exposes nothing publicly and exists only to prove that a failing step
/// rolls its schema change and its `user_version` update back together.
#[cfg(test)]
pub(crate) fn apply_up_for_test(
    connection: &mut Connection,
    version: u32,
    statements: &str,
) -> Result<(), AppError> {
    apply_step(connection, version, statements, true)
}
