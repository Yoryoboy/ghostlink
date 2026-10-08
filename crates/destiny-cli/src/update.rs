//! Lifecycle update runs before application runtime composition.
use crate::command::UpdateArgs;
#[cfg(target_os = "linux")]
use crate::installation::InstallationError;
use crate::{OutputMode, ProcessOutput};
use serde_json::json;

pub fn failure(mode: OutputMode, code: &str, exit_code: i32, message: &str) -> ProcessOutput {
    let body = match mode {
        OutputMode::Json => json!({"error":{"code":code,"message":message}}).to_string(),
        OutputMode::Human => format!("error: {message}"),
    };
    ProcessOutput {
        stdout: String::new(),
        stderr: format!("{body}\n"),
        exit_code,
    }
}
#[cfg(target_os = "linux")]
fn installation_failure(mode: OutputMode, error: InstallationError) -> ProcessOutput {
    failure(mode, error.code(), error.exit_code(), error.message())
}

/// Windows never starts or defers a helper while ghostlink.exe is running.
pub fn windows_refusal(mode: OutputMode) -> ProcessOutput {
    failure(
        mode,
        "unsupported_operation",
        40,
        "Windows in-process update is not supported. Close Ghostlink and stop destiny-mcp in your MCP hosts, then run the official sibling ghostlink-update.exe directly from its installation directory in a separate PowerShell session: .\\ghostlink-update.exe (or .\\ghostlink-update.exe --version <VERSION>). If it is absent, use the official Windows release installer. Restart MCP hosts afterward. Nothing was launched or deferred.",
    )
}

#[cfg(target_os = "linux")]
fn valid_version(version: Option<&str>) -> bool {
    version.is_none_or(|v| {
        !v.is_empty()
            && v.len() <= 128
            && !v.starts_with('-')
            && v.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    })
}

pub fn production(args: &UpdateArgs, mode: OutputMode) -> ProcessOutput {
    #[cfg(windows)]
    {
        let _ = args;
        windows_refusal(mode)
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    {
        if !valid_version(args.version.as_deref()) {
            return failure(
                mode,
                "invalid_request",
                2,
                "Use a non-empty release version containing only ASCII letters, digits, dots, hyphens or plus signs; it must not start with a hyphen.",
            );
        }
        let cwd = match std::env::current_dir() {
            Ok(path) => path,
            Err(_) => return installation_failure(mode, InstallationError::LocalIo),
        };
        // homedir::my_home (used upstream) prefers HOME, even if empty.
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .or_else(std::env::home_dir);
        let receipt = match crate::installation::receipt_path(
            |key| std::env::var_os(key),
            &cwd,
            home.as_deref(),
        ) {
            Ok(path) => path,
            Err(error) => return installation_failure(mode, error),
        };
        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(_) => return installation_failure(mode, InstallationError::LocalIo),
        };
        run_at(args, mode, &receipt, &exe)
    }
    #[cfg(not(any(
        windows,
        all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
    )))]
    {
        let _ = args;
        failure(
            mode,
            "unsupported_operation",
            40,
            "Official update is supported only on Linux x86_64 GNU. Use your installation's original distribution method on this platform.",
        )
    }
}

/// Explicit paths are the offline fixture seam; production always uses current_exe.
#[cfg(target_os = "linux")]
pub fn run_at(
    args: &UpdateArgs,
    mode: OutputMode,
    receipt: &std::path::Path,
    exe: &std::path::Path,
) -> ProcessOutput {
    use crate::installation::Installation;

    if !valid_version(args.version.as_deref()) {
        return failure(
            mode,
            "invalid_request",
            2,
            "The requested release version is invalid.",
        );
    }
    let installation = match Installation::validate(receipt, exe) {
        Ok(value) => value,
        Err(error) => return installation_failure(mode, error),
    };
    let _lock = match installation.lock() {
        Ok(lock) => lock,
        Err(error) => return installation_failure(mode, error),
    };
    // Recheck after acquiring the per-installation lock.
    let installation = match Installation::validate(receipt, exe) {
        Ok(value) => value,
        Err(error) => return installation_failure(mode, error),
    };
    let mut command = child_command(&installation, args, std::env::vars_os().map(|(key, _)| key));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            return failure(
                mode,
                "updater_failed",
                41,
                "The validated updater could not start. Check its permissions and runtime requirements; no elevation is attempted.",
            );
        }
    };
    // Concurrent drain avoids pipe deadlock. Retain at most 4 KiB per stream;
    // never render child bytes, which may contain credentials or terminal codes.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out = std::thread::spawn(move || drain(stdout));
    let err = std::thread::spawn(move || drain(stderr));
    let status = child.wait();
    let drained = out
        .join()
        .ok()
        .and_then(Result::ok)
        .zip(err.join().ok().and_then(Result::ok));
    let (Ok(status), Some((stdout_truncated, stderr_truncated))) = (status, drained) else {
        return failure(
            mode,
            "updater_failed",
            41,
            "The updater outcome could not be observed. Inspect the installation before retrying; it may have changed.",
        );
    };
    if !status.success() {
        return failure(
            mode,
            "updater_failed",
            41,
            "The updater failed. Child diagnostics were suppressed for safety. Check connectivity, permissions and the requested release; files may have changed. No rollback is guaranteed.",
        );
    }
    let message = "The updater completed successfully; this does not prove a requested version was installed (it may already be current). Configuration, data and credentials are preserved. Restart MCP hosts to load destiny-mcp. No atomic paired update or rollback is guaranteed.";
    let body = match mode {
        OutputMode::Human => message.to_string(),
        OutputMode::Json => json!({"kind":"update", "status":"updater_completed", "requested_version":args.version,
            "installed_version":null, "message":message, "stdout_truncated":stdout_truncated, "stderr_truncated":stderr_truncated}).to_string(),
    };
    ProcessOutput::success(body)
}

/// Build an OS argument vector and explicitly scrub routing overrides.
/// Environment keys are injectable so tests need not mutate process-global env.
#[cfg(target_os = "linux")]
pub fn child_command(
    installation: &crate::installation::Installation,
    args: &UpdateArgs,
    environment_keys: impl IntoIterator<Item = std::ffi::OsString>,
) -> std::process::Command {
    use std::process::{Command, Stdio};
    let mut command = Command::new(&installation.updater);
    if let Some(version) = &args.version {
        command.arg("--version").arg(version);
    }
    // Installer download URLs, forced dirs, app-name and receipt overrides must
    // not redirect the validated update. Preserve the upstream GitHub token.
    for key in environment_keys {
        let text = key.to_string_lossy();
        if text.starts_with("GHOSTLINK_")
            || text.starts_with("INSTALLER_")
            || text.starts_with("CARGO_DIST_")
            || (text.starts_with("AXOUPDATER_") && text != "AXOUPDATER_GITHUB_TOKEN")
        {
            command.env_remove(key);
        }
    }
    command
        .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
        .env_remove("AXOUPDATER_INSTALLER_PATH")
        .env("AXOUPDATER_APP_NAME", "ghostlink")
        .env(
            "AXOUPDATER_CONFIG_PATH",
            installation
                .receipt
                .parent()
                .expect("validated receipt parent"),
        )
        .env("CARGO_HOME", &installation.root)
        .env("GHOSTLINK_NO_MODIFY_PATH", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(target_os = "linux")]
fn drain(mut stream: impl std::io::Read) -> std::io::Result<bool> {
    let mut retained = Vec::with_capacity(4096);
    let mut buffer = [0u8; 4096];
    let mut truncated = false;
    loop {
        let count = match stream.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(truncated);
        }
        let keep = count.min(4096 - retained.len());
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}
