use destiny_mcp::schema::{SetLockStateArgs, set_lock_state_tool};
use serde_json::json;

// Exercise the shared SQLite/CLI lock contract alongside MCP transport tests.
#[path = "../../destiny-cli/tests/lock_pipeline.rs"]
mod cli_lock_contract;

#[test]
fn lock_output_local_state_covers_noop_refusal_and_persistence_failure() {
    use destiny_output::lock::{LockOutput, LockStatus};
    let mut output = LockOutput::refused(destiny_app::MutationError::MissingItem, true);
    for (status, state) in [
        (LockStatus::Refused, "unchanged"),
        (LockStatus::VerifiedNoop, "unchanged"),
        (LockStatus::Applied, "updated"),
        (LockStatus::Incomplete, "refresh_pending"),
        (LockStatus::VerificationFailed, "refresh_pending"),
    ] {
        output.status = status;
        assert_eq!(output.json()["local_state"], state);
        assert!(output.json().get("profile_refresh").is_none());
        assert_eq!(
            output.render_text().contains("local state refresh pending"),
            state == "refresh_pending"
        );
    }
}

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
