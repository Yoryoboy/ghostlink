use super::*;
use destiny_app::agents::{Plan, execute};
use std::sync::atomic::{AtomicUsize, Ordering};

fn fixture() -> LocalAgentFiles {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let home = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/agent-tests")
        .join(format!("{}-{id}", std::process::id()));
    fs::create_dir_all(home.join("bin")).unwrap();
    LocalAgentFiles::new(home.clone(), home.join("bin/ghostlink"), false)
}
fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}
fn text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}
struct Lookup(bool);
impl PathLookup for Lookup {
    fn resolve(&self, name: &str) -> Option<PathBuf> {
        self.0.then(|| PathBuf::from(format!("/fake/{name}")))
    }
}
#[derive(Default)]
struct FakeRunner {
    calls: Vec<(Agent, PathBuf, Vec<OsString>)>,
    replies: Vec<CommandOutput>,
}
impl CommandRunner for FakeRunner {
    fn run(
        &mut self,
        agent: Agent,
        program: &Path,
        args: &[OsString],
    ) -> Result<CommandOutput, IntegrationError> {
        self.calls.push((agent, program.into(), args.into()));
        Ok(if self.replies.is_empty() {
            reply(true, "")
        } else {
            self.replies.remove(0)
        })
    }
}
fn reply(success: bool, output: &str) -> CommandOutput {
    CommandOutput {
        success,
        exit_status: Some(if success { 0 } else { 1 }),
        output: output.into(),
    }
}
fn plan(action: Action, skill: bool, mcp: bool) -> Plan {
    Plan { action, skill, mcp }
}
#[test]
fn cli_defaults_and_step_selection_use_only_injected_locations_and_runner() {
    use destiny_cli::command::Command;
    for flag in [None, Some("--skill"), Some("--mcp")] {
        let mut files = fixture();
        write(&files.home.join("bin/destiny-mcp"), "fake");
        let mut argv = vec!["ghostlink", "agents", "install", "--agent", "pi", "--json"];
        argv.extend(flag);
        let cli = destiny_cli::parse_process(argv).unwrap();
        let Command::Agents(args) = cli.command else {
            panic!("agents");
        };
        let mut runner = FakeRunner::default();
        let output = destiny_cli::agents::run_agents(
            &args.command,
            destiny_cli::OutputMode::Json,
            &mut files,
            &Lookup(true),
            &mut runner,
        );
        assert_eq!(output.exit_code, 0);
        let json: Value = serde_json::from_str(&output.stdout).unwrap();
        assert_eq!(json["agents"][0]["skill"].is_null(), flag == Some("--mcp"));
        assert_eq!(json["agents"][0]["mcp"].is_null(), flag == Some("--skill"));
        assert_eq!(runner.calls.len(), usize::from(flag != Some("--skill")));
    }
}
#[test]
fn detection_matrix() {
    for agent in Agent::ALL {
        for on_path in [false, true] {
            for directory in [false, true] {
                let mut files = fixture();
                if directory {
                    fs::create_dir_all(files.home.join(agent.config_dir())).unwrap();
                }
                let reports = execute(
                    &[agent],
                    plan(Action::Status, false, false),
                    &mut files,
                    &Lookup(on_path),
                    &mut FakeRunner::default(),
                );
                assert_eq!(reports[0].detected, on_path || directory);
            }
        }
    }
}
#[test]
fn skill_lifecycle_and_foreign_protection() {
    for agent in Agent::ALL {
        let mut files = fixture();
        let dir = files.home.join(agent.skill_dir()).join("ghostlink");
        for (action, expected) in [
            (Action::Status, Outcome::NotInstalled),
            (Action::Install, Outcome::Installed),
            (Action::Install, Outcome::AlreadyInstalled),
        ] {
            assert_eq!(files.skill(agent, action), Ok(expected));
        }
        assert_eq!(text(&dir.join("SKILL.md")), SKILL);
        assert_eq!(text(&dir.join("references/exit-codes.md")), EXIT_CODES);
        write(&dir.join("personal.txt"), "keep");
        assert_eq!(files.skill(agent, Action::Uninstall), Ok(Outcome::Removed));
        assert_eq!(text(&dir.join("personal.txt")), "keep");
        let absent = files.skill(agent, Action::Uninstall);
        assert_eq!(absent, Ok(Outcome::NotInstalled));
        write(&dir.join("SKILL.md"), "---\nname: other\n---\n");
        for action in [Action::Uninstall, Action::Install] {
            assert_eq!(
                files.skill(agent, action),
                Err(IntegrationError::ForeignSkill)
            );
        }
    }
}
#[test]
fn absolute_sibling_resolution_and_missing_binary() {
    let mut files = fixture();
    let missing = Err(IntegrationError::MissingMcpBinary);
    assert_eq!(files.mcp_executable(), missing);
    for windows in [false, true] {
        files.windows = windows;
        assert_eq!(files.mcp_executable(), missing);
        let path = files.home.join("bin").join(mcp_filename(windows));
        write(&path, "fake");
        assert_eq!(files.mcp_executable(), Ok(path));
    }
    files.executable = "relative/ghostlink".into();
    assert_eq!(files.mcp_executable(), missing);
}
#[test]
fn exact_argv_duplicate_replacement_and_removal() {
    for agent in [Agent::Claude, Agent::Codex, Agent::Pi] {
        let mut files = fixture();
        let exe = files.home.join("bin/destiny-mcp");
        write(&exe, "fake");
        let mut runner = FakeRunner {
            replies: vec![
                reply(false, "already exists"),
                reply(true, ""),
                reply(true, ""),
            ],
            ..Default::default()
        };
        let report = execute(
            &[agent],
            plan(Action::Install, false, true),
            &mut files,
            &Lookup(true),
            &mut runner,
        );
        assert_eq!(report[0].mcp, Some(Outcome::AlreadyInstalled));
        let mut add: Vec<OsString> = vec!["mcp".into(), "add".into()];
        let mut remove: Vec<OsString> = vec!["mcp".into(), "remove".into()];
        if agent == Agent::Claude {
            add.extend(["--scope", "user"].map(Into::into));
            remove.extend(["--scope", "user"].map(Into::into));
        }
        add.extend(["ghostlink", "--"].map(Into::into));
        add.push(exe.into_os_string());
        remove.push("ghostlink".into());
        for (call, expected) in runner.calls.iter().zip([&add, &remove, &add]) {
            assert_eq!(&call.2, expected);
            assert_eq!(call.1, PathBuf::from(format!("/fake/{}", agent.name())));
        }
        runner.calls.clear();
        runner.replies = vec![reply(true, "ghostlink"), reply(true, "")];
        let report = execute(
            &[agent],
            plan(Action::Uninstall, false, true),
            &mut files,
            &Lookup(true),
            &mut runner,
        );
        assert_eq!(report[0].mcp, Some(Outcome::Removed));
        assert_eq!(runner.calls[0].2, ["mcp", "list"].map(OsString::from));
        assert_eq!(runner.calls[1].2, remove);
    }
}
#[test]
fn opencode_merge_backup_idempotency_and_uninstall() {
    let mut files = fixture();
    let exe = files.home.join("bin/destiny-mcp");
    let path = files.home.join(".config/opencode/opencode.json");
    let backup = path.with_file_name("opencode.json.ghostlink-backup");
    let original = "{\"theme\":\"dark\",\"mcp\":{\"other\":{\"enabled\":false}}}";
    write(&path, original);
    let result = files.opencode(Action::Install, Some(&exe));
    assert_eq!(result, Ok(Outcome::Installed));
    let json: Value = serde_json::from_str(&text(&path)).unwrap();
    assert_eq!(json["theme"], "dark");
    assert_eq!(json["mcp"]["other"]["enabled"], false);
    assert_eq!(
        json["mcp"]["ghostlink"]["command"][0],
        exe.to_str().unwrap()
    );
    assert_eq!(text(&backup), original);
    let result = files.opencode(Action::Install, Some(&exe));
    assert_eq!(result, Ok(Outcome::AlreadyInstalled));
    assert_eq!(
        files.opencode(Action::Uninstall, None),
        Ok(Outcome::Removed)
    );
    let json: Value = serde_json::from_str(&text(&path)).unwrap();
    assert!(json["mcp"].get("ghostlink").is_none());
    assert!(json["mcp"].get("other").is_some());
    assert_eq!(text(&backup), original);
    let result = files.opencode(Action::Uninstall, None);
    assert_eq!(result, Ok(Outcome::NotInstalled));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
    let before = text(&path);
    let mut next_run = LocalAgentFiles::new(files.home, files.executable, false);
    next_run.opencode(Action::Install, Some(&exe)).unwrap();
    assert_eq!(text(&backup), before);
}
#[test]
fn opencode_creation_and_manual_outcomes_leave_original_untouched() {
    let mut files = fixture();
    let exe = files.home.join("bin/destiny-mcp");
    let path = files.home.join(".config/opencode/opencode.json");
    let result = files.opencode(Action::Uninstall, None);
    assert_eq!(result, Ok(Outcome::NotInstalled));
    let result = files.opencode(Action::Install, Some(&exe));
    assert_eq!(result, Ok(Outcome::Installed));
    let json: Value = serde_json::from_str(&text(&path)).unwrap();
    assert_eq!(json["$schema"], "https://opencode.ai/config.json");
    for original in ["{//comment\n}", "not json", "[]", "{\"mcp\":null}"] {
        let mut files = fixture();
        let path = files.home.join(".config/opencode/opencode.json");
        write(&path, original);
        for action in [Action::Install, Action::Uninstall] {
            let result = files.opencode(action, Some(&exe)).unwrap();
            let Outcome::ManualStepRequired { snippet } = result else {
                panic!("manual");
            };
            assert!(snippet.contains("ghostlink"));
            assert_eq!(text(&path), original);
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        }
    }
}
#[test]
fn failed_steps_do_not_stop_other_agents_or_skills() {
    let mut files = fixture();
    write(&files.home.join("bin/destiny-mcp"), "fake");
    let mut runner = FakeRunner {
        replies: vec![reply(false, "failure"), reply(true, "")],
        ..Default::default()
    };
    let reports = execute(
        &[Agent::Claude, Agent::Codex],
        plan(Action::Install, true, true),
        &mut files,
        &Lookup(true),
        &mut runner,
    );
    assert_eq!(reports[0].skill, Some(Outcome::Installed));
    let error = IntegrationError::Command {
        agent: Agent::Claude,
        exit_status: Some(1),
    };
    // Absence from PATH is a normal status, not a failed external command.
    let absent = execute(
        &[Agent::Pi],
        plan(Action::Status, false, true),
        &mut files,
        &Lookup(false),
        &mut runner,
    );
    assert_eq!(absent[0].mcp, Some(Outcome::NotInstalled));
    assert_eq!(reports[0].mcp, Some(Outcome::Failed(error)));
    assert_eq!(reports[1].mcp, Some(Outcome::Installed));
    assert_eq!(reports[1].skill, Some(Outcome::Installed));
}
