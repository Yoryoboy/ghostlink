//! Offline canonical-input and shared-pipeline pull contracts.
use destiny_mcp::schema::{PullItemArgs, pull_item_tool};
use serde_json::json;

#[path = "support/pull.rs"]
mod support;

#[test]
fn pull_schema_is_closed_and_canonical_positive_int64_only() {
    for invalid in [
        json!({}),
        json!({"item":123}),
        json!({"item":null}),
        json!({"item":"123","quantity":1}),
        json!({"item":"123","character":"2"}),
        json!({"item":"123","membership":"123"}),
        json!({"item":"123","destination":"vault"}),
        json!({"item":"123","hash":7}),
    ] {
        assert!(serde_json::from_value::<PullItemArgs>(invalid).is_err());
    }
    for item in [
        "",
        "0",
        "-1",
        "+1",
        "01",
        " 1",
        "1 ",
        "1\n",
        "1.0",
        "Ace",
        "١",
        "9223372036854775808",
    ] {
        assert!(PullItemArgs { item: item.into() }.instance_id().is_err());
    }
    for item in ["1", "9223372036854775807"] {
        assert_eq!(
            PullItemArgs { item: item.into() }
                .instance_id()
                .unwrap()
                .as_str(),
            item
        );
    }
    let schema = pull_item_tool().schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["item"]));
    assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
    assert_eq!(schema["properties"]["item"]["maxLength"], 19);
}

#[test]
fn pull_dispatch_and_json_match_cli_without_redispatch() {
    support::parity();
}

#[test]
fn pull_direct_service_rejects_before_any_io() {
    support::reject_before_io();
}
