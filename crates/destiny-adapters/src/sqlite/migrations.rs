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
pub(crate) const LATEST_VERSION: u32 = 32;

/// One explicit migration step with an up and a down direction.
struct Migration {
    version: u32,
    up: &'static str,
    down: &'static str,
}

/// Ordered, monotonic migration descriptors. Versions must be contiguous from 1.
const MIGRATIONS: [Migration; 32] = [
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
    Migration {
        version: 4,
        up: SCHEMA_V4_UP,
        down: SCHEMA_V4_DOWN,
    },
    Migration {
        version: 5,
        up: SCHEMA_V5_UP,
        down: SCHEMA_V5_DOWN,
    },
    Migration {
        version: 6,
        up: SCHEMA_V6_UP,
        down: SCHEMA_V6_DOWN,
    },
    Migration {
        version: 7,
        up: SCHEMA_V7_UP,
        down: SCHEMA_V7_DOWN,
    },
    Migration {
        version: 8,
        up: SCHEMA_V8_UP,
        down: SCHEMA_V8_DOWN,
    },
    Migration {
        version: 9,
        up: SCHEMA_V9_UP,
        down: SCHEMA_V9_DOWN,
    },
    Migration {
        version: 10,
        up: SCHEMA_V10_UP,
        down: SCHEMA_V10_DOWN,
    },
    Migration {
        version: 11,
        up: SCHEMA_V11_UP,
        down: SCHEMA_V11_DOWN,
    },
    Migration {
        version: 12,
        up: SCHEMA_V12_UP,
        down: SCHEMA_V12_DOWN,
    },
    Migration {
        version: 13,
        up: SCHEMA_V13_UP,
        down: SCHEMA_V13_DOWN,
    },
    Migration {
        version: 14,
        up: SCHEMA_V14_UP,
        down: SCHEMA_V14_DOWN,
    },
    Migration {
        version: 15,
        up: SCHEMA_V15_UP,
        down: SCHEMA_V15_DOWN,
    },
    Migration {
        version: 16,
        up: SCHEMA_V16_UP,
        down: SCHEMA_V16_DOWN,
    },
    Migration {
        version: 17,
        up: SCHEMA_V17_UP,
        down: SCHEMA_V17_DOWN,
    },
    Migration {
        version: 18,
        up: SCHEMA_V18_UP,
        down: SCHEMA_V18_DOWN,
    },
    Migration {
        version: 19,
        up: SCHEMA_V19_UP,
        down: SCHEMA_V19_DOWN,
    },
    Migration {
        version: 20,
        up: SCHEMA_V20_UP,
        down: SCHEMA_V20_DOWN,
    },
    Migration {
        version: 21,
        up: "CREATE TABLE saved_loadouts (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), normalized_loadouts TEXT NOT NULL) STRICT;",
        down: "CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1)); INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM saved_loadouts) THEN 0 ELSE 1 END; DROP TABLE downgrade_guard; DROP TABLE saved_loadouts;",
    },
    Migration {
        version: 22,
        up: SCHEMA_V22_UP,
        down: "DROP TABLE definition_search;",
    },
    Migration {
        version: 23,
        up: SCHEMA_V23_UP,
        down: "CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1)); INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM builds) THEN 0 ELSE 1 END; DROP TABLE downgrade_guard; DROP TABLE builds;",
    },
    Migration {
        version: 24,
        up: SCHEMA_V24_UP,
        down: SCHEMA_V24_DOWN,
    },
    Migration {
        version: 25,
        up: SCHEMA_V25_UP,
        down: SCHEMA_V25_DOWN,
    },
    Migration {
        version: 26,
        up: SCHEMA_V26_UP,
        down: SCHEMA_V26_DOWN,
    },
    Migration {
        version: 27,
        up: SCHEMA_V27_UP,
        down: SCHEMA_V27_DOWN,
    },
    Migration {
        version: 28,
        up: SCHEMA_V28_UP,
        down: SCHEMA_V28_DOWN,
    },
    Migration {
        version: 29,
        up: SCHEMA_V29_UP,
        down: SCHEMA_V29_DOWN,
    },
    Migration {
        version: 30,
        up: SCHEMA_V30_UP,
        down: SCHEMA_V30_DOWN,
    },
    Migration {
        version: 31,
        up: SCHEMA_V31_UP,
        down: SCHEMA_V31_DOWN,
    },
    Migration {
        version: 32,
        up: SCHEMA_V32_UP,
        down: SCHEMA_V32_DOWN,
    },
];
const SCHEMA_V32_UP: &str = r#"
CREATE TABLE definition_material_inputs (
    definition_hash INTEGER PRIMARY KEY REFERENCES definitions(hash) ON DELETE CASCADE,
    requirement_hash INTEGER CHECK (requirement_hash BETWEEN 0 AND 4294967295),
    capacity_type INTEGER CHECK (capacity_type BETWEEN 0 AND 4294967295),
    availability INTEGER CHECK (availability BETWEEN 0 AND 4294967295),
    CHECK (requirement_hash IS NOT NULL OR capacity_type IS NOT NULL OR availability IS NOT NULL)
) STRICT;
CREATE TABLE material_requirement_sets (
    hash INTEGER PRIMARY KEY CHECK (hash BETWEEN 0 AND 4294967295),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
CREATE TABLE material_requirements (
    hash INTEGER NOT NULL REFERENCES material_requirement_sets(hash) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 255),
    item_hash INTEGER NOT NULL CHECK (item_hash BETWEEN 0 AND 4294967295),
    count INTEGER NOT NULL CHECK (count BETWEEN 0 AND 4294967295),
    PRIMARY KEY (hash, ordinal)
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V32_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM definition_material_inputs) OR EXISTS (SELECT 1 FROM material_requirement_sets) OR EXISTS (SELECT 1 FROM material_requirements) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE material_requirements;
DROP TABLE material_requirement_sets;
DROP TABLE definition_material_inputs;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V31_UP: &str = r#"
CREATE TABLE objective_styles (
    hash INTEGER PRIMARY KEY REFERENCES objective_definitions(hash) ON DELETE CASCADE,
    ui_style INTEGER NOT NULL CHECK (ui_style BETWEEN 0 AND 4294967295)
) STRICT;
CREATE TABLE item_objective_sockets (
    instance_id TEXT NOT NULL, socket_index INTEGER NOT NULL,
    PRIMARY KEY (instance_id, socket_index),
    FOREIGN KEY (instance_id, socket_index) REFERENCES item_sockets(instance_id, socket_index) ON DELETE CASCADE
) STRICT;
CREATE TABLE item_plug_objectives (
    instance_id TEXT NOT NULL, socket_index INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 127),
    objective_hash INTEGER NOT NULL CHECK (objective_hash BETWEEN 0 AND 4294967295),
    progress INTEGER CHECK (progress BETWEEN 0 AND 4294967295),
    PRIMARY KEY (instance_id, socket_index, ordinal),
    FOREIGN KEY (instance_id, socket_index) REFERENCES item_objective_sockets(instance_id, socket_index) ON DELETE CASCADE
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V31_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM objective_styles) OR EXISTS (SELECT 1 FROM item_objective_sockets) OR EXISTS (SELECT 1 FROM item_plug_objectives) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE item_plug_objectives;
DROP TABLE item_objective_sockets;
DROP TABLE objective_styles;
"#;
const SCHEMA_V30_UP: &str = r#"
CREATE TABLE item_reusable_sockets (
    instance_id TEXT NOT NULL, socket_index INTEGER NOT NULL,
    PRIMARY KEY (instance_id, socket_index),
    FOREIGN KEY (instance_id, socket_index) REFERENCES item_sockets(instance_id, socket_index) ON DELETE CASCADE
) STRICT;
CREATE TABLE item_reusable_plugs (
    instance_id TEXT NOT NULL, socket_index INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 65535),
    plug_hash INTEGER NOT NULL CHECK (plug_hash BETWEEN 0 AND 4294967295),
    can_insert INTEGER NOT NULL CHECK (can_insert IN (0,1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0,1)),
    PRIMARY KEY (instance_id, socket_index, ordinal),
    FOREIGN KEY (instance_id, socket_index) REFERENCES item_reusable_sockets(instance_id, socket_index) ON DELETE CASCADE
) STRICT;
ALTER TABLE catalog_set_perks ADD COLUMN name TEXT CHECK (length(name) <= 256);
ALTER TABLE catalog_set_perks ADD COLUMN icon TEXT CHECK (length(icon) <= 1024);
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V30_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM item_reusable_sockets) OR EXISTS (SELECT 1 FROM item_reusable_plugs) OR EXISTS (SELECT 1 FROM catalog_set_perks WHERE name IS NOT NULL OR icon IS NOT NULL) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE item_reusable_plugs;
DROP TABLE item_reusable_sockets;
ALTER TABLE catalog_set_perks DROP COLUMN icon;
ALTER TABLE catalog_set_perks DROP COLUMN name;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V29_UP: &str = r#"
CREATE TABLE socket_catalogs (
    kind INTEGER NOT NULL CHECK (kind IN (2,3,4)),
    hash INTEGER NOT NULL CHECK (hash BETWEEN 0 AND 4294967295),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128),
    category_hash INTEGER CHECK (category_hash BETWEEN 0 AND 4294967295),
    randomize INTEGER CHECK (randomize IN (0,1)),
    style INTEGER CHECK (style BETWEEN 0 AND 4294967295),
    name TEXT CHECK (length(name) <= 256),
    CHECK ((kind = 2 AND category_hash IS NOT NULL AND randomize IS NOT NULL AND style IS NULL AND name IS NULL)
        OR (kind = 3 AND category_hash IS NULL AND randomize IS NULL AND style IS NOT NULL AND name IS NOT NULL)
        OR (kind = 4 AND category_hash IS NULL AND randomize IS NULL AND style IS NULL AND name IS NULL)),
    PRIMARY KEY (kind, hash)
) STRICT;
CREATE TABLE catalog_plug_whitelist (
    kind INTEGER NOT NULL CHECK (kind = 2), hash INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 1023),
    category_hash INTEGER NOT NULL CHECK (category_hash BETWEEN 0 AND 4294967295),
    identifier TEXT NOT NULL CHECK (length(identifier) <= 256),
    PRIMARY KEY (hash, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES socket_catalogs(kind, hash) ON DELETE CASCADE
) STRICT;
CREATE TABLE catalog_plug_items (
    kind INTEGER NOT NULL CHECK (kind = 4), hash INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 65535),
    item_hash INTEGER NOT NULL CHECK (item_hash BETWEEN 0 AND 4294967295),
    can_roll INTEGER NOT NULL CHECK (can_roll IN (0,1)),
    PRIMARY KEY (hash, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES socket_catalogs(kind, hash) ON DELETE CASCADE
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V29_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM socket_catalogs) OR EXISTS (SELECT 1 FROM catalog_plug_whitelist) OR EXISTS (SELECT 1 FROM catalog_plug_items) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE catalog_plug_whitelist;
DROP TABLE catalog_plug_items;
DROP TABLE socket_catalogs;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

