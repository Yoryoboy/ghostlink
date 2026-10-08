use destiny_mcp::schema::{SetLockStateArgs, set_lock_state_tool};
use serde_json::json;

// Exercise the shared SQLite/CLI lock contract alongside MCP transport tests.
#[path = "../../destiny-cli/tests/lock_pipeline.rs"]
mod cli_lock_contract;

#[test]
fn lock_schema_requires_boolean_state_and_bounded_item() {
    for invalid in [
        json!({}),
        json!({"item":"123"}),
        json!({"item":"123","locked":"true"}),
        json!({"item":"123","locked":true,"character":"456"}),
    ] {
        assert!(serde_json::from_value::<SetLockStateArgs>(invalid).is_err());
    }
    for item in [
        "".into(),
        "bad\nitem".into(),
        "x".repeat(257),
        "İ".repeat(129),
    ] {
        assert!(SetLockStateArgs { item, locked: true }.validate().is_err());
    }
    let schema = set_lock_state_tool().schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"]["locked"]["type"], "boolean");
    let required = schema["required"].as_array().unwrap();
    assert!(required.contains(&json!("item")) && required.contains(&json!("locked")));
}
