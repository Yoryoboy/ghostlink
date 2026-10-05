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
                stdout: format!("{}\n", self.default_handler),
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
