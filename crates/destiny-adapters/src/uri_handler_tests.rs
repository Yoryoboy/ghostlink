use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use destiny_core::ErrorCategory;
use destiny_core::uri_handler::{HandlerRegistration, HandlerWarningKind, UriHandlerRegistrar};

use super::*;

const QUERY: &str = "xdg-mime query default x-scheme-handler/ghostlink";
const PORTAL_LOOKUP: &str = "busctl --user call org.freedesktop.impl.portal.PermissionStore \
     /org/freedesktop/impl/portal/PermissionStore org.freedesktop.impl.portal.PermissionStore \
     Lookup ss desktop-used-apps x-scheme-handler/ghostlink";

/// A private scratch directory removed when dropped.
struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ghostlink-uri-handler-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn paths(&self) -> LinuxHandlerPaths {
        LinuxHandlerPaths {
            applications_dir: self.0.join("data/applications"),
            record_file: self.0.join("data/ghostlink/previous-uri-handler"),
            mimeapps_list: self.0.join("config/mimeapps.list"),
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A scripted command runner that also models `xdg-mime default`.
#[derive(Default)]
struct FakeRunner {
    default_handler: String,
    scoped_mime: Option<PathBuf>,
    responses: HashMap<String, CommandOutput>,
    unavailable: Vec<&'static str>,
    calls: Vec<String>,
}

impl FakeRunner {
    fn with_default(handler: &str) -> Self {
        Self {
            default_handler: handler.to_owned(),
            ..Self::default()
        }
    }

    fn respond(mut self, command: &str, success: bool, stdout: &str) -> Self {
        self.responses.insert(
            command.to_owned(),
            CommandOutput {
                success,
                stdout: stdout.to_owned(),
            },
        );
        self
    }

    fn calls_to(&self, program: &str) -> Vec<&str> {
        self.calls
            .iter()
            .filter(|call| call.starts_with(program))
            .map(String::as_str)
            .collect()
    }
}

impl HandlerCommandRunner for FakeRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, AppError> {
        let command = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.calls.push(command.clone());
        if self.unavailable.contains(&program) {
            return Err(AppError::new(ErrorCategory::UnsupportedOperation));
        }
        if let Some(output) = self.responses.get(&command) {
            return Ok(output.clone());
        }
        if command == QUERY {
            return Ok(CommandOutput {
                success: true,
                stdout: format!(
                    "{}\n",
                    if self.default_handler == DESKTOP_ID {
                        self.scoped_mime
                            .as_ref()
                            .map(|p| fake_mime_default(p))
                            .unwrap_or_else(|| self.default_handler.clone())
                    } else {
                        self.default_handler.clone()
                    }
                ),
            });
        }
        if let ["default", handler, "x-scheme-handler/ghostlink"] = args
            && program == "xdg-mime"
        {
            self.default_handler = (*handler).to_owned();
        }
        if program == "busctl" {
            return Ok(CommandOutput {
                success: false,
                stdout: String::new(),
            });
        }
        Ok(CommandOutput {
            success: true,
            stdout: String::new(),
        })
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_cleanup_absent_is_read_only() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(""));
    let plan = registrar.scoped_cleanup_plan().expect("plan");
    assert!(plan.actions.is_empty());
    assert!(!dir.paths().applications_dir.exists());
    let report = registrar.execute_scoped_cleanup(&plan);
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(report.completed.is_empty());
    assert!(registrar.runner.calls.iter().all(|call| call == QUERY));
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_active_default_without_owned_rewrite_preserves_handler() {
    for record in [None, Some(""), Some("invalid record")] {
        let dir = TestDir::new();
        let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));
        write(
            &registrar.desktop_file(),
            &registrar.desktop_entry().unwrap(),
        );
        write(
            &dir.paths().mimeapps_list,
            "[Default Applications]\ntext/html=browser.desktop;\n",
        );
        if let Some(record) = record {
            write(&dir.paths().record_file, record);
        }
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(plan.actions.is_empty(), "{plan:?}");
        assert!(!plan.skips.is_empty());
        let report = registrar.execute_scoped_cleanup(&plan);
        assert!(report.completed.is_empty());
        assert!(registrar.desktop_file().exists());
        if let Some(record) = record {
            assert_eq!(
                fs::read_to_string(&dir.paths().record_file).unwrap(),
                record
            );
        }
    }
}

