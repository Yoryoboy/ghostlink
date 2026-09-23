//! Real child-process stdio integration for the Destiny MCP read tools.
//!
//! This suite is compiled only with the non-default `offline-fixtures` feature.
//! It spawns the clearly test-only `offline-fixture` binary, drives a complete
//! MCP session over the child's stdin/stdout (initialize, discovery, all three
//! reads, a protocol failure, a sanitized tool failure, cancellation, and a
//! clean shutdown), and asserts the wire contract directly.
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
use tokio::process::{ChildStdin, ChildStdout, Command};

/// Bounded wait for every read, write, and process exit.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Must match the sentinel embedded by `offline_fixture.rs`.
const SENTINEL: &str = "offline-fixture-secret-sentinel";

/// The membership identifier the fixture resolves; it must never reach a client.
const MEMBERSHIP_IDENTIFIER: &str = "4611686018484639825";

const LIST_CHARACTERS_TOOL: &str = "list_characters";
const SEARCH_INVENTORY_TOOL: &str = "search_inventory";
const INSPECT_ITEM_TOOL: &str = "inspect_item";

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

/// Sends one request and returns the matching response frame.
///
/// Any interleaved protocol notification without the expected id is skipped, but
/// it still had to be a valid protocol frame to be read at all.
async fn request(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    id: u64,
    method: &str,
    params: Option<Value>,
) -> Value {
    let mut frame = json!({ "jsonrpc": "2.0", "id": id, "method": method });
    if let Some(params) = params {
        frame["params"] = params;
    }
    write_frame(stdin, &frame).await;
    loop {
        let response = read_frame(reader).await;
        if response.get("id").and_then(Value::as_u64) == Some(id) {
            return response;
        }
    }
}

/// Sends one notification frame.
async fn notify(stdin: &mut ChildStdin, method: &str, params: Option<Value>) {
    let mut frame = json!({ "jsonrpc": "2.0", "method": method });
    if let Some(params) = params {
        frame["params"] = params;
    }
    write_frame(stdin, &frame).await;
}

/// Calls one tool with the given bounded arguments.
async fn call_tool(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<ChildStdout>,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    request(
        stdin,
        reader,
        id,
        "tools/call",
        Some(json!({ "name": name, "arguments": arguments })),
    )
    .await
}

#[tokio::test]
async fn child_process_stdio_completes_initialize_discovery_calls_and_shutdown() {
    let executable = env!("CARGO_BIN_EXE_offline-fixture");
    let mut child = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("the offline fixture spawns");
    let mut stdin = child.stdin.take().expect("child stdin is piped");
    let stdout = child.stdout.take().expect("child stdout is piped");
    let mut stderr = child.stderr.take().expect("child stderr is piped");
    let mut reader = BufReader::new(stdout);

    // --- initialize -----------------------------------------------------------
    let initialize = request(
        &mut stdin,
        &mut reader,
        1,
        "initialize",
        Some(json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "offline-fixture-test", "version": "0.0.1" }
        })),
    )
    .await;
    assert_eq!(initialize["result"]["serverInfo"]["name"], "destiny-mcp");
    assert!(
        initialize["result"]["capabilities"]["tools"].is_object(),
        "the server advertises the tools capability"
    );
    notify(&mut stdin, "notifications/initialized", None).await;

    // --- discovery ------------------------------------------------------------
    let list = request(&mut stdin, &mut reader, 2, "tools/list", None).await;
    let tools = list["result"]["tools"]
        .as_array()
        .expect("tools/list returns an array");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name"))
        .collect();
    assert_eq!(
        names,
        [
            LIST_CHARACTERS_TOOL,
            SEARCH_INVENTORY_TOOL,
            INSPECT_ITEM_TOOL
        ],
        "the server discovers exactly the three semantic reads in stable order"
    );
    for tool in tools {
        assert!(
            tool.get("inputSchema").is_some(),
            "every tool has an input schema"
        );
        assert!(
            tool.get("outputSchema").is_some(),
            "every tool has a structured output schema"
        );
    }

    // --- list_characters ------------------------------------------------------
    let characters = call_tool(&mut stdin, &mut reader, 3, LIST_CHARACTERS_TOOL, json!({})).await;
    assert_eq!(characters["result"]["isError"], false);
    assert_eq!(
        characters["result"]["structuredContent"]["characters"][0]["id"],
        "warlock"
    );

    // --- search_inventory -----------------------------------------------------
    let search = call_tool(
        &mut stdin,
        &mut reader,
        4,
        SEARCH_INVENTORY_TOOL,
        json!({ "text": "gjallar" }),
    )
    .await;
    assert_eq!(search["result"]["isError"], false);
    assert_eq!(
        search["result"]["structuredContent"]["candidates"][0]["hash"],
        1
    );

    // --- inspect_item ---------------------------------------------------------
    let inspect = call_tool(
        &mut stdin,
        &mut reader,
        5,
        INSPECT_ITEM_TOOL,
        json!({ "instance_id": "inst-a" }),
    )
    .await;
    assert_eq!(inspect["result"]["isError"], false);
    assert_eq!(inspect["result"]["structuredContent"]["status"], "found");
    assert_eq!(
        inspect["result"]["structuredContent"]["instance"]["instance_id"],
        "inst-a"
    );

    // --- unknown tool is a fixed protocol error -------------------------------
    let unknown = call_tool(&mut stdin, &mut reader, 6, "not_a_tool", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(unknown["error"]["message"], "unknown tool");

    // --- a bound violation is a sanitized tool error --------------------------
    let bounded = call_tool(
        &mut stdin,
        &mut reader,
        7,
        SEARCH_INVENTORY_TOOL,
        json!({ "limit": 0 }),
    )
    .await;
    assert_eq!(bounded["result"]["isError"], true);
    assert_eq!(
        bounded["result"]["structuredContent"]["code"],
        "provider_validation"
    );

    // --- cancellation, then a clean shutdown ----------------------------------
    notify(
        &mut stdin,
        "notifications/cancelled",
        Some(json!({ "requestId": 7, "reason": "test finished" })),
    )
    .await;
    drop(stdin);

    let status = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .expect("the child exits within the bounded timeout")
        .expect("the child status is readable");
    assert!(status.success(), "the fixture exited cleanly: {status}");

    let mut stderr_text = String::new();
    tokio::time::timeout(TIMEOUT, stderr.read_to_string(&mut stderr_text))
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
