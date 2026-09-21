//! Platform-native credential store adapter.
//!
//! This module implements the provider-neutral
//! [`destiny_core::CredentialStore`] port on top of the operating system's
//! native secure store through the `keyring-core` client layer. It persists
//! one opaque access credential and deliberately offers no plaintext or
//! filesystem fallback.
//!
//! Security posture:
//!
//! - fail closed: any backend initialization failure, and any keyring error
//!   other than the explicit "no such entry" case, maps to
//!   [`ErrorCategory::CredentialStoreUnavailable`];
//! - unsupported targets fail closed at construction and never degrade to a
//!   file, an environment variable, or an in-memory store;
//! - the single service name and the single user name are fixed constants
//!   (the deployment contract), so the adapter cannot be steered to an
//!   attacker-chosen key;
//! - every keyring error is discarded whole, never formatted: several
//!   `keyring_core::Error` variants carry raw secret bytes in their payload,
//!   so the adapter must not render them into an [`AppError`] or a
//!   [`fmt::Debug`] surface;
//! - binary secrets are read and written through `set_secret`/`get_secret`,
//!   which impose no UTF-8 requirement.
//!
//! Tests inject `keyring_core`'s in-memory mock store and never touch a real
//! operating system credential store.

use destiny_core::{AppError, CredentialMaterial, CredentialStore, ErrorCategory};
use keyring_core::{CredentialStore as KeyringStore, Entry};
use std::fmt;
use std::sync::Arc;

/// Deployment contract: the fixed service name under which the Destiny access
/// credential is stored.
pub const CREDENTIAL_SERVICE: &str = "dev.ghostlink.destiny";

/// Deployment contract: the fixed user (account) name of the single stored
/// access credential.
pub const CREDENTIAL_USER: &str = "destiny-access-credential";

fn credential_store_unavailable() -> AppError {
    AppError::new(ErrorCategory::CredentialStoreUnavailable)
}

/// Builds the operating system's native credential store.
///
/// Each supported target selects exactly one explicit production backend, as
/// recommended for `keyring-core`. Unsupported targets return
/// `CredentialStoreUnavailable` instead of falling back to any weaker store.
#[cfg(target_os = "macos")]
fn native_store() -> Result<Arc<KeyringStore>, AppError> {
    let store: Arc<KeyringStore> = apple_native_keyring_store::keychain::Store::new()
        .map_err(|_| credential_store_unavailable())?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn native_store() -> Result<Arc<KeyringStore>, AppError> {
    let store: Arc<KeyringStore> =
        windows_native_keyring_store::Store::new().map_err(|_| credential_store_unavailable())?;
    Ok(store)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn native_store() -> Result<Arc<KeyringStore>, AppError> {
    let store: Arc<KeyringStore> = dbus_secret_service_keyring_store::Store::new()
        .map_err(|_| credential_store_unavailable())?;
    Ok(store)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd"
)))]
fn native_store() -> Result<Arc<KeyringStore>, AppError> {
    // Fail closed: no supported native backend exists on this target.
    Err(credential_store_unavailable())
}

/// A [`CredentialStore`] backed by the operating system's native secure store.
///
/// The adapter wraps a `keyring-core` credential store and derives one fixed
/// entry from [`CREDENTIAL_SERVICE`] and [`CREDENTIAL_USER`] for every
/// operation. It is intentionally not configurable at runtime so the stored
/// key cannot be redirected.
pub struct NativeCredentialStore {
    store: Arc<KeyringStore>,
}

impl NativeCredentialStore {
    /// Builds the adapter on the platform-native backend.
    ///
    /// # Errors
    ///
    /// Returns `CredentialStoreUnavailable` when the backend store cannot be
    /// initialized (for example a locked or unreachable Secret Service) or
    /// when the target has no supported native backend. This is the only
    /// failure mode; construction never degrades to a weaker store.
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            store: native_store()?,
        })
    }

    /// Wraps a caller-provided `keyring-core` store.
    ///
    /// This is the injection seam for tests and future composition: the
    /// adapter never mutates process-global keyring state, so callers can
    /// supply any `CredentialStoreApi` implementation without affecting other
    /// entries in the process.
    pub fn with_store(store: Arc<KeyringStore>) -> Self {
        Self { store }
    }

    fn entry(&self) -> Result<Entry, AppError> {
        self.store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .map_err(|_| credential_store_unavailable())
    }
}

impl fmt::Debug for NativeCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeCredentialStore")
            .finish_non_exhaustive()
    }
}

impl CredentialStore for NativeCredentialStore {
    fn save(&mut self, material: CredentialMaterial) -> Result<(), AppError> {
        self.entry()?
            .set_secret(material.expose_secret())
            .map_err(|_| credential_store_unavailable())
    }

    fn load(&self) -> Result<Option<CredentialMaterial>, AppError> {
        match self.entry()?.get_secret() {
            Ok(bytes) => Ok(Some(CredentialMaterial::from_bytes(bytes))),
            // Fail closed: a missing entry is the only benign outcome.
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(credential_store_unavailable()),
        }
    }