const SCHEMA_V28_UP: &str = r#"
CREATE TABLE armor_catalogs (
    kind INTEGER NOT NULL CHECK (kind IN (0,1)),
    hash INTEGER NOT NULL CHECK (hash BETWEEN 0 AND 4294967295),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128),
    name TEXT CHECK ((kind = 0 AND name IS NULL) OR (kind = 1 AND name IS NOT NULL AND length(name) BETWEEN 1 AND 256)),
    maximum_value INTEGER CHECK ((kind = 0 AND (maximum_value IS NULL OR maximum_value BETWEEN -2147483648 AND 2147483647)) OR (kind = 1 AND maximum_value IS NULL)),
    PRIMARY KEY (kind, hash)
) STRICT;
CREATE TABLE catalog_scaled_stats (
    kind INTEGER NOT NULL CHECK (kind = 0), hash INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 63),
    stat_hash INTEGER NOT NULL CHECK (stat_hash BETWEEN 0 AND 4294967295),
    maximum_value INTEGER NOT NULL CHECK (maximum_value BETWEEN -2147483648 AND 2147483647),
    numeric INTEGER NOT NULL CHECK (numeric IN (0,1)),
    PRIMARY KEY (hash, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES armor_catalogs(kind, hash) ON DELETE CASCADE
) STRICT;
CREATE TABLE catalog_interpolation (
    kind INTEGER NOT NULL CHECK (kind = 0), hash INTEGER NOT NULL, stat_ordinal INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 255),
    value INTEGER NOT NULL CHECK (value BETWEEN -2147483648 AND 2147483647),
    weight INTEGER NOT NULL CHECK (weight BETWEEN -2147483648 AND 2147483647),
    PRIMARY KEY (hash, stat_ordinal, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES armor_catalogs(kind, hash) ON DELETE CASCADE,
    FOREIGN KEY (hash, stat_ordinal) REFERENCES catalog_scaled_stats(hash, ordinal) ON DELETE CASCADE
) STRICT;
CREATE TABLE catalog_set_items (
    kind INTEGER NOT NULL CHECK (kind = 1), hash INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 1023),
    item_hash INTEGER NOT NULL CHECK (item_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (hash, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES armor_catalogs(kind, hash) ON DELETE CASCADE
) STRICT;
CREATE TABLE catalog_set_perks (
    kind INTEGER NOT NULL CHECK (kind = 1), hash INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 63),
    required_count INTEGER NOT NULL CHECK (required_count BETWEEN -2147483648 AND 2147483647),
    perk_hash INTEGER NOT NULL CHECK (perk_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (hash, ordinal),
    FOREIGN KEY (kind, hash) REFERENCES armor_catalogs(kind, hash) ON DELETE CASCADE
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V28_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM armor_catalogs) OR EXISTS (SELECT 1 FROM catalog_scaled_stats) OR EXISTS (SELECT 1 FROM catalog_interpolation) OR EXISTS (SELECT 1 FROM catalog_set_items) OR EXISTS (SELECT 1 FROM catalog_set_perks) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE catalog_interpolation;
DROP TABLE catalog_scaled_stats;
DROP TABLE catalog_set_items;
DROP TABLE catalog_set_perks;
DROP TABLE armor_catalogs;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

const SCHEMA_V27_UP: &str = r#"
CREATE TABLE definition_socket_entries (
    definition_hash INTEGER NOT NULL REFERENCES definitions(hash) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 31),
    socket_type_hash INTEGER NOT NULL CHECK (socket_type_hash BETWEEN 0 AND 4294967295),
    initial_hash INTEGER NOT NULL CHECK (initial_hash BETWEEN 0 AND 4294967295),
    reusable_set_hash INTEGER CHECK (reusable_set_hash BETWEEN 0 AND 4294967295),
    randomized_set_hash INTEGER CHECK (randomized_set_hash BETWEEN 0 AND 4294967295),
    plug_sources INTEGER CHECK (plug_sources BETWEEN 0 AND 4294967295),
    default_visible INTEGER CHECK (default_visible IN (0, 1)),
    PRIMARY KEY (definition_hash, ordinal)
) STRICT;
CREATE TABLE definition_socket_plugs (
    definition_hash INTEGER NOT NULL,
    socket_index INTEGER NOT NULL CHECK (socket_index BETWEEN 0 AND 31),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 1023),
    plug_hash INTEGER NOT NULL CHECK (plug_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (definition_hash, socket_index, ordinal),
    FOREIGN KEY (definition_hash, socket_index) REFERENCES definition_socket_entries(definition_hash, ordinal) ON DELETE CASCADE
) STRICT;
CREATE TABLE definition_socket_categories (
    definition_hash INTEGER NOT NULL REFERENCES definitions(hash) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 31),
    category_hash INTEGER NOT NULL CHECK (category_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (definition_hash, ordinal)
) STRICT;
CREATE TABLE definition_socket_indexes (
    definition_hash INTEGER NOT NULL,
    category_ordinal INTEGER NOT NULL CHECK (category_ordinal BETWEEN 0 AND 31),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 31),
    socket_index INTEGER NOT NULL CHECK (socket_index BETWEEN 0 AND 31),
    PRIMARY KEY (definition_hash, category_ordinal, ordinal),
    FOREIGN KEY (definition_hash, category_ordinal) REFERENCES definition_socket_categories(definition_hash, ordinal) ON DELETE CASCADE,
    FOREIGN KEY (definition_hash, socket_index) REFERENCES definition_socket_entries(definition_hash, ordinal) ON DELETE CASCADE
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V27_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM definition_socket_entries) OR EXISTS (SELECT 1 FROM definition_socket_plugs) OR EXISTS (SELECT 1 FROM definition_socket_categories) OR EXISTS (SELECT 1 FROM definition_socket_indexes) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE definition_socket_indexes;
DROP TABLE definition_socket_categories;
DROP TABLE definition_socket_plugs;
DROP TABLE definition_socket_entries;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