// Fake query observes only the fixture's user default after a scoped rewrite.
fn fake_mime_default(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap();
    let mut in_defaults = false;
    for line in text.lines() {
        if line.starts_with('[') {
            in_defaults = line == "[Default Applications]";
        }
        if in_defaults && let Some(value) = scheme_value(line) {
            return value.split(';').next().unwrap_or_default().to_owned();
        }
    }
    String::new()
}

#[cfg(target_os = "linux")]
fn scoped_fixture(dir: &TestDir, default: &str) -> LinuxUriHandlerRegistrar<FakeRunner> {
    let mut registrar = registrar(dir, FakeRunner::with_default(default));
    registrar.runner.scoped_mime = Some(dir.paths().mimeapps_list);
    write(
        &registrar.desktop_file(),
        &registrar.desktop_entry().unwrap(),
    );
    write(&dir.paths().record_file, "previous.desktop\n");
    write(
        &dir.paths().mimeapps_list,
        "# untouched\n[Default Applications]\nx-scheme-handler/ghostlink=ghostlink-callback.desktop;fallback.desktop;\ntext/html=browser.desktop;\n[Added Associations]\nx-scheme-handler/ghostlink=foreign.desktop;ghostlink-callback.desktop;\n[Removed Associations]\nx-scheme-handler/ghostlink=ghostlink-callback.desktop;removed.desktop;\n",
    );
    registrar
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_exact_install_restores_in_shared_mime_and_preserves_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    fs::set_permissions(
        &dir.paths().mimeapps_list,
        fs::Permissions::from_mode(0o640),
    )
    .unwrap();
    let before = fs::read_to_string(&dir.paths().mimeapps_list).unwrap();
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert_eq!(plan.actions.len(), 3);
    assert!(plan.skips.is_empty());
    assert_eq!(
        fs::read_to_string(&dir.paths().mimeapps_list).unwrap(),
        before
    );
    assert!(registrar.runner.calls.iter().all(|c| c == QUERY));
    let report = registrar.execute_scoped_cleanup(&plan);
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(report.completed.len(), 3);
    assert!(!registrar.desktop_file().exists());
    assert!(!dir.paths().record_file.exists());
    let mime = fs::read_to_string(&dir.paths().mimeapps_list).unwrap();
    assert_eq!(
        mime,
        "# untouched\n[Default Applications]\nx-scheme-handler/ghostlink=previous.desktop;fallback.desktop;\ntext/html=browser.desktop;\n[Added Associations]\nx-scheme-handler/ghostlink=foreign.desktop;\n[Removed Associations]\nx-scheme-handler/ghostlink=removed.desktop;\n"
    );
    assert_eq!(
        fs::metadata(&dir.paths().mimeapps_list)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    assert!(registrar.runner.calls.iter().all(|c| c == QUERY));
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_foreign_default_is_not_replaced_by_previous() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, "selected.desktop");
    write(
        &dir.paths().mimeapps_list,
        "[Default Applications]\nx-scheme-handler/ghostlink=selected.desktop;ghostlink-callback.desktop;\ntext/html=browser.desktop;\n",
    );
    let plan = registrar.scoped_cleanup_plan().unwrap();
    let report = registrar.execute_scoped_cleanup(&plan);
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(registrar.runner.default_handler, "selected.desktop");
    assert_eq!(
        fs::read_to_string(&dir.paths().mimeapps_list).unwrap(),
        "[Default Applications]\nx-scheme-handler/ghostlink=selected.desktop;\ntext/html=browser.desktop;\n"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_changed_default_since_preview_refuses_all_mutation() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    let plan = registrar.scoped_cleanup_plan().unwrap();
    registrar.runner.default_handler = "new.desktop".into();
    let report = registrar.execute_scoped_cleanup(&plan);
    assert_eq!(report.errors.len(), 1);
    assert!(report.completed.is_empty());
    assert!(registrar.desktop_file().exists());
    assert!(dir.paths().record_file.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_modified_and_other_install_desktops_preserve_everything() {
    for change in [
        "Exec=/other/ghostlink callback %u",
        "Name=Custom",
        "Exec=/opt/ghostlink/bin/ghostlink callback %u\nExec=/evil",
    ] {
        let dir = TestDir::new();
        let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
        write(
            &registrar.desktop_file(),
            &format!("{}\n{change}\n", registrar.desktop_entry().unwrap()),
        );
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(plan.actions.is_empty());
        assert!(!plan.skips.is_empty());
        let report = registrar.execute_scoped_cleanup(&plan);
        assert!(report.completed.is_empty());
        assert!(dir.paths().record_file.exists());
        assert!(registrar.runner.calls.is_empty());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_orphan_and_invalid_records_are_preserved() {
    let dir = TestDir::new();
    write(&dir.paths().record_file, "previous.desktop\n");
    let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert!(plan.actions.is_empty());
    assert!(plan.skips[0].guidance.contains("Orphan"));
    for record in [
        "../evil.desktop\n",
        "previous.desktop\nother.desktop\n",
        " previous.desktop\n",
        "ghostlink-callback.desktop\n",
    ] {
        let dir = TestDir::new();
        let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
        write(&dir.paths().record_file, record);
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(!plan.skips.is_empty());
        let report = registrar.execute_scoped_cleanup(&plan);
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(
            fs::read_to_string(&dir.paths().record_file).unwrap(),
            record
        );
        assert!(
            !fs::read_to_string(&dir.paths().mimeapps_list)
                .unwrap()
                .contains(DESKTOP_ID)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_empty_record_never_restores_and_ambiguous_mime_is_preserved() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    write(&dir.paths().record_file, "\n");
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert!(matches!(
        &plan.actions[0],
        HandlerCleanupAction::RewriteMime { restore: None, .. }
    ));
    assert!(registrar.execute_scoped_cleanup(&plan).errors.is_empty());
    for mime in [
        "[Default Applications]\nx-scheme-handler/ghostlink=ghostlink-callback.desktop;\nx-scheme-handler/ghostlink=other.desktop;\n",
        "[Custom]\nx-scheme-handler/ghostlink=ghostlink-callback.desktop;\n",
        "[Default Applications]\nx-scheme-handler/ghostlink=other.desktop;ghostlink-callback.desktop;\n",
    ] {
        let dir = TestDir::new();
        let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
        write(&dir.paths().mimeapps_list, mime);
        registrar.runner.scoped_mime = None; // Model a higher-priority Ghostlink default.
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(plan.actions.is_empty(), "{plan:?}");
        assert!(!plan.skips.is_empty());
        assert_eq!(
            fs::read_to_string(&dir.paths().mimeapps_list).unwrap(),
            mime
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_symlink_leaves_and_ancestors_are_preserved() {
    use std::os::unix::fs::symlink;
    // No fixture deletion: inject fresh symlink paths instead.
    for leaf in [true, false] {
        let dir = TestDir::new();
        let outside = dir.0.join("outside");
        write(&outside.join(DESKTOP_ID), "untouched");
        let mut paths = dir.paths();
        if leaf {
            fs::create_dir_all(&paths.applications_dir).unwrap();
            symlink(
                outside.join(DESKTOP_ID),
                paths.applications_dir.join(DESKTOP_ID),
            )
            .unwrap();
        } else {
            paths.applications_dir = dir.0.join("linked");
            symlink(&outside, &paths.applications_dir).unwrap();
        }
        let mut registrar =
            LinuxUriHandlerRegistrar::new(paths, PathBuf::from(EXE), FakeRunner::default());
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(plan.actions.is_empty());
        assert!(plan.skips.iter().any(|s| s.guidance.contains("symlink")));
        assert!(registrar.execute_scoped_cleanup(&plan).completed.is_empty());
        assert_eq!(
            fs::read_to_string(outside.join(DESKTOP_ID)).unwrap(),
            "untouched"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_bounded_read_and_snapshot_change_preserve_files() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    let plan = registrar.scoped_cleanup_plan().unwrap();
    write(&dir.paths().record_file, "changed.desktop\n");
    let report = registrar.execute_scoped_cleanup(&plan);
    assert_eq!(report.errors.len(), 1);
    assert!(report.completed.is_empty());
    write(&dir.paths().record_file, &"a".repeat(READ_LIMIT + 1));
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert!(plan.actions.is_empty());
    assert!(plan.skips.iter().any(|s| s.guidance.contains("limit")));
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_temp_collision_is_structured_and_preserves_existing_file() {
    use std::os::unix::fs::symlink;
    for linked in [false, true] {
        let dir = TestDir::new();
        let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
        let temp = dir
            .paths()
            .mimeapps_list
            .with_file_name(".mimeapps.list.ghostlink-scoped-tmp");
        if linked {
            let target = dir.0.join("target");
            write(&target, "keep");
            symlink(target, &temp).unwrap();
        } else {
            write(&temp, "keep");
        }
        let plan = registrar.scoped_cleanup_plan().unwrap();
        let report = registrar.execute_scoped_cleanup(&plan);
        assert_eq!(report.errors.len(), 1);
        assert!(report.completed.is_empty());
        assert_eq!(fs::read_to_string(&temp).unwrap(), "keep");
        assert!(registrar.desktop_file().exists());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_still_selected_after_mime_rewrite_preserves_desktop_and_record() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    registrar.runner.scoped_mime = None; // Higher-priority desktop-specific MIME file.
    let plan = registrar.scoped_cleanup_plan().unwrap();
    let report = registrar.execute_scoped_cleanup(&plan);
    assert_eq!(report.completed.len(), 1);
    assert_eq!(report.errors.len(), 1);
    assert!(registrar.desktop_file().exists());
    assert!(dir.paths().record_file.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_missing_mime_with_current_default_requires_manual_cleanup() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));
    write(
        &registrar.desktop_file(),
        &registrar.desktop_entry().unwrap(),
    );
    write(&dir.paths().record_file, "previous.desktop\n");
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert!(plan.actions.is_empty());
    assert!(!plan.skips.is_empty());
    assert!(registrar.execute_scoped_cleanup(&plan).completed.is_empty());
    assert!(registrar.desktop_file().exists());
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_record_and_mime_symlinks_require_manual_cleanup() {
    use std::os::unix::fs::symlink;
    for record in [false, true] {
        let dir = TestDir::new();
        let mut paths = dir.paths();
        let target = dir.0.join("untouched");
        write(&target, "previous.desktop\n");
        let linked = dir.0.join("linked");
        symlink(&target, &linked).unwrap();
        if record {
            paths.record_file = linked;
        } else {
            paths.mimeapps_list = linked;
        }
        let mut registrar = LinuxUriHandlerRegistrar::new(
            paths,
            PathBuf::from(EXE),
            FakeRunner::with_default(DESKTOP_ID),
        );
        write(
            &registrar.desktop_file(),
            &registrar.desktop_entry().unwrap(),
        );
        let plan = registrar.scoped_cleanup_plan().unwrap();
        assert!(plan.actions.is_empty());
        assert!(plan.skips.iter().any(|s| s.guidance.contains("symlink")));
        assert_eq!(fs::read_to_string(&target).unwrap(), "previous.desktop\n");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_without_current_default_strips_only_owned_values() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, "");
    let plan = registrar.scoped_cleanup_plan().unwrap();
    assert!(matches!(
        &plan.actions[0],
        HandlerCleanupAction::RewriteMime { restore: None, .. }
    ));
    let report = registrar.execute_scoped_cleanup(&plan);
    assert!(report.errors.is_empty(), "{report:?}");
    let mime = fs::read_to_string(&dir.paths().mimeapps_list).unwrap();
    assert!(!mime.contains("previous.desktop"));
    assert!(mime.contains("fallback.desktop"));
    assert!(mime.contains("text/html=browser.desktop;"));
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_public_preview_fields_cannot_expand_authority() {
    let dir = TestDir::new();
    let mut registrar = scoped_fixture(&dir, DESKTOP_ID);
    let outside = dir.0.join("unrelated");
    write(&outside, "keep");
    let mut plan = registrar.scoped_cleanup_plan().unwrap();
    plan.actions.push(HandlerCleanupAction::RemoveDesktop {
        path: outside.clone(),
    });
    let report = registrar.execute_scoped_cleanup(&plan);
    assert!(!report.errors.is_empty());
    assert!(report.completed.is_empty());
    assert_eq!(fs::read_to_string(outside).unwrap(), "keep");
}

#[cfg(target_os = "linux")]
#[test]
fn scoped_rechecks_after_query_and_reports_partial_success() {
    struct ChangingRunner {
        calls: usize,
        path: PathBuf,
        mime: PathBuf,
        at: usize,
    }
    impl HandlerCommandRunner for ChangingRunner {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, AppError> {
            assert_eq!(program, "xdg-mime");
            assert_eq!(args, ["query", "default", SCHEME_MIME]);
            self.calls += 1;
            if self.calls == self.at {
                write(&self.path, "external change\n");
            }
            Ok(CommandOutput {
                success: true,
                stdout: fake_mime_default(&self.mime),
            })
        }
    }
    for (at, completed) in [(3, 0), (4, 1), (5, 2)] {
        let dir = TestDir::new();
        let fixture = scoped_fixture(&dir, DESKTOP_ID);
        let mut registrar = LinuxUriHandlerRegistrar::new(
            dir.paths(),
            PathBuf::from(EXE),
            ChangingRunner {
                calls: 0,
                path: dir.paths().record_file,
                mime: dir.paths().mimeapps_list,
                at,
            },
        );
        let plan = registrar.scoped_cleanup_plan().unwrap();
        let report = registrar.execute_scoped_cleanup(&plan);
        assert_eq!(report.completed.len(), completed, "{report:?}");
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(
            fs::read_to_string(&dir.paths().record_file).unwrap(),
            "external change\n"
        );
        drop(fixture);
    }
}

const EXE: &str = "/opt/ghostlink/bin/ghostlink";

fn registrar(dir: &TestDir, runner: FakeRunner) -> LinuxUriHandlerRegistrar<FakeRunner> {
    LinuxUriHandlerRegistrar::new(dir.paths(), PathBuf::from(EXE), runner)
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("parent directory");
    std::fs::write(path, content).expect("write fixture");
}

fn installed_entry(dir: &TestDir, exec: &str) {
    write(
        &dir.paths().applications_dir.join(DESKTOP_ID),
        &format!("[Desktop Entry]\nType=Application\nExec={exec}\n"),
    );
}

fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
    move |key| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| OsString::from(value))
    }
}

#[test]
fn paths_prefer_absolute_xdg_directories_and_fall_back_to_home() {
    let paths = LinuxHandlerPaths::resolve(lookup(&[
        ("HOME", "/home/a"),
        ("XDG_DATA_HOME", "/xdg/data"),
        ("XDG_CONFIG_HOME", "relative/config"),
    ]))
    .expect("paths");

    assert_eq!(
        paths.applications_dir,
        PathBuf::from("/xdg/data/applications")
    );
    assert_eq!(
        paths.record_file,
        PathBuf::from("/xdg/data/ghostlink/previous-uri-handler")
    );
    assert_eq!(
        paths.mimeapps_list,
        PathBuf::from("/home/a/.config/mimeapps.list")
    );
}

#[test]
fn paths_without_an_absolute_home_are_a_configuration_error() {
    for pairs in [&[][..], &[("HOME", "relative")][..]] {
        let error = LinuxHandlerPaths::resolve(lookup(pairs)).expect_err("no home");
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
    }
}

#[test]
fn exec_values_quote_reserved_characters_and_refuse_unescapable_paths() {
    assert_eq!(
        exec_value(Path::new("/usr/bin/ghostlink")).expect("plain"),
        "/usr/bin/ghostlink callback %u"
    );
    assert_eq!(
        exec_value(Path::new("/opt/my tools/ghostlink")).expect("spaced"),
        "\"/opt/my tools/ghostlink\" callback %u"
    );
    for refused in [
        "relative/ghostlink",
        "/a$b/ghostlink",
        "/100%/ghostlink",
        "/a\"b",
    ] {
        let error = exec_value(Path::new(refused)).expect_err(refused);
        assert_eq!(error.category(), ErrorCategory::ConfigurationError);
    }
}

#[test]
fn an_empty_association_is_missing() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(""));

    let status = registrar.status().expect("status");

    assert_eq!(status.registration, HandlerRegistration::Missing);
    assert!(status.warnings.is_empty());
}

#[test]
fn another_application_is_reported_by_desktop_id() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));

    let status = registrar.status().expect("status");

    assert_eq!(
        status.registration,
        HandlerRegistration::Elsewhere {
            handler: "google-chrome.desktop".to_owned()
        }
    );
}

#[test]
fn the_entry_is_current_only_when_it_runs_this_executable() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));
    assert!(matches!(
        registrar.status().expect("missing entry").registration,
        HandlerRegistration::Elsewhere { .. }
    ));

    installed_entry(&dir, "/old/ghostlink callback %u");
    assert!(matches!(
        registrar.status().expect("other executable").registration,
        HandlerRegistration::Elsewhere { .. }
    ));

    installed_entry(&dir, &format!("{EXE} callback %u"));
    assert_eq!(
        registrar.status().expect("current").registration,
        HandlerRegistration::Current
    );
}

#[test]
fn an_unexpected_association_answer_is_refused() {
    let dir = TestDir::new();
    let runner = FakeRunner::default().respond(QUERY, true, "--evil;rm\n");
    let mut registrar = registrar(&dir, runner);

    let error = registrar.status().expect_err("refused");

    assert_eq!(error.category(), ErrorCategory::LocalStorage);
}

#[test]
fn missing_xdg_utils_make_the_registrar_unsupported() {
    let dir = TestDir::new();
    let mut runner = FakeRunner::default();
    runner.unavailable.push("xdg-mime");
    let mut registrar = registrar(&dir, runner);

    let error = registrar.status().expect_err("unsupported");

    assert_eq!(error.category(), ErrorCategory::UnsupportedOperation);
}

#[test]
fn the_install_plan_lists_every_change_and_changes_nothing() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));

    let plan = registrar.install_plan().expect("plan");

    assert_eq!(plan.changes.len(), 4, "{plan:?}");
    assert!(plan.changes[0].contains("google-chrome.desktop"));
    assert!(plan.changes[1].contains(&format!("{EXE} callback %u")));
    assert!(plan.changes[2].contains("update-desktop-database"));
    assert!(plan.changes[3].contains("xdg-mime default ghostlink-callback.desktop"));
    assert!(!dir.paths().applications_dir.exists());
    assert!(!dir.paths().record_file.exists());
    assert_eq!(registrar.runner.calls, [QUERY]);
}

#[test]
fn install_records_the_previous_handler_and_takes_effect() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));

    let report = registrar.install().expect("install");

    assert_eq!(report.changes.len(), 4, "{report:?}");
    let record = std::fs::read_to_string(&dir.paths().record_file).expect("record");
    assert_eq!(record, "google-chrome.desktop\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir.paths().record_file)
            .expect("record metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let entry =
        std::fs::read_to_string(dir.paths().applications_dir.join(DESKTOP_ID)).expect("entry");
    assert!(entry.contains(&format!("\nExec={EXE} callback %u\n")));
    assert!(entry.contains("MimeType=x-scheme-handler/ghostlink;"));
    assert_eq!(
        registrar.status().expect("status").registration,
        HandlerRegistration::Current
    );
}

#[test]
fn reinstalling_keeps_the_original_previous_handler() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));
    registrar.install().expect("first install");

    let plan = registrar.install_plan().expect("plan");
    registrar.install().expect("second install");

    assert_eq!(plan.changes.len(), 3, "{plan:?}");
    let record = std::fs::read_to_string(&dir.paths().record_file).expect("record");
    assert_eq!(record, "google-chrome.desktop\n");
}

