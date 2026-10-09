use destiny_app::{
    ItemLocation, MutationError, ProviderRejection, StepAction, StepOutcome, TransferRejection,
};
use destiny_cli::OutputMode;
use destiny_cli::move_output::{MoveOutput, MoveStatus, step_view};

#[test]
fn definitive_provider_rejections_keep_incomplete_exit_and_actionable_text() {
    for reason in [
        ProviderRejection::ItemNotFound,
        ProviderRejection::Throttled,
    ] {
        let mut output = MoveOutput::refused(MutationError::MissingItem);
        output.status = MoveStatus::Incomplete;
        output.reason = None;
        output.steps.push(step_view(
            1,
            &StepAction::Transfer {
                from: ItemLocation::Vault,
                to: ItemLocation::Vault,
            },
            StepOutcome::Rejected(TransferRejection::Provider(reason)),
            &[],
        ));
        let process = destiny_cli::runner::move_process_output(OutputMode::Json, &output);
        assert_eq!(process.exit_code, 41);
        assert!(process.stdout.is_empty());
        let value: serde_json::Value = serde_json::from_str(&process.stderr).unwrap();
        assert_eq!(value["steps"][0]["outcome"], "rejected");
        assert_eq!(value["steps"][0]["reason"], reason.reason());
        assert_eq!(value["steps"][0]["message"], reason.message());
        assert!(output.render_text().contains(reason.message()));
    }
}
