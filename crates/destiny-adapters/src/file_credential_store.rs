//! Plaintext file credential store adapter.
//!
//! This module implements the provider-neutral
//! [`destiny_core::CredentialStore`] port as one small JSON file at a
//! caller-resolved per-user path (by default
//! `~/.config/ghostlink/credentials.json`). It needs no keyring, Secret
//! Service or D-Bus session, so it works on headless servers.
//!
//! The file is **not encrypted**: any process running as the same user can
//! read it. The adapter only keeps it private to that user:
//!
//! - a missing parent directory is created `0700` on Unix and the file is
//!   always written `0600`; on load, a file that group or others may access
//!   fails closed, as `config.env` does. On Windows the per-user profile
//!   directory's inherited access control applies;
//! - writes are atomic: a uniquely named sibling temporary file is written,
//!   flushed and renamed over the target, so a reader never sees a partial
//!   file and a failed write leaves the previous credential intact;
//! - every failure maps to [`ErrorCategory::CredentialStoreUnavailable`] and
//!   never renders the path, the file contents or any token byte;
//! - parsing is strict and bounded: unknown versions or fields, missing or
//!   unpaired fields, and empty or oversized tokens fail closed;
//! - token refresh is serialized across processes with an exclusive lock on a
//!   private, empty sibling `credentials.json.lock`.
//!
//! File format, version 1 (expiries are Unix seconds; the two refresh fields
//! are present together or not at all):
//!
//! ```json
//! {
//!   "version": 1,
//!   "access_token": "...",
//!   "access_expires_at": 1760000000,
//!   "refresh_token": "...",
//!   "refresh_expires_at": 1767776000
//! }
//! ```

use destiny_core::{
    AccessCredential, AccessCredentialMetadata, AppError, CredentialMaterial,
    CredentialRefreshGuard, CredentialStore, ErrorCategory, RefreshGrant, RefreshToken, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The file layout written and accepted by this build.
const FILE_VERSION: u8 = 1;

/// Strict upper bound on each stored token, matching the former keyring record.
const MAX_TOKEN_LENGTH: usize = 4 * 1024;

/// Strict upper bound on the whole file, read with one byte of slack.
const MAX_FILE_LENGTH: usize = 64 * 1024;

fn credential_store_unavailable() -> AppError {
    AppError::new(ErrorCategory::CredentialStoreUnavailable)
}

/// The on-disk shape. It derives no `Debug`, so its tokens cannot be rendered.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    version: u8,
    access_token: String,
    access_expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_expires_at: Option<u64>,
}

/// Accepts only a nonempty, bounded, UTF-8 token; Bungie tokens are ASCII.
fn token_text(bytes: &[u8]) -> Result<String, AppError> {
    if bytes.is_empty() || bytes.len() > MAX_TOKEN_LENGTH {
        return Err(credential_store_unavailable());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| credential_store_unavailable())
}