const SCHEMA_V26_UP: &str = r#"
CREATE TABLE definition_armor (
    definition_hash INTEGER PRIMARY KEY REFERENCES definitions(hash) ON DELETE CASCADE,
    stat_group_hash INTEGER CHECK (stat_group_hash BETWEEN 0 AND 4294967295),
    plug_category_hash INTEGER CHECK (plug_category_hash BETWEEN 0 AND 4294967295),
    aspect_capacity INTEGER CHECK (aspect_capacity BETWEEN -2147483648 AND 2147483647),
    equipable_item_set_hash INTEGER CHECK (equipable_item_set_hash BETWEEN 0 AND 4294967295)
) STRICT;
CREATE TABLE definition_investment_stats (
    definition_hash INTEGER NOT NULL REFERENCES definitions(hash) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 63),
    stat_type_hash INTEGER NOT NULL CHECK (stat_type_hash BETWEEN 0 AND 4294967295),
    value INTEGER NOT NULL CHECK (value BETWEEN -2147483648 AND 2147483647),
    conditionally_active INTEGER NOT NULL CHECK (conditionally_active IN (0, 1)),
    PRIMARY KEY (definition_hash, ordinal)
) STRICT;
CREATE TABLE definition_energy_costs (
    definition_hash INTEGER PRIMARY KEY REFERENCES definitions(hash) ON DELETE CASCADE,
    energy_cost INTEGER NOT NULL CHECK (energy_cost BETWEEN -2147483648 AND 2147483647),
    energy_type INTEGER NOT NULL CHECK (energy_type BETWEEN 0 AND 4294967295),
    energy_type_hash INTEGER NOT NULL CHECK (energy_type_hash BETWEEN 0 AND 4294967295)
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V26_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM definition_armor) OR EXISTS (SELECT 1 FROM definition_investment_stats) OR EXISTS (SELECT 1 FROM definition_energy_costs) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE definition_energy_costs;
DROP TABLE definition_investment_stats;
DROP TABLE definition_armor;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

