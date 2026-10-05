//! Linux registration of the `ghostlink://` callback handler.
//!
//! The registration is the freedesktop one the operator runbook
//! (`packaging/ghostlink-callback-handler.md`) describes: a user desktop entry
//! `ghostlink-callback.desktop` whose `Exec` runs this executable's `callback`
//! command, selected as the default `x-scheme-handler/ghostlink` through
//! `xdg-mime`. The handler that was selected before the first install is kept
//! in a private record so uninstalling restores it instead of guessing.
//!
//! Every file location and every external command is injected, so the adapter
//! is tested without touching the real desktop configuration. Two known
//! pitfalls are detected and reported, never repaired automatically:
//! contradictory `mimeapps.list` sections and a remembered
//! `xdg-desktop-portal` "open with" choice.

use std::ffi::OsString;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use destiny_core::uri_handler::{
    HandlerChanges, HandlerRegistration, HandlerStatus, HandlerWarning, HandlerWarningKind,
    UriHandlerRegistrar,
};
use destiny_core::{AppError, ErrorCategory};

/// The desktop id of the callback handler entry.
pub const DESKTOP_ID: &str = "ghostlink-callback.desktop";

/// The portal desktop id, which is the desktop id without `.desktop`.
const PORTAL_ID: &str = "ghostlink-callback";

const SCHEME_MIME: &str = "x-scheme-handler/ghostlink";

/// Upper bound for command output and for every file this adapter reads.
const READ_LIMIT: usize = 256 * 1024;

const PORTAL_STORE: [&str; 4] = [
    "--user",
    "call",
    "org.freedesktop.impl.portal.PermissionStore",
    "/org/freedesktop/impl/portal/PermissionStore",
];
const PORTAL_INTERFACE: &str = "org.freedesktop.impl.portal.PermissionStore";

/// What one external command produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    /// Whether the command exited successfully.
    pub success: bool,
    /// The bounded standard output.
    pub stdout: String,
}

/// Runs the desktop tools the registration needs.
pub trait HandlerCommandRunner {
    /// Runs `program` with `args`.
    ///
    /// Fails with `UnsupportedOperation` only when the program cannot be
    /// started; a started program that fails is reported through
    /// [`CommandOutput::success`].
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, AppError>;
}

/// The production runner: no standard input, discarded standard error.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemHandlerCommandRunner;

impl HandlerCommandRunner for SystemHandlerCommandRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, AppError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| AppError::new(ErrorCategory::UnsupportedOperation))?;
        let mut stdout = Vec::new();
        let read = child
            .stdout
            .take()
            .map(|pipe| pipe.take(READ_LIMIT as u64 + 1).read_to_end(&mut stdout));
        let status = child.wait().map_err(|_| local_storage())?;
        if !matches!(read, Some(Ok(_))) || stdout.len() > READ_LIMIT {
            return Err(local_storage());
        }
        let stdout = String::from_utf8(stdout).map_err(|_| local_storage())?;
        Ok(CommandOutput {
            success: status.success(),
            stdout,
        })
    }
}

/// The per-user locations the registration reads and writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxHandlerPaths {
    /// The user applications directory that holds the desktop entry.
    pub applications_dir: PathBuf,
    /// The private record of the handler selected before the first install.
    pub record_file: PathBuf,
    /// The user `mimeapps.list`.
    pub mimeapps_list: PathBuf,
}

