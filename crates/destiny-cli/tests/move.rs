use clap::Parser;
use destiny_app::{ItemLocation, MutationError, StepAction, StepOutcome, TransferRejection};
use destiny_cli::move_output::*;
use destiny_cli::{Cli, OutputMode, parse_process};

#[test]
fn move_requires_exactly_one_destination() {
    for args in [
        vec!["ghostlink", "move"],
        vec!["ghostlink", "move", "123"],
        vec![
            "ghostlink",
            "move",
            "123",
            "--vault",
            "--character",
            "Hunter",
        ],
    ] {
        assert_eq!(parse_process(args).unwrap_err().exit_code, 2);
    }
    for args in [
        vec!["ghostlink", "move", "Ace of Spades", "--vault", "--json"],
        vec![
            "ghostlink",
            "move",
            "123",
            "--character",
            "Hunter",
            "--json",
        ],
        vec!["ghostlink", "move", "123", "--character", "456"],
    ] {
        assert!(Cli::try_parse_from(args).is_ok());
    }
}

#[test]
fn move_status_rendering_exit_codes_and_streams() {
    for (status, code) in [
        (MoveStatus::Applied, 0),
        (MoveStatus::VerifiedNoop, 0),
        (MoveStatus::Incomplete, 41),
        (MoveStatus::VerificationFailed, 42),
        (MoveStatus::Refused, 40),
    ] {
        let output = MoveOutput::from_shared(destiny_output::move_item::MoveOutput {
            status,
            item: Some(MoveItem {
                name: "Ace\n\u{1b}of Spades".into(),
                instance_id: "123".into(),
                slot: None,
            }),
            destination: Some(location_view(&ItemLocation::Vault, &[])),
            steps: vec![],
            reason: None,
            profile_refresh: ProfileRefresh::Refreshed,
            observed: None,
        });
        assert_eq!(output.exit_code(), code);
        assert_eq!(output.json()["status"], status.as_str());
        assert_eq!(output.json()["destination"]["kind"], "vault");
        assert!(output.json().get("character").is_none());
        let text = output.render_text();
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains("Ace\n"));
        assert!(!text.contains("equipped"));
        assert_eq!(
            text.contains("✔ verified"),
            status == MoveStatus::VerifiedNoop
        );
        for mode in [OutputMode::Human, OutputMode::Json] {
            let process = destiny_cli::runner::move_process_output(mode, &output);
            assert_eq!(process.exit_code, code);
            assert_eq!(process.stderr.is_empty(), code == 0);
            assert_eq!(process.stdout.is_empty(), code != 0);
        }
    }
}

#[test]
fn move_refusals_and_unattempted_steps_are_explained() {
    for (error, code, reason) in [
        (MutationError::MissingItem, 32, "item was not found"),
        (
            MutationError::MissingDestination,
            2,
            "character was not found",
        ),
        (MutationError::AmbiguousItem, 31, "use the instance id"),
        (
            MutationError::AmbiguousDestination,
            31,
            "use the character id",
        ),
    ] {
        let output = MoveOutput::refused(error);
        assert_eq!(output.exit_code(), code);
        assert!(output.render_text().contains(reason));
    }
    let action = StepAction::Transfer {
        from: ItemLocation::Vault,
        to: ItemLocation::Vault,
    };
    for outcome in [
        StepOutcome::Rejected(TransferRejection::DestinationFull),
        StepOutcome::Rejected(TransferRejection::Equipped),
        StepOutcome::Rejected(TransferRejection::NotTransferable),
        StepOutcome::Rejected(TransferRejection::InvalidRequest),
        StepOutcome::Uncertain,
        StepOutcome::Unattempted,
    ] {
        let mut output = MoveOutput::refused(MutationError::MissingItem);
        output.status = MoveStatus::Incomplete;
        output.steps.push(step_view(1, &action, outcome, &[]));
        let text = output.render_text();
        assert!(text.contains("1. transfer:"));
        if outcome == StepOutcome::Uncertain {
            assert!(text.contains("do not blindly retry"));
        }
        if outcome == StepOutcome::Unattempted {
            assert!(text.contains("not run"));
        }
    }
}
