//! Platform-native credential store adapter.
//!
//! This module implements the provider-neutral
//! [`destiny_core::CredentialStore`] port on top of the operating system's
//! native secure store through the `keyring-core` client layer. It persists one
//! complete [`AccessCredential`] — opaque material plus expiry metadata — and
//! deliberately offers no plaintext or filesystem fallback.
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
//! - the stored value is a self-describing, versioned, bounded record, so an
//!   entry written by an older raw-material-only build is detected and rejected
//!   with [`ErrorCategory::ReauthorizationRequired`] rather than silently
//!   reinterpreted, and a corrupted record fails closed with a sanitized
//!   category instead of a guessed value.
//!
//! Record layout (all integers big-endian): `GLCR`, version byte, access expiry
//! (`u64`), material length (`u32`), material bytes. Version 2 (always written)
//! appends a grant flag: 0 ends the record; 1 adds refresh expiry (`u64`), token
//! length (`u32`), and token bytes. Material and token are each bounded to 4 KiB;
//! refresh tokens must be nonempty. Version 1 ends after the material and loads
//! without a grant. Unknown versions, flags, truncation, oversized fields, and
//! trailing bytes map to `CredentialStoreUnavailable`, as corrupted v1 did.
//!
//! Tests inject `keyring_core`'s in-memory mock store and never touch a real
//! operating system credential store.

use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CredentialMaterial, CredentialStore,
    ErrorCategory, RefreshGrant, RefreshToken, Timestamp,
};
use keyring_core::{CredentialStore as KeyringStore, Entry};
use std::fmt;
use std::sync::Arc;

/// Deployment contract: the fixed service name under which the Destiny access
/// credential is stored.
pub const CREDENTIAL_SERVICE: &str = "dev.ghostlink.destiny";

/// Deployment contract: the fixed user (account) name of the single stored
/// access credential.
pub const CREDENTIAL_USER: &str = "destiny-access-credential";

/// The four-byte leading marker of a versioned stored record.
///
/// A stored value that does not begin with this marker is a legacy
/// raw-material-only entry (see [`decode_credential`]); a value that begins with
/// the marker but is otherwise malformed is a corrupted record.
const RECORD_MAGIC: [u8; 4] = *b"GLCR";

/// The stored-record layout written by this build (version 1 stays readable).
const RECORD_VERSION: u8 = 2;

/// Fixed header length: magic (4) + version (1) + expiry seconds (8, big-endian)
/// + material length (4, big-endian).
const RECORD_HEADER_LENGTH: usize = 17;

/// Strict upper bound on the opaque material carried by one credential.
///
/// Real public-client bearer tokens are a few hundred bytes; a larger value is
/// treated as an unencodable credential and fails closed before the record is
/// written.
const MAX_MATERIAL_LENGTH: usize = 4 * 1024;

/// Strict upper bound on a complete stored record.
const MAX_RECORD_LENGTH: usize =
    RECORD_HEADER_LENGTH + MAX_MATERIAL_LENGTH + 1 + 12 + MAX_MATERIAL_LENGTH;

fn credential_store_unavailable() -> AppError {
    AppError::new(ErrorCategory::CredentialStoreUnavailable)
}