fn encode(credential: &AccessCredential) -> Result<Vec<u8>, AppError> {
    let (refresh_token, refresh_expires_at) = match credential.refresh_grant() {
        Some(grant) => (
            Some(token_text(grant.token.as_bytes())?),
            Some(grant.expires_at.seconds()),
        ),
        None => (None, None),
    };
    let file = CredentialFile {
        version: FILE_VERSION,
        access_token: token_text(credential.material.expose_secret())?,
        access_expires_at: credential.metadata.expires_at.seconds(),
        refresh_token,
        refresh_expires_at,
    };
    let mut bytes = serde_json::to_vec_pretty(&file).map_err(|_| credential_store_unavailable())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<AccessCredential, AppError> {
    if bytes.len() > MAX_FILE_LENGTH {
        return Err(credential_store_unavailable());
    }
    // A parse error is discarded whole: it is never rendered into an error.
    let file: CredentialFile =
        serde_json::from_slice(bytes).map_err(|_| credential_store_unavailable())?;
    if file.version != FILE_VERSION {
        return Err(credential_store_unavailable());
    }
    let access_token = token_text(file.access_token.as_bytes())?;
    let refresh_grant = match (file.refresh_token, file.refresh_expires_at) {
        (None, None) => None,
        (Some(token), Some(expires_at)) => Some(RefreshGrant {
            token: RefreshToken::from_bytes(token_text(token.as_bytes())?.into_bytes())
                .map_err(|_| credential_store_unavailable())?,
            expires_at: Timestamp::new(expires_at),
        }),
        _ => return Err(credential_store_unavailable()),
    };
    Ok(AccessCredential {
        metadata: AccessCredentialMetadata {
            expires_at: Timestamp::new(file.access_expires_at),
        },
        material: CredentialMaterial::from_bytes(access_token.into_bytes()),
        refresh_grant,
    })
}

/// Creates a missing directory `0700`, and refuses an existing one that group or
/// others may write: they could unlink the credential or its refresh lock.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if fs::metadata(path)?.permissions().mode() & 0o022 != 0 {
        return Err(std::io::Error::from(ErrorKind::PermissionDenied));
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Persists the rename itself; best effort, since the data is already flushed.
#[cfg(unix)]
fn sync_dir(path: &Path) {
    if let Ok(dir) = File::open(path) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) {}

/// A [`CredentialStore`] backed by one private plaintext JSON file.
pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    /// Uses the file at `path`; nothing is touched until the first operation.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl fmt::Debug for FileCredentialStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileCredentialStore")
            .finish_non_exhaustive()
    }
}