const SCHEMA_V25_UP: &str = r#"
CREATE TABLE item_energy (
    instance_id TEXT PRIMARY KEY REFERENCES items(instance_id) ON DELETE CASCADE,
    energy_capacity INTEGER NOT NULL CHECK (energy_capacity BETWEEN 0 AND 4294967295),
    energy_used INTEGER NOT NULL CHECK (energy_used BETWEEN 0 AND 4294967295),
    energy_unused INTEGER NOT NULL CHECK (energy_unused BETWEEN 0 AND 4294967295),
    energy_type INTEGER CHECK (energy_type BETWEEN 0 AND 4294967295),
    energy_type_hash INTEGER CHECK (energy_type_hash BETWEEN 0 AND 4294967295)
) STRICT;
CREATE TABLE item_gear_tiers (
    instance_id TEXT PRIMARY KEY REFERENCES items(instance_id) ON DELETE CASCADE,
    gear_tier INTEGER NOT NULL CHECK (gear_tier BETWEEN 0 AND 4294967295)
) STRICT;
CREATE TABLE item_sockets (
    instance_id TEXT NOT NULL REFERENCES items(instance_id) ON DELETE CASCADE,
    socket_index INTEGER NOT NULL CHECK (socket_index BETWEEN 0 AND 31),
    plug_hash INTEGER CHECK (plug_hash BETWEEN 0 AND 4294967295),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    visible INTEGER NOT NULL CHECK (visible IN (0, 1)),
    PRIMARY KEY(instance_id, socket_index)
) STRICT;
"#;
const SCHEMA_V25_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM item_energy) OR EXISTS (SELECT 1 FROM item_gear_tiers) OR EXISTS (SELECT 1 FROM item_sockets) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
DROP TABLE item_sockets;
DROP TABLE item_gear_tiers;
DROP TABLE item_energy;
"#;

const SCHEMA_V24_UP: &str = r#"
ALTER TABLE definitions ADD COLUMN non_transferrable INTEGER CHECK (non_transferrable IS NULL OR non_transferrable IN (0, 1));
ALTER TABLE items ADD COLUMN transfer_status INTEGER CHECK (transfer_status IS NULL OR transfer_status BETWEEN 0 AND 4294967295);
"#;
const SCHEMA_V24_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (SELECT 1 FROM definitions WHERE non_transferrable IS NOT NULL) OR EXISTS (SELECT 1 FROM items WHERE transfer_status IS NOT NULL) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
ALTER TABLE definitions DROP COLUMN non_transferrable;
ALTER TABLE items DROP COLUMN transfer_status;
"#;

// Local user-authored definitions survive replacing the active synced profile.
// Membership is explicit, like active_profile, but not a cascading foreign key.
const SCHEMA_V23_UP: &str = r#"
CREATE TABLE builds (
    membership_id TEXT NOT NULL CHECK (length(membership_id) BETWEEN 1 AND 128 AND membership_id NOT GLOB '*[^0-9]*'),
    membership_type INTEGER NOT NULL CHECK (membership_type BETWEEN 0 AND 4294967295),
    id TEXT NOT NULL CHECK (length(hex(id)) BETWEEN 2 AND 256),
    name TEXT NOT NULL,
    class_type INTEGER NOT NULL CHECK (class_type BETWEEN 0 AND 3),
    source TEXT NOT NULL CHECK (source IN ('import','equipped','ingame')),
    created_at TEXT NOT NULL CHECK (
        length(created_at) BETWEEN 1 AND 20 AND created_at NOT GLOB '*[^0-9]*'
        AND (created_at = '0' OR created_at GLOB '[1-9]*')
        AND (length(created_at) < 20 OR created_at <= '18446744073709551615')
    ),
    updated_at TEXT NOT NULL CHECK (
        length(updated_at) BETWEEN 1 AND 20 AND updated_at NOT GLOB '*[^0-9]*'
        AND (updated_at = '0' OR updated_at GLOB '[1-9]*')
        AND (length(updated_at) < 20 OR updated_at <= '18446744073709551615')
    ),
    normalized_build TEXT NOT NULL,
    parameter_extra TEXT,
    PRIMARY KEY (membership_type, membership_id, id)
) STRICT;
CREATE INDEX builds_name ON builds(membership_type, membership_id, name);
CREATE INDEX builds_class ON builds(membership_type, membership_id, class_type);
"#;

const SCHEMA_V22_UP: &str = r#"
CREATE TABLE definition_search (
    hash INTEGER PRIMARY KEY REFERENCES definitions(hash) ON DELETE CASCADE,
    name TEXT NOT NULL,
    name_norm TEXT NOT NULL,
    item_type_norm TEXT,
    tier_norm TEXT,
    tags_norm TEXT NOT NULL,
    element TEXT,
    slot TEXT,
    class_type TEXT,
    perk INTEGER NOT NULL CHECK (perk IN (0, 1)),
    mod INTEGER NOT NULL CHECK (mod IN (0, 1)),
    archetype INTEGER NOT NULL CHECK (archetype IN (0, 1))
) STRICT;
"#;