impl LinuxHandlerPaths {
    /// Resolves the locations from the XDG base directories.
    ///
    /// An absolute `XDG_DATA_HOME` or `XDG_CONFIG_HOME` wins; otherwise the
    /// `$HOME/.local/share` and `$HOME/.config` defaults apply. Relative values
    /// are ignored, as the XDG specification requires.
    pub fn resolve(lookup: impl Fn(&str) -> Option<OsString>) -> Result<Self, AppError> {
        let absolute = |key: &str| {
            lookup(key)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home = absolute("HOME");
        let base = |key: &str, fallback: &str| {
            absolute(key)
                .or_else(|| home.as_ref().map(|home| home.join(fallback)))
                .ok_or_else(|| AppError::new(ErrorCategory::ConfigurationError))
        };
        let data = base("XDG_DATA_HOME", ".local/share")?;
        let config = base("XDG_CONFIG_HOME", ".config")?;
        Ok(Self {
            applications_dir: data.join("applications"),
            record_file: data.join("ghostlink").join("previous-uri-handler"),
            mimeapps_list: config.join("mimeapps.list"),
        })
    }

    /// Resolves the locations from the process environment.
    pub fn from_env() -> Result<Self, AppError> {
        Self::resolve(|key| std::env::var_os(key))
    }
}

/// The desktop-entry `Exec` value that runs `executable`'s callback command.
///
/// A path with spaces or other reserved characters is double-quoted. Paths
/// that would need escaping inside the quotes (`"`, `` ` ``, `$`, `\`), a
/// literal `%`, control characters, or a relative path are refused rather
/// than escaped, so the entry never depends on the subtle two-level escaping
/// rules of the desktop-entry specification.
pub fn exec_value(executable: &Path) -> Result<String, AppError> {
    let path = executable
        .to_str()
        .filter(|path| path.starts_with('/'))
        .ok_or_else(configuration_error)?;
    let unescapable = |c: char| c.is_control() || matches!(c, '"' | '`' | '$' | '\\' | '%');
    if path.chars().any(unescapable) {
        return Err(configuration_error());
    }
    let reserved = |c: char| {
        matches!(
            c,
            ' ' | '\'' | '>' | '<' | '~' | '|' | '&' | ';' | '*' | '?' | '#' | '(' | ')'
        )
    };
    if path.chars().any(reserved) {
        Ok(format!("\"{path}\" callback %u"))
    } else {
        Ok(format!("{path} callback %u"))
    }
}

/// The freedesktop callback-handler registrar.
pub struct LinuxUriHandlerRegistrar<R> {
    paths: LinuxHandlerPaths,
    executable: PathBuf,
    runner: R,
}

impl LinuxUriHandlerRegistrar<SystemHandlerCommandRunner> {
    /// The registrar for this process's executable and the user's
    /// XDG locations.
    pub fn for_current_executable() -> Result<Self, AppError> {
        let executable = std::env::current_exe().map_err(|_| configuration_error())?;
        Ok(Self::new(
            LinuxHandlerPaths::from_env()?,
            executable,
            SystemHandlerCommandRunner,
        ))
    }
}

impl<R: HandlerCommandRunner> LinuxUriHandlerRegistrar<R> {
    /// A registrar for `executable` using `paths` and `runner`.
    pub fn new(paths: LinuxHandlerPaths, executable: PathBuf, runner: R) -> Self {
        Self {
            paths,
            executable,
            runner,
        }
    }

    fn desktop_file(&self) -> PathBuf {
        self.paths.applications_dir.join(DESKTOP_ID)
    }

    fn desktop_entry(&self) -> Result<String, AppError> {
        Ok(format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Ghostlink OAuth Callback\n\
             Comment=Deliver a ghostlink://oauth/callback URI to a waiting ghostlink login\n\
             Exec={}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             MimeType={SCHEME_MIME};\n\
             Categories=Network;\n",
            exec_value(&self.executable)?
        ))
    }

    /// The desktop id `xdg-mime` currently selects, or an empty string.
    fn default_handler(&mut self) -> Result<String, AppError> {
        let output = self
            .runner
            .run("xdg-mime", &["query", "default", SCHEME_MIME])?;
        if !output.success {
            return Err(local_storage());
        }
        let handler = output.stdout.trim();
        if handler.is_empty() || is_desktop_id(handler) {
            Ok(handler.to_owned())
        } else {
            Err(local_storage())
        }
    }

    fn registration(&mut self) -> Result<HandlerRegistration, AppError> {
        let handler = self.default_handler()?;
        if handler.is_empty() {
            return Ok(HandlerRegistration::Missing);
        }
        if handler != DESKTOP_ID {
            return Ok(HandlerRegistration::Elsewhere { handler });
        }
        let expected = exec_value(&self.executable)?;
        let registration = match read_bounded(&self.desktop_file())? {
            None => HandlerRegistration::Elsewhere {
                handler: format!("{DESKTOP_ID} (desktop entry missing)"),
            },
            Some(entry) if entry_exec(&entry) == Some(expected.as_str()) => {
                HandlerRegistration::Current
            }
            Some(_) => HandlerRegistration::Elsewhere {
                handler: format!("{DESKTOP_ID} (another ghostlink executable)"),
            },
        };
        Ok(registration)
    }