impl CredentialStore for FileCredentialStore {
    fn save(&mut self, credential: AccessCredential) -> Result<(), AppError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let bytes = encode(&credential)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(credential_store_unavailable)?;
        create_private_dir(parent).map_err(|_| credential_store_unavailable())?;
        let temporary = parent.join(format!(
            ".credentials.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let written = write_private_file(&temporary, &bytes)
            .and_then(|()| fs::rename(&temporary, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(credential_store_unavailable());
        }
        sync_dir(parent);
        Ok(())
    }

    fn load(&self) -> Result<Option<AccessCredential>, AppError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(credential_store_unavailable()),
        };
        // Check the opened file, not a separate path lookup.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = file
                .metadata()
                .map_err(|_| credential_store_unavailable())?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(credential_store_unavailable());
            }
        }
        let mut bytes = Vec::new();
        file.take((MAX_FILE_LENGTH + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| credential_store_unavailable())?;
        decode(&bytes).map(Some)
    }

    /// Locks a private, empty sibling `<file>.lock`, never the credential file
    /// itself, which atomic saves replace. The lock is the operating system's
    /// exclusive advisory file lock and is released when the guard is dropped,
    /// including when the process exits.
    fn lock_for_refresh(&self) -> Result<CredentialRefreshGuard, AppError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(credential_store_unavailable)?;
        let mut name = self
            .path
            .file_name()
            .ok_or_else(credential_store_unavailable)?
            .to_os_string();
        name.push(".lock");
        create_private_dir(parent).map_err(|_| credential_store_unavailable())?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(parent.join(name))
            .map_err(|_| credential_store_unavailable())?;
        file.lock().map_err(|_| credential_store_unavailable())?;
        Ok(CredentialRefreshGuard::new(file))
    }

    fn delete(&mut self) -> Result<(), AppError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            // Deleting an absent credential is a successful no-op.
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(_) => Err(credential_store_unavailable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS: &str = "CANARY-ACCESS-TOKEN-7d1f";
    const REFRESH: &str = "CANARY-REFRESH-TOKEN-91ab";

    // Synthetic fixtures only; never touch the real user configuration.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ghostlink-credential-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn path(&self) -> PathBuf {
            self.0.join("ghostlink").join("credentials.json")
        }

        fn store(&self) -> FileCredentialStore {
            FileCredentialStore::new(self.path())
        }

        fn write_raw(&self, bytes: &[u8]) {
            std::fs::create_dir_all(self.path().parent().unwrap()).unwrap();
            std::fs::write(self.path(), bytes).unwrap();
            restrict(&self.path());
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_group_or_other_writable_directory_is_refused_for_saves_and_locks() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o770, 0o707, 0o777] {
            let fixture = Fixture::new();
            let dir = fixture.path().parent().unwrap().to_path_buf();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
            let error = fixture.store().save(with_grant()).unwrap_err();
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert!(fixture.store().lock_for_refresh().is_err(), "{mode:o}");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "{mode:o}");
        }
        // A common existing 0755 directory stays usable.
        let fixture = Fixture::new();
        let dir = fixture.path().parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        fixture.store().save(with_grant()).unwrap();
        assert!(fixture.store().lock_for_refresh().is_ok());
    }

    #[cfg(unix)]
    fn restrict(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(not(unix))]
    fn restrict(_path: &Path) {}

    fn access_only() -> AccessCredential {
        AccessCredential {
            metadata: AccessCredentialMetadata {
                expires_at: Timestamp::new(1_760_000_000),
            },
            material: CredentialMaterial::from_bytes(ACCESS.as_bytes().to_vec()),
            refresh_grant: None,
        }
    }

    fn with_grant() -> AccessCredential {
        AccessCredential {
            refresh_grant: Some(RefreshGrant {
                token: RefreshToken::from_bytes(REFRESH.as_bytes().to_vec()).unwrap(),
                expires_at: Timestamp::new(1_767_776_000),
            }),
            ..access_only()
        }
    }

    fn assert_same(left: &AccessCredential, right: &AccessCredential) {
        assert_eq!(left.metadata.expires_at, right.metadata.expires_at);
        assert_eq!(
            left.material.expose_secret(),
            right.material.expose_secret()
        );
        match (left.refresh_grant(), right.refresh_grant()) {
            (None, None) => {}
            (Some(left), Some(right)) => {
                assert_eq!(left.token.as_bytes(), right.token.as_bytes());
                assert_eq!(left.expires_at, right.expires_at);
            }
            _ => panic!("refresh grant presence differs"),
        }
    }

    fn assert_category(
        result: Result<Option<AccessCredential>, AppError>,
        category: ErrorCategory,
    ) {
        let error = result.expect_err("load must fail");
        assert_eq!(error.category(), category);
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains("CANARY"), "{rendered}");
    }

    #[test]
    fn missing_file_loads_as_none() {
        let fixture = Fixture::new();
        assert!(fixture.store().load().unwrap().is_none());
    }

    #[test]
    fn round_trips_with_and_without_refresh_grant() {
        for credential in [access_only(), with_grant()] {
            let fixture = Fixture::new();
            let mut store = fixture.store();
            store.save(credential.clone()).unwrap();
            assert_same(&store.load().unwrap().unwrap(), &credential);
        }
    }

    #[test]
    fn second_save_replaces_the_first_and_leaves_no_temporary_files() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store.save(with_grant()).unwrap();
        store.save(access_only()).unwrap();
        assert_same(&store.load().unwrap().unwrap(), &access_only());
        let entries: Vec<_> = std::fs::read_dir(fixture.path().parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("credentials.json")]);
    }

    #[test]
    fn stored_file_is_documented_plain_json() {
        let fixture = Fixture::new();
        fixture.store().save(with_grant()).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.path()).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "version": 1,
                "access_token": ACCESS,
                "access_expires_at": 1_760_000_000u64,
                "refresh_token": REFRESH,
                "refresh_expires_at": 1_767_776_000u64,
            })
        );
    }

    #[test]
    fn access_only_file_omits_refresh_fields() {
        let fixture = Fixture::new();
        fixture.store().save(access_only()).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.path()).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "version": 1,
                "access_token": ACCESS,
                "access_expires_at": 1_760_000_000u64,
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn new_directory_is_0700_and_file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fixture.store().save(with_grant()).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(fixture.path().parent().unwrap()), 0o700);
        assert_eq!(mode(&fixture.path()), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn group_or_world_readable_file_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fixture.store().save(with_grant()).unwrap();
        std::fs::set_permissions(fixture.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_category(
            fixture.store().load(),
            ErrorCategory::CredentialStoreUnavailable,
        );
    }

    #[test]
    fn malformed_files_fail_closed_without_leaking_contents() {
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"CANARY not json".to_vec(),
            b"[\"CANARY\"]".to_vec(),
            br#"{"version":2,"access_token":"CANARY","access_expires_at":1}"#.to_vec(),
            br#"{"access_token":"CANARY","access_expires_at":1}"#.to_vec(),
            br#"{"version":1,"access_expires_at":1}"#.to_vec(),
            br#"{"version":1,"access_token":"","access_expires_at":1}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY"}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY","access_expires_at":-1}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY","access_expires_at":1,"refresh_token":"CANARY"}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY","access_expires_at":1,"refresh_expires_at":2}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY","access_expires_at":1,"refresh_token":"","refresh_expires_at":2}"#.to_vec(),
            br#"{"version":1,"access_token":"CANARY","access_expires_at":1,"extra":"CANARY"}"#.to_vec(),
            format!(
                r#"{{"version":1,"access_token":"{}","access_expires_at":1}}"#,
                "C".repeat(5 * 1024)
            )
            .into_bytes(),
            vec![b' '; 65 * 1024],
        ];
        for bytes in cases {
            let fixture = Fixture::new();
            fixture.write_raw(&bytes);
            assert_category(
                fixture.store().load(),
                ErrorCategory::CredentialStoreUnavailable,
            );
        }
    }

    #[test]
    fn legacy_keyring_record_bytes_fail_closed() {
        let fixture = Fixture::new();
        fixture.write_raw(b"GLCR\x02CANARY");
        assert_category(
            fixture.store().load(),
            ErrorCategory::CredentialStoreUnavailable,
        );
    }

    #[test]
    fn oversized_or_non_utf8_tokens_are_refused_before_writing() {
        let oversized = AccessCredential {
            material: CredentialMaterial::from_bytes(vec![b'A'; 4 * 1024 + 1]),
            ..access_only()
        };
        let binary = AccessCredential {
            material: CredentialMaterial::from_bytes(vec![0xff, 0xfe]),
            ..access_only()
        };
        for credential in [oversized, binary] {
            let fixture = Fixture::new();
            let error = fixture.store().save(credential).unwrap_err();
            assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
            assert!(!fixture.path().exists());
        }
    }

    #[test]
    fn delete_removes_the_file_and_is_idempotent() {
        let fixture = Fixture::new();
        let mut store = fixture.store();
        store.save(with_grant()).unwrap();
        store.delete().unwrap();
        assert!(!fixture.path().exists());
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn unwritable_location_fails_closed() {
        let fixture = Fixture::new();
        // A regular file where the parent directory should be.
        std::fs::create_dir_all(&fixture.0).unwrap();
        std::fs::write(fixture.0.join("ghostlink"), b"not a directory").unwrap();
        let error = fixture.store().save(with_grant()).unwrap_err();
        assert_eq!(error.category(), ErrorCategory::CredentialStoreUnavailable);
    }

    #[test]
    fn refresh_lock_excludes_a_second_holder_until_released() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let fixture = Fixture::new();
        let first = fixture.store().lock_for_refresh().unwrap();
        let acquired = Arc::new(AtomicBool::new(false));
        let waiter = {
            // A separately opened lock file, as another process would open it.
            let store = fixture.store();
            let acquired = acquired.clone();
            std::thread::spawn(move || {
                let _second = store.lock_for_refresh().unwrap();
                acquired.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!acquired.load(Ordering::SeqCst));
        drop(first);
        waiter.join().unwrap();
        assert!(acquired.load(Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[test]
    fn refresh_lock_file_is_private_and_holds_no_credential() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        drop(fixture.store().lock_for_refresh().unwrap());
        let lock = fixture.path().with_file_name("credentials.json.lock");
        let metadata = std::fs::metadata(&lock).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(metadata.len(), 0);
        let dir = std::fs::metadata(fixture.path().parent().unwrap()).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn debug_never_renders_the_path_or_tokens() {
        let fixture = Fixture::new();
        let rendered = format!("{:?}", fixture.store());
        assert_eq!(rendered, "FileCredentialStore { .. }");
    }
}
