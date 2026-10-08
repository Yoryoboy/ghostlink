use destiny_cli::{OutputMode, command::Command, parse_process};

#[test]
fn update_parses_default_and_explicit_version() {
    for (args, expected) in [
        (vec!["ghostlink", "update"], None),
        (
            vec!["ghostlink", "--json", "update", "--version", "1.2.3"],
            Some("1.2.3"),
        ),
    ] {
        let cli = parse_process(args).unwrap();
        let Command::Update(args) = cli.command else {
            panic!("expected update")
        };
        assert_eq!(args.version.as_deref(), expected);
    }
    assert!(parse_process(["ghostlink", "update", "--version"]).is_err());
    assert!(parse_process(["ghostlink", "update", "--tag", "v1"]).is_err());
}

#[test]
fn windows_is_actionable_and_does_not_defer() {
    let out = destiny_cli::update::windows_refusal(OutputMode::Json);
    assert_eq!(out.exit_code, 40);
    assert!(out.stdout.is_empty());
    let json: serde_json::Value = serde_json::from_str(&out.stderr).unwrap();
    assert_eq!(json["error"]["code"], "unsupported_operation");
    assert!(out.stderr.contains(".\\\\ghostlink-update.exe"));
    assert!(out.stderr.contains("Nothing was launched or deferred"));
}