#[test]
fn installing_over_its_own_association_records_no_previous_handler() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));

    registrar.install().expect("install");

    let record = std::fs::read_to_string(&dir.paths().record_file).expect("record");
    assert_eq!(record, "\n");
}

#[test]
fn a_failed_association_change_fails_the_install() {
    let dir = TestDir::new();
    let runner = FakeRunner::with_default("").respond(
        "xdg-mime default ghostlink-callback.desktop x-scheme-handler/ghostlink",
        false,
        "",
    );
    let mut registrar = registrar(&dir, runner);

    let error = registrar.install().expect_err("failed");

    assert_eq!(error.category(), ErrorCategory::LocalStorage);
}

#[test]
fn a_missing_desktop_database_tool_does_not_fail_the_install() {
    let dir = TestDir::new();
    let mut runner = FakeRunner::with_default("");
    runner.unavailable.push("update-desktop-database");
    let mut registrar = registrar(&dir, runner);

    let report = registrar.install().expect("install");

    assert!(
        report
            .changes
            .iter()
            .any(|change| change.contains("update-desktop-database") && change.contains("skipped"))
    );
}

#[test]
fn uninstall_restores_the_recorded_handler_and_removes_everything() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));
    registrar.install().expect("install");

    let report = registrar.uninstall().expect("uninstall");

    assert!(!report.changes.is_empty());
    assert!(!dir.paths().applications_dir.join(DESKTOP_ID).exists());
    assert!(!dir.paths().record_file.exists());
    assert_eq!(registrar.runner.default_handler, "google-chrome.desktop");
}