// Canonical signed Unix seconds and nanoseconds, encoded by the inventory adapter.
// Additive migration leaves all legacy rows unknown; only profile rows may carry it.
const SCHEMA_V20_UP: &str = r#"
ALTER TABLE sync_metadata ADD COLUMN profile_minted_at TEXT
    CHECK (profile_minted_at IS NULL OR
        (dataset = 'profile_inventory' AND typeof(profile_minted_at) = 'text'));
"#;

const SCHEMA_V20_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (
    SELECT 1 FROM sync_metadata WHERE profile_minted_at IS NOT NULL
) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
ALTER TABLE sync_metadata DROP COLUMN profile_minted_at;
"#;

const SCHEMA_V19_UP: &str = r#"
ALTER TABLE sync_metadata ADD COLUMN invalidated INTEGER NOT NULL DEFAULT 0
    CHECK (typeof(invalidated) = 'integer' AND invalidated IN (0, 1));
"#;

const SCHEMA_V19_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (
    SELECT 1 FROM sync_metadata WHERE invalidated = 1
) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
ALTER TABLE sync_metadata DROP COLUMN invalidated;
"#;

// Additive only: no catalog or inventory table is rebuilt, and legacy rows
// remain unknown. STRICT affinity permits lossless integer coercion only.
const SCHEMA_V18_UP: &str = r#"
ALTER TABLE definitions ADD COLUMN does_postmaster_pull_have_side_effects INTEGER
    CHECK (does_postmaster_pull_have_side_effects IS NULL OR
        (typeof(does_postmaster_pull_have_side_effects) = 'integer' AND does_postmaster_pull_have_side_effects IN (0, 1)));
ALTER TABLE definitions ADD COLUMN is_instance_item INTEGER
    CHECK (is_instance_item IS NULL OR (typeof(is_instance_item) = 'integer' AND is_instance_item IN (0, 1)));
ALTER TABLE definitions ADD COLUMN max_stack_size INTEGER
    CHECK (max_stack_size IS NULL OR (typeof(max_stack_size) = 'integer' AND max_stack_size BETWEEN -2147483648 AND 2147483647));
ALTER TABLE definitions ADD COLUMN stack_unique_label TEXT
    CHECK (stack_unique_label IS NULL OR typeof(stack_unique_label) = 'text');
"#;

// Presence, not truthiness, forbids data loss. DROP COLUMN is supported by
// the project's bundled SQLite; the step transaction protects the guard and DDL.
const SCHEMA_V18_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN EXISTS (
    SELECT 1 FROM definitions WHERE does_postmaster_pull_have_side_effects IS NOT NULL
        OR is_instance_item IS NOT NULL OR max_stack_size IS NOT NULL OR stack_unique_label IS NOT NULL
) THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
ALTER TABLE definitions DROP COLUMN does_postmaster_pull_have_side_effects;
ALTER TABLE definitions DROP COLUMN is_instance_item;
ALTER TABLE definitions DROP COLUMN max_stack_size;
ALTER TABLE definitions DROP COLUMN stack_unique_label;
"#;

// Rebuild only stacks: ordinals, multiplicity and owner foreign keys survive.
// Missing coverage is deliberately NotObserved for migrated legacy snapshots.
const SCHEMA_V17_UP: &str = r#"
CREATE TEMP TABLE saved_stacks AS SELECT * FROM stacks;
DROP TABLE stacks;
CREATE TABLE stacks (
    ordinal INTEGER NOT NULL PRIMARY KEY CHECK (typeof(ordinal) = 'integer' AND ordinal >= 0),
    container TEXT NOT NULL CHECK (container IN ('profile_inventory', 'profile_currencies', 'character_inventory', 'postmaster')),
    owner TEXT REFERENCES characters(character_id) ON DELETE RESTRICT,
    bucket_hash INTEGER NOT NULL CHECK (typeof(bucket_hash) = 'integer' AND bucket_hash BETWEEN 0 AND 4294967295),
    item_hash INTEGER NOT NULL CHECK (typeof(item_hash) = 'integer' AND item_hash BETWEEN 0 AND 4294967295),
    quantity INTEGER NOT NULL CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 0 AND 2147483647 AND (quantity > 0 OR container = 'profile_currencies')),
    CHECK (
        (container IN ('profile_inventory', 'profile_currencies') AND owner IS NULL)
        OR (container IN ('character_inventory', 'postmaster') AND owner IS NOT NULL)
    )
) STRICT;
INSERT INTO stacks SELECT * FROM saved_stacks;
DROP TABLE saved_stacks;
CREATE TABLE inventory_coverage (
    id INTEGER NOT NULL PRIMARY KEY CHECK (id = 1),
    profile_currencies TEXT NOT NULL CHECK (profile_currencies IN ('not_observed', 'unavailable', 'available'))
) STRICT;
"#;

// Guard before permanent mutations; the step transaction also protects against
// constraint failures while restoring representable rows. Known qualification
// cannot be silently discarded, even when there are no currency rows.
const SCHEMA_V17_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN
    EXISTS (SELECT 1 FROM stacks WHERE quantity = 0)
    OR EXISTS (SELECT 1 FROM inventory_coverage WHERE id != 1 OR profile_currencies != 'not_observed')
    THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
CREATE TEMP TABLE saved_stacks AS SELECT * FROM stacks;
DROP TABLE stacks;
CREATE TABLE stacks (
    ordinal INTEGER NOT NULL PRIMARY KEY CHECK (typeof(ordinal) = 'integer' AND ordinal >= 0),
    container TEXT NOT NULL CHECK (container IN ('profile_inventory', 'profile_currencies', 'character_inventory', 'postmaster')),
    owner TEXT REFERENCES characters(character_id) ON DELETE RESTRICT,
    bucket_hash INTEGER NOT NULL CHECK (typeof(bucket_hash) = 'integer' AND bucket_hash BETWEEN 0 AND 4294967295),
    item_hash INTEGER NOT NULL CHECK (typeof(item_hash) = 'integer' AND item_hash BETWEEN 0 AND 4294967295),
    quantity INTEGER NOT NULL CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 1 AND 2147483647),
    CHECK (
        (container IN ('profile_inventory', 'profile_currencies') AND owner IS NULL)
        OR (container IN ('character_inventory', 'postmaster') AND owner IS NOT NULL)
    )
) STRICT;
INSERT INTO stacks SELECT * FROM saved_stacks;
DROP TABLE saved_stacks;
DROP TABLE inventory_coverage;
"#;

