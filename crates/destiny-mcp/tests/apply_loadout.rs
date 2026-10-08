use clap::Parser;
use destiny_app::{ApplyLoadoutResult, CharacterId, LoadoutActionOutcome, LoadoutActionRejection};
use destiny_cli::command::{Cli, Command};
use destiny_mcp::schema::ApplyLoadoutArgs;
use destiny_output::apply_loadout::ApplyLoadoutOutput;

#[test]
fn cli_and_mcp_resolve_identical_intent() {
    for (selector, slot, name) in [
        ("3", Some(3), None),
        ("  PvE ", None, Some("  PvE ".into())),
    ] {
        let cli = Cli::try_parse_from([
            "ghostlink",
            "apply-loadout",
            selector,
            "--character",
            "hunter",
        ])
        .unwrap();
        let Command::ApplyLoadout(cli) = cli.command else {
            panic!("wrong command")
        };
        let mcp = ApplyLoadoutArgs {
            character: "hunter".into(),
            slot,
            name,
        };
        assert_eq!(cli.request().unwrap(), mcp.request().unwrap());
    }
    for (slot, name) in [
        (None, None),
        (Some(3), Some("PvE".into())),
        (Some(0), None),
        (Some(13), None),
    ] {
        assert!(
            ApplyLoadoutArgs {
                character: "hunter".into(),
                slot,
                name
            }
            .request()
            .is_err()
        );
    }
}

#[test]
fn shared_failure_output_discards_untrusted_diagnostics_and_cleans_candidates() {
    let output = ApplyLoadoutOutput::from_result(Err(destiny_app::ApplyLoadoutError::App(
        destiny_app::AppError::invalid_argument("provider", "secret-sentinel"),
    )));
    assert!(!output.value.to_string().contains("secret-sentinel"));
    assert!(!output.render_text().contains("secret-sentinel"));
    let output =
        ApplyLoadoutOutput::from_result(Err(destiny_app::ApplyLoadoutError::UnknownName(vec![
            destiny_app::LoadoutCandidate {
                slot: 3,
                name: Some("PvE\n\u{1b}hostile".into()),
            },
        ])));
    assert_eq!(output.value["candidates"][0]["name"], "PvE??hostile");
    assert!(output.render_text().contains("[3] PvE??hostile"));
}

#[test]
fn shared_output_preserves_receipts_exits_and_sanitizes_human_and_json() {
    for receipt in [
        LoadoutActionOutcome::Applied,
        LoadoutActionOutcome::Rejected(LoadoutActionRejection::RestrictedLocation),
        LoadoutActionOutcome::AuthenticationRefused,
        LoadoutActionOutcome::Transient,
        LoadoutActionOutcome::Uncertain,
    ] {
        for local_state_updated in [true, false] {
            let output = ApplyLoadoutOutput::from_result(Ok(ApplyLoadoutResult {
                character: CharacterId::new("1"),
                slot: 3,
                name: Some("PvE\n\u{1b}hostile".into()),
                receipt,
                local_state_updated,
            }));
            assert_eq!(
                output.exit_code,
                if !local_state_updated {
                    42
                } else if receipt.applied() {
                    0
                } else {
                    41
                }
            );
            assert_eq!(output.value["name"], "PvE??hostile");
            assert!(!output.render_text().contains('\u{1b}'));
            assert!(!output.render_text().contains('\n'));
            assert!(output.value.get("membership").is_none());
            if !local_state_updated {
                assert_eq!(
                    output.value["reason"],
                    "local state could not be updated after the action"
                );
            } else if receipt
                == LoadoutActionOutcome::Rejected(LoadoutActionRejection::RestrictedLocation)
            {
                assert!(
                    output
                        .render_text()
                        .contains("go to orbit or a social space and retry")
                );
            }
        }
    }
}