#[test]
fn uninstall_without_a_previous_handler_removes_only_this_association() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default(""));
    registrar.install().expect("install");
    write(
        &dir.paths().mimeapps_list,
        "[Default Applications]\n\
         x-scheme-handler/ghostlink=ghostlink-callback.desktop\n\
         text/html=firefox.desktop\n\
         [Added Associations]\n\
         x-scheme-handler/ghostlink=ghostlink-callback.desktop;other.desktop;\n",
    );

    registrar.uninstall().expect("uninstall");

    let list = std::fs::read_to_string(&dir.paths().mimeapps_list).expect("list");
    assert_eq!(
        list,
        "[Default Applications]\n\
         text/html=firefox.desktop\n\
         [Added Associations]\n\
         x-scheme-handler/ghostlink=other.desktop;\n"
    );
    let restores = registrar
        .runner
        .calls_to("xdg-mime default")
        .into_iter()
        .filter(|call| !call.contains(DESKTOP_ID))
        .count();
    assert_eq!(restores, 0);
}

#[test]
fn uninstalling_nothing_reports_no_changes() {
    let dir = TestDir::new();
    let mut registrar = registrar(&dir, FakeRunner::with_default("google-chrome.desktop"));

    let report = registrar.uninstall().expect("uninstall");

    assert!(report.changes.is_empty(), "{report:?}");
    assert!(registrar.runner.calls_to("xdg-mime default").is_empty());
}

