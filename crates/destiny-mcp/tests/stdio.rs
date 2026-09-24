//! Real child-process stdio integration for the Destiny MCP tools.
//!
//! This suite is compiled only with the non-default `offline-fixtures` feature.
//! It spawns the clearly test-only `offline-fixture` binary, drives a complete
//! MCP session over the child's stdin/stdout, and asserts the wire contract
//! directly.
//!
//! A default fixture session proves initialize, discovery of the three reads
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

use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout, Command};

/// Bounded wait for every read, write, and process exit.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Must match the sentinel embedded by `offline_fixture.rs`.
const SENTINEL: &str = "offline-fixture-secret-sentinel";

/// The membership identifier the fixture resolves; it must never reach a client.
const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

/// The scenario selector the fixture reads at process start.
const SCENARIO_ENV: &str = "DESTINY_FIXTURE_SCENARIO";

const LIST_CHARACTERS_TOOL: &str = "list_characters";
const SEARCH_INVENTORY_TOOL: &str = "search_inventory";
const INSPECT_ITEM_TOOL: &str = "inspect_item";
const MOVE_ITEM_TOOL: &str = "move_item";

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
    assert!(
        !line.contains(SENTINEL),
        "a secret sentinel leaked onto stdout: {line}"
    );
    assert!(
        !line.contains(MEMBERSHIP_IDENTIFIER),
        "a membership identifier leaked onto stdout: {line}"
    );
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
        assert!(
            !stderr_text.contains(SENTINEL),
            "a secret sentinel leaked onto stderr: {stderr_text}"
        );
        assert!(
            !stderr_text.contains(MEMBERSHIP_IDENTIFIER),
            "a membership identifier leaked onto stderr: {stderr_text}"
        );
        assert!(
            !stderr_text.contains("\"jsonrpc\""),
            "protocol frames must not be written to stderr: {stderr_text}"
        );
    }
}

fn structured(response: &Value) -> &Value {
    &response["result"]["structuredContent"]
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
            MOVE_ITEM_TOOL
        ],
        "the product-equivalent server discovers the three reads plus move_item"
    );

    // --- the reads still work ----------------------------------------------------
    let characters = session.call_tool(LIST_CHARACTERS_TOOL, json!({})).await;
    assert_eq!(characters["result"]["isError"], false);
    let listed = structured(&characters)["characters"]
        .as_array()
        .expect("characters array");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], "warlock");

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
    assert!(
        !serde_json::to_string(&moved)
            .expect("serializes")
            .contains(SENTINEL),
        "the success response must not carry the opaque provider text"
    );

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

    // --- a missing item is a sanitized typed failure -----------------------------
    let missing = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "absent", "character_id": "warlock" }),
        )
        .await;
    assert_eq!(missing["result"]["isError"], true);
    assert_eq!(structured(&missing)["code"], "stale_state");

    // --- the opaque provider location never crosses the boundary -----------------
    let opaque = session
        .call_tool(
            MOVE_ITEM_TOOL,
            json!({ "instance_id": "inst-b", "to_vault": true }),
        )
        .await;
    assert_eq!(opaque["result"]["isError"], true);
    assert_eq!(structured(&opaque)["code"], "unsupported_operation");
    assert!(
        !serde_json::to_string(&opaque)
            .expect("serializes")
            .contains(SENTINEL),
        "an unsupported opaque source must not echo provider text"
    );

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
    assert!(
        !serde_json::to_string(&moved)
            .expect("serializes")
            .contains(SENTINEL)
    );

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
