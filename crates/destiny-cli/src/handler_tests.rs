use super::*;
use destiny_app::ErrorCategory;
use destiny_app::uri_handler::{
    CallbackChoice, HandlerRegistration, HandlerStatus, HandlerWarning, HandlerWarningKind,
    UnsupportedRegistrar,
};

#[derive(Default)]
struct FakeRegistrar {
    calls: Vec<&'static str>,
    registration: Option<HandlerRegistration>,
}

fn warning() -> HandlerWarning {
    HandlerWarning {
        kind: HandlerWarningKind::RememberedChoice,
        summary: "remembered\nchoice".into(),
        remedy: "reset\x1bchoice".into(),
    }
}

impl UriHandlerRegistrar for FakeRegistrar {
    fn status(&mut self) -> Result<HandlerStatus, AppError> {
        self.calls.push("status");
        Ok(HandlerStatus {
            registration: self
                .registration
                .clone()
                .unwrap_or(HandlerRegistration::Missing),
            warnings: vec![warning()],
        })
    }
    fn install_plan(&mut self) -> Result<HandlerChanges, AppError> {
        panic!("non-interactive path must not plan")
    }
    fn install(&mut self) -> Result<HandlerChanges, AppError> {
        self.calls.push("install");
        Ok(HandlerChanges {
            changes: vec!["write entry".into(), "set association".into()],
        })
    }
    fn uninstall(&mut self) -> Result<HandlerChanges, AppError> {
        self.calls.push("uninstall");
        Ok(HandlerChanges::default())
    }
}

#[test]
fn consent_accepts_only_complete_bounded_yes_lines() {
    for (line, accepted) in [
        ("y\n", true),
        (" YES \n", true),
        ("N\n", false),
        ("\n", false),
        ("garbage\n", false),
        ("", false),
    ] {
        let mut prompt = TerminalConsent {
            reader: line.as_bytes(),
            writer: Vec::new(),
        };
        assert_eq!(
            prompt
                .confirm(&HandlerChanges {
                    changes: vec!["write entry".into(), "set association".into()]
                })
                .unwrap(),
            accepted
        );
        let text = String::from_utf8(prompt.writer).unwrap();
        assert!(text.contains("1. write entry\n2. set association\n"));
        assert!(text.ends_with("Install the handler now? [y/N] "));
    }
    let line = format!("yes{}\n", " ".repeat(1024));
    let mut prompt = TerminalConsent {
        reader: line.as_bytes(),
        writer: Vec::new(),
    };
    assert!(!prompt.confirm(&HandlerChanges::default()).unwrap());
}

#[test]
fn noninteractive_and_headless_never_prompt_or_install() {
    struct NeverPrompt;
    impl InstallConsent for NeverPrompt {
        fn confirm(&mut self, _: &HandlerChanges) -> Result<bool, AppError> {
            panic!("prompted")
        }
    }
    for (input, output, mode) in [
        (false, true, OutputMode::Human),
        (true, false, OutputMode::Human),
        (true, true, OutputMode::Json),
    ] {
        let mut registrar = FakeRegistrar::default();
        let result = resolve_callback(&mut registrar, true, input, output, mode, &mut NeverPrompt);
        assert_eq!(result.choice, CallbackChoice::Paste);
        assert_eq!(registrar.calls, ["status"]);
        assert!(matches!(
            result.notices.as_slice(),
            [AutoCallbackNotice::NotInteractive(_)]
        ));
    }
    let mut registrar = FakeRegistrar::default();
    let result = resolve_callback(
        &mut registrar,
        false,
        true,
        true,
        OutputMode::Human,
        &mut NeverPrompt,
    );
    assert_eq!(result.choice, CallbackChoice::Paste);
    assert!(registrar.calls.is_empty());
    registrar.registration = Some(HandlerRegistration::Current);
    assert_eq!(
        resolve_callback(
            &mut registrar,
            true,
            false,
            false,
            OutputMode::Json,
            &mut NeverPrompt
        )
        .choice,
        CallbackChoice::Handler
    );
}

