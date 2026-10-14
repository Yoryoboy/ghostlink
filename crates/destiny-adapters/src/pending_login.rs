//! Pending headless login session adapter.
//!
//! `ghostlink login start` and `ghostlink login complete` run in different
//! processes, so the authorization the first one began is persisted as one small
//! JSON file, `pending-login.json`, next to the credential file:
//!
//! ```json
//! { "version": 1, "state": "...", "expires_at": 1760000300 }
//! ```
//!
//! The state is the unguessable CSRF value that binds a callback to the login
//! that initiated it; it is never rendered. The file follows the credential
//! file's rules: `0700` parent and `0600` file on Unix, atomic temporary file
//! plus rename, a file that group or others may access fails closed, parsing is
//! strict and bounded, and every failure maps to
//! [`ErrorCategory::LocalStorage`] without rendering the path or the contents.
//! Saving replaces any earlier pending session; deleting an absent one succeeds.
//! Completion claims the session with [`PendingLoginStore::take`], an atomic
//! rename, so only one process can ever use it.

use destiny_core::{AppError, AuthorizationState, ErrorCategory, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The file layout written and accepted by this build.
const FILE_VERSION: u8 = 1;

/// File name of the pending session, a sibling of the credential file.
const FILE_NAME: &str = "pending-login.json";

/// Strict upper bound on the stored state.
const MAX_STATE_LENGTH: usize = 256;

/// Strict upper bound on the whole file, read with one byte of slack.
const MAX_FILE_LENGTH: usize = 4 * 1024;

fn storage_failure() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// One authorization begun by `login start` and not yet completed.
///
/// It renders no state, so it cannot leak through `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct PendingLogin {
    pub state: AuthorizationState,
    pub expires_at: Timestamp,
}

impl fmt::Debug for PendingLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingLogin")
            .finish_non_exhaustive()
    }
}

/// The on-disk shape. It derives no `Debug`, so its state cannot be rendered.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingFile {
    version: u8,
    state: String,
    expires_at: u64,
}

/// Accepts only a nonempty, bounded, printable ASCII state; generated states are hex.
fn state_text(bytes: &[u8]) -> Result<String, AppError> {
    if bytes.is_empty() || bytes.len() > MAX_STATE_LENGTH || !bytes.iter().all(u8::is_ascii_graphic)
    {
        return Err(storage_failure());
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| storage_failure())
}

fn encode(pending: &PendingLogin) -> Result<Vec<u8>, AppError> {
    let file = PendingFile {
        version: FILE_VERSION,
        state: state_text(pending.state.as_bytes())?,
        expires_at: pending.expires_at.seconds(),
    };
    let mut bytes = serde_json::to_vec_pretty(&file).map_err(|_| storage_failure())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Result<PendingLogin, AppError> {
    if bytes.len() > MAX_FILE_LENGTH {
        return Err(storage_failure());
    }
    // A parse error is discarded whole: it is never rendered into an error.
    let file: PendingFile = serde_json::from_slice(bytes).map_err(|_| storage_failure())?;
    if file.version != FILE_VERSION {
        return Err(storage_failure());
    }
    Ok(PendingLogin {
        state: AuthorizationState::from_bytes(state_text(file.state.as_bytes())?.into_bytes()),
        expires_at: Timestamp::new(file.expires_at),
    })
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
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

/// The pending login file, one per credential file.
pub struct PendingLoginStore {
    path: PathBuf,
}

impl PendingLoginStore {
    /// Uses the file at `path`; nothing is touched until the first operation.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The pending file in the same directory as the credential file.
    pub fn beside(credentials_file: &Path) -> Result<Self, AppError> {
        credentials_file
            .parent()
            .map(|directory| Self::new(directory.join(FILE_NAME)))
            .ok_or_else(storage_failure)
    }

    /// Persists `pending` atomically, replacing any earlier pending session.
    pub fn save(&self, pending: &PendingLogin) -> Result<(), AppError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let bytes = encode(pending)?;
        let parent = self.path.parent().ok_or_else(storage_failure)?;
        create_private_dir(parent).map_err(|_| storage_failure())?;
        let temporary = parent.join(format!(
            ".pending-login.{}.{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let written = write_private_file(&temporary, &bytes)
            .and_then(|()| fs::rename(&temporary, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
            return Err(storage_failure());
        }
        Ok(())
    }

    /// Reads the pending session; an absent file is `None`.
    pub fn load(&self) -> Result<Option<PendingLogin>, AppError> {
        read_pending(&self.path)
    }

    /// Claims the pending session exactly once and removes it.
    ///
    /// The file is first renamed to a name unique to this claim, which is
    /// atomic: of two processes completing the same login, only one obtains the
    /// session and the other sees `None`. The claimed file is then read and
    /// removed whether or not it is valid, so a session is never usable twice.
    pub fn take(&self) -> Result<Option<PendingLogin>, AppError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = self.path.parent().ok_or_else(storage_failure)?;
        let claimed = parent.join(format!(
            ".pending-login.{}.{}.claimed",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::rename(&self.path, &claimed) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_failure()),
        }
        let pending = read_pending(&claimed);
        let removed = fs::remove_file(&claimed);
        let pending = pending?;
        removed.map_err(|_| storage_failure())?;
        Ok(pending)
    }

    /// Removes the pending session and reports whether one existed.
    pub fn delete(&self) -> Result<bool, AppError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(storage_failure()),
        }
    }
}

/// Reads one pending file; an absent file is `None`.
fn read_pending(path: &Path) -> Result<Option<PendingLogin>, AppError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(storage_failure()),
    };
    // Check the opened file, not a separate path lookup.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = file.metadata().map_err(|_| storage_failure())?;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(storage_failure());
        }
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE_LENGTH + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| storage_failure())?;
    decode(&bytes).map(Some)
}

