//! Installation-owned cleanup, deliberately separate from generic uninstall.
use super::*;

/// A previewed mutation. Shared MIME files are rewritten, never deleted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandlerCleanupAction {
    RewriteMime {
        path: PathBuf,
        restore: Option<String>,
    },
    RemoveDesktop {
        path: PathBuf,
    },
    RemoveRecord {
        path: PathBuf,
    },
}

/// A preserved path or a failed operation, with actionable guidance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HandlerCleanupIssue {
    pub path: PathBuf,
    pub guidance: String,
}

/// Read-only preview. Private snapshots bind execution to this installation.
#[derive(Clone, Debug)]
pub struct HandlerCleanupPlan {
    pub paths: LinuxHandlerPaths,
    pub actions: Vec<HandlerCleanupAction>,
    pub skips: Vec<HandlerCleanupIssue>,
    executable: PathBuf,
    snapshots: Vec<(PathBuf, Option<Snapshot>)>,
    association: String,
    replacement: Option<String>,
}

/// Partial successes remain visible if a subsequent operation fails.
#[derive(Clone, Debug, Default)]
pub struct HandlerCleanupReport {
    pub completed: Vec<HandlerCleanupAction>,
    pub skips: Vec<HandlerCleanupIssue>,
    pub errors: Vec<HandlerCleanupIssue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    text: String,
    #[cfg(target_os = "linux")]
    identity: (u64, u64, u32, i64, i64, i64, i64),
}

fn issue(path: &Path, guidance: impl Into<String>) -> HandlerCleanupIssue {
    HandlerCleanupIssue {
        path: path.to_owned(),
        guidance: guidance.into(),
    }
}

impl<R: HandlerCommandRunner> LinuxUriHandlerRegistrar<R> {
    /// Queries the association but never writes or invokes a mutating command.
    /// Only the exact generated desktop entry proves installation ownership.
    pub fn scoped_cleanup_plan(&mut self) -> Result<HandlerCleanupPlan, AppError> {
        let mut plan = HandlerCleanupPlan {
            paths: self.paths.clone(),
            actions: Vec::new(),
            skips: Vec::new(),
            executable: self.executable.clone(),
            snapshots: Vec::new(),
            association: String::new(),
            replacement: None,
        };
        if !cfg!(target_os = "linux") {
            plan.skips.push(issue(
                &self.desktop_file(),
                "Automatic handler cleanup is Linux-only; remove the handler manually.",
            ));
            return Ok(plan);
        }
        for path in [
            self.desktop_file(),
            self.paths.record_file.clone(),
            self.paths.mimeapps_list.clone(),
        ] {
            match snapshot(&path) {
                Ok(value) => plan.snapshots.push((path, value)),
                Err(reason) => plan.skips.push(issue(
                    &path,
                    format!(
                        "Preserved: {reason}. Inspect this path manually; do not follow symlinks."
                    ),
                )),
            }
        }
        if !plan.skips.is_empty() {
            return Ok(plan);
        }
        let desktop = &plan.snapshots[0].1;
        let Some(desktop) = desktop else {
            if plan.snapshots[1].1.is_some() {
                plan.skips.push(issue(
                    &self.paths.record_file,
                    "Orphan previous-handler record preserved; inspect and remove manually.",
                ));
            }
            return Ok(plan);
        };
        if desktop.text != self.desktop_entry()? {
            plan.skips.push(issue(&self.desktop_file(), "Desktop entry is modified or belongs to another installation; handler and record preserved. Inspect manually."));
            return Ok(plan);
        }
        plan.association = self.default_handler()?;
        let record = plan.snapshots[1].1.as_ref();
        let previous = record.and_then(|s| valid_record(&s.text));
        if record.is_some() && previous.is_none() {
            plan.skips.push(issue(
                &self.paths.record_file,
                "Ambiguous previous-handler record preserved; select a default manually.",
            ));
        }
        let restore = previous.filter(|id| !id.is_empty() && plan.association == DESKTOP_ID);
        if let Some(mime) = &plan.snapshots[2].1 {
            match rewrite_mime(&mime.text, restore) {
                Ok(replacement) => {
                    if replacement != mime.text {
                        plan.replacement = Some(replacement);
                        plan.actions.push(HandlerCleanupAction::RewriteMime {
                            path: self.paths.mimeapps_list.clone(),
                            restore: restore.map(str::to_owned),
                        });
                    }
                }
                Err(reason) => {
                    plan.skips.push(issue(&self.paths.mimeapps_list, format!("Preserved handler: {reason}. Select the default and remove Ghostlink associations manually.")));
                    return Ok(plan);
                }
            }
        } else if plan.association == DESKTOP_ID {
            plan.skips.push(issue(&self.paths.mimeapps_list, "Current Ghostlink default is not represented in this MIME file; handler preserved. Change the default manually."));
            return Ok(plan);
        }
        if plan.association == DESKTOP_ID && plan.replacement.is_none() {
            plan.skips.push(issue(&self.desktop_file(), "Active Ghostlink selection cannot be cleared by an owned MIME rewrite; handler and record preserved. Change the default manually."));
            return Ok(plan);
        }
        plan.actions.push(HandlerCleanupAction::RemoveDesktop {
            path: self.desktop_file(),
        });
        if previous.is_some() {
            plan.actions.push(HandlerCleanupAction::RemoveRecord {
                path: self.paths.record_file.clone(),
            });
        }
        Ok(plan)
    }

