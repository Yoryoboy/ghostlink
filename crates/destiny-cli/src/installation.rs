//! Fail-closed official installation validation, shared by lifecycle commands.
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

const RECEIPT_NAME: &str = "ghostlink-receipt.json";
const MANUAL: &str = "No supported official installation was validated. If installed with Cargo, rebuild/reinstall with Cargo; if managed by a package manager, use that manager. For a manual copy, download the official release or use its installer. Do not overwrite a managed installation.";

#[derive(Clone, Copy, Debug)]
pub enum InstallationError {
    Invalid,
    MissingUpdater,
    Busy,
    LocalIo,
}
impl InstallationError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "unsupported_installation",
            Self::MissingUpdater => "updater_missing",
            Self::Busy => "lifecycle_busy",
            Self::LocalIo => "local_storage",
        }
    }
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Invalid | Self::MissingUpdater => 40,
            Self::Busy => 43,
            Self::LocalIo => 51,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Invalid => MANUAL,
            Self::MissingUpdater => {
                "The official sibling ghostlink-update is missing or unsafe. Reinstall using the official release installer; do not substitute an updater from PATH."
            }
            Self::Busy => {
                "Another lifecycle command holds this installation's lock, or its lock file is unsafe. Wait for the command to finish; inspect foreign lock files manually, never delete them automatically."
            }
            Self::LocalIo => {
                "Installation files could not be accessed safely. Check ownership and permissions; no automatic elevation is attempted."
            }
        }
    }
}

