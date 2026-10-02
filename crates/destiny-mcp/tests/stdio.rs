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
//! post-action refresh, and destination verification), a repeated request that
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
const MOVE_ITEM_TOOL: &str = "move_item";
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
    async fn shutdown(mut self) {
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
    assert!(tool["outputSchema"].is_object());
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
    assert!(tool["outputSchema"].is_object());
    let response = session.call_tool(WEEKLY_RESET_TOOL, json!({})).await;
    assert_eq!(response["result"]["isError"], false);
    let reset = structured(&response);
    assert_eq!(reset["kind"], "weekly_reset");
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
    session.shutdown().await;
}

#[tokio::test]
async fn child_process_stdio_completes_the_one_call_move_journey() {
    let mut session = FixtureSession::spawn(None).await;

    // --- discovery: mutation is default-enabled ---------------------------------
    let names = session.list_tool_names().await;
    assert_eq!(
        names,
        [
            LIST_CHARACTERS_TOOL,
            SEARCH_INVENTORY_TOOL,
            INSPECT_ITEM_TOOL,
            WEEKLY_RESET_TOOL,
            "weekly",
            MOVE_ITEM_TOOL
        ],
        "the product-equivalent server discovers the four reads plus move_item"
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

    // --- one move_item request completes the whole journey -----------------------
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-a", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], false);
    let success = structured(&moved);
    assert_eq!(success["status"], "verified_success");
    assert_eq!(success["effects"]["item_id"], "inst-a");
    assert_eq!(success["effects"]["via_vault"], false);
    assert_eq!(success["effects"]["source"]["kind"], "vault");
    assert_eq!(success["effects"]["destination"]["kind"], "character");
    assert_eq!(success["effects"]["destination"]["character_id"], "warlock");
    assert_eq!(success["observed"]["kind"], "character");
    assert_eq!(success["observed"]["character_id"], "warlock");

    // Exactly the planned steps were dispatched: a Vault move is one transfer.
    // The refresh and verification are proven by the destination being observed
    // at all: the projection only reflects the transfer through the forced
    // post-action refresh.
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
            json!({ "text": "The Last Word", "instances": { "item_hashes": [5] } }),
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
    assert_eq!(success["status"], "verified_success");

    // The resolved concrete instance, not the text, is what was moved, and the
    // route is character -> Vault -> character, so the Vault is used exactly once.
    assert_eq!(success["effects"]["item_id"], "inst-e");
    assert_eq!(success["effects"]["via_vault"], true);
    assert_eq!(success["effects"]["source"]["kind"], "character");
    assert_eq!(success["effects"]["source"]["character_id"], "zavala");
    assert_eq!(success["effects"]["destination"]["kind"], "character");
    assert_eq!(success["effects"]["destination"]["character_id"], "warlock");

    // Post-refresh verification: only the forced refresh after the transfer can
    // make the destination observable from the shared world.
    assert_eq!(success["observed"]["kind"], "character");
    assert_eq!(success["observed"]["character_id"], "warlock");

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

    // The fresh preflight observed the requested end state, so the plan's only
    // step stays unattempted and no transfer is dispatched.
    let steps = no_op["steps"].as_array().expect("step evidence");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["outcome"]["status"], "unattempted");

    session.shutdown().await;
}

#[tokio::test]
async fn move_item_reports_a_changed_preflight_as_a_typed_sanitized_failure() {
    let mut session = FixtureSession::spawn(Some("preflight-change")).await;

    // The plan was built from the seed source, but the fresh preflight observes
    // the world's changed location, so the move is rejected before any dispatch.
    let moved = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-c", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(moved["result"]["isError"], true);
    assert_eq!(structured(&moved)["code"], "stale_state");
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
    assert_eq!(moved["result"]["isError"], false);
    let incomplete = structured(&moved);
    assert_eq!(incomplete["status"], "incomplete");
    assert_eq!(incomplete["partial"], true);
    assert_eq!(incomplete["observed"]["kind"], "vault");
    assert_eq!(incomplete["effects"]["via_vault"], true);

    let steps = incomplete["steps"].as_array().expect("step evidence");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["outcome"]["status"], "completed");
    assert_eq!(steps[1]["outcome"]["status"], "rejected");
    assert_eq!(steps[1]["outcome"]["reason"], "not_transferable");

    session.shutdown().await;
}