    /// Executes only an unchanged preview. Association queries are repeated before
    /// mutation; no old default is restored over a user's new choice. Restoration
    /// edits only the owned user MIME default, not portals or system-wide defaults.
    pub fn execute_scoped_cleanup(&mut self, plan: &HandlerCleanupPlan) -> HandlerCleanupReport {
        let mut report = HandlerCleanupReport {
            skips: plan.skips.clone(),
            ..Default::default()
        };
        if plan.actions.is_empty() {
            return report;
        }
        if !cfg!(target_os = "linux")
            || self.paths != plan.paths
            || self.executable != plan.executable
        {
            report.errors.push(issue(
                &self.desktop_file(),
                "Preview belongs to another installation or platform; preview again.",
            ));
            return report;
        }
        // Rebuild the preview: public display fields are not mutation authority.
        let fresh = match self.scoped_cleanup_plan() {
            Ok(fresh) => fresh,
            Err(_) => {
                report.errors.push(issue(
                    &self.desktop_file(),
                    "Cannot recheck ownership or current association; preview again.",
                ));
                return report;
            }
        };
        if fresh.snapshots != plan.snapshots
            || fresh.association != plan.association
            || fresh.actions != plan.actions
            || fresh.replacement != plan.replacement
        {
            report.errors.push(issue(
                &self.desktop_file(),
                "Files or association changed since preview; preserved. Preview again.",
            ));
            return report;
        }
        let mut expected = plan.snapshots.clone();
        for action in &plan.actions {
            // Query first, then recheck every snapshot immediately before mutation.
            // A MIME rewrite can itself change the default, so its expected result
            // is used for subsequent queries rather than the cached preview value.
            let association = match self.default_handler() {
                Ok(value) => value,
                Err(_) => {
                    report.errors.push(issue(
                        &self.paths.mimeapps_list,
                        "Cannot recheck current default; remaining files preserved.",
                    ));
                    break;
                }
            };
            let rewritten = report.completed.iter().find_map(|a| match a {
                HandlerCleanupAction::RewriteMime { restore, .. } => Some(restore),
                _ => None,
            });
            let association_ok = match rewritten {
                None => association == plan.association,
                Some(Some(previous)) => association == *previous,
                Some(None) => association != DESKTOP_ID,
            };
            let still_active = matches!(action, HandlerCleanupAction::RemoveDesktop { .. })
                && association == DESKTOP_ID;
            if still_active || !association_ok || !snapshots_match(&expected) {
                report.errors.push(issue(&self.desktop_file(), "Snapshot or association changed before mutation; remaining files preserved. Preview again."));
                break;
            }
            let (path, result) = match action {
                HandlerCleanupAction::RewriteMime { path, .. } => (
                    path,
                    guarded_replace(
                        path,
                        plan.replacement.as_deref().unwrap_or_default(),
                        &expected,
                    ),
                ),
                HandlerCleanupAction::RemoveDesktop { path }
                | HandlerCleanupAction::RemoveRecord { path } => (
                    path,
                    fs::remove_file(path)
                        .map_err(|e| format!("Removal failed: {e}; inspect manually.")),
                ),
            };
            if let Err(reason) = result {
                report.errors.push(issue(path, reason));
                break;
            }
            report.completed.push(action.clone());
            match snapshot(path) {
                Ok(value) => {
                    let result_matches = match action {
                        HandlerCleanupAction::RewriteMime { .. } => value
                            .as_ref()
                            .is_some_and(|s| Some(s.text.as_str()) == plan.replacement.as_deref()),
                        _ => value.is_none(),
                    };
                    if !result_matches {
                        report.errors.push(issue(path, "Mutation completed but result changed concurrently; remaining files preserved."));
                        break;
                    }
                    if let Some((_, old)) = expected.iter_mut().find(|(p, _)| p == path) {
                        *old = value;
                    }
                }
                Err(reason) => {
                    report.errors.push(issue(
                        path,
                        format!("Mutation completed but cannot snapshot result: {reason}"),
                    ));
                    break;
                }
            }
        }
        report
    }
}