// Foreign keys stay enabled. Back up every cascading child before dropping
// items, and restore against the same parent name (never rename the old parent).
// All temporary tables and changes belong to the runner's step transaction.
const SCHEMA_V16_UP: &str = r#"
CREATE TEMP TABLE saved_items AS SELECT * FROM items;
CREATE TEMP TABLE saved_perks AS SELECT * FROM item_perks;
CREATE TEMP TABLE saved_plugs AS SELECT * FROM item_plugs;
CREATE TEMP TABLE saved_stats AS SELECT * FROM item_stats;
CREATE TEMP TABLE saved_available AS SELECT * FROM item_available_plugs;
DROP TABLE items;
CREATE TABLE items (
    instance_id TEXT NOT NULL PRIMARY KEY CHECK (length(instance_id) BETWEEN 1 AND 128),
    item_hash INTEGER NOT NULL CHECK (typeof(item_hash) = 'integer'),
    location_kind TEXT NOT NULL CHECK (location_kind IN ('vault', 'character', 'postmaster', 'other')),
    location_character_id TEXT REFERENCES characters(character_id) ON DELETE RESTRICT,
    location_other TEXT CHECK (location_other IS NULL OR length(location_other) BETWEEN 1 AND 256),
    quantity INTEGER NOT NULL CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 0 AND 4294967295),
    equipped INTEGER NOT NULL CHECK (typeof(equipped) = 'integer' AND equipped IN (0, 1)),
    locked INTEGER NOT NULL CHECK (typeof(locked) = 'integer' AND locked IN (0, 1)),
    power INTEGER CHECK (power IS NULL OR (typeof(power) = 'integer' AND power BETWEEN 0 AND 65535)),
    CHECK (
        (location_kind = 'vault' AND location_character_id IS NULL AND location_other IS NULL)
        OR (location_kind IN ('character', 'postmaster') AND location_character_id IS NOT NULL AND location_other IS NULL)
        OR (location_kind = 'other' AND location_character_id IS NULL AND location_other IS NOT NULL)
    ),
    CHECK (location_kind != 'postmaster' OR equipped = 0)
) STRICT;
CREATE INDEX items_definition_hash_idx ON items(item_hash);
CREATE INDEX items_location_idx ON items(location_kind, location_character_id);
INSERT INTO items SELECT * FROM saved_items;
INSERT INTO item_perks SELECT * FROM saved_perks;
INSERT INTO item_plugs SELECT * FROM saved_plugs;
INSERT INTO item_stats SELECT * FROM saved_stats;
INSERT INTO item_available_plugs SELECT * FROM saved_available;
DROP TABLE saved_items;
DROP TABLE saved_perks;
DROP TABLE saved_plugs;
DROP TABLE saved_stats;
DROP TABLE saved_available;
CREATE TABLE stacks (
    ordinal INTEGER NOT NULL PRIMARY KEY CHECK (typeof(ordinal) = 'integer' AND ordinal >= 0),
    container TEXT NOT NULL CHECK (container IN ('profile_inventory', 'profile_currencies', 'character_inventory', 'postmaster')),
    owner TEXT REFERENCES characters(character_id) ON DELETE RESTRICT,
    bucket_hash INTEGER NOT NULL CHECK (typeof(bucket_hash) = 'integer' AND bucket_hash BETWEEN 0 AND 4294967295),
    item_hash INTEGER NOT NULL CHECK (typeof(item_hash) = 'integer' AND item_hash BETWEEN 0 AND 4294967295),
    quantity INTEGER NOT NULL CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 1 AND 2147483647),
    CHECK (
        (container IN ('profile_inventory', 'profile_currencies') AND owner IS NULL)
        OR (container IN ('character_inventory', 'postmaster') AND owner IS NOT NULL)
    )
) STRICT;
"#;

// The guard fails before changing any permanent table. Rollback preserves both
// schema and data for observations that v15 cannot represent.
const SCHEMA_V16_DOWN: &str = r#"
CREATE TEMP TABLE downgrade_guard (safe INTEGER CHECK (safe = 1));
INSERT INTO downgrade_guard SELECT CASE WHEN
    EXISTS (SELECT 1 FROM stacks) OR EXISTS (SELECT 1 FROM items WHERE location_kind = 'postmaster')
    THEN 0 ELSE 1 END;
