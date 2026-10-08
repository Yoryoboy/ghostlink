//! Offline stack pull boundary and shared CLI/MCP pipeline contracts.
use destiny_mcp::schema::{PullStackArgs, pull_stack_tool};
use serde_json::json;

#[path = "support/pull_stack.rs"]
mod support;

#[test]
fn schema_is_closed_and_canonical() {
    for invalid in [
        json!({}),
        json!({"item_hash":7,"amount":"3","character":"2"}),
        json!({"item_hash":"7","amount":"3","character":"2","membership":"1"}),
    ] {
        assert!(serde_json::from_value::<PullStackArgs>(invalid).is_err());
    }
    for (field, values) in [
        ("item_hash", vec!["-1", "+1", "01", "4294967296"]),
        ("amount", vec!["0", "-1", "01", "2147483648"]),
        ("character", vec!["0", "01", "9223372036854775808"]),
    ] {
        for value in values
            .into_iter()
            .chain(["", " 1", "1 ", "1\n", "1.0", "١"])
        {
            let mut args = json!({"item_hash":"7","amount":"3","character":"2"});
            args[field] = json!(value);
            assert!(
                serde_json::from_value::<PullStackArgs>(args)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
    for hash in ["0", "4294967295"] {
        let args: PullStackArgs = serde_json::from_value(
            json!({"item_hash":hash,"amount":"2147483647","character":"9223372036854775807"}),
        )
        .unwrap();
        assert!(args.validate().is_ok());
    }
    let schema = pull_stack_tool().schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["required"],
        json!(["item_hash", "amount", "character"])
    );
    assert_eq!(schema["properties"].as_object().unwrap().len(), 3);
}

#[test]
fn shared_pipeline_parity_and_single_action() {
    support::parity();
}

#[test]
fn direct_service_rejects_before_io() {
    support::reject_before_io();
}
