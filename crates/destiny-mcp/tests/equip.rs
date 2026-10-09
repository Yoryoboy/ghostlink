//! Exact CLI/MCP parity through the real shared planner, executor and SQLite.
#[path = "support/equip.rs"]
mod support;

#[test]
fn rc002_local_publication_cannot_overtake_dispatch() {
    support::serialized_publication();
}

#[test]
fn rc002_following_equip_can_precede_read_publication() {
    support::equip_before_publication();
}

#[test]
fn equip_json_matches_cli_for_every_execution_shape() {
    for (scenario, item, character, status) in [
        ("equip-success", "123", "warlock", "applied"),
        ("equip-success", "Ace of Spades", "WaRlOcK", "applied"),
        ("equip-noop", "123", "warlock", "verified_noop"),
        ("equip-provider-stale", "123", "warlock", "applied"),
        ("equip-success", "missing", "warlock", "refused"),
        ("equip-success", "123", "missing", "refused"),
        ("equip-ambiguous", "Ace of Spades", "warlock", "refused"),
        ("equip-partial", "123", "warlock", "incomplete"),
        ("equip-auth", "123", "warlock", "incomplete"),
        ("equip-auth-concurrent", "123", "warlock", "incomplete"),
        (
            "equip-auth-unavailable-refresh",
            "123",
            "warlock",
            "incomplete",
        ),
        ("equip-auth-refresh-failure", "123", "warlock", "incomplete"),
        ("equip-uncertain", "123", "warlock", "incomplete"),
        ("equip-uncertain-no-effect", "123", "warlock", "incomplete"),
        ("equip-refresh-failure", "123", "warlock", "applied"),
        ("equip-contradiction", "123", "warlock", "applied"),
    ] {
        let cli = support::cli_json(scenario, item, character);
        let mcp = support::mcp_json(scenario, item, character);
        assert_eq!(mcp, cli, "{scenario}: entire JSON must match");
        assert_eq!(mcp["status"], status, "{scenario}");
        assert!(mcp.get("refusal_exit_code").is_none());
        if scenario.starts_with("equip-auth") {
            assert_eq!(mcp["steps"][0]["outcome"], "completed");
            assert_eq!(mcp["steps"][1]["outcome"], "authentication_refused");
            assert_eq!(mcp["steps"][1]["reason"], "provider authentication failed");
        }
        if scenario == "equip-auth-concurrent" {
            assert!(mcp["reason"].as_str().unwrap().contains("authentication"));
            assert_eq!(mcp["profile_refresh"], "not_required");
            assert!(mcp["observed"].is_null());
        }
        if matches!(
            scenario,
            "equip-refresh-failure"
                | "equip-auth-refresh-failure"
                | "equip-auth-unavailable-refresh"
        ) {
            assert_eq!(mcp["profile_refresh"], "not_required");
            assert!(mcp["observed"].is_null());
        }
    }
}

#[test]
fn rc001_schema_rejects_raw_and_normalized_oversize() {
    for item in ["x".repeat(257), "İ".repeat(129)] {
        assert!(
            destiny_mcp::schema::EquipItemArgs {
                item,
                character: "warlock".into()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        destiny_mcp::schema::EquipItemArgs {
            item: "1234567890123456789".into(),
            character: "warlock".into()
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn rc001_direct_service_rejects_before_clock_for_absent_and_stale_projection() {
    support::reject_before_io();
}

#[test]
fn equip_schema_requires_both_selectors_and_rejects_unknown_fields_and_controls() {
    use destiny_mcp::schema::{EquipItemArgs, equip_item_tool};
    use serde_json::json;
    for invalid in [
        json!({}),
        json!({"item":"123"}),
        json!({"character":"warlock"}),
        json!({"item":"123","character":"warlock","approval":"token"}),
    ] {
        assert!(serde_json::from_value::<EquipItemArgs>(invalid).is_err());
    }
    for (item, character) in [
        ("", "warlock"),
        ("123", ""),
        ("123\n", "warlock"),
        ("123", "war\tlock"),
    ] {
        assert!(
            EquipItemArgs {
                item: item.into(),
                character: character.into()
            }
            .validate()
            .is_err()
        );
    }
    let schema = equip_item_tool().schema_as_json_value();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["properties"]["item"]["maxLength"],
        destiny_app::MAX_SEARCH_TEXT_LENGTH
    );
    assert_eq!(schema["properties"]["character"]["maxLength"], 512);
    let required = schema.get("required").unwrap().as_array().unwrap();
    assert!(required.contains(&json!("item")) && required.contains(&json!("character")));
}
