//! Real child-process stdio integration for the Destiny MCP tools.
//!
//! This suite is compiled only with the non-default `offline-fixtures` feature.
//! It spawns the clearly test-only `offline-fixture` binary, drives a complete
//! MCP session over the child's stdin/stdout, and asserts the wire contract
//! directly.
//!
//! A default fixture session proves initialize, discovery of the four reads
//! plus the default-enabled `move_item`, all three reads, the complete one-call
//! mutation journey (semantic planning, fresh preflight, exactly one transfer,
//! acknowledgement and freshness invalidation), a repeated request that
//! fails closed without replay, a sanitized missing-item failure, and the secret
//! sentinel never crossing either stream. Fresh fixture processes selected by
//! `DESTINY_FIXTURE_SCENARIO` additionally prove an already-satisfied verified
//! no-op, a changed preflight, and an honest partial execution.
//!
//! The client is intentionally hand-rolled over newline-delimited JSON-RPC so
//! the test owns the raw stdout stream: every frame must parse as JSON-RPC, and
//! no secret sentinel, membership identifier, or raw diagnostic may appear in
//! either stream.
#![cfg(feature = "offline-fixtures")]

use destiny_app::test_support::leak_sentinels;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

#[tokio::test]
async fn builds_stdio_import_list_show_export_delete_and_ambiguity() {
    let mut session = FixtureSession::spawn(Some("success")).await;
    let dim =
        json!({"id":"original", "name":"Raid", "classType":1, "equipped":[], "unequipped":[]})
            .to_string();
    let saved = session
        .call_tool("save_build", json!({"source":"import", "input":dim}))
        .await;
    assert_eq!(saved["result"]["isError"], false, "{saved}");
    let id = saved["result"]["structuredContent"]["summary"]["id"].clone();
    let shown = session.call_tool("get_build", json!({"id":id})).await;
    assert_eq!(
        saved["result"]["structuredContent"],
        shown["result"]["structuredContent"]
    );
    let export: Value = serde_json::from_str(
        shown["result"]["structuredContent"]["dim_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(export["name"], "Raid");
    assert_ne!(export["id"], "original");
    session
        .call_tool("save_build", json!({"source":"import", "input":dim}))
        .await;
    let ambiguous = session.call_tool("get_build", json!({"name":"raid"})).await;
    assert_eq!(ambiguous["result"]["isError"], true);
    assert_eq!(
        ambiguous["result"]["structuredContent"]["error"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let list = session.call_tool("list_builds", json!({})).await;
    assert_eq!(
        list["result"]["structuredContent"]["builds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for _ in 0..2 {
        let deleted = session.call_tool("delete_build", json!({"id":id})).await;
        assert_eq!(deleted["result"]["isError"], false);
    }
    let share = session
        .call_tool(
            "save_build",
            json!({"source":"import", "input":"https://dim.gg/abcdefg"}),
        )
        .await;
    assert_eq!(
        share["result"]["structuredContent"]["error"]["reason"],
        "share_link_requires_online_import"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn build_sources_stdio_report_omissions_and_unresolved_items() {
    let mut session = FixtureSession::spawn(Some("build-sources")).await;
    let equipped = session
        .call_tool(
            "save_build",
            json!({"source":"equipped", "character":"1", "name":"Equipped"}),
        )
        .await;
    assert_eq!(equipped["result"]["isError"], false, "{equipped}");
    // This character has no equipped fixture items: no missing socket data
    // should be fabricated merely because the source is equipped.
    assert_eq!(
        equipped["result"]["structuredContent"]["omissions"],
        json!([])
    );
    assert_eq!(
        equipped["result"]["structuredContent"]["equipped_socket_overrides"],
        json!([])
    );
    let saved = session
        .call_tool(
            "save_build",
            json!({"source":"in_game_loadout", "character":"1", "slot":1}),
        )
        .await;
    assert_eq!(saved["result"]["isError"], false, "{saved}");
    assert_eq!(
        saved["result"]["structuredContent"]["unresolved_items"],
        json!(["404"])
    );
    let id = saved["result"]["structuredContent"]["summary"]["id"].clone();
    let shown = session.call_tool("get_build", json!({"id":id})).await;
    assert!(shown["result"]["structuredContent"]["unresolved_items"].is_null());
    assert_eq!(
        shown["result"]["structuredContent"]["omissions"],
        json!(["source_capture_evidence_not_persisted"])
    );
    let analyzed = session
        .call_tool("analyze_build", json!({"id":id,"character":"1"}))
        .await;
    assert_eq!(analyzed["result"]["isError"], false, "{analyzed}");
    let analysis = &analyzed["result"]["structuredContent"];
    assert_eq!(analysis["kind"], "build_analysis");
    assert_eq!(analysis["counts"]["missing"], 0);
    assert_eq!(analysis["findings"].as_array().unwrap().len(), 13);
    // Substitutes: the fixture has no full armor set, so the answer is a typed unknown.
    let substitutes = session
        .call_tool(
            "build_substitutes",
            json!({"id":id,"character":"1","limit":5}),
        )
        .await;
    assert_eq!(substitutes["result"]["isError"], false, "{substitutes}");
    let ranked = &substitutes["result"]["structuredContent"];
    assert_eq!(ranked["kind"], "build_substitutes");
    assert_eq!(ranked["limit"], 5);
    assert_eq!(ranked["evaluation"], "not_evaluated");
    assert_eq!(ranked["sets"], json!([]));
    let too_many = session
        .call_tool("build_substitutes", json!({"id":id,"limit":201}))
        .await;
    assert_eq!(too_many["result"]["isError"], true);
    let too_few = session
        .call_tool("build_substitutes", json!({"id":id,"limit":0}))
        .await;
    assert_eq!(too_few["result"]["isError"], true);
    let invalid_analysis = session
        .call_tool("analyze_build", json!({"id":id,"name":"other"}))
        .await;
    assert_eq!(invalid_analysis["result"]["isError"], true);
    let invalid = session
        .call_tool("save_build", json!({"source":"equipped", "input":"bad"}))
        .await;
    assert_eq!(invalid["result"]["isError"], true);
    session.shutdown().await;
}

#[tokio::test]
async fn dim_query_stdio_and_invalid_query_contract() {
    let mut session = FixtureSession::spawn(Some("success")).await;
    let all = session
        .call_tool(SEARCH_INVENTORY_TOOL, json!({"query":""}))
        .await;
    assert_eq!(all["result"]["isError"], false, "{all}");
    assert!(
        !all["result"]["structuredContent"]["hits"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let query = session
        .call_tool(
            SEARCH_INVENTORY_TOOL,
            json!({"query":"is:invault", "equipped":false}),
        )
        .await;
    let flags = session
        .call_tool(
            SEARCH_INVENTORY_TOOL,
            json!({"location":"vault", "equipped":false}),
        )
        .await;
    assert_eq!(query["result"]["isError"], false, "{query}");
    assert_eq!(
        query["result"]["structuredContent"]["hits"],
        flags["result"]["structuredContent"]["hits"]
    );
    let invalid = session
        .call_tool(
            SEARCH_INVENTORY_TOOL,
            json!({"query":"is:weapon mystery:x"}),
        )
        .await;
    assert_eq!(invalid["result"]["isError"], true);
    let error = &invalid["result"]["structuredContent"];
    assert_eq!(error["code"], "invalid_query");
    assert_eq!(error["kind"], "unknown_filter");
    assert_eq!(error["span"], json!({"start":10,"end":19}));
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("unknown filter")
    );
    assert!(
        error["supported_filters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "is:weapon")
    );
    let unsupported = session
        .call_tool(SEARCH_INVENTORY_TOOL, json!({"query":"wishlist:x"}))
        .await;
    assert_eq!(
        unsupported["result"]["structuredContent"]["kind"],
        "unsupported_filter"
    );
    assert!(
        unsupported["result"]["structuredContent"]
            .get("supported_filters")
            .is_none()
    );
    session.shutdown().await;
}

#[tokio::test]
async fn action_descriptions_share_the_ordinary_sync_contract() {
    let mut session = FixtureSession::spawn(Some("success")).await;
    let id = session.next_id();
    let response = session.request(id, "tools/list", Some(json!({}))).await;
    let tools = response["result"]["tools"].as_array().unwrap();
    for name in [
        "move_item",
        "equip_item",
        "equip_items",
        "set_lock_state",
        "pull_item",
        "pull_stack",
        "apply_loadout",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        let description = tool["description"].as_str().unwrap();
        for phrase in [
            "Plans from Ghostlink's local projection",
            "Bungie is the final validator",
            "Acknowledged actions update local state immediately",
            "Never retry an uncertain outcome",
        ] {
            assert!(description.contains(phrase), "{name}: {description}");
        }
        assert!(!description.contains("forces sync"));
        assert!(!description.contains("freshness is invalidated"));
    }
    session.shutdown().await;
}

/// Bounded wait for every read, write, and process exit.
const TIMEOUT: Duration = Duration::from_secs(15);

/// The membership identifier the fixture resolves; it must never reach a client.
const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

/// Every canary that must never appear on either process stream or in a
/// serialized response, drawn from the shared leak set.
const STREAM_SENTINELS: [&str; 4] = [
    leak_sentinels::API_KEY,
    leak_sentinels::BEARER_TOKEN,
    leak_sentinels::RAW_PAYLOAD,
    MEMBERSHIP_IDENTIFIER,
];

/// The scenario selector the fixture reads at process start.
const SCENARIO_ENV: &str = "DESTINY_FIXTURE_SCENARIO";

const LIST_CHARACTERS_TOOL: &str = "list_characters";
const SEARCH_INVENTORY_TOOL: &str = "search_inventory";
const INSPECT_ITEM_TOOL: &str = "inspect_item";
const GET_SUBCLASS_TOOL: &str = "get_subclass";
const GET_ARTIFACT_TOOL: &str = "get_artifact";
const GET_LOADOUTS_TOOL: &str = "get_loadouts";
const MOVE_ITEM_TOOL: &str = "move_item";
#[path = "support/equip.rs"]
mod equip_support;

#[tokio::test]
async fn apply_build_stdio_is_composed_and_reports_typed_errors_without_dispatch() {
    let mut session = FixtureSession::spawn(None).await;
    let response = session
        .call_tool("apply_build", json!({"id":"missing", "dry_run":true}))
        .await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(structured(&response)["error"]["code"], "item_not_found");
    for invalid in [json!({}), json!({"id":"a", "name":"b"})] {
        let response = session.call_tool("apply_build", invalid).await;
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(structured(&response)["code"], "provider_validation");
    }
    let id = session.next_id();
    let tools = session.request(id, "tools/list", None).await;
    let tool = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "apply_build")
        .unwrap()
        .clone();
    assert!(tool["description"].as_str().unwrap().len() <= 500);
    assert!(tool["inputSchema"].get("oneOf").is_none());
    session.shutdown().await;
}

#[tokio::test]
async fn batch_equip_stdio_reports_per_item_outcomes_and_storage_failure() {
    for (scenario, outcome, error) in [
        ("batch-success", "applied", false),
        ("batch-noop", "verified_no_op", false),
        ("batch-mixed", "incomplete", true),
        ("batch-omitted", "incomplete", true),
        ("batch-location", "incomplete", true),
        ("batch-uncertain", "incomplete", true),
        ("batch-storage", "local_state_failed", true),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let response = session
            .call_tool(
                "equip_items",
                json!({"character":"warlock", "items":["101", "Second"]}),
            )
            .await;
        let output = structured(&response);
        assert_eq!(output["outcome"], outcome, "{scenario}");
        assert_eq!(response["result"]["isError"], error, "{scenario}");
        assert_eq!(output["items"].as_array().unwrap().len(), 2);
        if scenario == "batch-mixed" {
            assert_eq!(output["items"][1]["code"], 99);
        }
        if scenario == "batch-omitted" {
            assert_eq!(output["items"][1]["status"], "not_reported");
        }
        if scenario == "batch-location" {
            assert_eq!(output["reason"], "go to orbit or a social space and retry");
        }
        session.shutdown().await;
    }
}

#[tokio::test]
async fn batch_equip_stdio_refuses_invalid_and_missing_items() {
    let mut session = FixtureSession::spawn(None).await;
    let response = session
        .call_tool(
            "equip_items",
            json!({"character":"warlock", "items":["missing", "absent"]}),
        )
        .await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(structured(&response)["kind"], "equip_items");
    let response = session
        .call_tool(
            "equip_items",
            json!({"character":"warlock", "items":["missing"]}),
        )
        .await;
    assert_eq!(response["result"]["isError"], true);
    session.shutdown().await;
}

#[tokio::test]
async fn apply_loadout_stdio_contract() {
    for (scenario, receipt, outcome) in [
        ("loadout-success", "applied", "applied"),
        ("loadout-location", "rejected", "failed"),
        ("loadout-auth", "authentication_refused", "failed"),
        ("loadout-transient", "transient", "failed"),
        ("loadout-uncertain", "uncertain", "failed"),
        ("loadout-storage", "applied", "local_state_failed"),
        ("loadout-partial", "applied", "local_state_failed"),
        ("loadout-subclass", "applied", "local_state_failed"),
        (
            "loadout-missing-definition",
            "applied",
            "local_state_failed",
        ),
        ("loadout-unsupported", "applied", "local_state_failed"),
        // The seasonal artifact skip is informational, never partial.
        ("loadout-artifact", "applied", "applied"),
    ] {
        for args in [
            json!({"character":"hunter","slot":3}),
            json!({"character":"1","name":"  pVe "}),
        ] {
            let mut session = FixtureSession::spawn(Some(scenario)).await;
            let response = session.call_tool("apply_loadout", args).await;
            let output = structured(&response);
            assert_eq!(output["kind"], "apply_loadout");
            assert_eq!(output["character"], "1");
            assert_eq!(output["slot"], 3);
            assert_eq!(output["name"], "PvE");
            assert_eq!(output["receipt"], receipt);
            assert_eq!(output["outcome"], outcome);
            assert_eq!(response["result"]["isError"], outcome != "applied");
            let local = &output["local_state"];
            assert_eq!(local["persisted"], scenario != "loadout-storage");
            assert_eq!(
                local["status"],
                match scenario {
                    "loadout-success" | "loadout-artifact" => "updated",
                    "loadout-storage" => "failed",
                    "loadout-partial"
                    | "loadout-subclass"
                    | "loadout-missing-definition"
                    | "loadout-unsupported" => "partial",
                    _ => "not_attempted",
                }
            );
            match scenario {
                "loadout-partial" => {
                    assert_eq!(local["unresolved_items"][0]["item"]["instance_id"], "404");
                    assert_eq!(local["unresolved_items"][0]["reason"], "not_owned");
                    assert!(local["unresolved_items"][0]["item"]["item_hash"].is_null());
                }
                "loadout-subclass" => {
                    assert_eq!(local["skipped"][0]["reason"], "subclass_not_held")
                }
                "loadout-missing-definition" => {
                    assert_eq!(local["unresolved_items"][0]["reason"], "missing_definition")
                }
                "loadout-unsupported" | "loadout-artifact" => {
                    assert_eq!(local["unresolved_items"], json!([]));
                    assert_eq!(local["skipped"][0]["reason"], "unsupported_bucket")
                }
                _ => {
                    assert_eq!(local["unresolved_items"], json!([]));
                    assert_eq!(local["skipped"], json!([]));
                }
            }
            if scenario == "loadout-location" {
                assert_eq!(output["reason"], "go to orbit or a social space and retry");
            }
            match scenario {
                "loadout-storage" => assert_eq!(
                    output["reason"],
                    "local state could not be updated after the action"
                ),
                "loadout-partial"
                | "loadout-subclass"
                | "loadout-missing-definition"
                | "loadout-unsupported" => assert_eq!(
                    output["reason"],
                    "some loadout items were not applied; see local_state"
                ),
                "loadout-success" | "loadout-artifact" => assert!(output["reason"].is_null()),
                _ => (),
            }
            session.shutdown().await;
        }
    }
    let mut session = FixtureSession::spawn(Some("loadout-success")).await;
    for args in [
        json!({"character":"hunter"}),
        json!({"character":"hunter","slot":3,"name":"PvE"}),
        json!({"character":"hunter","slot":13}),
    ] {
        let response = session.call_tool("apply_loadout", args).await;
        assert_eq!(response["result"]["isError"], true);
    }
    let response = session
        .call_tool("apply_loadout", json!({"character":"hunter","name":"Pv"}))
        .await;
    assert_eq!(structured(&response)["code"], "unknown_loadout_name");
    assert_eq!(structured(&response)["candidates"][0]["slot"], 3);
    let response = session
        .call_tool("apply_loadout", json!({"character":"hunter","slot":1}))
        .await;
    assert_eq!(structured(&response)["code"], "empty_slot");
    // Invalid calls did not consume the sole queued action acknowledgement.
    let response = session
        .call_tool("apply_loadout", json!({"character":"hunter","slot":3}))
        .await;
    assert_eq!(response["result"]["isError"], false);
    session.shutdown().await;
}

#[tokio::test]
async fn loadouts_stdio_summary_then_one_detail_by_slot_or_name() {
    let mut session = FixtureSession::spawn(Some("loadouts-detail")).await;
    let summary = session
        .call_tool("get_loadouts", json!({"character":"hunter"}))
        .await;
    assert_eq!(summary["result"]["isError"], false);
    assert_eq!(
        structured(&summary)["characters"][0]["loadouts"][0],
        json!({
            "slot":3,"name":"PvE","subclass":null,"items":[{"slot":"kinetic","name":"Weapon"}]
        })
    );
    let mut previous = None;
    for selector in [json!(3), json!("3"), json!(" pVe ")] {
        let response = session
            .call_tool(
                "get_loadout",
                json!({"character":"hunter","loadout":selector}),
            )
            .await;
        assert_eq!(response["result"]["isError"], false, "{response}");
        let detail = structured(&response);
        assert_eq!(detail["kind"], "loadout");
        assert_eq!(
            detail["plug_insights"],
            json!([{"description":"Official text","clarity":"Research"}])
        );
        for plug in detail["items"][0]["plugs"].as_array().unwrap() {
            assert_eq!(plug["insight"], 0);
            assert!(plug.get("description").is_none());
            assert!(plug.get("clarity").is_none());
        }
        if let Some(previous) = &previous {
            assert_eq!(previous, detail);
        }
        previous = Some(detail.clone());
        // The existing dual text + structuredContent transport remains unchanged.
        assert_eq!(
            serde_json::from_str::<Value>(
                response["result"]["content"][0]["text"].as_str().unwrap()
            )
            .unwrap(),
            *detail
        );
    }
    session.shutdown().await;
}

#[tokio::test]
async fn stack_stdio_acknowledged_refused_uncertain_and_invalid() {
    for (scenario, disposition) in [
        ("stack-success", "applied"),
        ("stack-pending", "applied"),
        ("stack-refused", "refused"),
        ("stack-uncertain", "uncertain"),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let id = session.next_id();
        let discovery = session.request(id, "tools/list", None).await;
        let tools = discovery["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 25);
        let tool = tools.iter().find(|t| t["name"] == "pull_stack").unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["annotations"]["readOnlyHint"], false);
        assert_eq!(tool["annotations"]["destructiveHint"], true);
        assert_eq!(tool["annotations"]["idempotentHint"], false);
        assert_eq!(tool["annotations"]["openWorldHint"], true);
        let response = session
            .call_tool(
                "pull_stack",
                json!({"item_hash":7,"amount":"3","character":"2"}),
            )
            .await;
        assert_eq!(response["error"]["code"], -32602);
        for (hash, amount, character) in [("01", "3", "2"), ("7", "0", "2"), ("7", "3", "0")] {
            let response = session
                .call_tool(
                    "pull_stack",
                    json!({"item_hash":hash,"amount":amount,"character":character}),
                )
                .await;
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(structured(&response)["code"], "provider_validation");
        }
        let response = session
            .call_tool(
                "pull_stack",
                json!({"item_hash":"7","amount":"3","character":"2"}),
            )
            .await;
        let output = structured(&response);
        assert_eq!(output["kind"], "stack_pull");
        assert_eq!(output["disposition"], disposition, "{response}");
        assert_eq!(response["result"]["isError"], disposition != "applied");
        assert!(output.get("observations").is_none());
        session.shutdown().await;
    }
}

#[tokio::test]
async fn pull_stdio_arrival_receipt_refusal_and_invalid_inputs() {
    for (scenario, disposition, receipt) in [
        ("pull-success", "applied", "acknowledged"),
        ("pull-pending", "applied", "acknowledged"),
        ("pull-uncertain", "uncertain", "uncertain"),
        ("pull-refused-arrival", "refused", "rejected"),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let id = session.next_id();
        let discovery = session.request(id, "tools/list", None).await;
        let tools = discovery["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 25);
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == "pull_item")
            .unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert_eq!(tool["annotations"]["readOnlyHint"], false);
        assert_eq!(tool["annotations"]["idempotentHint"], false);
        for args in [
            json!({}),
            json!({"item":123}),
            json!({"item":"123","quantity":1}),
        ] {
            let response = session.call_tool("pull_item", args).await;
            assert_eq!(response["error"]["code"], -32602);
        }
        for item in ["0", "01", "+123", "123\n", "Ace", "9223372036854775808"] {
            let response = session.call_tool("pull_item", json!({"item":item})).await;
            assert_eq!(response["result"]["isError"], true);
            assert_eq!(structured(&response)["code"], "provider_validation");
        }
        let response = session.call_tool("pull_item", json!({"item":"123"})).await;
        let output = structured(&response);
        assert_eq!(output["kind"], "instanced_pull");
        assert_eq!(output["disposition"], disposition, "{scenario}: {response}");
        assert_eq!(output["receipt"]["kind"], receipt);
        assert_eq!(response["result"]["isError"], disposition != "applied");
        assert_eq!(output["dispatch"]["item"]["instance_id"], "123");
        assert_eq!(output["dispatch"]["destination"]["character_id"], "2");
        assert!(output.get("observations").is_none());
        let arrived = matches!(scenario, "pull-success" | "pull-refused-arrival");
        if arrived {
            let repeated = session.call_tool("pull_item", json!({"item":"123"})).await;
            assert_eq!(repeated["result"]["isError"], true);
            assert_eq!(structured(&repeated)["disposition"], "refused");
            assert!(structured(&repeated)["dispatch"].is_null());
        }
        session.shutdown().await;
    }
    let mut session = FixtureSession::spawn(Some("pull-gateway-error")).await;
    let response = session.call_tool("pull_item", json!({"item":"123"})).await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(structured(&response)["failure"]["stage"], "gateway");
    assert!(structured(&response)["receipt"].is_null());
    session.shutdown().await;
}

#[tokio::test]
async fn pull_storage_failure_is_error_even_after_acknowledgement() {
    for (scenario, tool, args) in [
        ("pull-storage", "pull_item", json!({"item":"123"})),
        (
            "stack-storage",
            "pull_stack",
            json!({"item_hash":"7", "amount":"3", "character":"2"}),
        ),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let response = session.call_tool(tool, args).await;
        assert_eq!(response["result"]["isError"], true);
        let output = structured(&response);
        assert_eq!(output["disposition"], "local_state_failed");
        assert_eq!(output["local_state"], "refresh_pending");
        assert_eq!(output["receipt"]["kind"], "acknowledged");
        assert_eq!(output["failure"]["category"], "local_storage");
        assert!(
            output["failure"]["message"]
                .as_str()
                .unwrap()
                .contains("local state could not be updated after the action")
        );
        assert!(output.get("observations").is_none());
        session.shutdown().await;
    }
}

#[tokio::test]
async fn postmaster_stdio_committed_service_journey_and_refusals() {
    for (scenario, coverage) in [
        ("postmaster-not-observed", "not_observed"),
        ("postmaster-unavailable", "unavailable"),
        ("postmaster-available", "available"),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let id = session.next_id();
        let discovery = session.request(id, "tools/list", None).await;
        let tool = discovery["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "get_postmaster")
            .unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        let all = session.call_tool("get_postmaster", json!({})).await;
        assert_eq!(all["result"]["isError"], false);
        let output = structured(&all);
        assert_eq!(output["kind"], "postmaster");
        assert_eq!(output["profile_currencies_coverage"], coverage);
        assert_eq!(output["metadata"]["checked_at"], 10000);
        assert_eq!(output["metadata"]["freshness"], "fresh");
        assert_eq!(output["metadata"]["source"], "profile");
        assert_eq!(output["characters"].as_array().unwrap().len(), 2);
        let owner = &output["characters"][0];
        assert_eq!(owner["character_id"], "warlock");
        assert_eq!(owner["instances"][0]["instance_id"], "postmaster-gear");
        assert_eq!(owner["instances"][0]["name"], "Gjallarhorn");
        assert_eq!(owner["stacks"].as_array().unwrap().len(), 3);
        assert_eq!(owner["stacks"][0], owner["stacks"][1]);
        assert_eq!(owner["stacks"][2]["bucket_hash"], 1469714392);
        assert!(owner["stacks"][2]["name"].is_null());
        assert!(owner["stacks"][0].get("instance_id").is_none());
        assert!(owner["instances"][0].get("bucket_hash").is_none());
        assert_eq!(output["characters"][1]["stacks"], json!([]));
        for selector in ["warlock", "WaRlOcK"] {
            let selected = session
                .call_tool("get_postmaster", json!({"character": selector}))
                .await;
            assert_eq!(structured(&selected)["characters"], json!([owner]));
        }
        let empty = session
            .call_tool("get_postmaster", json!({"character": "Titan"}))
            .await;
        assert_eq!(structured(&empty)["characters"][0]["instances"], json!([]));
        let null = session
            .call_tool("get_postmaster", json!({"character": null}))
            .await;
        assert_eq!(structured(&null), output);
        for invalid in [
            json!({"quantity":1}),
            json!({"character":1}),
            json!({"pull":true}),
        ] {
            let rejected = session.call_tool("get_postmaster", invalid).await;
            assert_eq!(rejected["error"]["code"], -32602);
        }
        for selector in ["missing".to_owned(), "".into(), "a".repeat(513)] {
            let rejected = session
                .call_tool("get_postmaster", json!({"character":selector}))
                .await;
            assert_eq!(rejected["result"]["isError"], true);
            assert_eq!(structured(&rejected)["code"], "provider_validation");
        }
        session.shutdown().await;
    }
    let mut session = FixtureSession::spawn(Some("empty-characters")).await;
    let empty = session.call_tool("get_postmaster", json!({})).await;
    assert_eq!(structured(&empty)["characters"], json!([]));
    session.shutdown().await;
    for (scenario, selector, code) in [
        (
            "postmaster-auth-refused",
            json!({}),
            "provider_authentication",
        ),
        ("postmaster-local-storage", json!({}), "local_storage"),
        (
            "postmaster-refresh-refused",
            json!({}),
            "provider_unavailable",
        ),
        (
            "ambiguous-character",
            json!({"character":"Titan"}),
            "ambiguous_match",
        ),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let result = session.call_tool("get_postmaster", selector).await;
        assert_eq!(result["result"]["isError"], true);
        assert_eq!(structured(&result)["code"], code);
        assert!(structured(&result).get("characters").is_none());
        session.shutdown().await;
    }
}

#[tokio::test]
async fn lock_stdio_applied_noop_refusal_and_incomplete_outcomes() {
    for (scenario, item, locked, status) in [
        ("success", "inst-a", true, "applied"),
        ("success", "inst-a", false, "verified_noop"),
        ("success", "missing", true, "refused"),
        ("lock-refused", "inst-a", true, "incomplete"),
        ("lock-provider-stale", "inst-a", true, "applied"),
        ("lock-uncertain", "inst-a", true, "incomplete"),
        ("lock-uncertain-no-effect", "inst-a", true, "incomplete"),
        ("lock-refresh-failure", "inst-a", true, "applied"),
    ] {
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let response = session
            .call_tool("set_lock_state", json!({"item":item,"locked":locked}))
            .await;
        let output = &response["result"]["structuredContent"];
        assert_eq!(output["status"], status, "{scenario}: {response}");
        assert_eq!(output["locked"], locked);
        assert_eq!(
            output["local_state"],
            match status {
                "applied" => "updated",
                "incomplete" => "refresh_pending",
                _ => "unchanged",
            }
        );
        assert!(output.get("profile_refresh").is_none());
        if status == "applied" || status == "incomplete" {
            assert!(output["observed"].is_null());
        }
        assert_eq!(
            response["result"]["isError"],
            !matches!(status, "applied" | "verified_noop")
        );
        if status == "applied" && scenario == "success" {
            let repeated = session
                .call_tool("set_lock_state", json!({"item":item,"locked":locked}))
                .await;
            assert_eq!(
                repeated["result"]["structuredContent"]["status"],
                "verified_noop"
            );
            let unlock = session
                .call_tool("set_lock_state", json!({"item":item,"locked":false}))
                .await;
            assert_eq!(unlock["result"]["structuredContent"]["status"], "applied");
        }
        session.shutdown().await;
    }
}

#[tokio::test]
async fn lock_stdio_rejects_invalid_schema_without_dispatch() {
    let mut session = FixtureSession::spawn(Some("success")).await;
    for invalid in [
        json!({"item":"inst-a"}),
        json!({"item":"inst-a","locked":"true"}),
        json!({"item":"inst-a","locked":true,"character":"warlock"}),
    ] {
        let response = session.call_tool("set_lock_state", invalid).await;
        assert!(response.get("error").is_some());
    }
    let response = session
        .call_tool("set_lock_state", json!({"item":"inst-a","locked":false}))
        .await;
    assert_eq!(
        response["result"]["structuredContent"]["status"],
        "verified_noop"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn equip_stdio_json_matches_cli_for_all_outcomes() {
    for (scenario, item) in [
        ("equip-success", "123"),
        ("equip-noop", "123"),
        ("equip-provider-stale", "123"),
        ("equip-success", "missing"),
        ("equip-partial", "123"),
        ("equip-auth", "123"),
        ("equip-auth-concurrent", "123"),
        ("equip-auth-unavailable-refresh", "123"),
        ("equip-auth-refresh-failure", "123"),
        ("equip-uncertain", "123"),
        ("equip-uncertain-no-effect", "123"),
        ("equip-refresh-failure", "123"),
        ("equip-contradiction", "123"),
    ] {
        let expected = equip_support::cli_json(scenario, item, "warlock");
        assert_eq!(expected, equip_support::mcp_json(scenario, item, "warlock"));
        let mut session = FixtureSession::spawn(Some(scenario)).await;
        let result = session
            .call_tool("equip_item", json!({"item":item,"character":"warlock"}))
            .await;
        assert_eq!(
            result["result"]["structuredContent"], expected,
            "{scenario}"
        );
        assert_eq!(
            result["result"]["isError"],
            !matches!(
                expected["status"].as_str().unwrap(),
                "applied" | "verified_success" | "verified_noop"
            )
        );
        session.shutdown().await;
    }
}

#[tokio::test]
async fn equip_stdio_invalid_arguments_never_mutate() {
    equip_support::reject_before_io();
    let mut session = FixtureSession::spawn(Some("equip-success")).await;
    for invalid in [
        json!({"item":"123"}),
        json!({"item":"123","character":"warlock","membership":"secret"}),
    ] {
        let response = session.call_tool("equip_item", invalid).await;
        assert_eq!(response["error"]["code"], -32602);
    }
    for invalid in [
        json!({"item":"123\n","character":"warlock"}),
        json!({"item":"123","character":""}),
        json!({"item":"x".repeat(257),"character":"warlock"}),
        json!({"item":"İ".repeat(129),"character":"warlock"}),
    ] {
        let response = session.call_tool("equip_item", invalid).await;
        assert_eq!(response["result"]["isError"], true);
    }
    let response = session
        .call_tool("equip_item", json!({"item":"123","character":"warlock"}))
        .await;
    assert_eq!(response["result"]["structuredContent"]["status"], "applied");
    assert_eq!(
        response["result"]["structuredContent"]["steps"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    session.shutdown().await;
}

#[tokio::test]
async fn equip_is_discovered_and_refuses_missing_item() {
    let mut session = FixtureSession::spawn(None).await;
    let id = session.next_id();
    let tools = session.request(id, "tools/list", Some(json!({}))).await;
    assert!(
        tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "equip_item")
    );
    let id = session.next_id();
    let result = session
        .request(
            id,
            "tools/call",
            Some(json!({"name":"equip_item","arguments":{"item":"missing","character":"warlock"}})),
        )
        .await;
    assert_eq!(result["result"]["structuredContent"]["status"], "refused");
    assert_eq!(result["result"]["isError"], true);
    session.shutdown().await;
}
const WEEKLY_RESET_TOOL: &str = "weekly_reset";

/// Reads exactly one newline-delimited JSON-RPC frame from the child's stdout.
///
/// Every line must parse as JSON and advertise protocol version `2.0`, which is
/// what proves stdout carries protocol frames only. A secret sentinel or
/// membership identifier in a frame fails the test immediately.
async fn read_frame(reader: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    let read = tokio::time::timeout(TIMEOUT, reader.read_line(&mut line))
        .await
        .expect("a stdout frame arrives within the timeout")
        .expect("stdout is readable");
    assert!(read > 0, "stdout closed before an expected response");
    for sentinel in STREAM_SENTINELS {
        assert!(
            !line.contains(sentinel),
            "a sentinel leaked onto stdout: {line}"
        );
    }
    let frame: Value = serde_json::from_str(&line).expect("stdout carries only JSON frames");
    assert_eq!(frame["jsonrpc"], "2.0", "stdout is protocol-only: {line}");
    frame
}

/// Writes one newline-delimited JSON-RPC frame to the child's stdin.
async fn write_frame(stdin: &mut ChildStdin, frame: &Value) {
    let mut bytes = serde_json::to_vec(frame).expect("a frame serializes");
    bytes.push(b'\n');
    tokio::time::timeout(TIMEOUT, stdin.write_all(&bytes))
        .await
        .expect("a write completes within the timeout")
        .expect("stdin is writable");
    tokio::time::timeout(TIMEOUT, stdin.flush())
        .await
        .expect("a flush completes within the timeout")
        .expect("stdin is flushable");
}

/// One live offline-fixture child process and its MCP session.
struct FixtureSession {
    child: tokio::process::Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    stderr: ChildStderr,
    next_id: u64,
}

impl FixtureSession {
    /// Spawns the fixture and completes the initialize handshake.
    async fn spawn(scenario: Option<&str>) -> Self {
        let executable = env!("CARGO_BIN_EXE_offline-fixture");
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Never inherit an ambient scenario: the default session must be the
            // clean success contract.
            .env_remove(SCENARIO_ENV);
        if let Some(scenario) = scenario {
            command.env(SCENARIO_ENV, scenario);
        }
        let mut child = command.spawn().expect("the offline fixture spawns");
        let stdin = child.stdin.take().expect("child stdin is piped");
        let stdout = child.stdout.take().expect("child stdout is piped");
        let stderr = child.stderr.take().expect("child stderr is piped");
        let mut session = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            stderr,
            next_id: 0,
        };
        session.initialize().await;
        session
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Completes the initialize handshake and the initialized notification.
    async fn initialize(&mut self) {
        let id = self.next_id();
        let response = self
            .request(
                id,
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "offline-fixture-test", "version": "0.0.1" }
                })),
            )
            .await;
        assert_eq!(response["result"]["serverInfo"]["name"], "destiny-mcp");
        assert!(
            response["result"]["capabilities"]["tools"].is_object(),
            "the server advertises the tools capability"
        );
        self.notify("notifications/initialized", None).await;
    }

    /// Sends one request and returns the matching response frame.
    async fn request(&mut self, id: u64, method: &str, params: Option<Value>) -> Value {
        let mut frame = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        write_frame(&mut self.stdin, &frame).await;
        loop {
            let response = read_frame(&mut self.reader).await;
            if response.get("id").and_then(Value::as_u64) == Some(id) {
                return response;
            }
        }
    }

    /// Sends one notification frame.
    async fn notify(&mut self, method: &str, params: Option<Value>) {
        let mut frame = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        write_frame(&mut self.stdin, &frame).await;
    }

    /// Calls one tool with bounded arguments and returns its response frame.
    async fn call_tool(&mut self, name: &str, arguments: Value) -> Value {
        let id = self.next_id();
        self.request(
            id,
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
        .await
    }

    /// Lists the discovered tool names in stable order.
    async fn list_tool_names(&mut self) -> Vec<String> {
        let id = self.next_id();
        let response = self.request(id, "tools/list", None).await;
        response["result"]["tools"]
            .as_array()
            .expect("tools/list returns an array")
            .iter()
            .map(|tool| tool["name"].as_str().expect("a tool name").to_owned())
            .collect()
    }

    /// Cancels, closes stdin, and asserts a clean shutdown with clean streams.
    async fn shutdown(mut self) -> String {
        let id = self.next_id();
        self.notify(
            "notifications/cancelled",
            Some(json!({ "requestId": id, "reason": "test finished" })),
        )
        .await;
        drop(self.stdin);

        let status = tokio::time::timeout(TIMEOUT, self.child.wait())
            .await
            .expect("the child exits within the bounded timeout")
            .expect("the child status is readable");
        assert!(status.success(), "the fixture exited cleanly: {status}");

        let mut stderr_text = String::new();
        tokio::time::timeout(TIMEOUT, self.stderr.read_to_string(&mut stderr_text))
            .await
            .expect("stderr is readable within the bounded timeout")
            .expect("stderr reads to end");
        for sentinel in STREAM_SENTINELS {
            assert!(
                !stderr_text.contains(sentinel),
                "a sentinel leaked onto stderr: {stderr_text}"
            );
        }
        assert!(
            !stderr_text.contains("\"jsonrpc\""),
            "protocol frames must not be written to stderr: {stderr_text}"
        );
        stderr_text
    }
}

/// Asserts no shared secret canary appears in a serialized response value.
fn assert_no_secret_in(value: &Value) {
    let rendered = serde_json::to_string(value).expect("the response serializes");
    for sentinel in leak_sentinels::SECRETS {
        assert!(
            !rendered.contains(sentinel),
            "a secret sentinel reached a serialized response: {rendered}"
        );
    }
}

fn structured(response: &Value) -> &Value {
    &response["result"]["structuredContent"]
}

#[tokio::test]
async fn every_tool_output_schema_declares_an_object_root() {
    let mut session = FixtureSession::spawn(None).await;
    let id = session.next_id();
    let listed = session.request(id, "tools/list", None).await;
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    assert!(tools.iter().any(|tool| tool["name"] == "inspect_item"));
    assert!(tools.iter().any(|tool| tool["name"] == "move_item"));
    let invalid: Vec<_> = tools
        .iter()
        .filter(|tool| {
            tool.get("outputSchema")
                .is_some_and(|schema| schema["type"] != "object")
        })
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    session.shutdown().await;
    assert!(
        invalid.is_empty(),
        "outputSchema.type must be object: {invalid:?}"
    );
}

#[tokio::test]
async fn weekly_stdio_contract() {
    let mut session = FixtureSession::spawn(None).await;
    let id = session.next_id();
    let listed = session.request(id, "tools/list", None).await;
    let tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "weekly")
        .expect("weekly tool listed");
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(tool["outputSchema"]["type"], "object");
    for (args, expected_id) in [
        (json!({}), "warlock"),
        (json!({"character": "warlock"}), "warlock"),
        (json!({"character": "titan"}), "zavala"),
    ] {
        let response = session.call_tool("weekly", args).await;
        assert_eq!(structured(&response)["character"]["id"], expected_id);
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(structured(&response)["kind"], "weekly");
        assert!(structured(&response)["unclassified_unresolved"].is_number());
        let weekly = structured(&response);
        assert!(weekly.get("newest").is_none());
        assert!(weekly.get("portal_focus").is_none());
        assert_eq!(weekly["override"]["root"], "Override: Europa");
        assert_eq!(weekly["override"]["variants"].as_array().unwrap().len(), 2);
        assert_eq!(weekly["expedition"]["root"], "Expedition: Cosmodrome");
        assert_eq!(
            weekly["vanguard_alerts"]["grandmaster"]["root"],
            "The Disgraced"
        );
        assert_eq!(weekly["vanguard_alerts"]["crucible"][0]["root"], "Control");
        assert_eq!(
            weekly["vanguard_alerts"]["quickplay"][0]["reward_names"],
            json!(["Gjallarhorn", "Fatebringer"])
        );
        assert_eq!(
            weekly["vanguard_alerts"]["crucible"][0]["variants"][0]["challenges"][0]["progress"],
            0
        );
    }
    let rejected = session.call_tool("weekly", json!({"unknown": true})).await;
    assert_eq!(rejected["error"]["code"], -32602);
    session.shutdown().await;
}

#[tokio::test]
async fn weekly_stdio_ambiguous_selector() {
    let mut session = FixtureSession::spawn(Some("ambiguous-character")).await;
    let response = session
        .call_tool("weekly", json!({"character": "titan"}))
        .await;
    assert_eq!(structured(&response)["code"], "ambiguous_match");
    assert_eq!(
        structured(&response)["message"],
        "the request matched multiple items"
    );
    for selector in ["unknown", "", " ", "id\nvalue"] {
        let response = session
            .call_tool("weekly", json!({"character": selector}))
            .await;
        assert_eq!(response["error"]["code"], -32602);
        assert_eq!(response["error"]["message"], "invalid tool arguments");
    }
    session.shutdown().await;
}

#[tokio::test]
async fn weekly_stdio_empty_characters_are_invalid_arguments() {
    let mut session = FixtureSession::spawn(Some("empty-characters")).await;
    let response = session.call_tool("weekly", json!({})).await;
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["message"], "invalid tool arguments");
    session.shutdown().await;
}

#[tokio::test]
async fn weekly_stdio_missing_definitions() {
    let mut session = FixtureSession::spawn(Some("definitions-unavailable")).await;
    let response = session.call_tool("weekly", json!({})).await;
    assert_eq!(structured(&response)["code"], "definitions_unavailable");
    session.shutdown().await;
}

#[tokio::test]
async fn weekly_reset_stdio_missing_definitions() {
    let mut session = FixtureSession::spawn(Some("definitions-unavailable")).await;
    let response = session.call_tool(WEEKLY_RESET_TOOL, json!({})).await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(structured(&response)["code"], "definitions_unavailable");
    assert_eq!(
        structured(&response)["message"],
        "definitions not loaded; run ghostlink sync-manifest"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn weekly_reset_stdio_dates() {
    let mut session = FixtureSession::spawn(None).await;
    let response = session
        .call_tool(WEEKLY_RESET_TOOL, json!({"date": "2026-10-13"}))
        .await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    let reset = structured(&response);
    assert_eq!(reset["current_week"], false);
    assert_eq!(reset["calendar"]["source"], "prediction");
    assert_eq!(reset["calendar"]["start"], "2026-10-13T17:00:00Z");
    assert_eq!(reset["calendar"]["end"], "2026-10-20T17:00:00Z");
    assert_eq!(reset["milestones_source"], "observed");
    assert_eq!(reset["milestones"], json!([]));
    let rotations = reset["calendar"]["rotations"].as_array().unwrap();
    assert_eq!(rotations.len(), 4);
    assert!(
        rotations
            .iter()
            .all(|r| r["status"] == "predicted" && !r["activities"].as_array().unwrap().is_empty())
    );
    assert_eq!(rotations[0]["activities"][0]["name"], "Vow of the Disciple");
    assert!(rotations[0]["activities"][0]["hash"].is_u64());
    let response = session
        .call_tool(WEEKLY_RESET_TOOL, json!({"date": "2026-06-15"}))
        .await;
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(structured(&response)["current_week"], false);
    for rotation in structured(&response)["calendar"]["rotations"]
        .as_array()
        .unwrap()
    {
        assert_eq!(rotation["status"], "no_data");
        assert_eq!(rotation["activities"], json!([]));
        assert!(rotation["coverage_start"].is_string());
        assert!(!rotation["coverage_note"].as_str().unwrap().is_empty());
    }
    for date in ["2026-13-01", "06/10/2026", "2026-02-29", "2026-10-6"] {
        let response = session
            .call_tool(WEEKLY_RESET_TOOL, json!({"date": date}))
            .await;
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(structured(&response)["code"], "provider_validation");
        assert_eq!(
            structured(&response)["message"],
            "invalid date: expected YYYY-MM-DD (a representable UTC calendar date)"
        );
    }
    let log = session.shutdown().await;
    assert!(log.contains("weekly milestones requests: 0"), "{log}");
}

#[tokio::test]
async fn weekly_reset_stdio_contract() {
    let mut session = FixtureSession::spawn(None).await;
    let id = session.next_id();
    let listed = session.request(id, "tools/list", None).await;
    let tool = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == WEEKLY_RESET_TOOL)
        .expect("weekly tool listed");
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    assert_eq!(tool["outputSchema"]["type"], "object");
    let date = &tool["inputSchema"]["properties"]["date"];
    assert!(date.is_object());
    assert!(date["description"].as_str().unwrap().contains("YYYY-MM-DD"));
    assert!(date["description"].as_str().unwrap().contains(
        "the weekly period in effect at the end of that UTC day; omit for the current week"
    ));
    assert!(
        !tool["inputSchema"]["required"]
            .as_array()
            .is_some_and(|r| r.contains(&json!("date")))
    );
    assert!(
        tool["description"]
            .as_str()
            .unwrap()
            .contains("community-derived")
    );
    for field in ["current_week", "calendar", "milestones_source"] {
        assert!(
            tool["outputSchema"]["properties"].get(field).is_some(),
            "{field}"
        );
    }
    let response = session.call_tool(WEEKLY_RESET_TOOL, json!({})).await;
    assert_eq!(response["result"]["isError"], false);
    let reset = structured(&response);
    assert_eq!(reset["kind"], "weekly_reset");
    assert_eq!(reset["current_week"], true);
    assert_eq!(reset["calendar"]["source"], "prediction");
    assert_eq!(reset["calendar"]["start"], "2026-10-06T17:00:00Z");
    assert_eq!(reset["milestones_source"], "observed");
    assert_eq!(reset["definition_version"], "fixture-definitions-v1");
    assert_eq!(reset["milestones"][0]["name"], "Weekly raid");
    assert_eq!(reset["milestones"][0]["start"], "1970-01-01T00:00:00Z");
    assert_eq!(reset["milestones"][0]["end"], Value::Null);
    assert_eq!(
        reset["milestones"][0]["activities"][0]["modifiers"][1]["description"],
        "{var:42}"
    );
    assert_eq!(
        reset["milestones"][0]["activities"][0]["modifiers"][0]["name"],
        Value::Null
    );
    assert_eq!(
        reset["milestones"][0]["activities"][0]["modifiers"][0]["resolved"],
        false
    );
    let rejected = session
        .call_tool(WEEKLY_RESET_TOOL, json!({"unknown": "sentinel"}))
        .await;
    assert_eq!(rejected["error"]["code"], -32602);
    // Explicit current date uses UTC end-of-day and also fetches live milestones.
    let response = session
        .call_tool(WEEKLY_RESET_TOOL, json!({"date": "2026-10-06"}))
        .await;
    assert_eq!(structured(&response)["current_week"], true);
    assert_eq!(
        structured(&response)["milestones"][0]["name"],
        "Weekly raid"
    );
    let log = session.shutdown().await;
    assert!(log.contains("weekly milestones requests: 2"), "{log}");
}

#[tokio::test]
async fn child_process_stdio_completes_the_one_call_move_journey() {
    let mut session = FixtureSession::spawn(None).await;

    // --- discovery: mutation is default-enabled ---------------------------------
    let names = session.list_tool_names().await;
    assert_eq!(
        names,
        [
            "get_postmaster",
            LIST_CHARACTERS_TOOL,
            SEARCH_INVENTORY_TOOL,
            INSPECT_ITEM_TOOL,
            GET_SUBCLASS_TOOL,
            GET_ARTIFACT_TOOL,
            GET_LOADOUTS_TOOL,
            "get_loadout",
            WEEKLY_RESET_TOOL,
            "weekly",
            "get_subclass_options",
            "list_builds",
            "get_build",
            "save_build",
            "delete_build",
            "analyze_build",
            "build_substitutes",
            MOVE_ITEM_TOOL,
            "equip_item",
            "set_lock_state",
            "pull_item",
            "pull_stack",
            "apply_loadout",
            "equip_items",
            "apply_build"
        ],
        "the product-equivalent server discovers reads, local builds and game mutations"
    );

    // --- the reads still work ----------------------------------------------------
    let characters = session.call_tool(LIST_CHARACTERS_TOOL, json!({})).await;
    assert_eq!(characters["result"]["isError"], false);
    let listed = structured(&characters)["characters"]
        .as_array()
        .expect("characters array");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], "warlock");
    assert!(
        listed[0]["name"]
            .as_str()
            .expect("the readable name crosses to the client")
            .contains(leak_sentinels::CONTROL),
        "the non-secret control proves the stdout stream carries real content"
    );

    let search = session
        .call_tool(SEARCH_INVENTORY_TOOL, json!({ "text": "gjallar" }))
        .await;
    assert_eq!(search["result"]["isError"], false);
    assert_eq!(structured(&search)["candidates"][0]["hash"], 1);

    let inspect = session
        .call_tool(INSPECT_ITEM_TOOL, json!({ "instance_id": "inst-a" }))
        .await;
    assert_eq!(structured(&inspect)["status"], "found");

    let subclass = session.call_tool(GET_SUBCLASS_TOOL, json!({})).await;
    assert_eq!(subclass["result"]["isError"], false);
    assert_eq!(structured(&subclass)["kind"], "subclass");
    assert!(
        structured(&subclass)["characters"]
            .as_array()
            .unwrap()
            .iter()
            .all(|character| character["subclass"].is_null())
    );

    let options = session
        .call_tool(
            "get_subclass_options",
            json!({"character": "warlock", "element": "arc"}),
        )
        .await;
    assert_eq!(options["result"]["isError"], false);
    assert_eq!(structured(&options)["kind"], "subclass_options");
    assert_eq!(
        structured(&options)["characters"].as_array().unwrap().len(),
        1
    );
    let invalid = session
        .call_tool("get_subclass_options", json!({"locked": true}))
        .await;
    assert!(invalid.get("error").is_some());

    let artifact = session.call_tool(GET_ARTIFACT_TOOL, json!({})).await;
    assert_eq!(artifact["result"]["isError"], false);
    assert_eq!(
        structured(&artifact),
        &json!({"kind": "artifact", "points": null, "characters": []})
    );
    let missing = session
        .call_tool(GET_ARTIFACT_TOOL, json!({"character": "missing"}))
        .await;
    assert_eq!(structured(&missing)["code"], "provider_validation");
    for value in ["".to_owned(), "a".repeat(513), "bad\u{7}".to_owned()] {
        let invalid = session
            .call_tool(GET_ARTIFACT_TOOL, json!({"character": value}))
            .await;
        assert_eq!(structured(&invalid)["code"], "provider_validation");
    }

    let loadouts = session.call_tool(GET_LOADOUTS_TOOL, json!({})).await;
    assert_eq!(loadouts["result"]["isError"], false);
    assert_eq!(
        structured(&loadouts),
        &json!({"kind": "loadouts", "characters": []})
    );
    let missing = session
        .call_tool(GET_LOADOUTS_TOOL, json!({"character": "missing"}))
        .await;
    assert_eq!(structured(&missing)["code"], "provider_validation");
    for value in [
        "".to_owned(),
        " ".to_owned(),
        "a".repeat(513),
        "bad\u{7}".to_owned(),
    ] {
        let invalid = session
            .call_tool(GET_LOADOUTS_TOOL, json!({"character": value}))
            .await;
        assert_eq!(structured(&invalid)["code"], "provider_validation");
    }
    for arguments in [json!({"unknown": true}), json!({"character": 1})] {
        let invalid = session.call_tool(GET_LOADOUTS_TOOL, arguments).await;
        assert_eq!(invalid["error"]["code"], -32602);
    }

    let detail = session.call_tool("get_loadout", json!({"loadout":3})).await;
    assert_eq!(detail["result"]["isError"], true);
    assert_eq!(structured(&detail)["code"], "empty_slot");
    let detail = session
        .call_tool("get_loadout", json!({"loadout":"Unknown"}))
        .await;
    assert_eq!(structured(&detail)["code"], "unknown_loadout_name");
    for arguments in [
        json!({}),
        json!({"loadout":true}),
        json!({"loadout":3,"unknown":true}),
    ] {
        let invalid = session.call_tool("get_loadout", arguments).await;
        assert_eq!(invalid["error"]["code"], -32602);
    }
    for arguments in [
        json!({"loadout":0}),
        json!({"loadout":" "}),
        json!({"loadout":"a".repeat(513)}),
        json!({"loadout":3,"character":"bad\u{7}"}),
    ] {
        let invalid = session.call_tool("get_loadout", arguments).await;
        assert_eq!(structured(&invalid)["code"], "provider_validation");
    }

    // --- one move_item request completes the whole journey -----------------------
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-a", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], false);
    let success = structured(&moved);
    assert_eq!(success["status"], "applied");
    assert_eq!(success["effects"]["item_id"], "inst-a");
    assert_eq!(success["effects"]["via_vault"], false);
    assert_eq!(success["effects"]["source"]["kind"], "vault");
    assert_eq!(success["effects"]["destination"]["kind"], "character");
    assert_eq!(success["effects"]["destination"]["character_id"], "warlock");
    assert!(success.get("observed").is_none());

    // Exactly the planned steps were acknowledged, without claiming observation.
    let steps = success["steps"].as_array().expect("step evidence");
    assert_eq!(
        steps.len(),
        1,
        "a Vault move uses exactly one transfer call"
    );
    assert_eq!(steps[0]["ordinal"], 1);
    assert_eq!(steps[0]["from"]["kind"], "vault");
    assert_eq!(steps[0]["to"]["character_id"], "warlock");
    assert_eq!(steps[0]["outcome"]["status"], "completed");
    assert_no_secret_in(&moved);

    // --- a repeated request fails closed without replay --------------------------
    let repeat = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-a", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(repeat["result"]["isError"], true);
    assert_eq!(structured(&repeat)["code"], "unsupported_operation");
    assert!(
        structured(&repeat).get("steps").is_none(),
        "a rejected plan carries no step evidence"
    );
    assert_no_secret_in(&repeat);

    // --- a missing item is a sanitized typed failure -----------------------------
    let missing = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "absent", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(structured(&missing)["code"], "stale_state");
    assert_no_secret_in(&missing);

    // --- the opaque provider location never crosses the boundary -----------------
    let opaque = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-b", "to_vault": true }),
        )
        .await;
    assert_eq!(opaque["result"]["isError"], true);
    assert_eq!(structured(&opaque)["code"], "unsupported_operation");
    assert_no_secret_in(&opaque);

    session.shutdown().await;
}

#[tokio::test]
async fn search_inventory_stdio_defaults_to_compact_and_accepts_detail() {
    let mut session = FixtureSession::spawn(None).await;
    let query = json!({"text":"The Last Word", "instances":{"item_hashes":[5]}});
    let default = session
        .call_tool(SEARCH_INVENTORY_TOOL, query.clone())
        .await;
    let mut explicit = query.clone();
    explicit["detail"] = json!(false);
    let compact = session.call_tool(SEARCH_INVENTORY_TOOL, explicit).await;
    let mut detailed = query;
    detailed["detail"] = json!(true);
    let detail = session.call_tool(SEARCH_INVENTORY_TOOL, detailed).await;
    for result in [&default, &compact, &detail] {
        assert_eq!(result["result"]["isError"], false);
        assert_no_secret_in(result);
    }
    assert_eq!(structured(&default), structured(&compact));
    let tile = &structured(&default)["hits"][0];
    assert_eq!(tile["instance_id"], "inst-e");
    assert_eq!(tile["name"], "The Last Word");
    assert_eq!(
        tile["location"],
        json!({"kind":"character", "character_id":"zavala"})
    );
    for omitted in [
        "instance",
        "definition",
        "available_perks",
        "available_mods",
        "archetype",
        "stats",
    ] {
        assert!(tile.get(omitted).is_none(), "{omitted}");
    }
    assert_eq!(
        structured(&detail)["hits"][0]["instance"]["instance_id"],
        "inst-e"
    );
    assert_eq!(
        structured(&default)["candidates"],
        structured(&detail)["candidates"]
    );
    session.shutdown().await;
}

/// The named semantic journey: one `item_text` request, not an exact instance.
///
/// This is the in-memory MCP fixture, not the real-adapter journey composed in
/// `destiny-adapters` (DFJ-002): the fixture composes the read and mutation seams
/// with deterministic application test doubles, so it proves the MCP semantic
/// boundary, the ordered Vault-mediated plan, and the sanitized process result,
/// but it makes no Bungie or durable-storage claim. The named definition
/// `The Last Word` (hash 5) resolves to the single concrete instance `inst-e`,
/// which the fixture seeds on `zavala`, so the destination `warlock` forces the
/// two ordered Vault steps a character-to-character move requires.
#[tokio::test]
async fn move_item_semantic_text_resolves_the_named_instance_through_the_vault() {
    let mut session = FixtureSession::spawn(None).await;

    // --- the read projection resolves the same semantic term ---------------------
    // The search and inspection run on the fixture's read projection, which is
    // seeded from the same initial world, so the client can observe the named
    // definition and concrete instance before the single mutation call.
    // The concrete-hit list is requested explicitly, constrained to the single
    // definition hash the term resolved to (hash 5), which is the same bounded
    // definition-then-instance resolution the semantic move performs.
    let search = session
        .call_tool(
            SEARCH_INVENTORY_TOOL,
            json!({ "detail": true, "text": "The Last Word", "instances": { "item_hashes": [5] } }),
        )
        .await;
    assert_eq!(search["result"]["isError"], false);
    let searched = structured(&search);
    assert_eq!(
        searched["candidates"].as_array().expect("candidates").len(),
        1,
        "the normalized term matches exactly one definition"
    );
    assert_eq!(searched["candidates"][0]["hash"], 5);
    assert_eq!(searched["candidates"][0]["name"], "The Last Word");
    assert_eq!(
        searched["hits"].as_array().expect("hits").len(),
        1,
        "the named definition has exactly one concrete instance"
    );
    assert_eq!(searched["hits"][0]["instance"]["instance_id"], "inst-e");
    assert_eq!(
        searched["hits"][0]["instance"]["location"]["kind"],
        "character"
    );
    assert_eq!(
        searched["hits"][0]["instance"]["location"]["character_id"],
        "zavala"
    );
    assert_no_secret_in(&search);

    let inspect = session
        .call_tool(INSPECT_ITEM_TOOL, json!({ "instance_id": "inst-e" }))
        .await;
    assert_eq!(inspect["result"]["isError"], false);
    assert_eq!(structured(&inspect)["status"], "found");
    assert_eq!(structured(&inspect)["definition"]["hash"], 5);
    assert_eq!(structured(&inspect)["instance"]["instance_id"], "inst-e");
    assert_eq!(
        structured(&inspect)["instance"]["location"]["character_id"],
        "zavala"
    );
    assert_no_secret_in(&inspect);

    // --- exactly one semantic mutation call --------------------------------------
    // The named item and the exact destination are the only caller inputs; the
    // server resolves `item_text` to `inst-e` itself, so no instance identifier
    // crosses the boundary for the mutation.
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "item_text": "The Last Word", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], false);
    let success = structured(&moved);
    assert_eq!(success["status"], "applied");

    // The resolved concrete instance, not the text, is what was moved, and the
    // route is character -> Vault -> character, so the Vault is used exactly once.
    assert_eq!(success["effects"]["item_id"], "inst-e");
    assert_eq!(success["effects"]["via_vault"], true);
    assert_eq!(success["effects"]["source"]["kind"], "character");
    assert_eq!(success["effects"]["source"]["character_id"], "zavala");
    assert_eq!(success["effects"]["destination"]["kind"], "character");
    assert_eq!(success["effects"]["destination"]["character_id"], "warlock");

    // Acknowledgement does not claim a refreshed observation.
    assert!(success.get("observed").is_none());

    // Bounded evidence: the two ordered mediated steps, both completed.
    let steps = success["steps"].as_array().expect("step evidence");
    assert_eq!(
        steps.len(),
        2,
        "a character move is two Vault-mediated steps"
    );
    assert_eq!(steps[0]["ordinal"], 1);
    assert_eq!(steps[0]["from"]["kind"], "character");
    assert_eq!(steps[0]["from"]["character_id"], "zavala");
    assert_eq!(steps[0]["to"]["kind"], "vault");
    assert_eq!(steps[0]["outcome"]["status"], "completed");
    assert_eq!(steps[1]["ordinal"], 2);
    assert_eq!(steps[1]["from"]["kind"], "vault");
    assert_eq!(steps[1]["to"]["kind"], "character");
    assert_eq!(steps[1]["to"]["character_id"], "warlock");
    assert_eq!(steps[1]["outcome"]["status"], "completed");
    assert_no_secret_in(&moved);

    // Sanitized process result: a clean exit and no sentinel or protocol frame
    // on stderr.
    session.shutdown().await;
}

#[tokio::test]
async fn move_item_reports_a_verified_no_op_for_an_already_satisfied_destination() {
    let mut session = FixtureSession::spawn(Some("already-satisfied")).await;

    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-d", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], false);
    let no_op = structured(&moved);
    assert_eq!(no_op["status"], "verified_no_op");
    assert_eq!(no_op["observed"]["kind"], "character");
    assert_eq!(no_op["observed"]["character_id"], "warlock");

    // A committed local publication already shows the requested end state,
    // so the plan's only step stays unattempted without acquisition or dispatch.
    let steps = no_op["steps"].as_array().expect("step evidence");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["outcome"]["status"], "unattempted");

    session.shutdown().await;
}

#[tokio::test]
async fn move_item_reports_remote_divergence_as_a_typed_sanitized_provider_rejection() {
    let mut session = FixtureSession::spawn(Some("provider-rejection")).await;

    // Remote divergence is not re-read before dispatch. The simulated provider
    // is the final validator and refuses the locally planned source.
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-c", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], true);
    let incomplete = structured(&moved);
    assert_eq!(incomplete["status"], "incomplete");
    assert_eq!(incomplete["partial"], false);
    assert_eq!(incomplete["steps"][0]["outcome"]["status"], "rejected");
    assert_eq!(
        incomplete["steps"][0]["outcome"]["reason"],
        "not_transferable"
    );
    assert_no_secret_in(&moved);

    session.shutdown().await;
}

#[tokio::test]
async fn move_item_acknowledges_stale_profile_without_claiming_observation() {
    let mut session = FixtureSession::spawn(Some("move-provider-stale")).await;
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({"instance_id":"inst-a","character_id":"warlock"}),
        )
        .await;
    assert_eq!(moved["result"]["isError"], false);
    let pending = structured(&moved);
    assert_eq!(pending["status"], "applied");
    assert!(pending.get("observed").is_none());
    assert_eq!(pending["steps"].as_array().unwrap().len(), 1);
    assert_eq!(pending["steps"][0]["outcome"]["status"], "completed");
    assert_no_secret_in(&moved);
    session.shutdown().await;
}

#[tokio::test]
async fn move_item_reports_a_partial_execution_honestly() {
    let mut session = FixtureSession::spawn(Some("partial")).await;

    // A character-to-character move is two ordered steps. The second step is
    // refused, so the item is honestly stranded in the Vault with bounded
    // evidence and no rollback claim.
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-e", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], true);
    let incomplete = structured(&moved);
    assert_eq!(incomplete["status"], "incomplete");
    assert_eq!(incomplete["partial"], true);
    assert_eq!(incomplete["expected_location"]["kind"], "vault");
    assert!(incomplete.get("observed").is_none());
    assert_eq!(incomplete["effects"]["via_vault"], true);

    let steps = incomplete["steps"].as_array().expect("step evidence");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["outcome"]["status"], "completed");
    assert_eq!(steps[1]["outcome"]["status"], "rejected");
    assert_eq!(steps[1]["outcome"]["reason"], "not_transferable");

    session.shutdown().await;
}