DROP TABLE downgrade_guard;
CREATE TEMP TABLE saved_items AS SELECT * FROM items;
CREATE TEMP TABLE saved_perks AS SELECT * FROM item_perks;
CREATE TEMP TABLE saved_plugs AS SELECT * FROM item_plugs;
CREATE TEMP TABLE saved_stats AS SELECT * FROM item_stats;
CREATE TEMP TABLE saved_available AS SELECT * FROM item_available_plugs;
DROP TABLE items;
CREATE TABLE items (
    instance_id TEXT NOT NULL PRIMARY KEY CHECK (length(instance_id) BETWEEN 1 AND 128),
    item_hash INTEGER NOT NULL CHECK (typeof(item_hash) = 'integer'),
    location_kind TEXT NOT NULL CHECK (location_kind IN ('vault', 'character', 'other')),
    location_character_id TEXT REFERENCES characters(character_id) ON DELETE RESTRICT,
    location_other TEXT CHECK (location_other IS NULL OR length(location_other) BETWEEN 1 AND 256),
    quantity INTEGER NOT NULL CHECK (typeof(quantity) = 'integer' AND quantity BETWEEN 0 AND 4294967295),
    equipped INTEGER NOT NULL CHECK (typeof(equipped) = 'integer' AND equipped IN (0, 1)),
    locked INTEGER NOT NULL CHECK (typeof(locked) = 'integer' AND locked IN (0, 1)),
    power INTEGER CHECK (power IS NULL OR (typeof(power) = 'integer' AND power BETWEEN 0 AND 65535)),
    CHECK (
        (location_kind = 'vault' AND location_character_id IS NULL AND location_other IS NULL)
        OR (location_kind = 'character' AND location_character_id IS NOT NULL AND location_other IS NULL)
        OR (location_kind = 'other' AND location_character_id IS NULL AND location_other IS NOT NULL)
    )
) STRICT;
CREATE INDEX items_definition_hash_idx ON items(item_hash);
CREATE INDEX items_location_idx ON items(location_kind, location_character_id);
INSERT INTO items SELECT * FROM saved_items;
INSERT INTO item_perks SELECT * FROM saved_perks;
INSERT INTO item_plugs SELECT * FROM saved_plugs;
INSERT INTO item_stats SELECT * FROM saved_stats;
INSERT INTO item_available_plugs SELECT * FROM saved_available;
DROP TABLE saved_items;
DROP TABLE saved_perks;
DROP TABLE saved_plugs;
DROP TABLE saved_stats;
DROP TABLE saved_available;
DROP TABLE stacks;
"#;