impl fmt::Debug for PendingLoginStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingLoginStore")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "CANARY-STATE-5c2e";

    // Synthetic fixtures only; never touch the real user configuration.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "ghostlink-pending-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn credentials(&self) -> PathBuf {
            self.0.join("ghostlink").join("credentials.json")
        }

        fn path(&self) -> PathBuf {
            self.0.join("ghostlink").join("pending-login.json")
        }

        fn store(&self) -> PendingLoginStore {
            PendingLoginStore::beside(&self.credentials()).unwrap()
        }

        fn write_raw(&self, bytes: &[u8], mode: u32) {
            fs::create_dir_all(self.path().parent().unwrap()).unwrap();
            fs::write(self.path(), bytes).unwrap();
            set_mode(&self.path(), mode);
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(not(unix))]
    fn set_mode(_path: &Path, _mode: u32) {}

    fn pending(state: &str, expires_at: u64) -> PendingLogin {
        PendingLogin {
            state: AuthorizationState::from_bytes(state.as_bytes()),
            expires_at: Timestamp::new(expires_at),
        }
    }

    fn assert_sanitized(error: &AppError) {
        assert_eq!(error.category(), ErrorCategory::LocalStorage);
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(STATE));
        assert!(!rendered.contains("ghostlink-pending-test"));
    }

    #[test]
    fn the_pending_file_sits_beside_the_credential_file() {
        let fixture = Fixture::new();
        fixture
            .store()
            .save(&pending(STATE, 1_760_000_300))
            .unwrap();
        assert!(fixture.path().is_file());
        assert!(!fixture.credentials().exists());
    }

    #[test]
    fn a_saved_session_round_trips_in_the_documented_format() {
        let fixture = Fixture::new();
        fixture
            .store()
            .save(&pending(STATE, 1_760_000_300))
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.path()).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"version":1,"state":STATE,"expires_at":1_760_000_300u64})
        );
        assert_eq!(
            fixture.store().load().unwrap(),
            Some(pending(STATE, 1_760_000_300))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_and_a_new_parent_is_0700() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        fixture.store().save(&pending(STATE, 1)).unwrap();
        let file = fs::metadata(fixture.path()).unwrap().permissions().mode();
        let dir = fs::metadata(fixture.path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file & 0o777, 0o600);
        assert_eq!(dir & 0o777, 0o700);
    }

    #[test]
    fn a_missing_file_loads_as_none_and_delete_is_idempotent() {
        let fixture = Fixture::new();
        assert_eq!(fixture.store().load().unwrap(), None);
        fixture.store().delete().unwrap();
        fixture.store().save(&pending(STATE, 1)).unwrap();
        fixture.store().delete().unwrap();
        fixture.store().delete().unwrap();
        assert_eq!(fixture.store().load().unwrap(), None);
    }

    #[test]
    fn delete_reports_whether_a_session_was_removed() {
        let fixture = Fixture::new();
        assert!(!fixture.store().delete().unwrap());
        fixture.store().save(&pending(STATE, 1)).unwrap();
        assert!(fixture.store().delete().unwrap());
        assert!(!fixture.store().delete().unwrap());
    }

    #[test]
    fn take_claims_the_session_exactly_once_and_leaves_no_file() {
        let fixture = Fixture::new();
        assert_eq!(fixture.store().take().unwrap(), None);
        fixture.store().save(&pending(STATE, 7)).unwrap();
        assert_eq!(fixture.store().take().unwrap(), Some(pending(STATE, 7)));
        assert_eq!(fixture.store().take().unwrap(), None);
        assert_eq!(fixture.store().load().unwrap(), None);
        let names: Vec<_> = fs::read_dir(fixture.path().parent().unwrap())
            .unwrap()
            .collect();
        assert!(names.is_empty(), "no claimed file is left behind");
    }

    #[test]
    fn take_removes_an_invalid_session_and_fails_closed() {
        let fixture = Fixture::new();
        fixture.write_raw(b"not json", 0o600);
        assert_sanitized(&fixture.store().take().unwrap_err());
        assert_eq!(fixture.store().load().unwrap(), None);
        assert_eq!(
            fs::read_dir(fixture.path().parent().unwrap())
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn take_refuses_a_group_or_other_accessible_session() {
        let fixture = Fixture::new();
        fixture.write_raw(
            format!(r#"{{"version":1,"state":"{STATE}","expires_at":1}}"#).as_bytes(),
            0o644,
        );
        assert_sanitized(&fixture.store().take().unwrap_err());
    }

    #[test]
    fn saving_replaces_the_earlier_session_and_leaves_no_temporary_file() {
        let fixture = Fixture::new();
        fixture.store().save(&pending("first-state", 10)).unwrap();
        fixture.store().save(&pending("second-state", 20)).unwrap();
        assert_eq!(
            fixture.store().load().unwrap(),
            Some(pending("second-state", 20))
        );
        let names: Vec<_> = fs::read_dir(fixture.path().parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["pending-login.json"]);
    }

    #[test]
    fn malformed_oversized_or_wrong_version_files_fail_closed_without_leaking() {
        let oversized = format!(
            r#"{{"version":1,"state":"{STATE}","expires_at":1,"pad":"{}"}}"#,
            "x".repeat(MAX_FILE_LENGTH)
        );
        let long_state = format!(
            r#"{{"version":1,"state":"{}","expires_at":1}}"#,
            "a".repeat(MAX_STATE_LENGTH + 1)
        );
        let cases = [
            "not json".to_owned(),
            format!(r#"{{"version":2,"state":"{STATE}","expires_at":1}}"#),
            format!(r#"{{"version":1,"state":"{STATE}"}}"#),
            format!(r#"{{"version":1,"state":"{STATE}","expires_at":1,"extra":true}}"#),
            r#"{"version":1,"state":"","expires_at":1}"#.to_owned(),
            format!(r#"{{"version":1,"state":"{STATE} spaced","expires_at":1}}"#),
            format!(r#"{{"version":1,"state":"{STATE}","expires_at":-1}}"#),
            oversized,
            long_state,
        ];
        for case in cases {
            let fixture = Fixture::new();
            fixture.write_raw(case.as_bytes(), 0o600);
            let error = fixture.store().load().unwrap_err();
            assert_sanitized(&error);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_group_or_other_accessible_file_fails_closed() {
        let fixture = Fixture::new();
        fixture.write_raw(
            format!(r#"{{"version":1,"state":"{STATE}","expires_at":1}}"#).as_bytes(),
            0o644,
        );
        assert_sanitized(&fixture.store().load().unwrap_err());
    }

    #[test]
    fn an_unwritable_location_fails_sanitized() {
        let fixture = Fixture::new();
        fixture.write_raw(b"x", 0o600);
        // The credential "directory" is a regular file, so the parent cannot be created.
        let blocked = PendingLoginStore::new(fixture.path().join("nested").join(FILE_NAME));
        assert_sanitized(&blocked.save(&pending(STATE, 1)).unwrap_err());
    }

    #[test]
    fn debug_output_never_renders_the_state_or_the_path() {
        let fixture = Fixture::new();
        let rendered = format!("{:?} {:?}", pending(STATE, 1), fixture.store());
        assert!(!rendered.contains(STATE));
        assert!(!rendered.contains("ghostlink-pending-test"));
    }
}