/// Match axoupdater 0.10.0 precedence: presence of a Unicode working-dir
/// override wins, CONFIG_PATH names a directory (not the receipt itself), then
/// existing XDG app directory, then HOME/.config. First existing receipt wins.
/// Injection keeps tests isolated from the operator's actual configuration.
pub fn receipt_path(
    mut env: impl FnMut(&str) -> Option<OsString>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, InstallationError> {
    let unicode = |value: Option<OsString>| value.and_then(|s| s.into_string().ok());
    let prefixes = if unicode(env("AXOUPDATER_CONFIG_WORKING_DIR")).is_some() {
        vec![cwd.to_path_buf()]
    } else if let Some(path) = unicode(env("AXOUPDATER_CONFIG_PATH")) {
        vec![cwd.join(path)]
    } else {
        let mut prefixes = Vec::new();
        if let Some(path) = unicode(env("XDG_CONFIG_HOME")) {
            let app = cwd.join(path).join("ghostlink");
            if app.exists() {
                prefixes.push(app);
            }
        }
        if let Some(home) = home {
            prefixes.push(home.join(".config/ghostlink"));
        }
        prefixes
    };
    prefixes
        .into_iter()
        .map(|p| p.join(RECEIPT_NAME))
        .find(|p| p.exists())
        .ok_or(InstallationError::Invalid)
}

/// Reject lexical ambiguity and symlinks in every component, not just the leaf.
/// This is an ownership boundary, not protection against a hostile local user
/// replacing files after validation.
pub(crate) fn safe_path(path: &Path) -> Result<(), InstallationError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return Err(InstallationError::Invalid);
    }
    let normalized: PathBuf = path.components().collect();
    if normalized.as_os_str() != path.as_os_str() {
        return Err(InstallationError::Invalid);
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        let meta = fs::symlink_metadata(&prefix).map_err(|_| InstallationError::Invalid)?;
        if meta.file_type().is_symlink() {
            return Err(InstallationError::Invalid);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct Installation {
    pub receipt: PathBuf,
    pub root: PathBuf,
    pub bin_dir: PathBuf,
    pub updater: PathBuf,
}
impl Installation {
    /// Only the current cargo-dist 0.32.0 cargo-home layout is supported.
    /// Unknown layouts/providers, extra products, libraries and aliases refuse.
    pub fn validate(receipt: &Path, current_exe: &Path) -> Result<Self, InstallationError> {
        let installation = Self::validate_uninstall(receipt, current_exe)?;
        let updater = &installation.updater;
        safe_path(updater).map_err(|_| InstallationError::MissingUpdater)?;
        let meta = fs::metadata(updater).map_err(|_| InstallationError::MissingUpdater)?;
        if !meta.is_file() {
            return Err(InstallationError::MissingUpdater);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 == 0 {
                return Err(InstallationError::MissingUpdater);
            }
        }
        Ok(installation)
    }

    /// Receipt and products remain mandatory; the updater is optional for removal.
    pub fn validate_uninstall(
        receipt: &Path,
        current_exe: &Path,
    ) -> Result<Self, InstallationError> {
        if receipt.file_name().is_none_or(|name| name != RECEIPT_NAME) {
            return Err(InstallationError::Invalid);
        }
        safe_path(receipt)?;
        if !fs::metadata(receipt)
            .map_err(|_| InstallationError::Invalid)?
            .is_file()
        {
            return Err(InstallationError::Invalid);
        }
        let mut bytes = Vec::new();
        fs::File::open(receipt)
            .map_err(|_| InstallationError::Invalid)?
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| InstallationError::Invalid)?;
        if bytes.len() > 65536 {
            return Err(InstallationError::Invalid);
        }
        let r: Value = serde_json::from_slice(&bytes).map_err(|_| InstallationError::Invalid)?;
        let strings = |key: &str| -> Option<Vec<&str>> {
            r.get(key)?.as_array()?.iter().map(Value::as_str).collect()
        };
        let mut binaries = strings("binaries").ok_or(InstallationError::Invalid)?;
        binaries.sort_unstable();
        if r["source"]
            != serde_json::json!({"release_type":"github", "owner":"Yoryoboy", "name":"ghostlink", "app_name":"ghostlink"})
            || r["provider"]["source"] != "cargo-dist"
            || r["provider"]["version"] != "0.32.0"
            || r["install_layout"] != "cargo-home"
            || binaries != ["destiny-mcp", "ghostlink"]
            || strings("cdylibs") != Some(vec![])
            || strings("cstaticlibs") != Some(vec![])
            || !r["binary_aliases"]
                .as_object()
                .is_some_and(|a| a.is_empty())
            || !r["modify_path"].is_boolean()
            || !r["version"].as_str().is_some_and(|v| !v.is_empty())
        {
            return Err(InstallationError::Invalid);
        }
        let root = PathBuf::from(
            r["install_prefix"]
                .as_str()
                .ok_or(InstallationError::Invalid)?,
        );
        safe_path(&root)?;
        // Old updater prefix-normalization and filesystem-root installations
        // are unsupported; never infer ownership from a directory name alone.
        if root.parent().is_none() || root.file_name().is_none_or(|n| n == "bin") {
            return Err(InstallationError::Invalid);
        }
        let bin_dir = root.join("bin");
        for binary in binaries {
            let path = bin_dir.join(binary);
            safe_path(&path)?;
            if !fs::metadata(&path)
                .map_err(|_| InstallationError::Invalid)?
                .is_file()
            {
                return Err(InstallationError::Invalid);
            }
        }
        safe_path(current_exe)?;
        if current_exe != bin_dir.join("ghostlink") {
            return Err(InstallationError::Invalid);
        }
        let updater = bin_dir.join("ghostlink-update");
        Ok(Self {
            receipt: receipt.to_path_buf(),
            root,
            bin_dir,
            updater,
        })
    }

    /// Kernel lock shared by future lifecycle commands. The persistent file is
    /// never removed; a process exit releases flock without stale-lock guessing.
    #[cfg(target_os = "linux")]
    pub fn lock(&self) -> Result<LifecycleLock, InstallationError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.bin_dir.join(".ghostlink-lifecycle.lock"))
            .map_err(|_| InstallationError::Busy)?;
        if !lock
            .metadata()
            .map_err(|_| InstallationError::Busy)?
            .is_file()
        {
            return Err(InstallationError::Busy);
        }
        // SAFETY: flock only uses the valid descriptor held by `lock`.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(InstallationError::Busy);
        }
        Ok(LifecycleLock(lock))
    }
}

/// Explicit unlock also releases transient fork-inherited descriptors in other
/// threads' children; closing only our descriptor can leave those holding flock.
#[cfg(target_os = "linux")]
pub struct LifecycleLock(fs::File);

#[cfg(target_os = "linux")]
impl Drop for LifecycleLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the descriptor remains valid until after this Drop body.
        let _ = unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
