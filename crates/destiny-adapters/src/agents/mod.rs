//! Local agent files and processes; all locations and execution are injected.
pub mod scoped_cleanup;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use destiny_core::agents::*;
use serde_json::{Value, json};

pub const SKILL: &str = include_str!("../../../../skills/ghostlink/SKILL.md");
pub const EXIT_CODES: &str = include_str!("../../../../skills/ghostlink/references/exit-codes.md");
const LIMIT: u64 = 1024 * 1024;

pub struct LocalAgentFiles {
    pub home: PathBuf,
    pub executable: PathBuf,
    pub windows: bool,
    backup_written: bool,
}
impl LocalAgentFiles {
    pub fn new(home: PathBuf, executable: PathBuf, windows: bool) -> Self {
        Self {
            home,
            executable,
            windows,
            backup_written: false,
        }
    }
}
fn io(_: std::io::Error) -> IntegrationError {
    IntegrationError::Io
}
/// Refuse symlinks in every ancestor to avoid writing through agent-owned links.
fn safe(path: &Path) -> Result<(), IntegrationError> {
    if !path.is_absolute() {
        return Err(IntegrationError::UnsafePath);
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(IntegrationError::UnsafePath),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(io(e)),
        }
    }
    Ok(())
}
fn read(path: &Path) -> Result<Option<String>, IntegrationError> {
    safe(path)?;
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let mut text = String::new();
    file.take(LIMIT + 1).read_to_string(&mut text).map_err(io)?;
    if text.len() as u64 > LIMIT {
        return Err(IntegrationError::Io);
    }
    Ok(Some(text))
}
fn ours(text: &str) -> bool {
    // Windows checkouts and editors may store the skill with CRLF endings.
    let text = text.replace("\r\n", "\n");
    text.strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---"))
        .is_some_and(|(header, _)| header.lines().any(|line| line.trim() == "name: ghostlink"))
}
impl AgentFiles for LocalAgentFiles {
    fn config_exists(&self, agent: Agent) -> bool {
        self.home.join(agent.config_dir()).is_dir()
    }
    fn skill(&mut self, agent: Agent, action: Action) -> Result<Outcome, IntegrationError> {
        let dir = self.home.join(agent.skill_dir()).join("ghostlink");
        let skill = dir.join("SKILL.md");
        let refs = dir.join("references/exit-codes.md");
        let current = read(&skill)?;
        if current.as_deref().is_some_and(|s| !ours(s)) {
            return Err(IntegrationError::ForeignSkill);
        }
        if action == Action::Status {
            return Ok(if current.is_some() {
                Outcome::AlreadyInstalled
            } else {
                Outcome::NotInstalled
            });
        }
        safe(&refs)?;
        if action == Action::Uninstall {
            if current.is_none() {
                return Ok(Outcome::NotInstalled);
            }
            fs::remove_file(&skill).map_err(io)?;
            if refs.exists() {
                fs::remove_file(&refs).map_err(io)?;
            }
            // Preserve every unknown file; remove directories only when empty.
            for path in [dir.join("references"), dir] {
                if path.is_dir() && fs::read_dir(&path).map_err(io)?.next().is_none() {
                    fs::remove_dir(path).map_err(io)?;
                }
            }
            return Ok(Outcome::Removed);
        }
        if current.as_deref() == Some(SKILL) && read(&refs)?.as_deref() == Some(EXIT_CODES) {
            return Ok(Outcome::AlreadyInstalled);
        }
        // A directory without our marker must not have its existing reference overwritten.
        if current.is_none() && refs.exists() {
            return Err(IntegrationError::ForeignSkill);
        }
        fs::create_dir_all(refs.parent().expect("reference parent")).map_err(io)?;
        fs::write(&skill, SKILL).map_err(io)?;
        fs::write(&refs, EXIT_CODES).map_err(io)?;
        Ok(Outcome::Installed)
    }
    fn mcp_executable(&self) -> Result<PathBuf, IntegrationError> {
        let path = self
            .executable
            .parent()
            .map(|p| p.join(mcp_filename(self.windows)))
            .filter(|p| p.is_absolute() && p.is_file())
            .ok_or(IntegrationError::MissingMcpBinary)?;
        Ok(path)
    }
    fn opencode(
        &mut self,
        action: Action,
        executable: Option<&Path>,
    ) -> Result<Outcome, IntegrationError> {
        let path = self.home.join(".config/opencode/opencode.json");
        let original = read(&path)?;
        let entry = json!({"type":"local","command":[executable.map(|p| p.to_string_lossy())],"enabled":true});
        let manual = || Outcome::ManualStepRequired {
            snippet: if action == Action::Install {
                serde_json::to_string_pretty(&json!({"mcp":{"ghostlink":entry}}))
                    .expect("JSON value")
            } else {
                "Remove only mcp.ghostlink from opencode.json".into()
            },
        };
        let mut document = match original.as_deref() {
            Some(text) => match serde_json::from_str::<Value>(text) {
                Ok(value) if value.is_object() => value,
                _ => return Ok(manual()),
            },
            None if action != Action::Install => return Ok(Outcome::NotInstalled),
            None => json!({"$schema":"https://opencode.ai/config.json"}),
        };
        if document.get("mcp").is_some_and(|mcp| !mcp.is_object()) {
            return Ok(manual());
        }
        let present = document.get("mcp").and_then(|m| m.get("ghostlink"));
        if action == Action::Status {
            return Ok(if present.is_some() {
                Outcome::AlreadyInstalled
            } else {
                Outcome::NotInstalled
            });
        }
        if action == Action::Install && present == Some(&entry) {
            return Ok(Outcome::AlreadyInstalled);
        }
        if action == Action::Uninstall && present.is_none() {
            return Ok(Outcome::NotInstalled);
        }
        let object = document.as_object_mut().expect("validated object");
        let mcp = object
            .entry("mcp")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("validated MCP");
        if action == Action::Install {
            mcp.insert("ghostlink".into(), entry);
        } else {
            mcp.remove("ghostlink");
        }
        fs::create_dir_all(path.parent().expect("config parent")).map_err(io)?;
        if let Some(text) = original.filter(|_| !self.backup_written) {
            let backup = path.with_file_name("opencode.json.ghostlink-backup");
            safe(&backup)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(backup)
                .map_err(io)?;
            file.write_all(text.as_bytes()).map_err(io)?;
            file.sync_all().map_err(io)?;
            self.backup_written = true;
        }
        let bytes = serde_json::to_vec_pretty(&document).expect("JSON value");
        // create_new prevents collisions and symlink traversal. Failed temps are retained.
        let temp = path.with_file_name(format!(
            "opencode.json.ghostlink-{}.tmp",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(temp, path).map_err(io)?;
        Ok(if action == Action::Install {
            Outcome::Installed
        } else {
            Outcome::Removed
        })
    }
}

pub struct EnvironmentPathLookup {
    pub path: OsString,
    pub windows: bool,
}
impl PathLookup for EnvironmentPathLookup {
    fn resolve(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(&self.path)
            .filter(|dir| dir.is_absolute())
            .flat_map(|dir| {
                let names = if self.windows {
                    vec![format!("{name}.exe"), format!("{name}.cmd"), name.into()]
                } else {
                    vec![name.into()]
                };
                names.into_iter().map(move |name| dir.join(name))
            })
            .find(|path| {
                let Ok(meta) = fs::metadata(path) else {
                    return false;
                };
                if !meta.is_file() {
                    return false;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    self.windows || meta.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    true
                }
            })
    }
}
/// Drain pipes concurrently, retaining at most 8 KiB each. Timeout kills and reaps
/// the direct child; descendants holding pipes cannot block the caller.
pub struct SystemCommandRunner {
    pub timeout: Duration,
}
impl CommandRunner for SystemCommandRunner {
    fn run(
        &mut self,
        agent: Agent,
        program: &Path,
        args: &[OsString],
    ) -> Result<CommandOutput, IntegrationError> {
        let failure = || IntegrationError::Command {
            agent,
            exit_status: None,
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| failure())?;
        fn drain(pipe: impl Read + Send + 'static) -> std::sync::mpsc::Receiver<Vec<u8>> {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut retained = Vec::new();
                let mut pipe = pipe;
                let mut buffer = [0; 4096];
                while let Ok(n) = pipe.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    let take = n.min(8192usize.saturating_sub(retained.len()));
                    retained.extend_from_slice(&buffer[..take]);
                }
                let _ = tx.send(retained);
            });
            rx
        }
        let stdout = drain(child.stdout.take().expect("piped stdout"));
        let stderr = drain(child.stderr.take().expect("piped stderr"));
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(IntegrationError::Timeout { agent });
                }
            }
        };
        let mut bytes = stdout
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| IntegrationError::Timeout { agent })?;
        bytes.extend(
            stderr
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| IntegrationError::Timeout { agent })?,
        );
        Ok(CommandOutput {
            success: status.success(),
            exit_status: status.code(),
            output: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}
#[cfg(test)]
mod tests;
