//! RMCP tools-only server for the Destiny semantic reads.
//!
//! [`ReadToolServer`] adapts the provider-neutral [`InventoryReadService`] seam
//! to an [`rmcp::ServerHandler`]. It exposes exactly the three bounded read
//! tools, dispatches each `tools/call` through the shared argument conversion
//! and service, and returns the structured result or a sanitized tool error.
//!
//! # Boundaries
//!
//! - The handler holds a `Box<dyn InventoryReadService + Send>` behind a
//!   `std::sync::Mutex`, so the synchronous read service can be shared by the
//!   [`rmcp::ServerHandler`] without introducing a second service contract. The
//!   lock is only ever held for one synchronous dispatch, never across an
//!   `.await`.
//! - Dispatch is offloaded with [`tokio::task::spawn_blocking`]: the read
//!   service performs blocking provider and storage work, and the RMCP server
//!   loop is async. Keeping the blocking work off the async worker is what makes
//!   the composition safe for the SDK.
//! - No concrete adapter type appears here. Every failure is a sanitized
//!   [`crate::schema::McpError`] category or a fixed protocol error; a raw
//!   diagnostic, provider payload, or credential never crosses this boundary.

use std::fmt;
use std::sync::{Arc, Mutex};

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorCode, ErrorData, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::schema::{
    INSPECT_ITEM_TOOL, InspectItemArgs, LIST_CHARACTERS_TOOL, ListCharactersArgs, McpError,
    SEARCH_INVENTORY_TOOL, SearchInventoryArgs,
};
use crate::service::InventoryReadService;

type SharedReadService = Arc<Mutex<Box<dyn InventoryReadService + Send>>>;

/// The tools-only RMCP server over one shared read service.
///
/// The service is shared behind a mutex so the same `Send` read service can
/// answer concurrent `tools/call` requests one at a time. The server is
/// `Send + Sync + 'static`, as [`rmcp::ServerHandler`] requires.
pub struct ReadToolServer {
    service: SharedReadService,
}

impl ReadToolServer {
    /// Wraps one read service for the stdio server.
    pub fn new(service: Box<dyn InventoryReadService + Send>) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
        }
    }

    /// The exact tool descriptors this server advertises, in stable order.
    pub fn discover_tools(&self) -> ListToolsResult {
        ListToolsResult {
            tools: crate::schema::tool_specs(),
            ..Default::default()
        }
    }

    /// Synchronously dispatches one tool call through the shared service.
    ///
    /// This is the single dispatch path the async handler and the focused unit
    /// tests share. An unknown tool name and arguments that do not fit the
    /// bounded schema are fixed protocol errors; a rejected bound or a service
    /// failure is a sanitized structured tool error.
    pub fn dispatch(&self, name: &str, arguments: JsonObject) -> Result<CallToolResult, ErrorData> {
        dispatch_locked(&self.service, name, arguments)
    }
}

impl fmt::Debug for ReadToolServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReadToolServer")
    }
}

impl ServerHandler for ReadToolServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("destiny-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Read-only, bounded access to the signed-in Destiny inventory and Manifest definitions.",
            )
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        crate::schema::tool_specs()
            .into_iter()
            .find(|tool| tool.name == name)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(self.discover_tools())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Clone the shared handle so the blocking dispatch owns everything it
        // needs; the synchronous service runs off the async worker thread.
        let service = Arc::clone(&self.service);
        let name = request.name.to_string();
        let arguments = request.arguments.unwrap_or_default();
        let result =
            tokio::task::spawn_blocking(move || dispatch_locked(&service, &name, arguments))
                .await
                .map_err(|_| internal_error())?;
        result.map(CallToolResponse::from)
    }
}

/// Dispatches one bounded tool call while holding the shared service lock.
fn dispatch_locked(
    service: &Mutex<Box<dyn InventoryReadService + Send>>,
    name: &str,
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let mut guard = service.lock().map_err(|_| internal_error())?;
    match name {
        LIST_CHARACTERS_TOOL => dispatch_list_characters(guard.as_mut(), arguments),
        SEARCH_INVENTORY_TOOL => dispatch_search_inventory(guard.as_mut(), arguments),
        INSPECT_ITEM_TOOL => dispatch_inspect_item(guard.as_mut(), arguments),
        _ => Err(unknown_tool()),
    }
}

