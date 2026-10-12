//! Synchronous, application-owned SQLite projection store.
//!
//! The store owns exactly one [`rusqlite::Connection`]; it never discovers a
//! database path, starts an async runtime, opens a pool, or reaches the network.
//! Callers own the path and the process lifetime, and later repository modules in
//! this module tree share the connection through crate-private accessors.
//!
//! Every connection is configured before migrations run:
//!
//! - foreign key enforcement is enabled, so the declared constraints are real;
//! - the lock wait is bounded by [`BUSY_TIMEOUT_MILLIS`];
//! - `trusted_schema` is disabled, so a hostile schema cannot execute
//!   application-defined functions while it is parsed;
//! - file-backed databases use write-ahead logging with `synchronous = NORMAL`,
//!   while in-memory databases keep the default memory journal, because WAL is
//!   not meaningful for a database that never touches disk.
//!
//! Failure presentation is sanitized: every fallible public operation returns
//! [`AppError`], and no SQL text, filesystem path, stored value, SQLite message,
//! or provider value can escape through the error or its formatting.
//!
//! Opening a database migrates it to [`LATEST_SCHEMA_VERSION`]. A database whose
//! `PRAGMA user_version` is newer than this build fails closed instead of
//! degrading silently. Repository modules in this tree share the connection and
//! own their own transactions; the Manifest definition repository, the
//! single-active profile inventory repository, and the durable definition/instance
//! search repository are the three of them.

mod armor_catalogs;
mod armor_definitions;
mod builds;
mod clarity;
mod definition_sockets;
mod definitions;
mod inventory;
mod loadouts;
mod material_catalogs;
mod migrations;
mod search;
mod socket_catalogs;
mod world;

#[cfg(test)]
mod definition_tests;
#[cfg(test)]
mod inventory_effects_tests;
#[cfg(test)]
mod inventory_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod verification_tests;
#[cfg(test)]
mod world_tests;

use destiny_core::{AppError, ErrorCategory};
use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

/// Highest SQLite schema version this build can create or migrate to.
pub const LATEST_SCHEMA_VERSION: u32 = migrations::LATEST_VERSION;

/// Bounded lock wait applied to every connection, in milliseconds.
pub const BUSY_TIMEOUT_MILLIS: u32 = 5_000;

/// Journal policy applied to a connection after it opens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JournalPolicy {
    /// File-backed databases use write-ahead logging when the engine accepts it.
    WriteAheadLog,
    /// In-memory databases keep the default memory journal.
    Memory,
}

/// One owned SQLite connection with its applied policy and migrated schema.
#[derive(Debug)]
pub struct SqliteStore {
    connection: Connection,
}