fn valid_record(text: &str) -> Option<&str> {
    let id = text.strip_suffix('\n').unwrap_or(text);
    (id.is_empty() || (id != DESKTOP_ID && is_desktop_id(id))).then_some(id)
}

/// Recognized freedesktop sections only; duplicate/unknown scheme keys require
/// manual cleanup. Unrelated bytes and foreign values retain their order.
fn rewrite_mime(text: &str, restore: Option<&str>) -> Result<String, &'static str> {
    let mut section = "";
    let mut seen = std::collections::HashSet::new();
    let mut restored = false;
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed;
        }
        let Some(value) = scheme_value(line) else {
            output.push_str(line);
            continue;
        };
        if !matches!(
            section,
            "[Default Applications]" | "[Added Associations]" | "[Removed Associations]"
        ) || !seen.insert(section)
        {
            return Err("ambiguous MIME scheme sections");
        }
        let ids = value
            .split(';')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if ids.iter().any(|id| !is_desktop_id(id)) {
            return Err("invalid MIME association value");
        }
        if !ids.contains(&DESKTOP_ID) {
            output.push_str(line);
            continue;
        }
        let mut others = ids
            .iter()
            .copied()
            .filter(|id| *id != DESKTOP_ID)
            .collect::<Vec<_>>();
        if section == "[Default Applications]"
            && let Some(previous) = restore
        {
            if ids.first() != Some(&DESKTOP_ID) {
                return Err("Ghostlink is not the explicit first default");
            }
            others.retain(|id| *id != previous);
            others.insert(0, previous);
            restored = true;
        }
        if !others.is_empty() {
            output.push_str(line.split_once('=').expect("scheme key").0);
            output.push('=');
            output.push_str(&others.join(";"));
            output.push(';');
            if line.ends_with('\n') {
                output.push('\n');
            }
        }
    }
    if restore.is_some() && !restored {
        return Err("previous default cannot be restored from this MIME file");
    }
    Ok(output)
}

fn safe_path(path: &Path) -> Result<(), &'static str> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err("unsafe path");
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => return Err("symlink path"),
            Ok(meta) if ancestor != path && !meta.is_dir() => return Err("non-directory ancestor"),
            Ok(_) => (),
            Err(e) if e.kind() == ErrorKind::NotFound => (),
            Err(_) => return Err("unreadable path"),
        }
    }
    Ok(())
}

fn snapshot(path: &Path) -> Result<Option<Snapshot>, &'static str> {
    safe_path(path)?;
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Err("not a regular file"),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unreadable file"),
    };
    if meta.len() > READ_LIMIT as u64 {
        return Err("file exceeds read limit");
    }
    let text = read_bounded(path)
        .map_err(|_| "cannot read bounded UTF-8 file")?
        .ok_or("file disappeared")?;
    safe_path(path)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let after = fs::symlink_metadata(path).map_err(|_| "file disappeared")?;
        let identity = |m: &fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.mode(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if identity(&meta) != identity(&after) {
            return Err("file changed during read");
        }
        Ok(Some(Snapshot {
            text,
            identity: identity(&meta),
        }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = meta;
        Ok(Some(Snapshot { text }))
    }
}

fn snapshots_match(expected: &[(PathBuf, Option<Snapshot>)]) -> bool {
    expected
        .iter()
        .all(|(p, value)| snapshot(p).is_ok_and(|s| s == *value))
}

fn guarded_replace(
    path: &Path,
    text: &str,
    expected: &[(PathBuf, Option<Snapshot>)],
) -> Result<(), String> {
    let name = path
        .file_name()
        .ok_or("Missing MIME filename")?
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.ghostlink-scoped-tmp"));
    safe_path(&temporary).map_err(str::to_owned)?;
    let permissions = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).map_err(|e| {
        format!(
            "Exclusive temporary write failed at {}: {e}; existing temporary file preserved.",
            temporary.display()
        )
    })?;
    // On error retain our temporary file too: expose the exact path for manual
    // inspection rather than risk deleting a concurrently replaced pathname.
    let result = (|| {
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.set_permissions(permissions)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        if !snapshots_match(expected) {
            return Err("snapshot changed before rename".to_owned());
        }
        safe_path(&temporary).map_err(str::to_owned)?;
        let at_path = fs::symlink_metadata(&temporary).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = file.metadata().map_err(|e| e.to_string())?;
            if held.dev() != at_path.dev() || held.ino() != at_path.ino() {
                return Err("temporary file replaced".to_owned());
            }
        }
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    result.map_err(|e| {
        format!(
            "MIME replacement failed: {e}; inspect temporary path {} and preview again.",
            temporary.display()
        )
    })
}
