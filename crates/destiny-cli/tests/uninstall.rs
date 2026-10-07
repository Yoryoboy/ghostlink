use destiny_cli::{OutputMode, command::Command, parse_process};

#[test]
fn uninstall_accepts_preview_and_consent_but_never_purge() {
    let cli = parse_process(["ghostlink", "uninstall", "--dry-run", "--yes"]).unwrap();
    let Command::Uninstall(args) = cli.command else {
        panic!("uninstall")
    };
    assert!(args.dry_run && args.yes);
    assert!(parse_process(["ghostlink", "uninstall", "--purge"]).is_err());
}

#[test]
fn windows_manual_refusal_is_one_error_object() {
    let out = destiny_cli::uninstall::windows_refusal(OutputMode::Json);
    assert_eq!(out.exit_code, 40);
    assert!(out.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_str(&out.stderr).unwrap();
    assert_eq!(value["error"]["code"], "unsupported_operation");
    assert!(out.stderr.contains("PowerShell"));
    assert!(out.stderr.contains("Nothing was looked up"));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use destiny_adapters::uri_handler::{
        CommandOutput, DESKTOP_ID, HandlerCommandRunner, LinuxHandlerPaths,
        LinuxUriHandlerRegistrar,
    };
    use destiny_cli::command::UninstallArgs;
    use destiny_cli::installation::Installation;
    use serde_json::{Value, json};
    use std::os::unix::fs::symlink;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct FakeRunner {
        default: String,
    }
    impl HandlerCommandRunner for FakeRunner {
        fn run(
            &mut self,
            program: &str,
            args: &[&str],
        ) -> Result<CommandOutput, destiny_app::AppError> {
            assert_eq!(program, "xdg-mime");
            assert_eq!(args, ["query", "default", "x-scheme-handler/ghostlink"]);
            Ok(CommandOutput {
                success: true,
                stdout: format!("{}\n", self.default),
            })
        }
    }
    struct Fixture {
        base: PathBuf,
        root: PathBuf,
        home: PathBuf,
        receipt: PathBuf,
        exe: PathBuf,
        paths: LinuxHandlerPaths,
    }
    impl Fixture {
        fn new(updater: bool) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let base = std::env::temp_dir().join(format!(
                "ghostlink-uninstall-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&base).unwrap();
            let root = base.join("cargo");
            let home = base.join("home");
            fs::create_dir_all(root.join("bin")).unwrap();
            fs::create_dir_all(home.join(".config/ghostlink")).unwrap();
            fs::create_dir_all(home.join(".local/share/ghostlink")).unwrap();
            let receipt = home.join(".config/ghostlink/ghostlink-receipt.json");
            let exe = root.join("bin/ghostlink");
            fs::write(&exe, "dummy CLI, never executed").unwrap();
            fs::write(root.join("bin/destiny-mcp"), "dummy MCP").unwrap();
            if updater {
                fs::write(
                    root.join("bin/ghostlink-update"),
                    "dummy updater, never executed",
                )
                .unwrap();
            }
            for path in [
                home.join(".config/ghostlink/config"),
                home.join(".local/share/ghostlink/database.sqlite"),
                home.join(".local/share/ghostlink/runtime.sock"),
                root.join("bin/unknown.bak"),
                root.join("env"),
            ] {
                fs::write(path, "preserve me").unwrap();
            }
            let paths = LinuxHandlerPaths::resolve(|key| {
                (key == "HOME").then(|| home.clone().into_os_string())
            })
            .unwrap();
            let f = Self {
                base,
                root,
                home,
                receipt,
                exe,
                paths,
            };
            f.write_receipt(f.json());
            f
        }
        fn json(&self) -> Value {
            json!({"install_prefix":self.root,"install_layout":"cargo-home","binaries":["ghostlink","destiny-mcp"],"cdylibs":[],"cstaticlibs":[],"binary_aliases":{},"version":"0.1.0","modify_path":true,
                "source":{"release_type":"github","owner":"Yoryoboy","name":"ghostlink","app_name":"ghostlink"},"provider":{"source":"cargo-dist","version":"0.32.0"}})
        }
        fn write_receipt(&self, value: Value) {
            fs::write(&self.receipt, value.to_string()).unwrap();
        }
        fn registrar(&self, default: &str) -> LinuxUriHandlerRegistrar<FakeRunner> {
            LinuxUriHandlerRegistrar::new(
                self.paths.clone(),
                self.exe.clone(),
                FakeRunner {
                    default: default.into(),
                },
            )
        }
        fn run(&self, dry_run: bool, yes: bool) -> destiny_cli::ProcessOutput {
            destiny_cli::uninstall::run_at(
                &UninstallArgs { dry_run, yes },
                OutputMode::Json,
                &self.receipt,
                &self.exe,
                &self.home,
                &mut self.registrar(""),
                false,
                |_| panic!("JSON never prompts"),
            )
        }
        fn lock_path(&self) -> PathBuf {
            self.root.join("bin/.ghostlink-lifecycle.lock")
        }
        fn owned_desktop(&self) {
            fs::create_dir_all(&self.paths.applications_dir).unwrap();
            fs::write(self.paths.applications_dir.join(DESKTOP_ID), format!("[Desktop Entry]\nType=Application\nName=Ghostlink OAuth Callback\nComment=Deliver a ghostlink://oauth/callback URI to a waiting ghostlink login\nExec={} callback %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/ghostlink;\nCategories=Network;\n", self.exe.display())).unwrap();
        }
    }
    // Read fixture contents only; leaves all test artifacts in the private temp root.
    fn tree(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut result = Vec::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                result.push((path.clone(), vec![]));
                result.extend(tree(&path));
            } else {
                result.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
        result.sort();
        result
    }

    #[test]
    fn dry_run_is_byte_for_byte_read_only_even_without_updater_or_lock() {
        let f = Fixture::new(false);
        let before = tree(&f.base);
        let out = f.run(true, false);
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        let value: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(value["status"], "preview");
        assert!(
            value["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("absent")
        );
        assert_eq!(tree(&f.base), before);
        assert!(!f.lock_path().exists());
    }

    #[test]
    fn yes_removes_only_known_files_and_preserves_state_and_lock_inode() {
        for updater in [true, false] {
            let f = Fixture::new(updater);
            let before = tree(&f.base);
            let out = f.run(false, true);
            assert_eq!(out.exit_code, 0, "{}", out.stderr);
            let value: Value = serde_json::from_str(&out.stdout).unwrap();
            assert_eq!(value["status"], "removed_with_manual_followups");
            assert_eq!(
                value["removed"].as_array().unwrap().len(),
                if updater { 4 } else { 3 }
            );
            assert_eq!(value["manual_followups"].as_array().unwrap().len(), 3);
            for (path, bytes) in before {
                if path.is_dir()
                    || path == f.exe
                    || path == f.receipt
                    || path == f.root.join("bin/destiny-mcp")
                    || path == f.root.join("bin/ghostlink-update")
                {
                    continue;
                }
                assert_eq!(fs::read(path).unwrap(), bytes);
            }
            assert!(f.root.join("bin").exists());
            assert!(f.lock_path().is_file());
            assert!(!f.exe.exists());
            assert!(!f.receipt.exists());
        }
    }

    #[test]
    fn no_yes_json_noninteractive_and_default_no_never_create_lock() {
        let f = Fixture::new(false);
        let before = tree(&f.base);
        let out = f.run(false, false);
        assert_eq!(out.exit_code, 2);
        assert_eq!(
            serde_json::from_str::<Value>(&out.stderr).unwrap()["error"]["code"],
            "confirmation_required"
        );
        let out = destiny_cli::uninstall::run_at(
            &UninstallArgs {
                dry_run: false,
                yes: false,
            },
            OutputMode::Human,
            &f.receipt,
            &f.exe,
            &f.home,
            &mut f.registrar(""),
            true,
            |preview| {
                assert!(preview.contains("planned_removals"));
                assert!(preview.contains("manual_followups"));
                assert!(preview.contains("preserved_state"));
                false
            },
        );
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("cancelled"));
        assert_eq!(tree(&f.base), before);
    }

    #[test]
    fn wrong_receipt_root_source_layout_and_paths_refuse() {
        for (key, value) in [
            ("install_prefix", json!("/")),
            ("install_layout", json!("flat")),
            ("binaries", json!(["ghostlink", "../destiny-mcp"])),
            ("source", json!({"owner":"foreign"})),
        ] {
            let f = Fixture::new(false);
            let mut receipt = f.json();
            receipt[key] = value;
            f.write_receipt(receipt);
            assert_eq!(f.run(false, true).exit_code, 40);
            assert!(f.exe.exists());
            assert!(!f.lock_path().exists());
        }
        let f = Fixture::new(false);
        let out = destiny_cli::uninstall::run_at(
            &UninstallArgs {
                dry_run: false,
                yes: true,
            },
            OutputMode::Json,
            &f.receipt,
            &f.root.join("bin/destiny-mcp"),
            &f.home,
            &mut f.registrar(""),
            false,
            |_| panic!(),
        );
        assert_eq!(out.exit_code, 40);
        let link = f.base.join("receipt-link");
        symlink(&f.receipt, &link).unwrap();
        assert!(Installation::validate_uninstall(&link, &f.exe).is_err());
        let linkroot = f.base.join("cargo-link");
        symlink(&f.root, &linkroot).unwrap();
        let mut receipt = f.json();
        receipt["install_prefix"] = json!(linkroot);
        f.write_receipt(receipt);
        assert_eq!(f.run(false, true).exit_code, 40);
    }

    #[test]
    fn optional_unsafe_updater_is_skipped_not_followed() {
        let f = Fixture::new(false);
        symlink(&f.receipt, f.root.join("bin/ghostlink-update")).unwrap();
        let out = f.run(false, true);
        assert_eq!(out.exit_code, 0, "{}", out.stderr);
        assert!(
            fs::symlink_metadata(f.root.join("bin/ghostlink-update"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(out.stdout.contains("unsafe optional updater preserved"));
    }

    #[test]
    fn lock_contention_refuses_and_preview_never_takes_lock() {
        let f = Fixture::new(false);
        let install = Installation::validate_uninstall(&f.receipt, &f.exe).unwrap();
        let _lock = install.lock().unwrap();
        assert_eq!(f.run(true, false).exit_code, 0);
        assert_eq!(f.run(false, true).exit_code, 43);
        assert!(f.exe.exists());
        assert!(f.receipt.exists());
    }

    #[test]
    fn changed_snapshot_after_consent_preserves_every_binary() {
        let f = Fixture::new(false);
        let out = destiny_cli::uninstall::run_at(
            &UninstallArgs {
                dry_run: false,
                yes: false,
            },
            OutputMode::Human,
            &f.receipt,
            &f.exe,
            &f.home,
            &mut f.registrar(""),
            true,
            |_| {
                fs::write(f.root.join("bin/destiny-mcp"), "changed after preview").unwrap();
                true
            },
        );
        assert_eq!(out.exit_code, 41);
        assert!(out.stderr.contains("snapshots changed"));
        assert!(f.exe.exists());
        assert!(f.receipt.exists());
    }

    #[test]
    fn owned_active_handler_unsafe_skip_retains_binaries_and_receipt() {
        let f = Fixture::new(false);
        f.owned_desktop();
        fs::create_dir_all(f.paths.mimeapps_list.parent().unwrap()).unwrap();
        fs::write(
            &f.paths.mimeapps_list,
            "[Default Applications]\ntext/html=browser.desktop;\n",
        )
        .unwrap();
        let out = destiny_cli::uninstall::run_at(
            &UninstallArgs {
                dry_run: false,
                yes: true,
            },
            OutputMode::Json,
            &f.receipt,
            &f.exe,
            &f.home,
            &mut f.registrar(DESKTOP_ID),
            false,
            |_| panic!(),
        );
        assert_eq!(out.exit_code, 41, "{}", out.stdout);
        let value: Value = serde_json::from_str(&out.stderr).unwrap();
        assert_eq!(value["status"], "partial");
        assert_eq!(value["error"]["code"], "partial_failure");
        assert!(!value["handler_skips"].as_array().unwrap().is_empty());
        assert!(f.exe.exists());
        assert!(f.receipt.exists());
        assert!(f.paths.applications_dir.join(DESKTOP_ID).exists());
    }

    #[test]
    fn handler_filesystem_failure_retains_cli_and_receipt() {
        let f = Fixture::new(false);
        f.owned_desktop();
        fs::create_dir_all(f.paths.mimeapps_list.parent().unwrap()).unwrap();
        fs::write(
            &f.paths.mimeapps_list,
            format!("[Added Associations]\nx-scheme-handler/ghostlink={DESKTOP_ID};\n"),
        )
        .unwrap();
        fs::write(
            f.paths
                .mimeapps_list
                .with_file_name(".mimeapps.list.ghostlink-scoped-tmp"),
            "foreign temp",
        )
        .unwrap();
        let out = f.run(false, true);
        assert_eq!(out.exit_code, 41, "{}", out.stdout);
        assert!(
            !serde_json::from_str::<Value>(&out.stderr).unwrap()["errors"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(f.exe.exists());
        assert!(f.receipt.exists());
    }

    #[test]
    fn scoped_opencode_only_edits_exact_installation_command_and_keeps_backup() {
        for exact in [true, false] {
            let f = Fixture::new(false);
            let dir = f.home.join(".config/opencode");
            fs::create_dir_all(&dir).unwrap();
            let command = if exact {
                f.root.join("bin/destiny-mcp")
            } else {
                PathBuf::from("destiny-mcp")
            };
            let value = json!({"other":"keep","mcp":{"ghostlink":{"type":"local","command":[command]},"foreign":{"type":"remote"}}});
            fs::write(dir.join("opencode.json"), value.to_string()).unwrap();
            fs::write(dir.join("opencode.json.bak"), "unknown backup").unwrap();
            let out = f.run(false, true);
            assert_eq!(out.exit_code, 0, "{}", out.stderr);
            let result: Value =
                serde_json::from_slice(&fs::read(dir.join("opencode.json")).unwrap()).unwrap();
            assert_eq!(result["mcp"].get("ghostlink").is_none(), exact);
            assert_eq!(result["mcp"]["foreign"], value["mcp"]["foreign"]);
            assert_eq!(result["other"], "keep");
            assert_eq!(
                fs::read_to_string(dir.join("opencode.json.bak")).unwrap(),
                "unknown backup"
            );
        }
    }

    #[test]
    fn agent_filesystem_failure_is_partial_not_manual_registration_success() {
        let f = Fixture::new(false);
        fs::create_dir_all(f.home.join(".config/opencode/opencode.json")).unwrap();
        let out = f.run(false, true);
        assert_eq!(out.exit_code, 41);
        let report: Value = serde_json::from_str(&out.stderr).unwrap();
        assert!(
            report["agents"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["status"] == "failed")
        );
        assert!(f.exe.exists());
        assert!(f.receipt.exists());
        assert_eq!(
            report["retained_installation_files"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn symlink_product_and_wrong_receipt_filename_refuse_before_lock() {
        let f = Fixture::new(false);
        let root = f.base.join("alternate");
        fs::create_dir_all(root.join("bin")).unwrap();
        let exe = root.join("bin/ghostlink");
        fs::write(&exe, "dummy CLI").unwrap();
        symlink(f.root.join("bin/destiny-mcp"), root.join("bin/destiny-mcp")).unwrap();
        let mut receipt = f.json();
        receipt["install_prefix"] = json!(root);
        f.write_receipt(receipt);
        assert!(Installation::validate_uninstall(&f.receipt, &exe).is_err());
        assert!(!root.join("bin/.ghostlink-lifecycle.lock").exists());
        f.write_receipt(f.json());
        let renamed = f.base.join("unowned.json");
        fs::write(&renamed, f.json().to_string()).unwrap();
        assert!(Installation::validate_uninstall(&renamed, &f.exe).is_err());
    }

    #[test]
    fn early_dispatch_refuses_json_without_yes_before_all_factories() {
        use destiny_cli::{
            CallbackMode, CliError, LoginFactory, LoginOperations, ReadRuntime, RuntimeFactory,
        };
        struct PanicFactory;
        impl RuntimeFactory for PanicFactory {
            type Service = destiny_cli::ComposedReadService<destiny_cli::ProductionFoundations>;
            fn build(&mut self) -> Result<ReadRuntime<Self::Service>, CliError> {
                panic!("runtime")
            }
        }
        impl LoginFactory for PanicFactory {
            fn build(
                &mut self,
                _: destiny_app::auth_ux::AuthorizationMode,
                _: CallbackMode,
            ) -> Result<Box<dyn LoginOperations>, CliError> {
                panic!("login")
            }
        }
        impl destiny_cli::ManifestFactory for PanicFactory {
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                panic!("manifest")
            }
            fn definitions_missing(&mut self) -> Result<bool, CliError> {
                panic!("preflight")
            }
        }
        impl destiny_cli::runtime::WeeklyResetFactory for PanicFactory {
            fn weekly_reset(
                &mut self,
                _: Option<destiny_app::Timestamp>,
            ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
                panic!("weekly")
            }
        }
        let out = destiny_cli::runner::run_binary_with_public(
            &mut PanicFactory,
            &mut PanicFactory,
            &mut PanicFactory,
            &mut PanicFactory,
            &mut |_| panic!("presentation"),
            ["ghostlink", "--json", "uninstall"],
        );
        assert_eq!(out.exit_code, 2);
        assert!(out.stderr.contains("confirmation_required"));
        assert_eq!(
            destiny_cli::run_binary(["ghostlink", "--json", "uninstall"]).exit_code,
            2
        );
    }
}