#[test]
fn contradictory_mimeapps_sections_are_reported() {
    let dir = TestDir::new();
    installed_entry(&dir, &format!("{EXE} callback %u"));
    write(
        &dir.paths().mimeapps_list,
        "[Added Associations]\n\
         x-scheme-handler/ghostlink=google-chrome.desktop;\n\
         [Default Applications]\n\
         x-scheme-handler/ghostlink=ghostlink-callback.desktop\n",
    );
    let mut registrar = registrar(&dir, FakeRunner::with_default(DESKTOP_ID));

    let status = registrar.status().expect("status");

    assert_eq!(status.registration, HandlerRegistration::Current);
    assert_eq!(status.warnings.len(), 1);
    assert_eq!(
        status.warnings[0].kind,
        HandlerWarningKind::ConflictingAssociation
    );
}

#[test]
fn a_remembered_portal_choice_is_reported_with_its_fix() {
    let dir = TestDir::new();
    let runner = FakeRunner::with_default(DESKTOP_ID).respond(
        PORTAL_LOOKUP,
        true,
        "a{sas}v 2 \"com.google.Chrome\" 3 \"google-chrome\" \"3\" \"3\" \
         \"org.mozilla.firefox\" 3 \"ghostlink-callback\" \"3\" \"3\" s \"\"\n",
    );
    let mut registrar = registrar(&dir, runner);

    let status = registrar.status().expect("status");

    assert_eq!(status.warnings.len(), 1, "{status:?}");
    assert_eq!(
        status.warnings[0].kind,
        HandlerWarningKind::RememberedChoice
    );
    assert!(status.warnings[0].summary.contains("com.google.Chrome"));
    assert!(
        status.warnings[0]
            .remedy
            .contains("com.google.Chrome 3 ghostlink-callback 3 3")
    );
}

