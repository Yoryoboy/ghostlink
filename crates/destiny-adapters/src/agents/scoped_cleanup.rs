//! Conservative, installation-scoped cleanup, separate from generic agents uninstall.
//! Exact embedded skill content is eligible, but does not prove which installation
//! wrote it. Older versions and modified copies remain for manual cleanup.
//! Snapshot and symlink guards reduce accidental damage; they do not provide
//! hostile-local-actor race immunity. The caller decides whether to remove binaries
//! after partial integration cleanup. No external agent tools are ever invoked.
use super::{Agent, EXIT_CODES, LIMIT, SKILL};
use serde_json::Value;
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    UnsafePath,
    ContentMismatch,
    UnprovenMcp,
    InvalidJson,
    Changed,
    NotEmpty,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    Ready,
    Missing,
    Removed,
    Preserved(SkipReason),
    Failed(String),
}
#[derive(Clone, Debug)]
pub struct CleanupPath {
    pub path: PathBuf,
    pub outcome: CleanupOutcome,
}
#[derive(Clone, Debug)]
pub struct ManualFollowup {
    pub agent: Agent,
    pub instruction: &'static str,
}
/// Public preview fields are informational; private snapshots are the execution
/// authority. Planning performs bounded reads only, never creates directories.
pub struct CleanupPlan {
    pub(super) paths: Vec<CleanupPath>,
    pub manual_followups: Vec<ManualFollowup>,
    pending: Vec<(usize, Snapshot, Mutation)>,
}
#[derive(Debug)]
pub struct CleanupReport {
    pub paths: Vec<CleanupPath>,
    pub manual_followups: Vec<ManualFollowup>,
    /// Exclusive temporary files retained after failed/changed JSON writes.
    pub retained_temps: Vec<PathBuf>,
}
struct Snapshot {
    bytes: Vec<u8>,
    metadata: Metadata,
}
enum Mutation {
    File,
    Directory,
    Json(Vec<u8>),
}
fn failure(e: io::Error) -> CleanupOutcome {
    CleanupOutcome::Failed(e.to_string())
}
fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}
fn guard(path: &Path) -> Result<(), CleanupOutcome> {
    if !absolute(path) {
        return Err(CleanupOutcome::Preserved(SkipReason::UnsafePath));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(CleanupOutcome::Preserved(SkipReason::UnsafePath));
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(failure(e)),
        }
    }
    Ok(())
}
fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    let same = a.len() == b.len()
        && a.modified().ok() == b.modified().ok()
        && a.is_dir() == b.is_dir()
        && a.permissions() == b.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        same
    }
}
fn snapshot(path: &Path, directory: bool) -> Result<Snapshot, CleanupOutcome> {
    guard(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(CleanupOutcome::Missing),
        Err(e) => return Err(failure(e)),
    };
    if directory {
        if !metadata.is_dir() {
            return Err(CleanupOutcome::Preserved(SkipReason::UnsafePath));
        }
        return Ok(Snapshot {
            bytes: Vec::new(),
            metadata,
        });
    }
    if !metadata.is_file() {
        return Err(CleanupOutcome::Failed("Expected a regular file".into()));
    }
    if metadata.len() > LIMIT {
        return Err(CleanupOutcome::Failed(
            "File exceeds cleanup read limit".into(),
        ));
    }
    let file = fs::File::open(path).map_err(failure)?;
    let opened = file.metadata().map_err(failure)?;
    if !same_metadata(&metadata, &opened) {
        return Err(CleanupOutcome::Preserved(SkipReason::Changed));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(failure)?;
    if bytes.len() as u64 > LIMIT {
        return Err(CleanupOutcome::Failed(
            "File exceeds cleanup read limit".into(),
        ));
    }
    guard(path)?;
    let after = fs::symlink_metadata(path).map_err(failure)?;
    if !same_metadata(&metadata, &after) {
        return Err(CleanupOutcome::Preserved(SkipReason::Changed));
    }
    Ok(Snapshot { bytes, metadata })
}
impl CleanupPlan {
    pub fn paths(&self) -> &[CleanupPath] {
        &self.paths
    }

    fn add(
        &mut self,
        path: PathBuf,
        directory: bool,
        decide: impl FnOnce(&[u8]) -> Result<Mutation, SkipReason>,
    ) {
        let result = snapshot(&path, directory);
        let outcome = match result {
            Ok(s) => match decide(&s.bytes) {
                Ok(m) => {
                    self.pending.push((self.paths.len(), s, m));
                    CleanupOutcome::Ready
                }
                Err(reason) => CleanupOutcome::Preserved(reason),
            },
            Err(outcome) => outcome,
        };
        self.paths.push(CleanupPath { path, outcome });
    }
}
/// Both locations must be injected absolute paths without parent traversal.
/// The MCP path identifies this installation; it is not executed or read.
pub fn plan_cleanup(home: &Path, mcp_executable: &Path) -> io::Result<CleanupPlan> {
    if !absolute(home) || !absolute(mcp_executable) || mcp_executable.to_str().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected absolute HOME and UTF-8 MCP executable paths without parent traversal",
        ));
    }
    let mut plan = CleanupPlan {
        paths: Vec::new(), pending: Vec::new(),
        manual_followups: [Agent::Claude, Agent::Codex, Agent::Pi].into_iter().map(|agent| ManualFollowup {
            agent,
            instruction: "Inspect the ghostlink MCP registration and remove it manually only if its executable belongs to this installation; name-only removal cannot prove ownership.",
        }).collect(),
    };
    for agent in Agent::ALL {
        let dir = home.join(agent.skill_dir()).join("ghostlink");
        for (path, expected) in [
            (dir.join("SKILL.md"), SKILL),
            (dir.join("references/exit-codes.md"), EXIT_CODES),
        ] {
            plan.add(path, false, |bytes| {
                if bytes == expected.as_bytes() {
                    Ok(Mutation::File)
                } else {
                    Err(SkipReason::ContentMismatch)
                }
            });
        }
        for path in [dir.join("references"), dir] {
            plan.add(path, true, |_| Ok(Mutation::Directory));
        }
    }
    plan.add(
        home.join(".config/opencode/opencode.json"),
        false,
        |bytes| {
            let mut document: Value =
                serde_json::from_slice(bytes).map_err(|_| SkipReason::InvalidJson)?;
            if !document.is_object() {
                return Err(SkipReason::InvalidJson);
            }
            let Some(mcp) = document.get_mut("mcp") else {
                return Err(SkipReason::UnprovenMcp);
            };
            let Some(mcp) = mcp.as_object_mut() else {
                return Err(SkipReason::InvalidJson);
            };
            let Some(entry) = mcp.get("ghostlink") else {
                return Err(SkipReason::UnprovenMcp);
            };
            if entry.get("type").and_then(Value::as_str) != Some("local")
                || entry.get("command")
                    != Some(&serde_json::json!([mcp_executable.to_str().unwrap()]))
            {
                return Err(SkipReason::UnprovenMcp);
            }
            mcp.remove("ghostlink");
            // No backup is created or touched. Only the exact entry is removed.
            serde_json::to_vec_pretty(&document)
                .map(Mutation::Json)
                .map_err(|_| SkipReason::InvalidJson)
        },
    );
    Ok(plan)
}
fn recheck(path: &Path, old: &Snapshot, directory: bool) -> Result<(), CleanupOutcome> {
    let current = snapshot(path, directory)?;
    // Child removals change directory timestamps/length; still require the same
    // directory identity and permissions, then rely on nonrecursive remove_dir.
    let same = if directory {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            current.metadata.dev() == old.metadata.dev()
                && current.metadata.ino() == old.metadata.ino()
                && current.metadata.permissions() == old.metadata.permissions()
        }
        #[cfg(not(unix))]
        {
            current.metadata.permissions() == old.metadata.permissions()
        }
    } else {
        same_metadata(&current.metadata, &old.metadata) && current.bytes == old.bytes
    };
    if same {
        Ok(())
    } else {
        Err(CleanupOutcome::Preserved(SkipReason::Changed))
    }
}
fn replace(
    path: &Path,
    old: &Snapshot,
    bytes: &[u8],
    retained: &mut Vec<PathBuf>,
) -> Result<(), CleanupOutcome> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| CleanupOutcome::Failed(e.to_string()))?;
    let name: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temp = path.with_file_name(format!(".ghostlink-cleanup-{name}.tmp"));
    guard(&temp)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(failure)?;
    retained.push(temp.clone());
    // Failed writes retain the exclusive temp for inspection, never touch backups.
    // On Unix start private, then restore the original mode after writing.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(failure)?;
    }
    file.write_all(bytes).map_err(failure)?;
    file.set_permissions(old.metadata.permissions())
        .map_err(failure)?;
    file.sync_all().map_err(failure)?;
    drop(file);
    recheck(path, old, false)?;
    guard(&temp)?;
    fs::rename(&temp, path).map_err(failure)?;
    retained.pop();
    Ok(())
}
/// Consume the plan, recheck each snapshot immediately before mutation, and
/// continue independent paths after failures. Never recursively removes anything.
/// Empty shared agent roots are not removed, only ghostlink and its references.
pub fn execute_cleanup(mut plan: CleanupPlan) -> CleanupReport {
    let mut retained_temps = Vec::new();
    for (index, snapshot, mutation) in plan.pending {
        let path = &plan.paths[index].path;
        let directory = matches!(mutation, Mutation::Directory);
        let result = recheck(path, &snapshot, directory).and_then(|()| match mutation {
            Mutation::File => fs::remove_file(path).map_err(failure),
            Mutation::Directory => match fs::remove_dir(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => {
                    Err(CleanupOutcome::Preserved(SkipReason::NotEmpty))
                }
                Err(e) => Err(failure(e)),
            },
            Mutation::Json(bytes) => replace(path, &snapshot, &bytes, &mut retained_temps),
        });
        plan.paths[index].outcome =
            result.map_or_else(|outcome| outcome, |()| CleanupOutcome::Removed);
    }
    CleanupReport {
        paths: plan.paths,
        manual_followups: plan.manual_followups,
        retained_temps,
    }
}
