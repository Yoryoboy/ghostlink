use destiny_app::agents::*;
use destiny_cli::{ProcessOutput, setup::*};

#[derive(Default)]
struct Prompt {
    offered: Vec<Agent>,
    selected: Vec<Agent>,
    mode: Option<InstallMode>,
    yes: bool,
    abort: usize,
    step: usize,
    text: String,
}
impl Prompt {
    fn next(&mut self) -> Result<(), Aborted> {
        self.step += 1;
        if self.abort == self.step {
            Err(Aborted)
        } else {
            Ok(())
        }
    }
}
impl Prompter for Prompt {
    fn agents(&mut self, detected: &[Agent]) -> Result<Vec<Agent>, Aborted> {
        self.next()?;
        self.offered = detected.to_vec();
        Ok(self.selected.clone())
    }
    fn mode(&mut self) -> Result<InstallMode, Aborted> {
        self.next()?;
        Ok(self.mode.unwrap_or(InstallMode::Both))
    }
    fn login(&mut self) -> Result<bool, Aborted> {
        self.next()?;
        Ok(self.yes)
    }
    fn show(&mut self, text: &str) {
        self.text.push_str(text);
    }
}
#[derive(Default)]
struct Engine {
    detected: Vec<Agent>,
    plans: Vec<(Vec<Agent>, bool, bool)>,
}
impl AgentEngine for Engine {
    fn detected(&self) -> Vec<Agent> {
        self.detected.clone()
    }
    fn install(&mut self, agents: &[Agent], plan: Plan) -> Vec<AgentReport> {
        assert_eq!(plan.action, Action::Install);
        self.plans.push((agents.to_vec(), plan.skill, plan.mcp));
        agents
            .iter()
            .map(|&agent| AgentReport {
                agent,
                detected: true,
                skill: plan.skill.then_some(Outcome::Installed),
                mcp: plan.mcp.then_some(Outcome::ManualStepRequired {
                    snippet: "manual-config-snippet".into(),
                }),
            })
            .collect()
    }
}
#[derive(Default)]
struct Marker(bool);
impl MarkerStore for Marker {
    fn complete(&self) -> bool {
        self.0
    }
    fn write(&mut self) -> Result<(), std::io::Error> {
        self.0 = true;
        Ok(())
    }
}
fn run(p: &mut Prompt, e: &mut Engine, m: &mut Marker) -> (ProcessOutput, usize) {
    let mut calls = 0;
    let result = wizard(p, e, m, &mut || {
        calls += 1;
        ProcessOutput {
            stdout: "logged-in".into(),
            stderr: "manifest-notice".into(),
            exit_code: 0,
        }
    });
    (result, calls)
}
#[test]
fn detected_modes_outcomes_login_and_completion() {
    for (mode, skill, mcp) in [
        (InstallMode::Both, true, true),
        (InstallMode::Mcp, false, true),
        (InstallMode::Skill, true, false),
    ] {
        for yes in [false, true] {
            let mut p = Prompt {
                selected: vec![Agent::OpenCode, Agent::Pi],
                mode: Some(mode),
                yes,
                ..Default::default()
            };
            let mut e = Engine {
                detected: vec![Agent::Claude, Agent::OpenCode, Agent::Pi],
                ..Default::default()
            };
            let mut m = Marker::default();
            let (out, calls) = run(&mut p, &mut e, &mut m);
            assert_eq!(out.exit_code, 0);
            assert_eq!(p.offered, e.detected);
            assert_eq!(e.plans, [(vec![Agent::OpenCode, Agent::Pi], skill, mcp)]);
            assert_eq!(calls, usize::from(yes));
            assert!(m.0);
            assert!(p.text.contains("Done. Try asking your agent"));
            if mcp {
                assert!(p.text.contains("opencode: detected"));
                assert!(p.text.contains("manual-config-snippet"));
            }
            if skill {
                assert!(p.text.contains("pi: detected; skill: installed"));
            }
            if yes {
                assert!(p.text.contains("manifest-notice"));
            }
        }
    }
}
#[test]
fn none_detected_and_zero_selected_skip_install() {
    for detected in [vec![], vec![Agent::Pi]] {
        let mut p = Prompt::default();
        let mut e = Engine {
            detected,
            ..Default::default()
        };
        let mut m = Marker::default();
        let (out, calls) = run(&mut p, &mut e, &mut m);
        assert_eq!(out.exit_code, 0);
        assert!(e.plans.is_empty());
        assert_eq!(calls, 0);
        assert!(m.0);
        assert_eq!(p.step, if e.detected.is_empty() { 1 } else { 2 });
        if e.detected.is_empty() {
            assert!(p.text.contains("No supported agents detected"));
        }
    }
}
#[test]
fn abort_at_each_prompt_leaves_no_marker_and_preserves_finished_install() {
    for abort in 1..=3 {
        let mut p = Prompt {
            abort,
            selected: vec![Agent::Pi],
            ..Default::default()
        };
        let mut e = Engine {
            detected: vec![Agent::Pi],
            ..Default::default()
        };
        let mut m = Marker::default();
        let (out, calls) = run(&mut p, &mut e, &mut m);
        assert_eq!(out.exit_code, 130);
        assert!(out.stderr.contains("Setup cancelled"));
        assert!(!m.0);
        assert_eq!(calls, 0);
        assert_eq!(e.plans.len(), usize::from(abort == 3));
    }
}
#[test]
fn entry_routes_first_run_and_rejects_unattended_explicit_setup() {
    use clap::{CommandFactory, Parser};
    use destiny_cli::{Cli, Command, OutputMode};
    assert!(matches!(
        Cli::try_parse_from(["ghostlink"]).unwrap().command,
        Command::Help
    ));
    assert!(matches!(
        Cli::try_parse_from(["ghostlink", "setup"]).unwrap().command,
        Command::Setup
    ));
    let expected_help = Cli::command().render_long_help().to_string();
    assert!(expected_help.contains("Usage: ghostlink [OPTIONS] <COMMAND>"));
    for explicit in [false, true] {
        for tty in [false, true] {
            for json in [false, true] {
                for complete in [false, true] {
                    let mut calls = 0;
                    let out = entry(
                        explicit,
                        tty,
                        if json {
                            OutputMode::Json
                        } else {
                            OutputMode::Human
                        },
                        &mut Marker(complete),
                        &mut |_| {
                            calls += 1;
                            ProcessOutput {
                                stdout: "wizard".into(),
                                stderr: String::new(),
                                exit_code: 0,
                            }
                        },
                    );
                    if explicit && (!tty || json) {
                        assert_eq!(out.exit_code, 2);
                        assert_eq!(calls, 0);
                        assert!(out.stdout.is_empty());
                        assert!(out.stderr.contains("ghostlink agents install"));
                        assert!(out.stderr.contains("ghostlink login"));
                        if json {
                            let value: serde_json::Value =
                                serde_json::from_str(&out.stderr).unwrap();
                            assert_eq!(value["error"]["code"], "invalid_request");
                        }
                    } else if explicit || (tty && !json && !complete) {
                        assert_eq!(calls, 1);
                        assert_eq!(out.stdout, "wizard");
                    } else {
                        assert_eq!(calls, 0);
                        assert_eq!(out.stdout, expected_help);
                        assert_eq!(out.exit_code, 0);
                    }
                }
            }
        }
    }
}
#[test]
fn detected_choices_are_all_preselected() {
    assert_eq!(
        agent_choices(&[Agent::Claude, Agent::Pi]),
        (vec!["Claude Code", "Pi"], vec![true, true])
    );
}
#[test]
fn process_boundary_non_tty_and_json_setup_never_composes_login() {
    // Pipes and an empty environment make this safe even with --nocapture in
    // a real TTY: the production wizard cannot run or inspect user agent files.
    let binary = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_ghostlink"))
            .args(args)
            .env_clear()
            .output()
            .unwrap()
    };
    for args in [vec!["setup"], vec!["--json", "setup"]] {
        let out = binary(&args);
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(
            String::from_utf8(out.stderr)
                .unwrap()
                .contains("ghostlink login")
        );
    }
    let help = binary(&["--help"]);
    for args in [vec![], vec!["--json"]] {
        let out = binary(&args);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(out.stdout, help.stdout);
        assert_eq!(out.stderr, help.stderr);
    }
}
#[test]
fn first_run_gate() {
    for tty in [false, true] {
        for json in [false, true] {
            for complete in [false, true] {
                assert_eq!(should_setup(tty, json, complete), tty && !json && !complete);
            }
        }
    }
}