    fn warnings(&mut self) -> Result<Vec<HandlerWarning>, AppError> {
        let mut warnings = Vec::new();
        if let Some(list) = read_bounded(&self.paths.mimeapps_list)? {
            let mut handlers = scheme_lines(&list)
                .filter_map(|value| value.split(';').map(str::trim).find(|id| !id.is_empty()))
                .collect::<Vec<_>>();
            handlers.sort_unstable();
            handlers.dedup();
            if handlers.len() > 1 {
                warnings.push(HandlerWarning {
                    kind: HandlerWarningKind::ConflictingAssociation,
                    summary: format!(
                        "{} names different handlers for {SCHEME_MIME} in different sections: {}.",
                        self.paths.mimeapps_list.display(),
                        handlers.join(", ")
                    ),
                    remedy: format!(
                        "Make every {SCHEME_MIME} line in {} start with {DESKTOP_ID}.",
                        self.paths.mimeapps_list.display()
                    ),
                });
            }
        }
        warnings.extend(self.portal_warnings());
        Ok(warnings)
    }

    /// Remembered portal choices for another application; best effort, since
    /// many desktops run no portal at all.
    fn portal_warnings(&mut self) -> Vec<HandlerWarning> {
        let mut args = PORTAL_STORE.to_vec();
        args.extend([
            PORTAL_INTERFACE,
            "Lookup",
            "ss",
            "desktop-used-apps",
            SCHEME_MIME,
        ]);
        let Ok(output) = self.runner.run("busctl", &args) else {
            return Vec::new();
        };
        if !output.success {
            return Vec::new();
        }
        portal_choices(&output.stdout)
            .into_iter()
            .filter(|(_, desktop)| desktop != PORTAL_ID)
            .map(|(app, desktop)| HandlerWarning {
                kind: HandlerWarningKind::RememberedChoice,
                summary: format!(
                    "xdg-desktop-portal remembers {desktop} for ghostlink:// links opened by {app} and may launch it instead of this handler."
                ),
                remedy: format!(
                    "Run: busctl --user call org.freedesktop.impl.portal.PermissionStore /org/freedesktop/impl/portal/PermissionStore {PORTAL_INTERFACE} SetPermission sbssas desktop-used-apps true {SCHEME_MIME} {app} 3 {PORTAL_ID} 3 3"
                ),
            })
            .collect()
    }

    fn run_required(&mut self, program: &str, args: &[&str]) -> Result<(), AppError> {
        match self.runner.run(program, args)? {
            CommandOutput { success: true, .. } => Ok(()),
            _ => Err(local_storage()),
        }
    }

    /// Refreshes the desktop database; the tool is optional on many desktops.
    fn refresh_database(&mut self) -> String {
        let dir = self.paths.applications_dir.display().to_string();
        match self.runner.run("update-desktop-database", &[&dir]) {
            Ok(CommandOutput { success: true, .. }) => {
                format!("ran: update-desktop-database {dir}")
            }
            _ => format!("skipped: update-desktop-database {dir} (unavailable or failed)"),
        }
    }

    /// Removes this entry from every `mimeapps.list` scheme line; reports
    /// whether the file changed.
    fn strip_mimeapps(&self) -> Result<bool, AppError> {
        let Some(list) = read_bounded(&self.paths.mimeapps_list)? else {
            return Ok(false);
        };
        let mut changed = false;
        let mut kept = String::with_capacity(list.len());
        for line in list.split_inclusive('\n') {
            let Some(value) = scheme_value(line) else {
                kept.push_str(line);
                continue;
            };
            let ids = value
                .split(';')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .collect::<Vec<_>>();
            if !ids.contains(&DESKTOP_ID) {
                kept.push_str(line);
                continue;
            }
            changed = true;
            let others = ids
                .into_iter()
                .filter(|id| *id != DESKTOP_ID)
                .collect::<Vec<_>>();
            if !others.is_empty() {
                kept.push_str(&format!("{SCHEME_MIME}={};\n", others.join(";")));
            }
        }
        if changed {
            replace_file(&self.paths.mimeapps_list, kept.as_bytes())?;
        }
        Ok(changed)
    }
}