fn dispatch_list_characters(
    service: &mut (dyn InventoryReadService + Send),
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let arguments: ListCharactersArgs = deserialize(arguments)?;
    let request = arguments.into_request();
    Ok(service
        .list_characters(&request)
        .map_or_else(tool_error, |output| output.into_call_tool_result()))
}

fn dispatch_search_inventory(
    service: &mut (dyn InventoryReadService + Send),
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let arguments: SearchInventoryArgs = deserialize(arguments)?;
    let request = match arguments.into_request() {
        Ok(request) => request,
        Err(error) => return Ok(error.into_call_tool_result()),
    };
    Ok(service
        .search_inventory(&request)
        .map_or_else(tool_error, |output| output.into_call_tool_result()))
}

fn dispatch_inspect_item(
    service: &mut (dyn InventoryReadService + Send),
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let arguments: InspectItemArgs = deserialize(arguments)?;
    let request = match arguments.into_request() {
        Ok(request) => request,
        Err(error) => return Ok(error.into_call_tool_result()),
    };
    Ok(service
        .inspect_item(&request)
        .map_or_else(tool_error, |output| output.into_call_tool_result()))
}

/// Deserializes bounded tool arguments without echoing the raw input.
fn deserialize<T: DeserializeOwned>(arguments: JsonObject) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(arguments)).map_err(|_| invalid_arguments())
}

fn tool_error(error: McpError) -> CallToolResult {
    error.into_call_tool_result()
}

/// The fixed protocol error for a tool name the server does not expose.
fn unknown_tool() -> ErrorData {
    ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "unknown tool", None)
}

/// The fixed protocol error for arguments that do not fit the tool schema.
///
/// The offending value and the serde diagnostic are deliberately discarded: a
/// malformed request never reaches a tool, and its raw text never reaches a
/// client.
fn invalid_arguments() -> ErrorData {
    ErrorData::invalid_params("invalid tool arguments", None)
}

/// The fixed protocol error for an internal dispatch failure.
fn internal_error() -> ErrorData {
    ErrorData::new(ErrorCode::INTERNAL_ERROR, "internal server error", None)
}

#[cfg(test)]
mod tests {
    use super::ReadToolServer;
    use crate::schema::{
        InspectItemOutput, LIST_CHARACTERS_TOOL, ListCharactersOutput, McpError, QueryMetadataView,
        SEARCH_INVENTORY_TOOL, SearchInventoryOutput,
    };
    use crate::service::{InventoryReadService, ListCharactersRequest};
    use destiny_app::{
        ErrorCategory, InspectItemRequest, MAX_SEARCH_LIMIT, SearchInventoryRequest,
    };
    use rmcp::ServerHandler;
    use rmcp::model::JsonObject;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A deterministic recording read service with observable concurrency.
    #[derive(Clone, Debug)]
    struct RecordingService {
        calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
        delay: Duration,
        failure: Option<ErrorCategory>,
    }

    impl RecordingService {
        fn new() -> Self {
            Self {
                calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                inflight: Arc::new(AtomicUsize::new(0)),
                max_inflight: Arc::new(AtomicUsize::new(0)),
                delay: Duration::ZERO,
                failure: None,
            }
        }

        fn with_delay(delay: Duration) -> Self {
            Self {
                delay,
                ..Self::new()
            }
        }

        fn with_failure(category: ErrorCategory) -> Self {
            Self {
                failure: Some(category),
                ..Self::new()
            }
        }

        fn enter(&self, call: &'static str) {
            self.calls.lock().expect("call log lock").push(call);
            let current = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_inflight.fetch_max(current, Ordering::SeqCst);
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
        }

        fn leave(&self) {
            self.inflight.fetch_sub(1, Ordering::SeqCst);
        }

        fn recorded(&self) -> Vec<&'static str> {
            self.calls.lock().expect("call log lock").clone()
        }