impl SqliteStore {
    /// Opens a file-backed store, applies the connection policy, and migrates to
    /// [`LATEST_SCHEMA_VERSION`]. Missing parent directories are created with
    /// owner-only access on Unix; permissions of existing directories stay intact.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(parent).map_err(|_| storage_failure())?;
        }
        let connection = Connection::open(path).map_err(|_| storage_failure())?;
        Self::initialize(connection, JournalPolicy::WriteAheadLog)
    }

    /// Opens a private in-memory store for tests and ephemeral work.
    pub fn open_in_memory() -> Result<Self, AppError> {
        let connection = Connection::open_in_memory().map_err(|_| storage_failure())?;
        Self::initialize(connection, JournalPolicy::Memory)
    }

    fn initialize(connection: Connection, journal: JournalPolicy) -> Result<Self, AppError> {
        let mut store = Self { connection };
        store.apply_connection_policy(journal)?;
        store.migrate_to(LATEST_SCHEMA_VERSION)?;
        Ok(store)
    }

    /// The schema version currently recorded by `PRAGMA user_version`.
    pub fn schema_version(&self) -> Result<u32, AppError> {
        migrations::read_user_version(self.connection())
    }

    /// Migrates to an explicit version in `0..=LATEST_SCHEMA_VERSION`.
    ///
    /// Every step commits its schema statements and its `user_version` update in
    /// one transaction, so a failing step leaves neither behind. Targets above
    /// [`LATEST_SCHEMA_VERSION`] are refused, and a stored version above this
    /// build's latest fails closed. The downward direction exists so downgrades
    /// stay explicit and testable; it is not a general recovery mechanism.
    pub fn migrate_to(&mut self, target_version: u32) -> Result<(), AppError> {
        migrations::migrate_to(self.connection_mut(), target_version)
    }

    /// Read-only connection access for sibling `sqlite` modules and tests.
    pub(crate) fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Mutable connection access for sibling `sqlite` repository modules that
    /// need an explicit transaction. It is deliberately not public.
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }

    /// Applies the mandatory per-connection policy before any migration runs.
    fn apply_connection_policy(&self, journal: JournalPolicy) -> Result<(), AppError> {
        self.connection
            .pragma_update(None, "foreign_keys", 1_i32)
            .map_err(|_| storage_failure())?;
        self.connection
            .busy_timeout(Duration::from_millis(u64::from(BUSY_TIMEOUT_MILLIS)))
            .map_err(|_| storage_failure())?;
        self.connection
            .pragma_update(None, "trusted_schema", 0_i32)
            .map_err(|_| storage_failure())?;
        if journal == JournalPolicy::WriteAheadLog {
            let mode: String = self
                .connection
                .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
                .map_err(|_| storage_failure())?;
            if !mode.eq_ignore_ascii_case("wal") {
                return Err(storage_failure());
            }
            self.connection
                .pragma_update(None, "synchronous", "NORMAL")
                .map_err(|_| storage_failure())?;
        }
        Ok(())
    }
}

/// The single sanitized category every SQLite failure maps to.
pub(crate) fn storage_failure() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// Encodes a `u64` as the canonical, unpadded decimal text the schema stores.
///
/// Canonical text is the only representation the canonical decimal `CHECK`
/// constraints accept and the only one [`parse_decimal`] round-trips, so this is
/// the single encoding used for stored `u64` values such as revisions and
/// freshness timestamps.
pub(crate) fn encode_decimal(value: u64) -> String {
    value.to_string()
}

/// Parses canonical unsigned decimal text into a `u64`, range-checked.
///
/// Empty, non-digit, leading-zero, and out-of-range text is rejected as a
/// sanitized local-storage failure, so a malformed stored value can never be
/// reinterpreted as a valid revision or timestamp.
pub(crate) fn parse_decimal(raw: &str) -> Result<u64, AppError> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(storage_failure());
    }
    if raw.len() > 1 && raw.starts_with('0') {
        return Err(storage_failure());
    }
    raw.parse::<u64>().map_err(|_| storage_failure())
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    struct TemporaryDirectory(std::path::PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(format!(
                    "database-parent-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    #[test]
    fn open_creates_nested_private_parents_and_preserves_existing_modes() {
        let root = TemporaryDirectory::new();
        #[cfg(unix)]
        let original = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            root.path().metadata().unwrap().permissions().mode() & 0o777
        };
        let path = root.path().join("one/two/ghostlink.db");
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.schema_version().unwrap(), LATEST_SCHEMA_VERSION);
        assert!(path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in [root.path().join("one"), root.path().join("one/two")] {
                assert_eq!(dir.metadata().unwrap().permissions().mode() & 0o777, 0o700);
            }
            assert_eq!(
                root.path().metadata().unwrap().permissions().mode() & 0o777,
                original
            );
        }
    }

    #[test]
    fn parent_and_open_failures_are_sanitized_storage_errors() {
        let root = TemporaryDirectory::new();
        let blocker = root.path().join("sentinel");
        std::fs::write(&blocker, b"not a directory").unwrap();
        for path in [
            blocker.join("nested/ghostlink.db"),
            root.path().to_path_buf(),
        ] {
            let error = SqliteStore::open(path).unwrap_err();
            assert_eq!(error.category(), ErrorCategory::LocalStorage);
            assert_eq!(error.to_string(), storage_failure().to_string());
            assert!(!format!("{error:?}").contains("sentinel"));
        }
    }
}