#[cfg(all(target_os = "linux", target_arch = "x86_64", target_env = "gnu"))]
mod linux {
    use super::*;
    use destiny_cli::command::UpdateArgs;
    use destiny_cli::installation::{Installation, receipt_path};
    use serde_json::{Value, json};
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    // Unique isolated fixtures are deliberately left in the temporary directory;
    // these tests never delete a foreign file or touch an installed updater.
    struct Fixture {
        root: PathBuf,
        receipt: PathBuf,
        exe: PathBuf,
        updater: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let base = std::env::temp_dir().join(format!(
                "ghostlink-update-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&base).unwrap();
            let root = base.join("cargo");
            fs::create_dir_all(root.join("bin")).unwrap();
            let exe = root.join("bin/ghostlink");
            let updater = root.join("bin/ghostlink-update");
            fs::write(&exe, "fake product").unwrap();
            fs::write(root.join("bin/destiny-mcp"), "fake MCP").unwrap();
            let fixture = Self {
                root,
                receipt: base.join("ghostlink-receipt.json"),
                exe,
                updater,
            };
            fixture.write_receipt(&fixture.json());
            fixture.script("exit 0");
            fixture
        }
        fn json(&self) -> Value {
            json!({"install_prefix":self.root,"install_layout":"cargo-home", "binaries":["ghostlink","destiny-mcp"],
                "cdylibs":[],"cstaticlibs":[],"binary_aliases":{},"version":"0.1.0","modify_path":true,
                "source":{"release_type":"github","owner":"Yoryoboy","name":"ghostlink","app_name":"ghostlink"},
                "provider":{"source":"cargo-dist","version":"0.32.0"}})
        }
        fn write_receipt(&self, value: &Value) {
            fs::write(&self.receipt, value.to_string()).unwrap();
        }
        fn script(&self, body: &str) {
            fs::write(&self.updater, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&self.updater, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fn run(&self, version: Option<&str>) -> destiny_cli::ProcessOutput {
            destiny_cli::update::run_at(
                &UpdateArgs {
                    version: version.map(str::to_owned),
                },
                OutputMode::Json,
                &self.receipt,
                &self.exe,
            )
        }
    }

    #[test]
    fn official_receipt_default_latest_and_version_are_forwarded() {
        for version in [None, Some("1.2.3+build.1")] {
            let f = Fixture::new();
            f.script(if version.is_some() {
                "[ \"$#\" = 2 ] && [ \"$1\" = --version ] && [ \"$2\" = 1.2.3+build.1 ] || exit 9"
            } else {
                "[ \"$#\" = 0 ] || exit 9"
            });
            let out = f.run(version);
            assert_eq!(out.exit_code, 0, "{}", out.stderr);
            assert!(out.stderr.is_empty());
            let value: Value = serde_json::from_str(&out.stdout).unwrap();
            assert_eq!(value["kind"], "update");
            assert_eq!(value["status"], "updater_completed");
            assert_eq!(value["requested_version"], json!(version));
            assert_eq!(value["installed_version"], Value::Null);
            assert!(out.stdout.contains("Restart MCP hosts"));
            assert_eq!(fs::read_to_string(&f.exe).unwrap(), "fake product");
        }
    }

    #[test]
    fn receipt_lookup_matches_override_precedence_and_fallback() {
        let f = Fixture::new();
        let cwd = f.receipt.parent().unwrap();
        let xdg = cwd.join("xdg");
        let home = cwd.join("home");
        fs::create_dir_all(xdg.join("ghostlink")).unwrap();
        fs::create_dir_all(home.join(".config/ghostlink")).unwrap();
        let fallback = home.join(".config/ghostlink/ghostlink-receipt.json");
        fs::write(&fallback, "{}").unwrap();
        let lookup = |pairs: &[(&str, &str)]| {
            receipt_path(
                |k| {
                    pairs
                        .iter()
                        .find(|(key, _)| *key == k)
                        .map(|(_, v)| (*v).into())
                },
                cwd,
                Some(&home),
            )
            .unwrap()
        };
        assert_eq!(
            lookup(&[
                ("AXOUPDATER_CONFIG_WORKING_DIR", "0"),
                ("AXOUPDATER_CONFIG_PATH", "missing")
            ]),
            f.receipt
        );
        assert_eq!(lookup(&[("AXOUPDATER_CONFIG_PATH", ".")]), f.receipt);
        assert_eq!(
            lookup(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]),
            fallback
        );
        let xdg_receipt = xdg.join("ghostlink/ghostlink-receipt.json");
        fs::write(&xdg_receipt, "invalid JSON").unwrap();
        assert_eq!(
            lookup(&[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]),
            xdg_receipt
        );
        assert!(receipt_path(|_| None, cwd, None).is_err());
        assert!(
            receipt_path(
                |k| (k == "AXOUPDATER_CONFIG_PATH").then(|| "missing".into()),
                cwd,
                Some(&home)
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_receipts_roots_and_binary_boundaries_refuse_before_child() {
        let f = Fixture::new();
        f.script("exit 99");
        let original = f.json();
        let mut cases = Vec::new();
        for (pointer, replacement) in [
            ("/source/owner", json!("other")),
            ("/source/name", json!("other")),
            ("/source/app_name", json!("other")),
            ("/source/release_type", json!("axo")),
            ("/provider/source", json!("other")),
            ("/provider/version", json!("0.12.0")),
            ("/install_layout", json!("flat")),
            ("/install_layout", json!("hierarchical")),
            ("/install_prefix", json!("/")),
            ("/install_prefix", json!("relative")),
            (
                "/install_prefix",
                json!(format!("{}/../cargo", f.root.display())),
            ),
            ("/install_prefix", json!(format!("{}/.", f.root.display()))),
            ("/install_prefix", json!(format!("{}//", f.root.display()))),
            ("/install_prefix", json!(f.root.parent().unwrap())),
            ("/binaries", json!(["ghostlink", "../destiny-mcp"])),
            (
                "/binaries",
                json!(["ghostlink", "destiny-mcp", "ghostlink"]),
            ),
            ("/binaries", json!(["ghostlink", "/destiny-mcp"])),
            ("/binaries", json!(["ghostlink.exe", "destiny-mcp.exe"])),
            ("/cdylibs", json!(["libbad.so"])),
            ("/cstaticlibs", json!(["libbad.a"])),
            ("/binary_aliases", json!({"ghostlink":["other"]})),
        ] {
            let mut value = original.clone();
            *value.pointer_mut(pointer).unwrap() = replacement;
            cases.push(value);
        }
        cases.push(json!({}));
        for value in cases {
            f.write_receipt(&value);
            let out = f.run(None);
            assert_eq!(out.exit_code, 40, "{value}");
            assert!(out.stdout.is_empty());
            assert_eq!(
                serde_json::from_str::<Value>(&out.stderr).unwrap()["error"]["code"],
                "unsupported_installation"
            );
        }
        f.write_receipt(&original);
        let other = Fixture::new();
        assert!(Installation::validate(&f.receipt, &other.exe).is_err());
        assert!(Installation::validate(&f.receipt, &f.root.join("bin/destiny-mcp")).is_err());
        assert!(Installation::validate(&f.root.join("absent.json"), &f.exe).is_err());
        fs::write(&f.receipt, "not JSON").unwrap();
        assert_eq!(f.run(None).exit_code, 40);
        fs::write(&f.receipt, vec![b' '; 65537]).unwrap();
        assert_eq!(f.run(None).exit_code, 40);
    }

    #[test]
    fn symlink_components_and_missing_or_unsafe_updater_refuse() {
        let f = Fixture::new();
        let alias = f.root.with_file_name("root-alias");
        symlink(&f.root, &alias).unwrap();
        let mut r = f.json();
        r["install_prefix"] = json!(alias);
        f.write_receipt(&r);
        assert_eq!(f.run(None).exit_code, 40);
        f.write_receipt(&f.json());
        let receipt_alias = f.receipt.with_file_name("receipt-alias.json");
        symlink(&f.receipt, &receipt_alias).unwrap();
        assert!(Installation::validate(&receipt_alias, &f.exe).is_err());
        // Move only owned fixture targets; no deletion or real install involved.
        fs::rename(&f.updater, f.root.join("saved-updater")).unwrap();
        assert!(f.run(None).stderr.contains("updater_missing"));
        symlink(f.root.join("saved-updater"), &f.updater).unwrap();
        assert!(f.run(None).stderr.contains("updater_missing"));
        let missing = Fixture::new();
        fs::rename(
            missing.root.join("bin/destiny-mcp"),
            missing.root.join("saved-mcp"),
        )
        .unwrap();
        assert_eq!(missing.run(None).exit_code, 40);
        let g = Fixture::new();
        fs::rename(&g.exe, g.root.join("saved-product")).unwrap();
        symlink(g.root.join("saved-product"), &g.exe).unwrap();
        assert_eq!(g.run(None).exit_code, 40);
        let h = Fixture::new();
        fs::rename(h.root.join("bin"), h.root.join("saved-bin")).unwrap();
        symlink(h.root.join("saved-bin"), h.root.join("bin")).unwrap();
        assert_eq!(h.run(None).exit_code, 40);
    }

    #[test]
    fn bounded_concurrent_drain_and_sanitized_failure() {
        let f = Fixture::new();
        f.script("i=0; while [ $i -lt 12000 ]; do printf 'secret-stdout\\n'; printf 'secret-stderr\\n' >&2; i=$((i+1)); done");
        let out = f.run(None);
        assert_eq!(out.exit_code, 0);
        let json: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(json["stdout_truncated"], true);
        assert_eq!(json["stderr_truncated"], true);
        assert!(out.stdout.len() < 1500);
        assert!(!out.stdout.contains("secret-"));
        f.script("printf 'secret-token\\033[31m' >&2; exit 7");
        let out = f.run(None);
        assert_eq!(out.exit_code, 41);
        assert!(out.stdout.is_empty());
        assert!(!out.stderr.contains("secret-token"));
        assert_eq!(
            serde_json::from_str::<Value>(&out.stderr).unwrap()["error"]["code"],
            "updater_failed"
        );
        fs::set_permissions(&f.updater, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(f.run(None).exit_code, 40);
    }

    #[test]
    fn child_receipt_is_pinned_and_custom_installer_is_absent() {
        let f = Fixture::new();
        f.script("[ \"$AXOUPDATER_APP_NAME\" = ghostlink ] || exit 8\n[ -z \"${AXOUPDATER_INSTALLER_PATH+x}\" ] || exit 8\n[ -z \"${AXOUPDATER_CONFIG_WORKING_DIR+x}\" ] || exit 8\n[ \"$AXOUPDATER_CONFIG_PATH\" = \"${CARGO_HOME%/cargo}\" ] || exit 8");
        assert_eq!(f.run(None).exit_code, 0);
        let installation = Installation::validate(&f.receipt, &f.exe).unwrap();
        let command = destiny_cli::update::child_command(
            &installation,
            &UpdateArgs { version: None },
            [
                "AXOUPDATER_INSTALLER_PATH",
                "AXOUPDATER_CONFIG_WORKING_DIR",
                "AXOUPDATER_APP_NAME",
                "AXOUPDATER_GITHUB_TOKEN",
                "GHOSTLINK_DOWNLOAD_URL",
                "INSTALLER_DOWNLOAD_URL",
                "GHOSTLINK_INSTALL_DIR",
                "CARGO_DIST_FORCE_INSTALL_DIR",
                "GHOSTLINK_UNMANAGED_INSTALL",
            ]
            .map(Into::into),
        );
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for key in [
            "AXOUPDATER_INSTALLER_PATH",
            "AXOUPDATER_CONFIG_WORKING_DIR",
            "GHOSTLINK_DOWNLOAD_URL",
            "INSTALLER_DOWNLOAD_URL",
            "GHOSTLINK_INSTALL_DIR",
            "CARGO_DIST_FORCE_INSTALL_DIR",
            "GHOSTLINK_UNMANAGED_INSTALL",
        ] {
            assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None), "{key}");
        }
        assert!(!env.contains_key(std::ffi::OsStr::new("AXOUPDATER_GITHUB_TOKEN")));
        assert_eq!(command.get_program(), installation.updater.as_os_str());
    }

    #[test]
    fn lifecycle_lock_refuses_concurrency_and_foreign_symlinks() {
        let f = Fixture::new();
        let installation = Installation::validate(&f.receipt, &f.exe).unwrap();
        let lock = installation.lock().unwrap();
        assert_eq!(f.run(None).exit_code, 43);
        drop(lock);
        assert_eq!(f.run(None).exit_code, 0);
        let g = Fixture::new();
        symlink(&g.receipt, g.root.join("bin/.ghostlink-lifecycle.lock")).unwrap();
        assert_eq!(g.run(None).exit_code, 43);
    }

    #[test]
    fn early_dispatch_never_builds_application_factories() {
        use destiny_cli::{
            CallbackMode, CliError, LoginFactory, LoginOperations, ReadRuntime, RuntimeFactory,
        };
        struct PanicFactory;
        impl RuntimeFactory for PanicFactory {
            type Service = destiny_cli::ComposedReadService<destiny_cli::ProductionFoundations>;
            fn build(&mut self) -> Result<ReadRuntime<Self::Service>, CliError> {
                panic!("runtime factory accessed")
            }
        }
        impl LoginFactory for PanicFactory {
            fn build(
                &mut self,
                _: destiny_app::auth_ux::AuthorizationMode,
                _: CallbackMode,
            ) -> Result<Box<dyn LoginOperations>, CliError> {
                panic!("login factory accessed")
            }
        }
        impl destiny_cli::ManifestFactory for PanicFactory {
            fn sync(&mut self) -> Result<destiny_app::ManifestSyncReport, CliError> {
                panic!("manifest factory accessed")
            }
            fn definitions_missing(&mut self) -> Result<bool, CliError> {
                panic!("manifest preflight accessed")
            }
        }
        impl destiny_cli::runtime::WeeklyResetFactory for PanicFactory {
            fn weekly_reset(
                &mut self,
                _: Option<destiny_app::Timestamp>,
            ) -> Result<destiny_app::weekly_reset::WeeklyReset, CliError> {
                panic!("weekly factory accessed")
            }
        }
        // Invalid version routes into lifecycle validation, before even receipt
        // lookup, so no operator installation/configuration is inspected.
        let out = destiny_cli::runner::run_binary_with_public(
            &mut PanicFactory,
            &mut PanicFactory,
            &mut PanicFactory,
            &mut PanicFactory,
            &mut |_| panic!("presentation accessed"),
            ["ghostlink", "--json", "update", "--version", "bad/value"],
        );
        assert_eq!(out.exit_code, 2);
        assert_eq!(
            serde_json::from_str::<Value>(&out.stderr).unwrap()["error"]["code"],
            "invalid_request"
        );
        let out =
            destiny_cli::run_binary(["ghostlink", "--json", "update", "--version", "bad/value"]);
        assert_eq!(out.exit_code, 2);
    }
}