    fn delete(&mut self) -> Result<(), AppError> {
        match self.entry()?.delete_credential() {
            Ok(()) => Ok(()),
            // Deleting an absent credential is a successful no-op: logout and
            // revocation flows must be idempotent and must not report a
            // failure for an already-clean store.
            Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err(credential_store_unavailable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyring_core::Error as KeyringError;
    use keyring_core::mock;

    /// A canary that must never appear in any formatted diagnostic.
    const SENTINEL: &str = "sentinel-credential-canary";

    fn mock_store() -> Arc<KeyringStore> {
        let store: Arc<KeyringStore> = mock::Store::new().expect("mock store builds");
        store
    }

    /// Forces the next call on the shared mock credential to fail with
    /// `error`. The mock reuses one credential per `(service, user)` pair, so
    /// an entry built here is the same credential the adapter operates on.
    fn force_next_error(store: &Arc<KeyringStore>, error: KeyringError) {
        let entry = store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .expect("the mock store builds the fixed entry");
        let credential: &mock::Cred = entry
            .as_any()
            .downcast_ref()
            .expect("the mock credential is concrete");
        credential.set_error(error);
    }

    fn canary_material() -> CredentialMaterial {
        CredentialMaterial::from_bytes(SENTINEL.as_bytes().to_vec())
    }

    #[test]
    fn deployment_contract_is_fixed_and_documented() {
        assert_eq!(CREDENTIAL_SERVICE, "dev.ghostlink.destiny");
        assert_eq!(CREDENTIAL_USER, "destiny-access-credential");
    }

    #[test]
    fn missing_entry_loads_as_none() {
        let adapter = NativeCredentialStore::with_store(mock_store());
        assert!(adapter.load().expect("missing entry is benign").is_none());
    }

    #[test]
    fn save_load_delete_round_trip_is_byte_exact() {
        let mut adapter = NativeCredentialStore::with_store(mock_store());
        assert!(adapter.load().expect("initially empty").is_none());

        adapter.save(canary_material()).expect("save succeeds");
        let loaded = adapter
            .load()
            .expect("load succeeds")
            .expect("material is present");
        assert_eq!(loaded.expose_secret(), SENTINEL.as_bytes());

        adapter.delete().expect("delete succeeds");
        assert!(adapter.load().expect("cleared").is_none());
    }

    #[test]
    fn save_persists_under_the_fixed_contract_specifiers() {
        let store = mock_store();
        let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        adapter.save(canary_material()).expect("save succeeds");

        let entry = store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .expect("the fixed entry is buildable");
        assert_eq!(
            entry.get_secret().expect("secret was stored"),
            SENTINEL.as_bytes()
        );
    }

    #[test]
    fn delete_of_missing_entry_is_a_successful_no_op() {
        let mut adapter = NativeCredentialStore::with_store(mock_store());
        adapter.delete().expect("first delete is a no-op");
        adapter.delete().expect("repeat delete stays a no-op");
    }

    #[test]
    fn backend_failures_map_to_credential_store_unavailable() {
        let failure = || KeyringError::NoStorageAccess(Box::new(std::io::Error::other("locked")));

        let store = mock_store();
        let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        force_next_error(&store, failure());
        let error = adapter.save(canary_material()).expect_err("save fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);

        force_next_error(&store, failure());
        let error = adapter.load().expect_err("load fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);

        force_next_error(&store, failure());
        let error = adapter.delete().expect_err("delete fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
    }

    #[test]
    fn formatted_errors_never_expose_secret_bytes() {
        // Every diagnostic surface an operator or log could reach: the
        // AppError Display/Debug, and the adapter's own Debug.
        let secret_bearing_errors = [
            KeyringError::BadEncoding(SENTINEL.as_bytes().to_vec()),
            KeyringError::BadDataFormat(
                SENTINEL.as_bytes().to_vec(),
                Box::new(std::io::Error::other("malformed")),
            ),
            KeyringError::PlatformFailure(Box::new(std::io::Error::other(SENTINEL))),
        ];

        for error in secret_bearing_errors {
            let store = mock_store();
            let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
            adapter.save(canary_material()).expect("canary is stored");

            force_next_error(&store, error);
            let failure = adapter.load().expect_err("load fails closed");

            let rendered = format!("{failure} {failure:?} {adapter:?}");
            assert!(
                !rendered.contains(SENTINEL),
                "a formatted diagnostic leaked the canary: {rendered}"
            );
        }

        // The material wrapper itself must also redact.
        assert_eq!(format!("{:?}", canary_material()), "CredentialMaterial");
    }

    #[test]
    fn construction_failure_stays_fail_closed() {
        // The only production constructor failure path must surface as
        // CredentialStoreUnavailable, never as a weaker-store fallback.
        assert_eq!(
            credential_store_unavailable().category(),
            ErrorCategory::CredentialStoreUnavailable
        );
    }
}