#[test]
fn an_unavailable_portal_is_not_a_warning() {
    let dir = TestDir::new();
    let mut runner = FakeRunner::with_default("");
    runner.unavailable.push("busctl");
    let mut registrar = registrar(&dir, runner);

    let status = registrar.status().expect("status");

    assert!(status.warnings.is_empty());
}

#[test]
fn paths_rendered_as_commands_are_shell_quoted() {
    let dir = TestDir::new();
    let mut paths = dir.paths();
    paths.applications_dir = dir.0.join("my data;id #/applications");
    let mut registrar = LinuxUriHandlerRegistrar::new(
        paths.clone(),
        PathBuf::from(EXE),
        FakeRunner::with_default(""),
    );

    let plan = registrar.install_plan().expect("plan");

    let expected = format!(
        "run: update-desktop-database '{}'",
        paths.applications_dir.display()
    );
    assert!(plan.changes.contains(&expected), "{plan:?}");
}

#[test]
fn an_existing_record_is_kept_but_restricted_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TestDir::new();
    let record = dir.paths().record_file;
    write(&record, "google-chrome.desktop\n");
    std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644))
        .expect("loosen record");
    let mut registrar = registrar(&dir, FakeRunner::with_default(""));

    let plan = registrar.install_plan().expect("plan");
    let report = registrar.install().expect("install");

    assert!(plan.changes[0].contains("0600"), "{plan:?}");
    assert!(report.changes[0].contains("0600"), "{report:?}");
    let mode = std::fs::metadata(&record)
        .expect("record metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(
        std::fs::read_to_string(&record).expect("record"),
        "google-chrome.desktop\n"
    );
}
