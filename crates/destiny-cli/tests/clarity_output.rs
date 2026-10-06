use destiny_app::{ClaritySyncReport, DefinitionVersion, ManifestSyncReport};
use destiny_cli::{
    output::{OutputMode, render_manifest_with_clarity},
    runtime::ManifestFactory,
};

#[test]
fn unavailable_clarity_keeps_manifest_exit_zero_and_json_keys() {
    struct Factory;
    impl ManifestFactory for Factory {
        fn sync(&mut self) -> Result<ManifestSyncReport, destiny_cli::CliError> {
            Ok(ManifestSyncReport::Unchanged {
                version: DefinitionVersion::new("v1"),
                record_count: 3,
                world_counts: [1; 5],
                stat_count: 1,
            })
        }
    }
    let output = destiny_cli::runner::run_manifest(&mut Factory, OutputMode::Json);
    assert_eq!(output.exit_code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["status"], "unchanged");
    assert_eq!(value["clarity"]["status"], "unavailable");
    let report = Factory.sync().unwrap();
    let clarity = ClaritySyncReport {
        status: "updated",
        version: Some("2.0623".into()),
        entries_stored: 10,
        entries_skipped: 2,
    };
    assert!(
        render_manifest_with_clarity(OutputMode::Human, &report, &clarity)
            .contains("clarity: updated")
    );
    let value: serde_json::Value = serde_json::from_str(&render_manifest_with_clarity(
        OutputMode::Json,
        &report,
        &clarity,
    ))
    .unwrap();
    assert_eq!(value["clarity"]["entries_skipped"], 2);
}