const SCHEMA_V15_UP: &str = r#"
CREATE TABLE clarity_insights (
    hash INTEGER PRIMARY KEY CHECK (hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    item_hash INTEGER CHECK (item_hash BETWEEN 0 AND 4294967295),
    item_name TEXT,
    text TEXT NOT NULL
) STRICT;
CREATE INDEX clarity_insights_name ON clarity_insights(name);
CREATE TABLE clarity_metadata (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version TEXT NOT NULL,
    checked_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
"#;
const SCHEMA_V15_DOWN: &str = r#"
DROP TABLE clarity_metadata;
DROP TABLE clarity_insights;
"#;

// Existing caches must republish even when the upstream version is unchanged.
const SCHEMA_V14_UP: &str = r#"
CREATE TABLE item_descriptions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    description TEXT NOT NULL CHECK (length(description) BETWEEN 1 AND 2048)
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V14_DOWN: &str = r#"
DROP TABLE IF EXISTS item_descriptions;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

// Legacy characters retain NULL until the next component-200 sync.
const SCHEMA_V13_UP: &str = r#"
ALTER TABLE characters ADD COLUMN date_last_played TEXT CHECK (
    date_last_played IS NULL OR (
        length(date_last_played) BETWEEN 1 AND 20
        AND date_last_played NOT GLOB '*[^0-9]*'
        AND (date_last_played = '0' OR date_last_played GLOB '[1-9]*')
        AND (length(date_last_played) < 20 OR date_last_played <= '18446744073709551615')
    )
);
"#;
const SCHEMA_V13_DOWN: &str = "ALTER TABLE characters DROP COLUMN date_last_played;";

const SCHEMA_V12_UP: &str = r#"
CREATE TABLE subclass_plug_options (
    scope TEXT NOT NULL CHECK (
        scope = 'profile' OR (
            length(scope) BETWEEN 1 AND 128
            AND scope NOT GLOB '*[^0-9]*'
            AND (scope = '0' OR scope GLOB '[1-9]*')
        )
    ),
    plug_hash INTEGER NOT NULL CHECK (plug_hash BETWEEN 0 AND 4294967295),
    can_insert INTEGER NOT NULL CHECK (can_insert IN (0, 1)),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    PRIMARY KEY (scope, plug_hash)
) STRICT;
"#;
const SCHEMA_V12_DOWN: &str = "DROP TABLE IF EXISTS subclass_plug_options;";

// Republish even an unchanged Manifest version to populate loadout names.
const SCHEMA_V11_UP: &str = r#"
CREATE TABLE loadout_name_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V11_DOWN: &str = r#"
DROP TABLE IF EXISTS loadout_name_definitions;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;

const SCHEMA_V10_UP: &str = r#"
CREATE TABLE item_available_plugs (
    instance_id TEXT NOT NULL REFERENCES items(instance_id) ON DELETE CASCADE,
    socket_index INTEGER NOT NULL CHECK (socket_index BETWEEN 0 AND 255),
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 63),
    plug_hash INTEGER NOT NULL CHECK (plug_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (instance_id, socket_index, ordinal),
    UNIQUE (instance_id, socket_index, plug_hash)
) STRICT;
"#;
const SCHEMA_V10_DOWN: &str = "DROP TABLE IF EXISTS item_available_plugs;";

const SCHEMA_V9_UP: &str = r#"
CREATE TABLE item_stats (
    instance_id TEXT NOT NULL REFERENCES items(instance_id) ON DELETE CASCADE,
    stat_hash INTEGER NOT NULL CHECK (stat_hash BETWEEN 0 AND 4294967295),
    value INTEGER NOT NULL CHECK (value BETWEEN -2147483648 AND 2147483647),
    PRIMARY KEY (instance_id, stat_hash)
) STRICT;
"#;
const SCHEMA_V9_DOWN: &str = "DROP TABLE IF EXISTS item_stats;";

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

const SCHEMA_V4_UP: &str = r#"
CREATE TABLE objective_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    progress_description TEXT NOT NULL CHECK (length(progress_description) <= 512),
    completion_value INTEGER NOT NULL CHECK (typeof(completion_value) = 'integer' AND completion_value BETWEEN -2147483648 AND 2147483647),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
CREATE TABLE activity_type_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    description TEXT NOT NULL CHECK (length(description) <= 2048),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
"#;
const SCHEMA_V4_DOWN: &str = r#"
DROP TABLE IF EXISTS objective_definitions;
DROP TABLE IF EXISTS activity_type_definitions;
"#;

// Keep the legacy element presentation column; these nullable attributes use
// stable lowercase names with closed value sets. Existing rows remain NULL.
// Invalidate only Manifest freshness: otherwise the version-aware publisher
// would skip downloading the same live version and never populate the fields.
const SCHEMA_V5_UP: &str = r#"
ALTER TABLE definitions ADD COLUMN damage_type TEXT
    CHECK (damage_type IS NULL OR damage_type IN ('kinetic','arc','solar','void','stasis','strand'));
ALTER TABLE definitions ADD COLUMN slot TEXT
    CHECK (slot IS NULL OR slot IN ('kinetic','energy','power','helmet','gauntlets','chest','legs','classitem','other','unknown'));
ALTER TABLE definitions ADD COLUMN class_type TEXT
    CHECK (class_type IS NULL OR class_type IN ('titan','hunter','warlock','any','unknown'));
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
// Freshness cannot be restored on downgrade; catalog and profile rows survive.
const SCHEMA_V5_DOWN: &str = r#"
ALTER TABLE definitions DROP COLUMN class_type;
ALTER TABLE definitions DROP COLUMN slot;
ALTER TABLE definitions DROP COLUMN damage_type;
"#;

// As in v5, invalidate Manifest freshness so an unchanged live version is
// downloaded again. Profile metadata and existing definition rows survive.
const SCHEMA_V6_UP: &str = r#"
ALTER TABLE definitions ADD COLUMN plug_category TEXT
    CHECK (plug_category IS NULL OR length(plug_category) BETWEEN 1 AND 256);
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V6_DOWN: &str = r#"
ALTER TABLE definitions DROP COLUMN plug_category;
"#;

/// Ordered enabled socket plugs; the ordinal bounds the list at 32 entries.
const SCHEMA_V7_UP: &str = r#"
CREATE TABLE item_plugs (
    instance_id TEXT NOT NULL
        REFERENCES items(instance_id) ON DELETE CASCADE
        CHECK (length(instance_id) BETWEEN 1 AND 128),
    ordinal INTEGER NOT NULL
        CHECK (typeof(ordinal) = 'integer' AND ordinal BETWEEN 0 AND 31),
    plug_hash INTEGER NOT NULL
        CHECK (typeof(plug_hash) = 'integer' AND plug_hash BETWEEN 0 AND 4294967295),
    PRIMARY KEY (instance_id, ordinal)
) STRICT;
"#;
const SCHEMA_V7_DOWN: &str = r#"
DROP TABLE IF EXISTS item_plugs;
"#;

// A same-version Manifest must be republished to populate the new stat catalog.
const SCHEMA_V8_UP: &str = r#"
CREATE TABLE stat_definitions (
    hash INTEGER PRIMARY KEY CHECK (typeof(hash) = 'integer' AND hash BETWEEN 0 AND 4294967295),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 256),
    definition_version TEXT NOT NULL CHECK (length(definition_version) BETWEEN 1 AND 128)
) STRICT;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
"#;
const SCHEMA_V8_DOWN: &str = r#"
DROP TABLE IF EXISTS stat_definitions;
DELETE FROM sync_metadata WHERE dataset = 'manifest_definitions';
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
    if forward && version == 22 {
        super::definitions::backfill_search(&transaction)?;
    }
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

#[cfg(test)]
mod filter_migration_tests {
    use super::*;

    #[test]
    fn plug_category_v6_up_down_invalidates_only_manifest_freshness() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate_to(&mut connection, 5).unwrap();
        connection.execute_batch("INSERT INTO definitions(hash,name,definition_version) VALUES(1,'Test','v1'); INSERT INTO sync_metadata VALUES('manifest_definitions','1','manifest','8','v1'),('profile_inventory','1','profile','2','v1');").unwrap();
        migrate_to(&mut connection, 6).unwrap();
        assert_eq!(read_user_version(&connection).unwrap(), 6);
        assert_eq!(
            connection
                .query_row("SELECT plug_category FROM definitions", [], |row| row
                    .get::<_, Option<String>>(0))
                .unwrap(),
            None
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sync_metadata WHERE dataset='manifest_definitions'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT revision FROM sync_metadata WHERE dataset='profile_inventory'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "2"
        );
        for invalid in ["".to_owned(), "x".repeat(257)] {
            assert!(
                connection
                    .execute("UPDATE definitions SET plug_category=?1", [invalid])
                    .is_err()
            );
        }
        connection
            .execute("UPDATE definitions SET plug_category='frames'", [])
            .unwrap();
        migrate_to(&mut connection, 5).unwrap();
        assert!(
            connection
                .prepare("SELECT plug_category FROM definitions")
                .is_err()
        );
        assert_eq!(
            connection
                .query_row("SELECT name FROM definitions", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "Test"
        );
        migrate_to(&mut connection, 6).unwrap();
    }

    #[test]
    fn search_filter_v5_up_down_invalidates_only_manifest_freshness() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate_to(&mut connection, 4).unwrap();
        connection.execute_batch("INSERT INTO definitions(hash,name,definition_version) VALUES(1,'Test','v1'); INSERT INTO sync_metadata VALUES('manifest_definitions','1','manifest','8','v1'),('profile_inventory','1','profile','2','v1');").unwrap();
        migrate_to(&mut connection, 5).unwrap();
        assert_eq!(read_user_version(&connection).unwrap(), 5);
        let attributes: (Option<String>, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT damage_type, slot, class_type FROM definitions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(attributes, (None, None, None));
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sync_metadata WHERE dataset='manifest_definitions'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT revision FROM sync_metadata WHERE dataset='profile_inventory'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "2"
        );
        for column in ["damage_type", "slot", "class_type"] {
            assert!(
                connection
                    .execute(&format!("UPDATE definitions SET {column}='invalid'"), [])
                    .is_err()
            );
        }
        migrate_to(&mut connection, 4).unwrap();
        assert!(connection.prepare("SELECT slot FROM definitions").is_err());
        assert_eq!(
            connection
                .query_row("SELECT name FROM definitions", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "Test"
        );
        migrate_to(&mut connection, 5).unwrap();
        assert_eq!(read_user_version(&connection).unwrap(), 5);
    }
}
