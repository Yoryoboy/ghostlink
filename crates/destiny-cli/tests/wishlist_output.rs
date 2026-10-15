use destiny_app::wishlist::{
    WishlistSourceReport, WishlistSourceStatus, WishlistSyncReport, WishlistSyncStatus,
};
use destiny_app::{
    ClaritySyncReport, DefinitionVersion, ManifestAndClarityReport, ManifestSyncReport,
};
use destiny_cli::{output::OutputMode, runner::run_manifest, runtime::ManifestFactory};

fn manifest() -> ManifestSyncReport {
    ManifestSyncReport::Unchanged {
        version: DefinitionVersion::new("v1"),
        record_count: 3,
        world_counts: [1; 5],
        stat_count: 1,
    }
}

struct Synced;
impl ManifestFactory for Synced {
    fn sync(&mut self) -> Result<ManifestSyncReport, destiny_cli::CliError> {
        Ok(manifest())
    }
    fn sync_with_clarity(&mut self) -> Result<ManifestAndClarityReport, destiny_cli::CliError> {
        Ok(ManifestAndClarityReport {
            manifest: manifest(),
            clarity: ClaritySyncReport::unavailable(),
            wishlists: WishlistSyncReport {
                status: WishlistSyncStatus::Partial,
                sources: vec![
                    WishlistSourceReport {
                        source: "voltron".into(),
                        status: WishlistSourceStatus::Updated,
                        rolls: 274_677,
                        skipped: 0,
                    },
                    WishlistSourceReport {
                        source: "/lists/mine\u{1b}[31m.txt".into(),
                        status: WishlistSourceStatus::Unavailable,
                        rolls: 0,
                        skipped: 0,
                    },
                ],
            },
        })
    }
}

#[test]
fn sync_output_reports_every_wishlist_source() {
    let output = run_manifest(&mut Synced, OutputMode::Json);
    assert_eq!(output.exit_code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["status"], "unchanged");
    assert_eq!(value["clarity"]["status"], "unavailable");
    assert_eq!(
        value["wishlists"],
        serde_json::json!({
            "status": "partial",
            "sources": [
                {"source": "voltron", "status": "updated", "rolls": 274677, "skipped": 0},
                {"source": "/lists/mine\u{1b}[31m.txt", "status": "unavailable", "rolls": 0, "skipped": 0}
            ]
        })
    );

    let human = run_manifest(&mut Synced, OutputMode::Human).stdout;
    let human = human.trim_end_matches('\n');
    assert!(human.contains("clarity: unavailable\n"), "{human}");
    assert!(
        human.ends_with(
            "\nwishlists: partial\n\
             wishlist voltron: updated, 274677 rolls, 0 skipped\n\
             wishlist /lists/mine?[31m.txt: unavailable, 0 rolls, 0 skipped"
        ),
        "{human}"
    );
}

#[test]
fn factories_without_wishlist_sync_report_them_unavailable() {
    struct Legacy;
    impl ManifestFactory for Legacy {
        fn sync(&mut self) -> Result<ManifestSyncReport, destiny_cli::CliError> {
            Ok(manifest())
        }
    }
    let output = run_manifest(&mut Legacy, OutputMode::Json);
    assert_eq!(output.exit_code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(
        value["wishlists"],
        serde_json::json!({"status": "unavailable", "sources": []})
    );
    let human = run_manifest(&mut Legacy, OutputMode::Human).stdout;
    let human = human.trim_end_matches('\n');
    assert!(human.ends_with("\nwishlists: unavailable"), "{human}");
}
