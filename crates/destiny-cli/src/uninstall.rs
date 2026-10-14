//! Conservative installation removal; preview and consent precede lock creation.
use crate::update::failure;
use crate::{OutputMode, ProcessOutput, command::UninstallArgs};

pub fn windows_refusal(mode: OutputMode) -> ProcessOutput {
    failure(
        mode,
        "unsupported_operation",
        40,
        "Windows self-uninstall is not supported. Close Ghostlink, stop destiny-mcp in MCP hosts and finish active login first. Use the original package manager for managed installations; otherwise inspect the official receipt and remove only its owned binaries and receipt from a separate PowerShell session. Remove verified integrations manually. Preserve configuration, databases and credentials. Nothing was looked up, launched or deferred.",
    )
}

pub fn production(args: &UninstallArgs, mode: OutputMode) -> ProcessOutput {
    #[cfg(windows)]
    {
        let _ = args;
        windows_refusal(mode)
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
    {
        use destiny_adapters::uri_handler::{
            LinuxHandlerPaths, LinuxUriHandlerRegistrar, SystemHandlerCommandRunner,
        };
        use std::io::{IsTerminal, Write};
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        if !args.dry_run && !args.yes && (mode == OutputMode::Json || !interactive) {
            return failure(
                mode,
                "confirmation_required",
                2,
                "Uninstall requires --yes in JSON or without an interactive terminal. Use --dry-run to preview first.",
            );
        }
        let paths = (|| {
            let cwd = std::env::current_dir().ok()?;
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .or_else(std::env::home_dir)?;
            let receipt =
                crate::installation::receipt_path(|key| std::env::var_os(key), &cwd, Some(&home))
                    .ok()?;
            Some((
                home,
                receipt,
                std::env::current_exe().ok()?,
                LinuxHandlerPaths::from_env().ok()?,
            ))
        })();
        let Some((home, receipt, exe, handler_paths)) = paths else {
            return failure(
                mode,
                "unsupported_installation",
                40,
                "Cannot resolve a supported installation safely. Use the original installation method; no files were changed.",
            );
        };
        let mut registrar =
            LinuxUriHandlerRegistrar::new(handler_paths, exe.clone(), SystemHandlerCommandRunner);
        run_at(
            args,
            mode,
            &receipt,
            &exe,
            &home,
            &mut registrar,
            interactive,
            |preview| {
                print!(
                    "{preview}\nStop MCP hosts and finish active login first. Process inactivity is not guaranteed.\nRemove these installation files and scoped integrations? [y/N] "
                );
                if std::io::stdout().flush().is_err() {
                    return false;
                }
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer).is_ok()
                    && matches!(answer.trim(), "y" | "Y" | "yes" | "YES")
            },
        )
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
            "Automatic uninstall supports only Linux x86_64 GNU. Use the original installation method and preserve user state.",
        )
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::installation::{Installation, safe_path};
    use destiny_adapters::{
        agents::scoped_cleanup as agents,
        uri_handler::{HandlerCleanupAction, HandlerCommandRunner, LinuxUriHandlerRegistrar},
    };
    use serde_json::{Value, json};
    use std::os::unix::fs::MetadataExt;
    use std::{fs, path::Path};

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Identity(u64, u64, u32, u64, i64, i64, i64, i64);
    fn identity(path: &Path) -> Option<Identity> {
        let m = fs::symlink_metadata(path).ok()?;
        Some(Identity(
            m.dev(),
            m.ino(),
            m.mode(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        ))
    }
    fn action_path(action: &HandlerCleanupAction) -> &Path {
        match action {
            HandlerCleanupAction::RewriteMime { path, .. }
            | HandlerCleanupAction::RemoveDesktop { path }
            | HandlerCleanupAction::RemoveRecord { path } => path,
        }
    }
    fn agent_path_report(path: &agents::CleanupPath) -> Value {
        let (status, reason) = match &path.outcome {
            agents::CleanupOutcome::Ready => ("ready", None),
            agents::CleanupOutcome::Missing => ("missing", None),
            agents::CleanupOutcome::Removed => ("completed", None),
            agents::CleanupOutcome::Preserved(reason) => ("preserved", Some(format!("{reason:?}"))),
            agents::CleanupOutcome::Failed(reason) => ("failed", Some(reason.clone())),
        };
        let action = if path
            .path
            .file_name()
            .is_some_and(|name| name == "opencode.json")
        {
            "remove_exact_mcp_entry_preserving_file"
        } else {
            "remove_exact_skill_file_or_empty_skill_directory"
        };
        json!({"path":path.path,"status":status,"reason":reason,"action":action})
    }

    fn render(mode: OutputMode, report: &Value, exit_code: i32) -> ProcessOutput {
        let body = if mode == OutputMode::Json {
            report.to_string()
        } else {
            // Pretty JSON keeps every preview/report field visible, including paths
            // and reasons, without hiding partial outcomes behind success prose.
            serde_json::to_string_pretty(report).expect("serializable report")
        };
        if exit_code == 0 {
            ProcessOutput::success(body)
        } else {
            ProcessOutput {
                stdout: String::new(),
                stderr: format!("{body}\n"),
                exit_code,
            }
        }
    }
    fn partial(mode: OutputMode, mut report: Value, message: &str) -> ProcessOutput {
        report["status"] = json!("partial");
        report["retained_installation_files"] = json!(
            report["planned_removals"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|path| !report["removed"].as_array().unwrap().contains(path))
                .collect::<Vec<_>>()
        );
        report["error"] = json!({"code":"partial_failure", "message":message});
        render(mode, &report, 41)
    }

    /// Fixture seam: production never accepts user-supplied installation paths.
    /// Confirmation receives the entire read-only preview and defaults to no.
    #[allow(clippy::too_many_arguments)]
    pub fn run_at<R: HandlerCommandRunner>(
        args: &UninstallArgs,
        mode: OutputMode,
        receipt: &Path,
        exe: &Path,
        home: &Path,
        registrar: &mut LinuxUriHandlerRegistrar<R>,
        interactive: bool,
        confirm: impl FnOnce(&str) -> bool,
    ) -> ProcessOutput {
        let install = match Installation::validate_uninstall(receipt, exe) {
            Ok(i) => i,
            Err(e) => return failure(mode, e.code(), e.exit_code(), e.message()),
        };
        let agent_plan = match agents::plan_cleanup(home, &install.bin_dir.join("destiny-mcp")) {
            Ok(p) => p,
            Err(_) => {
                return failure(
                    mode,
                    "local_storage",
                    51,
                    "Cannot safely preview agent paths; nothing changed.",
                );
            }
        };
        let handler_plan = match registrar.scoped_cleanup_plan() {
            Ok(p) => p,
            Err(_) => {
                return failure(
                    mode,
                    "local_storage",
                    51,
                    "Cannot safely preview handler ownership/association; nothing changed.",
                );
            }
        };
        let mut bins = vec![install.bin_dir.join("destiny-mcp")];
        let mut skipped = Vec::<Value>::new();
        match fs::symlink_metadata(&install.updater) {
            Ok(m) if m.is_file() && safe_path(&install.updater).is_ok() => bins.push(install.updater.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => skipped.push(json!({"path":install.updater,"reason":"optional updater absent"})),
            _ => skipped.push(json!({"path":install.updater,"reason":"unsafe optional updater preserved; inspect manually"})),
        }
        // CLI and receipt are the final pair. Keep CLI if receipt removal fails.
        bins.push(install.receipt.clone());
        bins.push(exe.to_owned());
        let snapshots: Vec<_> = bins
            .iter()
            .chain(agent_plan.paths().iter().map(|p| &p.path))
            .chain([
                &handler_plan.paths.record_file,
                &handler_plan.paths.mimeapps_list,
                &handler_plan
                    .paths
                    .applications_dir
                    .join("ghostlink-callback.desktop"),
            ])
            .map(|p| (p.clone(), identity(p)))
            .collect();
        let mut report = json!({
            "kind":"uninstall", "status":"preview", "planned_removals":bins,
            "agents":agent_plan.paths().iter().map(agent_path_report).collect::<Vec<_>>(),
            "handler_actions":handler_plan.actions.iter().map(|a| json!({"path":action_path(a),"action":format!("{a:?}")})).collect::<Vec<_>>(),
            "skipped":skipped, "removed":[], "changed":[], "errors":[],
            "manual_followups":agent_plan.manual_followups.iter().map(|f| format!("{:?}: {}",f.agent,f.instruction)).collect::<Vec<_>>(),
            "handler_skips":handler_plan.skips.iter().map(|s| json!({"path":s.path,"reason":s.guidance})).collect::<Vec<_>>(),
            "preserved": [home.join(".config/ghostlink"),home.join(".local/share/ghostlink"),install.root.clone(),install.bin_dir.join(".ghostlink-lifecycle.lock")],
            "preserved_state":"All config, SQLite databases, the credential file, runtime sockets, Cargo directories, PATH/shell configuration, unknown files and backups. External/custom config/data/runtime paths are untouched (not resolved; configuration is not loaded). XDG overrides remain preserved except the exact previewed integration files.",
            "limitations":"Stop MCP hosts and finish active login first; no process inactivity guarantee, elevation, termination, purge, recursive installation removal or rollback. Lock metadata is retained to avoid splitting live locks. Exact skill identity is not installation provenance. Manual registrations can remain after binary removal."
        });
        if !handler_plan.skips.is_empty() {
            report["expected_status"] = json!("partial");
            report["warnings"] = json!([
                "Handler cleanup has skips: handler files will be preserved as listed; installation files will still be removed. The run will end partial (exit 41) unless an earlier failure prevents removal."
            ]);
            report["manual_followups"].as_array_mut().unwrap().push(json!(format!(
                "Inspect preserved handler paths and verify ownership before manual deletion. Select an installed replacement with `xdg-mime default <other>.desktop x-scheme-handler/ghostlink` (replace <other>.desktop with its desktop ID), then verify with `xdg-mime query default x-scheme-handler/ghostlink`. Alternatively, remove Ghostlink from x-scheme-handler/ghostlink lines in {} (remove the whole line only if no other associations remain), and inspect higher-priority desktop-specific or applications mimeapps.list files if Ghostlink remains selected. Only after Ghostlink is no longer selected and ownership is verified, delete the desktop entry {} and previous-handler record {} if present. Preserve unrelated MIME associations and foreign files.",
                handler_plan.paths.mimeapps_list.display(),
                handler_plan.paths.applications_dir.join("ghostlink-callback.desktop").display(),
                handler_plan.paths.record_file.display(),
            )));
        }
        if args.dry_run {
            return render(mode, &report, 0);
        }
        if !args.yes {
            if mode == OutputMode::Json || !interactive {
                report["error"] = json!({"code":"confirmation_required","message":"Use --yes to authorize this preview; no mutation occurred."});
                report["status"] = json!("refused");
                return render(mode, &report, 2);
            }
            if !confirm(&serde_json::to_string_pretty(&report).expect("report")) {
                report["status"] = json!("cancelled");
                return render(mode, &report, 0);
            }
        }
        let _lock = match install.lock() {
            Ok(l) => l,
            Err(e) => return failure(mode, e.code(), e.exit_code(), e.message()),
        };
        if Installation::validate_uninstall(receipt, exe).is_err()
            || snapshots.iter().any(|(p, old)| identity(p) != *old)
        {
            return partial(
                mode,
                report,
                "Installation or integration snapshots changed since preview. No cleanup performed; preview again. Lock metadata may have been created.",
            );
        }
        let handler = registrar.execute_scoped_cleanup(&handler_plan);
        report["changed"] = json!(
            handler
                .completed
                .iter()
                .map(|a| json!({"path":action_path(a),"action":format!("{a:?}")}))
                .collect::<Vec<_>>()
        );
        report["errors"] = json!(
            handler
                .errors
                .iter()
                .map(|s| json!({"path":s.path,"reason":s.guidance}))
                .collect::<Vec<_>>()
        );
        report["handler_skips"] = json!(
            handler
                .skips
                .iter()
                .map(|s| json!({"path":s.path,"reason":s.guidance}))
                .collect::<Vec<_>>()
        );
        // Failed mutations/revalidation still abort. Read-only safety skips do
        // not hold receipt-owned installation files hostage to desktop settings.
        if !handler.errors.is_empty() {
            return partial(
                mode,
                report,
                "Handler cleanup failed or changed since preview; binaries and receipt retained. Inspect handler errors manually.",
            );
        }
        let expected_agent: Vec<_> = agent_plan
            .paths()
            .iter()
            .filter(|p| p.outcome == agents::CleanupOutcome::Ready)
            .map(|p| p.path.clone())
            .collect();
        let agent = agents::execute_cleanup(agent_plan);
        let agent_failed = agent.paths.iter().any(|p| {
            matches!(p.outcome, agents::CleanupOutcome::Failed(_))
                || (expected_agent.contains(&p.path)
                    && !matches!(
                        p.outcome,
                        agents::CleanupOutcome::Removed
                            | agents::CleanupOutcome::Preserved(agents::SkipReason::NotEmpty)
                    ))
        });
        report["agents"] = json!(
            agent
                .paths
                .iter()
                .map(agent_path_report)
                .collect::<Vec<_>>()
        );
        report["retained_temps"] = json!(agent.retained_temps);
        if agent_failed {
            return partial(
                mode,
                report,
                "Agent filesystem cleanup failed or changed; binaries and receipt retained. Individual completed changes are not rolled back.",
            );
        }
        for path in &bins {
            let old = snapshots
                .iter()
                .find(|(p, _)| p == path)
                .and_then(|(_, id)| id.as_ref());
            if safe_path(path).is_err() || identity(path).as_ref() != old {
                report["errors"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"path":path,"reason":"file identity/path changed; preserved"}));
                return partial(
                    mode,
                    report,
                    "Binary cleanup incomplete; inspect paths before retrying.",
                );
            }
            if let Err(e) = fs::remove_file(path) {
                report["errors"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"path":path,"reason":e.to_string()}));
                return partial(
                    mode,
                    report,
                    "File removal failed; remaining files retained. No rollback.",
                );
            }
            report["removed"].as_array_mut().unwrap().push(json!(path));
        }
        if !handler.skips.is_empty() {
            return partial(
                mode,
                report,
                "Installation files removed; handler cleanup skipped and listed handler files preserved. Follow manual_followups and handler_skips to finish safely.",
            );
        }
        report["status"] = json!("removed_with_manual_followups");
        render(mode, &report, 0)
    }
}
#[cfg(target_os = "linux")]
pub use linux::run_at;