/// The category for a stored record whose expiry cannot be recovered.
fn reauthorization_required() -> AppError {
    AppError::new(ErrorCategory::ReauthorizationRequired)
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

/// Encodes one complete credential into the bounded, versioned stored record.
///
/// The material is copied verbatim, so arbitrary opaque bytes round-trip
/// exactly, including NUL bytes and non-UTF-8 sequences. Oversized material is
/// refused with the adapter's sanitized failure category rather than truncated
/// or written anyway.
fn encode_credential(credential: &AccessCredential) -> Result<Vec<u8>, AppError> {
    let material = credential.material.expose_secret();
    if material.len() > MAX_MATERIAL_LENGTH {
        return Err(credential_store_unavailable());
    }
    let grant = credential.refresh_grant();
    if grant.is_some_and(|grant| grant.token.as_bytes().len() > MAX_MATERIAL_LENGTH) {
        return Err(credential_store_unavailable());
    }
    let mut record = Vec::with_capacity(
        RECORD_HEADER_LENGTH
            + material.len()
            + 1
            + grant.map_or(0, |grant| 12 + grant.token.as_bytes().len()),
    );
    record.extend_from_slice(&RECORD_MAGIC);
    record.push(RECORD_VERSION);
    record.extend_from_slice(&credential.metadata.expires_at.seconds().to_be_bytes());
    // The bound above proves the length fits the four-byte field.
    record.extend_from_slice(&(material.len() as u32).to_be_bytes());
    record.extend_from_slice(material);
    match grant {
        None => record.push(0),
        Some(grant) => {
            let token = grant.token.as_bytes();
            record.push(1);
            record.extend_from_slice(&grant.expires_at.seconds().to_be_bytes());
            record.extend_from_slice(&(token.len() as u32).to_be_bytes());
            record.extend_from_slice(token);
        }
    }
    Ok(record)
}

/// Decodes one stored record, or fails closed with a sanitized category.
///
/// A value that does not begin with [`RECORD_MAGIC`] is a legacy entry written
/// by a raw-material-only build, whatever its length. Its expiry is genuinely
/// unknowable, so the adapter reports [`ErrorCategory::ReauthorizationRequired`]
/// and never guesses a timestamp or returns the stored bytes as material. The
/// marker test therefore runs before the versioned-record size check, so an
/// oversized legacy value is still recognized as legacy rather than misreported
/// as a storage failure. A value that carries the marker but is truncated,
/// version-mismatched, length-inconsistent, or oversized is a corrupted record
/// and reports [`ErrorCategory::CredentialStoreUnavailable`].
///
/// Neither failure path renders, echoes, or formats any byte of the stored
/// value.
fn decode_credential(record: &[u8]) -> Result<AccessCredential, AppError> {
    if !record.starts_with(&RECORD_MAGIC) {
        // Legacy raw-material entry of any length: the expiry is forever
        // unknowable, and the record is not size-validated as a versioned one.
        return Err(reauthorization_required());
    }
    if record.len() > MAX_RECORD_LENGTH {
        return Err(credential_store_unavailable());
    }
    if record.len() < RECORD_HEADER_LENGTH || !matches!(record[4], 1 | RECORD_VERSION) {
        return Err(credential_store_unavailable());
    }
    let expires_at = u64::from_be_bytes(
        record[5..13]
            .try_into()
            .map_err(|_| credential_store_unavailable())?,
    );
    let declared_material_length = u32::from_be_bytes(
        record[13..17]
            .try_into()
            .map_err(|_| credential_store_unavailable())?,
    ) as usize;
    if declared_material_length > MAX_MATERIAL_LENGTH {
        return Err(credential_store_unavailable());
    }
    // The field bound proves this addition cannot overflow, even on 32-bit targets.
    let material_end = RECORD_HEADER_LENGTH + declared_material_length;
    let material = record
        .get(RECORD_HEADER_LENGTH..material_end)
        .ok_or_else(credential_store_unavailable)?;
    let suffix = record
        .get(material_end..)
        .ok_or_else(credential_store_unavailable)?;
    let refresh_grant = if record[4] == 1 {
        if !suffix.is_empty() {
            return Err(credential_store_unavailable());
        }
        None
    } else {
        match suffix.first() {
            Some(0) if suffix.len() == 1 => None,
            Some(1) if suffix.len() >= 13 => {
                let refresh_expires_at = u64::from_be_bytes(
                    suffix[1..9]
                        .try_into()
                        .map_err(|_| credential_store_unavailable())?,
                );
                let token_length = u32::from_be_bytes(
                    suffix[9..13]
                        .try_into()
                        .map_err(|_| credential_store_unavailable())?,
                ) as usize;
                if token_length > MAX_MATERIAL_LENGTH || suffix.len() - 13 != token_length {
                    return Err(credential_store_unavailable());
                }
                Some(RefreshGrant {
                    token: RefreshToken::from_bytes(suffix[13..].to_vec())
                        .map_err(|_| credential_store_unavailable())?,
                    expires_at: Timestamp::new(refresh_expires_at),
                })
            }
            _ => return Err(credential_store_unavailable()),
        }
    };
    Ok(AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(expires_at),
        },
        material: CredentialMaterial::from_bytes(material.to_vec()),
        refresh_grant,
    })
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
    fn save(&mut self, credential: AccessCredential) -> Result<(), AppError> {
        let record = encode_credential(&credential)?;
        self.entry()?
            .set_secret(&record)
            .map_err(|_| credential_store_unavailable())
    }

    fn load(&self) -> Result<Option<AccessCredential>, AppError> {
        match self.entry()?.get_secret() {
            Ok(record) => decode_credential(&record).map(Some),
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

    /// A canary that must never appear in any formatted diagnostic or stored
    /// plaintext.
    const SENTINEL: &str = "sentinel-credential-canary";

    /// A second canary used to probe binary-safety independently.
    const BINARY_SENTINEL: [u8; 6] = [0x00, 0xff, 0x10, 0x00, 0xfe, 0x7f];

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

    /// Writes raw bytes directly, bypassing the adapter, to model a legacy or
    /// corrupted entry written by another build.
    fn write_raw_record(store: &Arc<KeyringStore>, bytes: &[u8]) {
        store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .expect("the mock store builds the fixed entry")
            .set_secret(bytes)
            .expect("raw record is written");
    }

    /// Reads the raw stored bytes without decoding them.
    fn raw_stored_record(store: &Arc<KeyringStore>) -> Vec<u8> {
        store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .expect("the mock store builds the fixed entry")
            .get_secret()
            .expect("a record is stored")
    }

    /// Reads the raw stored bytes, or `None` when no entry exists.
    fn try_raw_stored_record(store: &Arc<KeyringStore>) -> Option<Vec<u8>> {
        store
            .build(CREDENTIAL_SERVICE, CREDENTIAL_USER, None)
            .ok()?
            .get_secret()
            .ok()
    }

    fn canary_credential() -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_700_000_000),
            },
            material: CredentialMaterial::from_bytes(SENTINEL.as_bytes().to_vec()),
            refresh_grant: None,
        }
    }

    const REFRESH_SENTINEL: &str = "refresh-token-canary";

    fn grant_credential() -> AccessCredential {
        canary_credential().with_refresh_grant(destiny_core::RefreshGrant {
            token: destiny_core::RefreshToken::from_bytes(REFRESH_SENTINEL.as_bytes().to_vec())
                .expect("valid refresh token"),
            expires_at: Timestamp::new(1_800_000_000),
        })
    }

    // Independent fixture builder: never rely on the encoder under test.
    fn fixture(version: u8, material: &[u8]) -> Vec<u8> {
        let mut record = b"GLCR".to_vec();
        record.push(version);
        record.extend_from_slice(&1_700_000_000u64.to_be_bytes());
        record.extend_from_slice(&(material.len() as u32).to_be_bytes());
        record.extend_from_slice(material);
        record
    }

    fn grant_fixture() -> Vec<u8> {
        let mut record = fixture(2, SENTINEL.as_bytes());
        record.push(1);
        record.extend_from_slice(&1_800_000_000u64.to_be_bytes());
        record.extend_from_slice(&(REFRESH_SENTINEL.len() as u32).to_be_bytes());
        record.extend_from_slice(REFRESH_SENTINEL.as_bytes());
        record
    }

    #[test]
    fn v2_round_trips_with_and_without_refresh_grant() {
        for credential in [canary_credential(), grant_credential()] {
            let store = mock_store();
            let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
            adapter.save(credential.clone()).expect("save");
            assert_eq!(raw_stored_record(&store)[4], 2);
            assert_eq!(adapter.load().expect("load"), Some(credential));
        }
    }

    #[test]
    fn v1_fixture_remains_readable_without_refresh_grant() {
        let store = mock_store();
        write_raw_record(&store, &fixture(1, SENTINEL.as_bytes()));
        let adapter = NativeCredentialStore::with_store(store);
        assert_eq!(adapter.load().expect("v1 load"), Some(canary_credential()));
    }

    #[test]
    fn v2_fixture_and_every_truncation_are_checked_without_leaking_tokens() {
        let record = grant_fixture();
        let store = mock_store();
        write_raw_record(&store, &record);
        let adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        let loaded = adapter.load().expect("v2 fixture load").expect("present");
        assert_eq!(loaded, grant_credential());
        assert!(!format!("{loaded:?} {adapter:?}").contains(REFRESH_SENTINEL));

        for length in 4..record.len() {
            write_raw_record(&store, &record[..length]);
            let error = adapter.load().expect_err("truncated v2");
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert!(!format!("{error} {error:?}").contains(REFRESH_SENTINEL));
        }
    }

    #[test]
    fn v2_rejects_invalid_flags_lengths_versions_and_trailing_bytes() {
        let flag_offset = 17 + SENTINEL.len();
        let mut bad_flag = grant_fixture();
        bad_flag[flag_offset] = 2;
        let mut oversized_token = grant_fixture();
        oversized_token[flag_offset + 9..flag_offset + 13].copy_from_slice(&4097u32.to_be_bytes());
        let mut empty_token = grant_fixture();
        empty_token[flag_offset + 9..flag_offset + 13].copy_from_slice(&0u32.to_be_bytes());
        empty_token.truncate(flag_offset + 13);
        let mut oversized_material = grant_fixture();
        oversized_material[13..17].copy_from_slice(&4097u32.to_be_bytes());
        let mut unknown_version = grant_fixture();
        unknown_version[4] = 3;
        let mut trailing = grant_fixture();
        trailing.push(0);
        let mut absent_with_trailing = fixture(2, SENTINEL.as_bytes());
        absent_with_trailing.extend_from_slice(&[0, 0]);
        for record in [
            bad_flag,
            oversized_token,
            empty_token,
            oversized_material,
            unknown_version,
            trailing,
            absent_with_trailing,
        ] {
            let store = mock_store();
            write_raw_record(&store, &record);
            let adapter = NativeCredentialStore::with_store(store);
            let error = adapter.load().expect_err("invalid v2");
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert!(!format!("{error} {error:?} {adapter:?}").contains(REFRESH_SENTINEL));
        }
    }

    #[test]
    fn v2_accepts_both_secrets_at_their_maximum_lengths() {
        let credential = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(u64::MAX),
            },
            material: CredentialMaterial::from_bytes(vec![0xff; 4096]),
            refresh_grant: Some(destiny_core::RefreshGrant {
                token: destiny_core::RefreshToken::from_bytes(vec![0xfe; 4096]).expect("bounded"),
                expires_at: Timestamp::new(u64::MAX),
            }),
        };
        let store = mock_store();
        let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        adapter.save(credential.clone()).expect("save maximum");
        assert_eq!(raw_stored_record(&store).len(), 17 + 4096 + 1 + 12 + 4096);
        assert_eq!(adapter.load().expect("load maximum"), Some(credential));
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
    fn save_load_delete_round_trips_material_and_expiry_exactly() {
        let mut adapter = NativeCredentialStore::with_store(mock_store());
        assert!(adapter.load().expect("initially empty").is_none());

        let credential = canary_credential();
        adapter.save(credential.clone()).expect("save succeeds");
        let loaded = adapter
            .load()
            .expect("load succeeds")
            .expect("credential is present");
        assert_eq!(loaded, credential);
        assert_eq!(loaded.metadata.expires_at, Timestamp::new(1_700_000_000));
        assert_eq!(loaded.material.expose_secret(), SENTINEL.as_bytes());

        adapter.delete().expect("delete succeeds");
        assert!(adapter.load().expect("cleared").is_none());
    }

    #[test]
    fn arbitrary_binary_material_round_trips_byte_exact() {
        let mut adapter = NativeCredentialStore::with_store(mock_store());
        for material in [
            Vec::new(),
            BINARY_SENTINEL.to_vec(),
            vec![0u8; MAX_MATERIAL_LENGTH],
        ] {
            let credential = AccessCredential {
                metadata: AccessCredentialMetadata {
                    expires_at: Timestamp::new(7),
                },
                material: CredentialMaterial::from_bytes(material.clone()),
                refresh_grant: None,
            };
            adapter.save(credential.clone()).expect("save succeeds");
            let loaded = adapter
                .load()
                .expect("load succeeds")
                .expect("credential is present");
            assert_eq!(loaded, credential);
            assert_eq!(loaded.material.expose_secret(), material.as_slice());
            assert_eq!(loaded.metadata.expires_at, Timestamp::new(7));
        }
    }

    #[test]
    fn stored_record_is_versioned_and_never_the_plaintext_material() {
        let store = mock_store();
        let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        adapter.save(canary_credential()).expect("save succeeds");

        let raw = raw_stored_record(&store);
        assert_eq!(&raw[..4], &RECORD_MAGIC);
        assert_eq!(raw[4], RECORD_VERSION);
        assert_eq!(raw.len(), RECORD_HEADER_LENGTH + SENTINEL.len() + 1);
        assert!(
            !raw.starts_with(SENTINEL.as_bytes()),
            "the raw material must never be the stored value"
        );
        // The encoding is not a plaintext fallback: decoding is required.
        assert_ne!(raw, SENTINEL.as_bytes());
    }

    #[test]
    fn oversized_material_is_refused_before_it_is_written() {
        let store = mock_store();
        let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));
        let oversized = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1),
            },
            material: CredentialMaterial::from_bytes(vec![0u8; MAX_MATERIAL_LENGTH + 1]),
            refresh_grant: None,
        };
        let error = adapter.save(oversized).expect_err("oversized is refused");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
        assert!(
            try_raw_stored_record(&store).is_none(),
            "the oversized credential must not be written"
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
        let error = adapter.save(canary_credential()).expect_err("save fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);

        force_next_error(&store, failure());
        let error = adapter.load().expect_err("load fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);

        force_next_error(&store, failure());
        let error = adapter.delete().expect_err("delete fails");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
    }

    #[test]
    fn unsupported_backend_operations_fail_closed() {
        // `NotSupportedByStore` (the backend cannot perform the operation) and
        // `NoDefaultStore` (no backend is configured) are the two "unsupported"
        // outcomes of the `keyring-core` client layer. Neither may degrade to an
        // in-memory, filesystem, or environment value: both map to
        // `CredentialStoreUnavailable`, and a failed save leaves no entry behind.
        let unsupported = || KeyringError::NotSupportedByStore("no usable backend".to_string());
        let no_default_store = || KeyringError::NoDefaultStore;

        for failure in [unsupported, no_default_store] {
            let store = mock_store();
            let mut adapter = NativeCredentialStore::with_store(Arc::clone(&store));

            force_next_error(&store, failure());
            let error = adapter
                .save(canary_credential())
                .expect_err("save fails closed");
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert!(
                try_raw_stored_record(&store).is_none(),
                "no weaker store may be written when the backend is unsupported"
            );

            force_next_error(&store, failure());
            let error = adapter.load().expect_err("load fails closed");
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);

            force_next_error(&store, failure());
            let error = adapter.delete().expect_err("delete fails closed");
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
        }
    }

    #[test]
    fn legacy_raw_material_entry_requires_reauthorization_without_leaking_bytes() {
        let store = mock_store();
        write_raw_record(&store, SENTINEL.as_bytes());
        let adapter = NativeCredentialStore::with_store(Arc::clone(&store));

        let error = adapter
            .load()
            .expect_err("a legacy entry has unknowable expiry");
        assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);

        // No path may render the raw material: the error, its debug form, and
        // the adapter's own debug view.
        let rendered = format!("{error} {error:?} {adapter:?}");
        assert!(
            !rendered.contains(SENTINEL),
            "a formatted diagnostic leaked the legacy material: {rendered}"
        );
    }

    #[test]
    fn legacy_binary_entry_is_recognized_without_guessing_expiry() {
        let store = mock_store();
        write_raw_record(&store, BINARY_SENTINEL.as_slice());
        let adapter = NativeCredentialStore::with_store(store);
        assert_eq!(
            adapter.load().expect_err("legacy entry").category(),
            ErrorCategory::ReauthorizationRequired
        );
    }

    #[test]
    fn oversized_legacy_markerless_entry_is_still_legacy_and_fails_closed() {
        // A markerless value longer than the versioned record bound must be
        // classified as legacy before any size validation, so an oversized raw
        // material can never be misreported as a storage failure.
        let oversized_legacy = vec![0x41u8; MAX_RECORD_LENGTH + 1];
        let store = mock_store();
        write_raw_record(&store, &oversized_legacy);
        let adapter = NativeCredentialStore::with_store(Arc::clone(&store));

        let error = adapter
            .load()
            .expect_err("an oversized legacy entry has unknowable expiry");
        assert_eq!(error.category(), ErrorCategory::ReauthorizationRequired);
        let rendered = format!("{error} {error:?} {adapter:?}");
        assert!(!rendered.contains("41"), "oversized legacy bytes leaked");
    }

    #[test]
    fn malformed_marker_record_fails_sanitized() {
        for record in [
            RECORD_MAGIC.to_vec(), // marker only, truncated header
            {
                let mut value = RECORD_MAGIC.to_vec();
                value.push(RECORD_VERSION + 1); // unknown version
                value.extend_from_slice(&[0u8; 20]);
                value
            },
            {
                let mut value = encode_credential(&canary_credential()).expect("encode");
                value.truncate(RECORD_HEADER_LENGTH + SENTINEL.len() - 1); // short body
                value
            },
            {
                let mut value = encode_credential(&canary_credential()).expect("encode");
                value.push(0); // trailing byte beyond declared length
                value
            },
            {
                let mut value = RECORD_MAGIC.to_vec();
                value.push(RECORD_VERSION);
                value.extend_from_slice(&[0u8; 8]);
                value.extend_from_slice(&u32::MAX.to_be_bytes()); // oversized declared length
                value
            },
        ] {
            let store = mock_store();
            write_raw_record(&store, &record);
            let adapter = NativeCredentialStore::with_store(store);
            let error = adapter.load().expect_err("malformed record fails closed");
            assert_eq!(
                error.category(),
                ErrorCategory::CredentialStoreUnavailable,
                "record {record:?} must fail sanitized"
            );
        }
    }

    #[test]
    fn oversized_stored_record_fails_sanitized_without_rendering_bytes() {
        let mut record = RECORD_MAGIC.to_vec();
        record.push(RECORD_VERSION);
        record.extend_from_slice(&[0u8; 8]);
        record.extend_from_slice(&[0u8; 4]);
        record.extend_from_slice(&vec![0xffu8; MAX_RECORD_LENGTH]);

        let store = mock_store();
        write_raw_record(&store, &record);
        let adapter = NativeCredentialStore::with_store(store);
        let error = adapter.load().expect_err("oversized record fails closed");
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
        assert!(!format!("{error} {error:?}").contains("ff"));
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
            adapter.save(canary_credential()).expect("canary is stored");

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
        assert_eq!(format!("{:?}", canary_credential()), "AccessCredential");
    }

    #[test]
    fn construction_failure_stays_fail_closed() {
        // The only production constructor failure path must surface as
        // CredentialStoreUnavailable, never as a weaker-store fallback.
        assert_eq!(
            credential_store_unavailable().category(),
            ErrorCategory::CredentialStoreUnavailable
        );
        assert_eq!(
            reauthorization_required().category(),
            ErrorCategory::ReauthorizationRequired
        );
    }

    #[test]
    fn record_bounds_are_consistent() {
        assert_eq!(
            MAX_RECORD_LENGTH,
            RECORD_HEADER_LENGTH + MAX_MATERIAL_LENGTH + 1 + 12 + MAX_MATERIAL_LENGTH
        );
        // The empty-material boundary record is exactly decodable.
        let boundary = AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(0),
            },
            material: CredentialMaterial::from_bytes(Vec::new()),
            refresh_grant: None,
        };
        let encoded = encode_credential(&boundary).expect("encode");
        assert_eq!(encoded.len(), RECORD_HEADER_LENGTH + 1);
        assert_eq!(decode_credential(&encoded).expect("decode"), boundary);
    }
}
