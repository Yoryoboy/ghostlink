use destiny_app::Timestamp;
use destiny_app::build::{BuildSource, BuildSummary, DimLoadoutError};
use destiny_app::builds::{BuildError, BuildReply};
use destiny_output::builds::BuildOutput;
use serde_json::json;

#[test]
fn build_list_and_delete_snapshots() {
    let output = BuildOutput::from_result(Ok(BuildReply::List(vec![BuildSummary {
        id: "id".into(),
        name: "Raid".into(),
        class_type: 1,
        source: BuildSource::Import,
        created_at: Timestamp::new(100),
        updated_at: Timestamp::new(100),
    }])));
    assert_eq!(
        output.value,
        json!({"kind":"builds", "builds":[{"id":"id", "name":"Raid", "class_type":1, "source":"import", "created_at":100, "updated_at":100}]})
    );
    assert_eq!(output.render_text(), "id  Raid (import)");
    let empty = BuildOutput::from_result(Ok(BuildReply::List(vec![])));
    assert_eq!(empty.render_text(), "No saved builds.");
    let deleted = BuildOutput::from_result(Ok(BuildReply::Deleted(false)));
    assert_eq!(
        deleted.value,
        json!({"kind":"delete_build", "deleted":false})
    );
    assert_eq!(deleted.render_text(), "Deleted: false");
}

#[test]
fn export_and_typed_offline_error() {
    let export = BuildOutput::from_result(Ok(BuildReply::Export("{\"name\":\"Raid\"}".into())));
    assert_eq!(export.render_text(), "{\"name\":\"Raid\"}");
    let share = BuildOutput::from_result(Err(BuildError::Codec(
        DimLoadoutError::ShareLinkRequiresOnlineImport,
    )));
    assert_eq!(share.exit_code, 40);
    assert_eq!(
        share.value["error"]["reason"],
        "share_link_requires_online_import"
    );
    assert!(share.render_text().contains("BE-009"));
    let missing = BuildOutput::from_result(Err(BuildError::NotFound));
    assert_eq!(missing.exit_code, 32);
    assert_eq!(missing.value["error"]["code"], "item_not_found");
}