        fn max_concurrency(&self) -> usize {
            self.max_inflight.load(Ordering::SeqCst)
        }
    }

    impl InventoryReadService for RecordingService {
        fn list_characters(
            &mut self,
            _request: &ListCharactersRequest,
        ) -> Result<ListCharactersOutput, McpError> {
            self.enter("list_characters");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(ListCharactersOutput {
                    characters: Vec::new(),
                    metadata: metadata(),
                }),
            };
            self.leave();
            result
        }

        fn search_inventory(
            &mut self,
            _request: &SearchInventoryRequest,
        ) -> Result<SearchInventoryOutput, McpError> {
            self.enter("search_inventory");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(SearchInventoryOutput {
                    candidates: Vec::new(),
                    hits: Vec::new(),
                    metadata: metadata(),
                }),
            };
            self.leave();
            result
        }

        fn inspect_item(
            &mut self,
            _request: &InspectItemRequest,
        ) -> Result<InspectItemOutput, McpError> {
            self.enter("inspect_item");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(InspectItemOutput::Missing),
            };
            self.leave();
            result
        }
    }

    fn metadata() -> QueryMetadataView {
        QueryMetadataView {
            dataset: "profile_inventory".into(),
            source: "profile".into(),
            revision: 1,
            checked_at: 0,
            definition_version: "fixture".into(),
            freshness: "fresh".into(),
        }
    }

    fn server(service: RecordingService) -> (ReadToolServer, RecordingService) {
        let handle = service.clone();
        (ReadToolServer::new(Box::new(service)), handle)
    }

    fn args(value: Value) -> JsonObject {
        value
            .as_object()
            .cloned()
            .expect("test arguments are a JSON object")
    }

    fn structured(result: &rmcp::model::CallToolResult) -> Value {
        result
            .structured_content
            .clone()
            .expect("a structured tool result")
    }

    #[test]
    fn discovery_exposes_exactly_the_three_reads_in_stable_order() {
        let (server, _handle) = server(RecordingService::new());
        let tools = server.discover_tools().tools;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [LIST_CHARACTERS_TOOL, SEARCH_INVENTORY_TOOL, "inspect_item"]
        );
        for tool in &tools {
            assert!(tool.output_schema.is_some());
        }
    }

    #[test]
    fn info_advertises_the_tools_capability_and_server_identity() {
        let (server, _handle) = server(RecordingService::new());
        let info = ServerHandler::get_info(&server);
        assert!(info.capabilities.tools.is_some());
        assert_eq!(info.server_info.name, "destiny-mcp");
    }

    #[test]
    fn get_tool_returns_only_the_discovered_tools() {
        let (server, _handle) = server(RecordingService::new());
        assert!(server.get_tool(LIST_CHARACTERS_TOOL).is_some());
        assert!(server.get_tool(SEARCH_INVENTORY_TOOL).is_some());
        assert!(server.get_tool("inspect_item").is_some());
        assert!(server.get_tool("delete_everything").is_none());
    }

    #[test]
    fn unknown_tool_is_a_fixed_protocol_error() {
        let (server, handle) = server(RecordingService::new());
        let error = server
            .dispatch("not_a_tool", JsonObject::new())
            .expect_err("unknown tool is a protocol error");
        assert_eq!(error.code.0, -32601);
        assert_eq!(error.message, "unknown tool");
        assert!(handle.recorded().is_empty());
    }

    #[test]
    fn malformed_arguments_are_a_fixed_protocol_error() {
        let (server, handle) = server(RecordingService::new());

        for (name, invalid) in [
            (LIST_CHARACTERS_TOOL, json!({"unknown": true})),
            (SEARCH_INVENTORY_TOOL, json!({"unknown": true})),
            (SEARCH_INVENTORY_TOOL, json!({"limit": "not-a-number"})),
            ("inspect_item", json!({"instance_id": 1})),
            ("inspect_item", json!({"instance_id": "inst", "unknown": 1})),
        ] {
            let error = server
                .dispatch(name, args(invalid))
                .expect_err("malformed arguments are a protocol error");
            assert_eq!(
                error.code.0, -32602,
                "tool {name} must reject malformed arguments"
            );
            assert_eq!(error.message, "invalid tool arguments");
        }

        // A malformed request never reaches the shared service.
        assert!(handle.recorded().is_empty());
    }

    #[test]
    fn each_tool_dispatches_bounded_arguments_to_the_shared_service() {
        let (server, handle) = server(RecordingService::new());

        let list = server
            .dispatch(LIST_CHARACTERS_TOOL, JsonObject::new())
            .expect("list dispatch succeeds");
        assert_eq!(list.is_error, Some(false));

        let search = server
            .dispatch(SEARCH_INVENTORY_TOOL, args(json!({"text": "gjallar"})))
            .expect("search dispatch succeeds");
        assert_eq!(search.is_error, Some(false));

        let inspect = server
            .dispatch("inspect_item", args(json!({"instance_id": "inst-a"})))
            .expect("inspect dispatch succeeds");
        assert_eq!(inspect.is_error, Some(false));
        assert_eq!(structured(&inspect), json!({"status": "missing"}));

        assert_eq!(
            handle.recorded(),
            ["list_characters", "search_inventory", "inspect_item"]
        );
    }

    #[test]
    fn bound_violations_return_a_sanitized_tool_error() {
        let (server, handle) = server(RecordingService::new());

        let over_limit = server
            .dispatch(SEARCH_INVENTORY_TOOL, args(json!({"limit": 0})))
            .expect("a bound violation is a tool-level error");
        assert_eq!(over_limit.is_error, Some(true));
        assert_eq!(structured(&over_limit)["code"], "provider_validation");

        let empty_identifier = server
            .dispatch("inspect_item", args(json!({"instance_id": ""})))
            .expect("an empty identifier is a tool-level error");
        assert_eq!(empty_identifier.is_error, Some(true));
        assert_eq!(structured(&empty_identifier)["code"], "provider_validation");

        let over_large = MAX_SEARCH_LIMIT + 1;
        let too_many = server
            .dispatch(SEARCH_INVENTORY_TOOL, args(json!({ "limit": over_large })))
            .expect("an over-large limit is a tool-level error");
        assert_eq!(too_many.is_error, Some(true));
        assert_eq!(structured(&too_many)["code"], "provider_validation");

        // The rejected arguments never reached the shared service.
        assert!(handle.recorded().is_empty());
    }

    #[test]
    fn service_failures_return_a_sanitized_tool_error_without_raw_diagnostics() {
        let (server, _handle) = server(RecordingService::with_failure(
            ErrorCategory::ProviderUnavailable,
        ));
        let result = server
            .dispatch(LIST_CHARACTERS_TOOL, JsonObject::new())
            .expect("a service failure is a tool-level error");
        assert_eq!(result.is_error, Some(true));
        let rendered = serde_json::to_string(&structured(&result)).expect("serializes");
        assert!(rendered.contains("provider_unavailable"));
        assert!(!rendered.contains("raw"));
        assert_eq!(
            structured(&result)["message"],
            "the provider is unavailable"
        );
    }

    #[test]
    fn concurrent_dispatch_is_serialized_by_the_shared_service() {
        let delay = Duration::from_millis(5);
        let (server, handle) = server(RecordingService::with_delay(delay));
        let server = Arc::new(server);

        let mut threads = Vec::new();
        for _ in 0..8 {
            let server = Arc::clone(&server);
            threads.push(std::thread::spawn(move || {
                server
                    .dispatch(LIST_CHARACTERS_TOOL, JsonObject::new())
                    .expect("concurrent dispatch succeeds")
            }));
        }
        for thread in threads {
            let result = thread.join().expect("thread completes");
            assert_eq!(result.is_error, Some(false));
        }

        assert_eq!(handle.recorded().len(), 8);
        assert_eq!(
            handle.max_concurrency(),
            1,
            "the mutex must serialize service access"
        );
    }

    #[test]
    fn repeated_dispatch_is_deterministic() {
        let (server, _handle) = server(RecordingService::new());
        let first = server
            .dispatch("inspect_item", args(json!({"instance_id": "inst-a"})))
            .expect("first dispatch");
        let second = server
            .dispatch("inspect_item", args(json!({"instance_id": "inst-a"})))
            .expect("second dispatch");
        assert_eq!(structured(&first), structured(&second));
    }
}