#[test]
fn every_notice_is_one_sanitized_line() {
    let notices = [
        AutoCallbackNotice::NoLocalBrowser,
        AutoCallbackNotice::StatusUnavailable(ErrorCategory::UnsupportedOperation),
        AutoCallbackNotice::NotInteractive(HandlerRegistration::Missing),
        AutoCallbackNotice::PromptFailed(ErrorCategory::UnsupportedOperation),
        AutoCallbackNotice::Declined,
        AutoCallbackNotice::Installed(HandlerChanges {
            changes: vec!["write\nentry".into()],
        }),
        AutoCallbackNotice::InstallFailed(ErrorCategory::UnsupportedOperation),
        AutoCallbackNotice::InstallNotEffective,
        AutoCallbackNotice::Warnings(vec![warning()]),
    ];
    for notice in notices {
        let text = render_notice(&notice);
        assert!(!text.is_empty());
        assert!(!text.chars().any(char::is_control));
    }
    assert!(render_notice(&AutoCallbackNotice::Declined).contains("ghostlink handler install"));
    assert!(render_notice(&AutoCallbackNotice::Warnings(vec![warning()])).contains("reset?choice"));
    assert!(
        render_notice(&AutoCallbackNotice::StatusUnavailable(
            ErrorCategory::UnsupportedOperation
        ))
        .contains("unsupported_operation")
    );
}

#[test]
fn explicit_commands_render_text_and_stable_json_without_prompting() {
    for registration in [
        HandlerRegistration::Current,
        HandlerRegistration::Missing,
        HandlerRegistration::Elsewhere {
            handler: "other.desktop".into(),
        },
    ] {
        for mode in [OutputMode::Human, OutputMode::Json] {
            let mut registrar = FakeRegistrar {
                registration: Some(registration.clone()),
                ..Default::default()
            };
            let result = run_handler(&mut registrar, HandlerCommand::Status, mode);
            assert_eq!(result.exit_code, 0);
            assert_eq!(registrar.calls, ["status"]);
            if mode == OutputMode::Json {
                let value: serde_json::Value = serde_json::from_str(&result.stdout).unwrap();
                assert_eq!(value["kind"], "uri_handler_status");
                assert_eq!(value["warnings"][0]["kind"], "remembered_choice");
                match registration {
                    HandlerRegistration::Current => {
                        assert_eq!(value["registration"], "current");
                        assert!(value["handler"].is_null());
                    }
                    HandlerRegistration::Missing => assert_eq!(value["registration"], "missing"),
                    HandlerRegistration::Elsewhere { .. } => {
                        assert_eq!(value["registration"], "elsewhere");
                        assert_eq!(value["handler"], "other.desktop");
                    }
                }
            } else {
                assert!(result.stdout.contains("remembered?choice: reset?choice"));
            }
        }
    }
    for mode in [OutputMode::Human, OutputMode::Json] {
        let mut registrar = FakeRegistrar::default();
        let installed = run_handler(&mut registrar, HandlerCommand::Install, mode);
        let removed = run_handler(&mut registrar, HandlerCommand::Uninstall, mode);
        assert_eq!(registrar.calls, ["install", "uninstall"]);
        if mode == OutputMode::Json {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&installed.stdout).unwrap(),
                serde_json::json!({"kind":"uri_handler_install", "changes":["write entry", "set association"]})
            );
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&removed.stdout).unwrap(),
                serde_json::json!({"kind":"uri_handler_uninstall", "changes":[]})
            );
        } else {
            assert_eq!(installed.stdout, "write entry\nset association\n");
            assert_eq!(removed.stdout, "Nothing was installed.\n");
        }
        let failed = run_handler(&mut UnsupportedRegistrar, HandlerCommand::Status, mode);
        assert_ne!(failed.exit_code, 0);
        assert!(failed.stdout.is_empty());
        assert_eq!(failed.exit_code, 40);
        assert_eq!(
            failed.stderr,
            format!(
                "{}\n",
                crate::output::render_error(
                    mode,
                    &crate::command::CliError::from_category(ErrorCategory::UnsupportedOperation)
                )
            )
        );
    }
}