impl<R: HandlerCommandRunner> UriHandlerRegistrar for LinuxUriHandlerRegistrar<R> {
    fn status(&mut self) -> Result<HandlerStatus, AppError> {
        Ok(HandlerStatus {
            registration: self.registration()?,
            warnings: self.warnings()?,
        })
    }

    fn install_plan(&mut self) -> Result<HandlerChanges, AppError> {
        let exec = exec_value(&self.executable)?;
        let mut changes = Vec::new();
        if !exists(&self.paths.record_file)? {
            let previous = previous_for_record(self.default_handler()?);
            changes.push(format!(
                "record the current handler ({}) in {}",
                describe_handler(&previous),
                self.paths.record_file.display()
            ));
        }
        changes.push(format!(
            "write {} with Exec={exec}",
            self.desktop_file().display()
        ));
        changes.push(format!(
            "run: update-desktop-database {}",
            self.paths.applications_dir.display()
        ));
        changes.push(format!("run: xdg-mime default {DESKTOP_ID} {SCHEME_MIME}"));
        Ok(HandlerChanges { changes })
    }

    fn install(&mut self) -> Result<HandlerChanges, AppError> {
        let entry = self.desktop_entry()?;
        let mut changes = Vec::new();
        if !exists(&self.paths.record_file)? {
            let previous = previous_for_record(self.default_handler()?);
            write_private_record(&self.paths.record_file, &previous)?;
            changes.push(format!(
                "recorded the current handler ({}) in {}",
                describe_handler(&previous),
                self.paths.record_file.display()
            ));
        }
        fs::create_dir_all(&self.paths.applications_dir).map_err(|_| local_storage())?;
        replace_file(&self.desktop_file(), entry.as_bytes())?;
        changes.push(format!("wrote {}", self.desktop_file().display()));
        changes.push(self.refresh_database());
        self.run_required("xdg-mime", &["default", DESKTOP_ID, SCHEME_MIME])?;
        changes.push(format!("ran: xdg-mime default {DESKTOP_ID} {SCHEME_MIME}"));
        Ok(HandlerChanges { changes })
    }

    fn uninstall(&mut self) -> Result<HandlerChanges, AppError> {
        let previous = match read_bounded(&self.paths.record_file)? {
            None => None,
            Some(record) => {
                let previous = record.trim().to_owned();
                if !previous.is_empty() && !is_desktop_id(&previous) {
                    return Err(configuration_error());
                }
                Some(previous)
            }
        };
        let mut changes = Vec::new();
        if self.strip_mimeapps()? {
            changes.push(format!(
                "removed {DESKTOP_ID} from {SCHEME_MIME} in {}",
                self.paths.mimeapps_list.display()
            ));
        }
        if let Some(previous) = previous.as_deref().filter(|id| !id.is_empty()) {
            self.run_required("xdg-mime", &["default", previous, SCHEME_MIME])?;
            changes.push(format!("ran: xdg-mime default {previous} {SCHEME_MIME}"));
        }
        let desktop_file = self.desktop_file();
        if exists(&desktop_file)? {
            fs::remove_file(&desktop_file).map_err(|_| local_storage())?;
            changes.push(format!("removed {}", desktop_file.display()));
            changes.push(self.refresh_database());
        }
        if previous.is_some() {
            fs::remove_file(&self.paths.record_file).map_err(|_| local_storage())?;
            changes.push(format!("removed {}", self.paths.record_file.display()));
        }
        Ok(HandlerChanges { changes })
    }
}

fn configuration_error() -> AppError {
    AppError::new(ErrorCategory::ConfigurationError)
}

fn local_storage() -> AppError {
    AppError::new(ErrorCategory::LocalStorage)
}