// Exercise the real PR-007 orchestration through ports, without filesystem
// access, PATH execution, or real agent subprocesses.
#[derive(Default)]
struct Files {
    skills: Vec<Agent>,
    opencode_calls: usize,
}
impl AgentFiles for Files {
    fn config_exists(&self, agent: Agent) -> bool {
        agent == Agent::OpenCode
    }
    fn skill(&mut self, agent: Agent, action: Action) -> Result<Outcome, IntegrationError> {
        assert_eq!(action, Action::Install);
        self.skills.push(agent);
        Ok(Outcome::AlreadyInstalled)
    }
    fn mcp_executable(&self) -> Result<std::path::PathBuf, IntegrationError> {
        Ok(std::env::current_dir().unwrap().join("fake-destiny-mcp"))
    }
    fn opencode(
        &mut self,
        action: Action,
        executable: Option<&std::path::Path>,
    ) -> Result<Outcome, IntegrationError> {
        assert_eq!(action, Action::Install);
        assert!(executable.unwrap().is_absolute());
        self.opencode_calls += 1;
        Ok(Outcome::ManualStepRequired {
            snippet: "opencode manual snippet".into(),
        })
    }
}
struct Lookup;
impl PathLookup for Lookup {
    fn resolve(&self, name: &str) -> Option<std::path::PathBuf> {
        (name == "pi").then(|| std::env::current_dir().unwrap().join("fake-pi"))
    }
}
#[derive(Default)]
struct Runner(usize);
impl CommandRunner for Runner {
    fn run(
        &mut self,
        agent: Agent,
        _: &std::path::Path,
        args: &[std::ffi::OsString],
    ) -> Result<CommandOutput, IntegrationError> {
        assert_eq!(agent, Agent::Pi);
        assert_eq!(
            &args[..3],
            ["mcp", "add", "ghostlink"].map(std::ffi::OsString::from)
        );
        self.0 += 1;
        Err(IntegrationError::Timeout { agent })
    }
}
#[test]
fn real_engine_detection_modes_manual_results_and_failures() {
    for mode in [InstallMode::Both, InstallMode::Mcp, InstallMode::Skill] {
        let mut files = Files::default();
        let mut runner = Runner::default();
        let mut p = Prompt {
            selected: vec![Agent::OpenCode, Agent::Pi, Agent::Codex],
            mode: Some(mode),
            ..Default::default()
        };
        let mut marker = Marker::default();
        let out = wizard(
            &mut p,
            &mut IntegrationEngine {
                files: &mut files,
                lookup: &Lookup,
                runner: &mut runner,
            },
            &mut marker,
            &mut || panic!("login was declined"),
        );
        assert_eq!(p.offered, [Agent::OpenCode, Agent::Pi]);
        assert!(marker.0);
        assert_eq!(
            files.skills.len(),
            if mode == InstallMode::Mcp { 0 } else { 2 }
        );
        if mode == InstallMode::Skill {
            assert_eq!(out.exit_code, 0);
            assert_eq!(runner.0, 0);
            assert_eq!(files.opencode_calls, 0);
        } else {
            assert_eq!(out.exit_code, 51);
            assert_eq!(runner.0, 1);
            assert_eq!(files.opencode_calls, 1);
            assert!(p.text.contains("opencode manual snippet"));
            assert!(p.text.contains("pi: detected"));
            assert!(p.text.contains("Timeout"));
        }
    }
}
#[test]
fn failed_login_still_reaches_end_and_marker_failure_is_nonzero() {
    let mut p = Prompt {
        yes: true,
        ..Default::default()
    };
    let mut marker = Marker::default();
    let out = wizard(&mut p, &mut Engine::default(), &mut marker, &mut || {
        ProcessOutput {
            stdout: String::new(),
            stderr: "fake login failure".into(),
            exit_code: 12,
        }
    });
    assert_eq!(out.exit_code, 12);
    assert!(marker.0);
    assert!(p.text.contains("fake login failure"));
    assert!(!p.text.contains("Done. Try asking"));
    struct Unwritable;
    impl MarkerStore for Unwritable {
        fn complete(&self) -> bool {
            false
        }
        fn write(&mut self) -> Result<(), std::io::Error> {
            Err(std::io::Error::other("fake storage failure"))
        }
    }
    let out = wizard(
        &mut Prompt::default(),
        &mut Engine::default(),
        &mut Unwritable,
        &mut || panic!("login was declined"),
    );
    assert_ne!(out.exit_code, 0);
    assert!(out.stderr.contains("completion marker"));
}