/// A plain desktop id: `[A-Za-z0-9._-]+.desktop`, not starting with `-`.
fn is_desktop_id(value: &str) -> bool {
    value.len() > ".desktop".len()
        && value.ends_with(".desktop")
        && !value.starts_with('-')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Restoring to this entry after uninstalling it would be meaningless.
fn previous_for_record(handler: String) -> String {
    if handler == DESKTOP_ID {
        String::new()
    } else {
        handler
    }
}

fn describe_handler(handler: &str) -> &str {
    if handler.is_empty() { "none" } else { handler }
}

fn exists(path: &Path) -> Result<bool, AppError> {
    path.try_exists().map_err(|_| local_storage())
}

/// Reads a bounded UTF-8 file; a missing file is `None`.
fn read_bounded(path: &Path) -> Result<Option<String>, AppError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(local_storage()),
    };
    let mut bytes = Vec::new();
    file.take(READ_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| local_storage())?;
    if bytes.len() > READ_LIMIT {
        return Err(local_storage());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| local_storage())
}

/// Replaces `path` atomically, keeping the permissions of an existing file.
fn replace_file(path: &Path, contents: &[u8]) -> Result<(), AppError> {
    let name = path
        .file_name()
        .ok_or_else(local_storage)?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.ghostlink-tmp"));
    let permissions = fs::metadata(path).ok().map(|meta| meta.permissions());
    let written = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Some(permissions) = permissions {
            fs::set_permissions(&temporary, permissions)?;
        }
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(local_storage());
    }
    Ok(())
}

/// Creates the previous-handler record owner-only and never overwrites one.
fn write_private_record(path: &Path, handler: &str) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(local_storage)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent).map_err(|_| local_storage())?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| local_storage())?;
    file.write_all(format!("{handler}\n").as_bytes())
        .map_err(|_| local_storage())
}

/// The value of an `x-scheme-handler/ghostlink=` line, if `line` is one.
fn scheme_value(line: &str) -> Option<&str> {
    let (key, value) = line.split_once('=')?;
    (key.trim() == SCHEME_MIME).then(|| value.trim())
}

fn scheme_lines(list: &str) -> impl Iterator<Item = &str> {
    list.lines().filter_map(scheme_value)
}

/// The `Exec` value of a desktop entry.
fn entry_exec(entry: &str) -> Option<&str> {
    entry
        .lines()
        .find_map(|line| line.strip_prefix("Exec="))
        .map(str::trim)
}

/// Parses a `busctl` `Lookup` reply, `a{sas}v N ("app" K "id" …)… <variant>`,
/// into `(application id, remembered desktop id)` pairs. Entries with an
/// application or desktop id outside a plain identifier alphabet are dropped,
/// because they are rendered into a suggested command.
fn portal_choices(reply: &str) -> Vec<(String, String)> {
    let tokens = busctl_tokens(reply);
    let mut tokens = tokens.iter().map(String::as_str);
    if tokens.next() != Some("a{sas}v") {
        return Vec::new();
    }
    let Some(entries) = tokens.next().and_then(|count| count.parse::<usize>().ok()) else {
        return Vec::new();
    };
    let plain = |value: &str| {
        !value.is_empty()
            && !value.starts_with('-')
            && value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    let mut choices = Vec::new();
    for _ in 0..entries {
        let (Some(app), Some(count)) = (tokens.next(), tokens.next()) else {
            break;
        };
        let Ok(count) = count.parse::<usize>() else {
            break;
        };
        let values = tokens.by_ref().take(count).collect::<Vec<_>>();
        if let Some(desktop) = values.first()
            && plain(app)
            && plain(desktop)
        {
            choices.push((app.to_owned(), (*desktop).to_owned()));
        }
    }
    choices
}

/// Splits `busctl` output into whitespace-separated tokens, unquoting
/// double-quoted strings.
fn busctl_tokens(reply: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = reply.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut token = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => token.extend(chars.next()),
                    c => token.push(c),
                }
            }
            tokens.push(token);
        } else {
            let mut token = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
            tokens.push(token);
        }
    }
    tokens
}

// The fixtures use Unix absolute paths.
#[cfg(all(test, unix))]
#[path = "uri_handler_tests.rs"]
mod tests;
