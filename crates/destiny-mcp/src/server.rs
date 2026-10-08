//! RMCP tools-only server for the Destiny semantic tools.
//!
//! [`ReadToolServer`] adapts the provider-neutral [`InventoryReadService`] seam
//! to an [`rmcp::ServerHandler`]. On its own it exposes exactly the four
//! bounded read tools, dispatches each `tools/call` through the shared argument
//! conversion and service, and returns the structured result or a sanitized tool
//! error. When constructed with the optional adapter-free [`MutationToolService`]
//! seam it additionally advertises and dispatches the single intent-authorized
//! `move_item` tool, so a read-only server keeps precisely the read-only
//! discovery and dispatch it had before.
//!
//! # Boundaries
//!
//! - The handler holds a `Box<dyn InventoryReadService + Send>` and an optional
//!   `Box<dyn MutationToolService + Send>` behind `std::sync::Mutex`, so both
//!   synchronous services can be shared by the [`rmcp::ServerHandler`] without
//!   introducing a second service contract. A lock is only ever held for one
//!   synchronous dispatch, never across an `.await`. One outer operation guard
//!   serializes both seams: local refresh publication cannot overtake mutation
//!   preflight, ordered dispatch, or freshness invalidation.
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
    INSPECT_ITEM_TOOL, InspectItemArgs, LIST_CHARACTERS_TOOL, ListCharactersArgs, MOVE_ITEM_TOOL,
    McpError, MoveItemArgs, SEARCH_INVENTORY_TOOL, SearchInventoryArgs, move_item_tool,
};
use crate::service::{InventoryReadService, MutationToolService};

type SharedReadService = Arc<Mutex<Box<dyn InventoryReadService + Send>>>;
type SharedMutationService = Arc<Mutex<Box<dyn MutationToolService + Send>>>;

/// The instructions a read-only server advertises.
///
/// This is the exact text the read-only construction has always advertised; it
/// is kept verbatim so an unconfigured server reports the same capability it
/// always did.
const READ_ONLY_INSTRUCTIONS: &str =
    "Read-only, bounded access to the signed-in Destiny inventory and Manifest definitions.";

/// The instructions a server with a composed mutation seam advertises.
///
/// The text must not claim the server is read-only: composing the seam adds the
/// single intent-authorized `move_item` mutation, so the advertised capability
/// has to match the discovery set.
const MUTATION_INSTRUCTIONS: &str = "Bounded access to the signed-in Destiny inventory and Manifest definitions, plus the single intent-authorized `move_item`, `equip_item`, and `set_lock_state`, `pull_item`, `pull_stack`, `apply_loadout`, and `equip_items` mutations.";

/// The tools-only RMCP server over one shared read service and an optional
/// mutation seam.
///
/// Each service is shared behind its own mutex. An outer operation guard spans
/// the entire call on either seam, including all snapshot publication paths.
/// Lock ordering is operation -> service -> individual gateway/repository calls.
/// Sync and freshness invalidation never reenter the operation guard. This protects
/// only this server's local publication, not external game state or other processes.
/// The server is `Send + Sync + 'static`, as
/// [`rmcp::ServerHandler`] requires.
pub struct ReadToolServer {
    operation: Arc<Mutex<()>>,
    service: SharedReadService,
    mutation: Option<SharedMutationService>,
}

impl ReadToolServer {
    /// Wraps one read service for the stdio server, exposing only the reads.
    ///
    /// This is the read-only construction: the server discovers exactly the
    /// four read tools and rejects `move_item` as an unknown tool, so an
    /// unconfigured mutation seam cannot be told apart from one that was never
    /// built.
    pub fn new(service: Box<dyn InventoryReadService + Send>) -> Self {
        Self {
            operation: Arc::new(Mutex::new(())),
            service: Arc::new(Mutex::new(service)),
            mutation: None,
        }
    }

    /// Wraps one read service and one mutation seam for the stdio server.
    ///
    /// The mutation seam is adapter-free: it is only a
    /// [`MutationToolService`], so this crate still names no concrete adapter.
    /// Configuring it advertises the single `move_item` tool and routes its
    /// call through the seam; every read keeps its unchanged behavior.
    pub fn with_mutation(
        service: Box<dyn InventoryReadService + Send>,
        mutation: Box<dyn MutationToolService + Send>,
    ) -> Self {
        Self {
            operation: Arc::new(Mutex::new(())),
            service: Arc::new(Mutex::new(service)),
            mutation: Some(Arc::new(Mutex::new(mutation))),
        }
    }

    /// The exact tool descriptors this server advertises, in stable order.
    ///
    /// The four reads are always present and deterministic. `move_item` is
    /// appended only when a mutation seam was composed, so the discovery set is
    /// a function of the construction and never of the request.
    pub fn discover_tools(&self) -> ListToolsResult {
        let mut tools = crate::schema::tool_specs();
        if self.mutation.is_some() {
            tools.push(move_item_tool());
            tools.push(crate::schema::equip_item_tool());
            tools.push(crate::schema::set_lock_state_tool());
            tools.push(crate::schema::pull_item_tool());
            tools.push(crate::schema::pull_stack_tool());
            tools.push(crate::schema::apply_loadout_tool());
            tools.push(crate::schema::equip_items_tool());
        }
        ListToolsResult {
            tools,
            ..Default::default()
        }
    }

    /// Synchronously dispatches one tool call through the selected seam.
    ///
    /// This is the single dispatch path the async handler and the focused unit
    /// tests share. An unknown tool name and arguments that do not fit the
    /// bounded schema are fixed protocol errors; a rejected bound or a service
    /// failure is a sanitized structured tool error. `move_item` routes through
    /// the mutation seam only when one was composed; otherwise it stays an
    /// unknown tool.
    pub fn dispatch(&self, name: &str, arguments: JsonObject) -> Result<CallToolResult, ErrorData> {
        dispatch_operation(
            &self.operation,
            &self.service,
            self.mutation_for(name),
            name,
            arguments,
        )
    }

    /// The mutation seam to use for one tool name, if it is composed.
    fn mutation_for(&self, name: &str) -> Option<&SharedMutationService> {
        if name == MOVE_ITEM_TOOL
            || name == crate::schema::EQUIP_ITEM_TOOL
            || name == crate::schema::SET_LOCK_STATE_TOOL
            || name == crate::schema::PULL_ITEM_TOOL
            || name == crate::schema::PULL_STACK_TOOL
            || name == crate::schema::APPLY_LOADOUT_TOOL
            || name == crate::schema::EQUIP_ITEMS_TOOL
        {
            self.mutation.as_ref()
        } else {
            None
        }
    }

    /// The instructions this server advertises for its own capabilities.
    ///
    /// A read-only server reports exactly [`READ_ONLY_INSTRUCTIONS`]; a server
    /// with a composed mutation seam reports [`MUTATION_INSTRUCTIONS`] instead,
    /// so the advertised capability never contradicts the discovered tools.
    fn instructions(&self) -> &'static str {
        if self.mutation.is_some() {
            MUTATION_INSTRUCTIONS
        } else {
            READ_ONLY_INSTRUCTIONS
        }
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
            .with_server_info(Implementation::new(
                "destiny-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(self.instructions())
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.discover_tools()
            .tools
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
        let name = request.name.to_string();
        let arguments = request.arguments.unwrap_or_default();
        // Clone the selected shared handle so the blocking dispatch owns
        // everything it needs; the synchronous service runs off the async
        // worker thread. A read-only server keeps the mutation name unknown.
        let operation = Arc::clone(&self.operation);
        let service = Arc::clone(&self.service);
        let mutation = self.mutation_for(&name).map(Arc::clone);
        let result = tokio::task::spawn_blocking(move || {
            dispatch_operation(&operation, &service, mutation.as_ref(), &name, arguments)
        })
        .await
        .map_err(|_| internal_error())?;
        result.map(CallToolResponse::from)
    }
}

/// Single operation boundary for both direct dispatch and the async handler.
/// Acquire before any service work, release only after verification/no-op/error.
fn dispatch_operation(
    operation: &Mutex<()>,
    service: &SharedReadService,
    mutation: Option<&SharedMutationService>,
    name: &str,
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let _operation = operation.lock().map_err(|_| internal_error())?;
    match mutation {
        Some(mutation) => dispatch_mutation_locked(mutation, name, arguments),
        None => dispatch_locked(service, name, arguments),
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
        "get_postmaster" => {
            let arguments: crate::schema::GetPostmasterArgs = deserialize(arguments)?;
            let character = match arguments.into_request() {
                Ok(character) => character,
                Err(error) => return Ok(error.into_call_tool_result()),
            };
            Ok(guard
                .get_postmaster(character.as_ref())
                .map_or_else(tool_error, |output| output.into_call_tool_result()))
        }
        LIST_CHARACTERS_TOOL => dispatch_list_characters(guard.as_mut(), arguments),
        SEARCH_INVENTORY_TOOL => dispatch_search_inventory(guard.as_mut(), arguments),
        INSPECT_ITEM_TOOL => dispatch_inspect_item(guard.as_mut(), arguments),
        crate::schema::GET_SUBCLASS_TOOL => dispatch_get_subclass(guard.as_mut(), arguments),
        crate::schema::GET_SUBCLASS_OPTIONS_TOOL => {
            let arguments: crate::schema::GetSubclassOptionsArgs = deserialize(arguments)?;
            let (character, element) = match arguments.into_request() {
                Ok(request) => request,
                Err(error) => return Ok(error.into_call_tool_result()),
            };
            Ok(guard
                .get_subclass_options(character.as_ref(), element)
                .map_or_else(tool_error, |output| output.into_call_tool_result()))
        }
        crate::schema::GET_LOADOUTS_TOOL => {
            let arguments: crate::schema::GetLoadoutsArgs = deserialize(arguments)?;
            let character = match arguments.into_request() {
                Ok(character) => character,
                Err(error) => return Ok(error.into_call_tool_result()),
            };
            Ok(guard
                .get_loadouts(character.as_ref())
                .map_or_else(tool_error, |output| output.into_call_tool_result()))
        }
        crate::schema::GET_ARTIFACT_TOOL => {
            let arguments: crate::schema::GetArtifactArgs = deserialize(arguments)?;
            let character = match arguments.into_request() {
                Ok(character) => character,
                Err(error) => return Ok(error.into_call_tool_result()),
            };
            Ok(guard
                .get_artifact(character.as_ref())
                .map_or_else(tool_error, |output| output.into_call_tool_result()))
        }
        crate::schema::WEEKLY_TOOL => {
            let args: crate::schema::WeeklyArgs = deserialize(arguments)?;
            if args.character.as_ref().is_some_and(|s| {
                s.is_empty() || s.chars().count() > crate::schema::MAX_IDENTIFIER_LENGTH
            }) {
                return Err(invalid_arguments());
            }
            match guard.weekly(&args) {
                Ok(output) => Ok(output.into_call_tool_result()),
                Err(crate::service::WeeklyReadError::InvalidArguments) => Err(invalid_arguments()),
                Err(crate::service::WeeklyReadError::Tool(error)) => Ok(tool_error(error)),
            }
        }
        crate::schema::WEEKLY_RESET_TOOL => {
            let args: crate::schema::WeeklyResetArgs = deserialize(arguments)?;
            Ok(guard
                .weekly_reset(&args)
                .map_or_else(tool_error, |output| output.into_call_tool_result()))
        }
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

fn dispatch_get_subclass(
    service: &mut (dyn InventoryReadService + Send),
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    let arguments: crate::schema::GetSubclassArgs = deserialize(arguments)?;
    let character = match arguments.into_request() {
        Ok(character) => character,
        Err(error) => return Ok(error.into_call_tool_result()),
    };
    Ok(service
        .get_subclass(character.as_ref())
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

/// Dispatches one `move_item` call while holding the shared mutation lock.
///
/// Arguments are bounded and converted before the service runs, so an absent or
/// ambiguous selector is a sanitized tool error and never reaches the seam. A
/// rejected bound or a service failure is likewise a sanitized structured tool
/// error; the lock is held only for this one synchronous call.
fn dispatch_mutation_locked(
    mutation: &Mutex<Box<dyn MutationToolService + Send>>,
    name: &str,
    arguments: JsonObject,
) -> Result<CallToolResult, ErrorData> {
    if name == crate::schema::PULL_STACK_TOOL {
        let args: crate::schema::PullStackArgs = deserialize(arguments)?;
        if let Err(error) = args.validate() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard.pull_stack(&args).map_or_else(tool_error, |output| {
            let mut result = CallToolResult::structured(output.json());
            result.is_error = Some(!output.is_success());
            result
        }));
    }
    if name == crate::schema::PULL_ITEM_TOOL {
        let args: crate::schema::PullItemArgs = deserialize(arguments)?;
        if let Err(error) = args.instance_id() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard.pull_item(&args).map_or_else(tool_error, |output| {
            let mut result = CallToolResult::structured(output.json());
            result.is_error = Some(!output.is_success());
            result
        }));
    }
    if name == crate::schema::EQUIP_ITEMS_TOOL {
        let args: crate::schema::EquipItemsArgs = deserialize(arguments)?;
        if let Err(error) = args.request() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard.equip_items(&args).map_or_else(tool_error, |output| {
            let mut result = CallToolResult::structured(output.value);
            result.is_error = Some(output.exit_code != 0);
            result
        }));
    }
    if name == crate::schema::APPLY_LOADOUT_TOOL {
        let args: crate::schema::ApplyLoadoutArgs = deserialize(arguments)?;
        if let Err(error) = args.request() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard
            .apply_loadout(&args)
            .map_or_else(tool_error, |output| {
                let mut result = CallToolResult::structured(output.value);
                result.is_error = Some(output.exit_code != 0);
                result
            }));
    }
    if name == crate::schema::SET_LOCK_STATE_TOOL {
        let args: crate::schema::SetLockStateArgs = deserialize(arguments)?;
        if let Err(error) = args.validate() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard
            .set_lock_state(&args)
            .map_or_else(tool_error, |output| {
                let is_error = !matches!(
                    output.status,
                    destiny_output::lock::LockStatus::Applied
                        | destiny_output::lock::LockStatus::VerifiedNoop
                );
                let mut result = CallToolResult::structured(output.json());
                result.is_error = Some(is_error);
                result
            }));
    }
    if name == crate::schema::EQUIP_ITEM_TOOL {
        let args: crate::schema::EquipItemArgs = deserialize(arguments)?;
        if let Err(error) = args.validate() {
            return Ok(error.into_call_tool_result());
        }
        let mut guard = mutation.lock().map_err(|_| internal_error())?;
        return Ok(guard.equip_item(&args).map_or_else(tool_error, |output| {
            let is_error = !matches!(
                output.status,
                destiny_output::equip::EquipStatus::Applied
                    | destiny_output::equip::EquipStatus::VerifiedNoop
            );
            let mut result = CallToolResult::structured(output.json());
            result.is_error = Some(is_error);
            result
        }));
    }
    let arguments: MoveItemArgs = deserialize(arguments)?;
    let request = match arguments.into_request() {
        Ok(request) => request,
        Err(error) => return Ok(error.into_call_tool_result()),
    };
    let mut guard = mutation.lock().map_err(|_| internal_error())?;
    Ok(guard
        .move_item(&request)
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
        AppliedMoveView, InspectItemOutput, LIST_CHARACTERS_TOOL, ListCharactersOutput,
        LocationView, MOVE_ITEM_TOOL, McpError, MoveEffectsView, MoveItemArgs, MoveItemOutput,
        QueryMetadataView, SEARCH_INVENTORY_TOOL, SearchInventoryOutput,
    };
    use crate::service::{
        InventoryReadService, ListCharactersRequest, MoveItemRequest, MutationToolService,
    };
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
        fn get_postmaster(
            &mut self,
            _: Option<&destiny_app::CharacterId>,
        ) -> Result<crate::schema::GetPostmasterOutput, McpError> {
            self.enter("get_postmaster");
            Err(McpError::new(
                destiny_app::ErrorCategory::ProviderValidation,
            ))
        }
        fn get_loadouts(
            &mut self,
            character: Option<&destiny_app::CharacterId>,
        ) -> Result<crate::schema::GetLoadoutsOutput, McpError> {
            assert_eq!(character.map(|id| id.as_str()), Some("hunter"));
            self.enter("get_loadouts");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(crate::schema::GetLoadoutsOutput { characters: vec![] }),
            };
            self.leave();
            result
        }
        fn get_artifact(
            &mut self,
            character: Option<&destiny_app::CharacterId>,
        ) -> Result<crate::schema::GetArtifactOutput, McpError> {
            assert_eq!(character.map(|id| id.as_str()), Some("hunter"));
            self.enter("get_artifact");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(crate::schema::GetArtifactOutput {
                    points: None,
                    characters: vec![],
                }),
            };
            self.leave();
            result
        }
        fn get_subclass_options(
            &mut self,
            character: Option<&destiny_app::CharacterId>,
            element: Option<destiny_app::SubclassElement>,
        ) -> Result<crate::schema::GetSubclassOptionsOutput, McpError> {
            assert_eq!(character.map(|id| id.as_str()), Some("hunter"));
            assert_eq!(element, Some(destiny_app::SubclassElement::Arc));
            self.enter("get_subclass_options");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(crate::schema::GetSubclassOptionsOutput {
                    reference_character_id: None,
                    characters: vec![],
                }),
            };
            self.leave();
            result
        }
        fn get_subclass(
            &mut self,
            character: Option<&destiny_app::CharacterId>,
        ) -> Result<crate::schema::GetSubclassOutput, McpError> {
            assert_eq!(character.map(|id| id.as_str()), Some("hunter"));
            self.enter("get_subclass");
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(crate::schema::GetSubclassOutput {
                    characters: Vec::new(),
                }),
            };
            self.leave();
            result
        }

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
                    resolved: Default::default(),
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

    /// A deterministic recording mutation seam with observable concurrency.
    #[derive(Clone, Debug)]
    struct RecordingMutationService {
        requests: Arc<std::sync::Mutex<Vec<MoveItemRequest>>>,
        inflight: Arc<AtomicUsize>,
        max_inflight: Arc<AtomicUsize>,
        delay: Duration,
        failure: Option<ErrorCategory>,
    }

    impl RecordingMutationService {
        fn new() -> Self {
            Self {
                requests: Arc::new(std::sync::Mutex::new(Vec::new())),
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

        fn recorded(&self) -> Vec<MoveItemRequest> {
            self.requests.lock().expect("request log lock").clone()
        }

        fn max_concurrency(&self) -> usize {
            self.max_inflight.load(Ordering::SeqCst)
        }
    }

    impl MutationToolService for RecordingMutationService {
        fn move_item(&mut self, request: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
            self.requests
                .lock()
                .expect("request log lock")
                .push(request.clone());
            let current = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_inflight.fetch_max(current, Ordering::SeqCst);
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            let result = match self.failure {
                Some(category) => Err(McpError::new(category)),
                None => Ok(move_success_output()),
            };
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            result
        }
    }

    /// One bounded successful move output for the recording seam.
    fn move_success_output() -> MoveItemOutput {
        MoveItemOutput::Applied(AppliedMoveView {
            effects: MoveEffectsView {
                item_id: "inst-a".into(),
                via_vault: false,
                source: LocationView::Vault,
                destination: LocationView::Character {
                    character_id: "warlock".into(),
                },
            },
            steps: Vec::new(),
            reason: "Bungie accepted the action; not re-read".into(),
        })
    }

    fn mutation_server(
        service: RecordingService,
    ) -> (ReadToolServer, RecordingService, RecordingMutationService) {
        let read_handle = service.clone();
        let mutation = RecordingMutationService::new();
        let mutation_handle = mutation.clone();
        (
            ReadToolServer::with_mutation(Box::new(service), Box::new(mutation)),
            read_handle,
            mutation_handle,
        )
    }

    fn exact_move_arguments() -> Value {
        json!({"instance_id": "inst-a", "to_vault": true})
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
    fn rc002_dispatch_holds_the_actual_operation_guard_until_service_returns() {
        use std::sync::{TryLockError, mpsc};
        const BOUND: Duration = Duration::from_secs(5);
        struct PausedMutation {
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl MutationToolService for PausedMutation {
            fn move_item(&mut self, _: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
                self.entered.send(()).unwrap();
                self.release.recv_timeout(BOUND).unwrap();
                Ok(move_success_output())
            }
        }
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let server = Arc::new(ReadToolServer::with_mutation(
            Box::new(RecordingService::new()),
            Box::new(PausedMutation {
                entered: entered_tx,
                release: release_rx,
            }),
        ));
        let worker_server = server.clone();
        let worker = std::thread::spawn(move || {
            let result = worker_server.dispatch(MOVE_ITEM_TOOL, args(exact_move_arguments()));
            done_tx.send(()).unwrap();
            result
        });
        entered_rx.recv_timeout(BOUND).unwrap();
        // Directly inspect the production exclusion primitive, so an absence
        // timeout in the real-stack interleaving test is not scheduling proof.
        assert!(matches!(
            server.operation.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(BOUND).unwrap();
        assert!(worker.join().unwrap().is_ok());
        assert!(server.operation.try_lock().is_ok());
        assert!(
            server
                .dispatch(LIST_CHARACTERS_TOOL, Default::default())
                .is_ok()
        );
    }

    #[test]
    fn pull_dispatch_validates_before_seam_and_holds_existing_operation_guard() {
        use std::sync::{TryLockError, mpsc};
        const BOUND: Duration = Duration::from_secs(5);
        struct PausedPull {
            entered: mpsc::Sender<String>,
            release: mpsc::Receiver<()>,
        }
        impl MutationToolService for PausedPull {
            fn move_item(&mut self, _: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
                panic!("pull must not dispatch move")
            }
            fn pull_item(
                &mut self,
                request: &crate::schema::PullItemArgs,
            ) -> Result<destiny_output::instanced_pull::InstancedPullOutput, McpError> {
                self.entered.send(request.item.clone()).unwrap();
                self.release.recv_timeout(BOUND).unwrap();
                Err(McpError::new(ErrorCategory::ProviderUnavailable))
            }
        }
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = Arc::new(ReadToolServer::with_mutation(
            Box::new(RecordingService::new()),
            Box::new(PausedPull {
                entered: entered_tx,
                release: release_rx,
            }),
        ));
        for item in ["0", "01", "9223372036854775808"] {
            let result = server
                .dispatch("pull_item", args(json!({"item":item})))
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert!(entered_rx.try_recv().is_err());
        let worker_server = server.clone();
        let worker = std::thread::spawn(move || {
            worker_server.dispatch("pull_item", args(json!({"item":"123"})))
        });
        assert_eq!(entered_rx.recv_timeout(BOUND).unwrap(), "123");
        assert!(matches!(
            server.operation.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        release_tx.send(()).unwrap();
        let result = worker.join().unwrap().unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured(&result)["code"], "provider_unavailable");
        assert!(entered_rx.try_recv().is_err());
        assert!(server.operation.try_lock().is_ok());
    }

    #[test]
    fn stack_dispatch_validates_and_holds_operation_guard() {
        use std::sync::{TryLockError, mpsc};
        const BOUND: Duration = Duration::from_secs(5);
        struct PausedStack(mpsc::Sender<String>, mpsc::Receiver<()>);
        impl MutationToolService for PausedStack {
            fn move_item(&mut self, _: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
                panic!("not move")
            }
            fn pull_stack(
                &mut self,
                request: &crate::schema::PullStackArgs,
            ) -> Result<destiny_output::stack_pull::StackPullOutput, McpError> {
                self.0.send(request.amount.clone()).unwrap();
                self.1.recv_timeout(BOUND).unwrap();
                Err(McpError::new(ErrorCategory::ProviderUnavailable))
            }
        }
        let (tx, rx) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let server = Arc::new(ReadToolServer::with_mutation(
            Box::new(RecordingService::new()),
            Box::new(PausedStack(tx, wait)),
        ));
        let result = server
            .dispatch(
                "pull_stack",
                args(json!({"item_hash":"7","amount":"0","character":"2"})),
            )
            .unwrap();
        assert_eq!(structured(&result)["code"], "provider_validation");
        assert!(rx.try_recv().is_err());
        let cloned = server.clone();
        let worker = std::thread::spawn(move || {
            cloned.dispatch(
                "pull_stack",
                args(json!({"item_hash":"7","amount":"3","character":"2"})),
            )
        });
        assert_eq!(rx.recv_timeout(BOUND).unwrap(), "3");
        assert!(matches!(
            server.operation.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap().unwrap().is_error, Some(true));
        assert!(rx.try_recv().is_err());
        assert!(server.operation.try_lock().is_ok());
    }

    #[test]
    fn stack_discovery_mutation_only_and_default_fails_closed() {
        let (configured, _, mutation) = mutation_server(RecordingService::new());
        let tool = configured.get_tool("pull_stack").unwrap();
        assert_eq!(tool.schema_as_json_value()["additionalProperties"], false);
        let result = configured
            .dispatch(
                "pull_stack",
                args(json!({"item_hash":"7","amount":"3","character":"2"})),
            )
            .unwrap();
        assert_eq!(structured(&result)["code"], "configuration_error");
        assert!(mutation.recorded().is_empty());
        let (readonly, _) = server(RecordingService::new());
        assert!(readonly.get_tool("pull_stack").is_none());
        assert_eq!(
            readonly
                .dispatch(
                    "pull_stack",
                    args(json!({"item_hash":"7","amount":"3","character":"2"}))
                )
                .unwrap_err()
                .code
                .0,
            -32601
        );
    }

    #[test]
    fn legacy_mutation_fake_pull_default_fails_closed_without_moving() {
        let (server, _, mutation) = mutation_server(RecordingService::new());
        let result = server
            .dispatch("pull_item", args(json!({"item":"123"})))
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured(&result)["code"], "configuration_error");
        assert!(mutation.recorded().is_empty());
    }

    #[test]
    fn pull_discovery_is_closed_id_only_and_mutation_only() {
        let (configured, _, _) = mutation_server(RecordingService::new());
        let tool = configured
            .discover_tools()
            .tools
            .into_iter()
            .find(|tool| tool.name == "pull_item")
            .expect("configured server exposes pull_item");
        let schema = tool.schema_as_json_value();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["item"]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
        let (readonly, _) = server(RecordingService::new());
        assert!(readonly.get_tool("pull_item").is_none());
        let error = readonly
            .dispatch("pull_item", args(json!({"item":"123"})))
            .expect_err("read-only construction must not dispatch pulls");
        assert_eq!(error.code.0, -32601);
    }

    #[test]
    fn discovery_exposes_exactly_the_four_reads_in_stable_order() {
        let (server, _handle) = server(RecordingService::new());
        let tools = server.discover_tools().tools;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_postmaster",
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                "inspect_item",
                "get_subclass",
                "get_artifact",
                "get_loadouts",
                "weekly_reset",
                "weekly",
                "get_subclass_options"
            ]
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
    fn read_only_construction_advertises_the_exact_read_only_instructions() {
        let (server, _handle) = server(RecordingService::new());
        let info = ServerHandler::get_info(&server);
        assert_eq!(
            info.instructions.as_deref(),
            Some(
                "Read-only, bounded access to the signed-in Destiny inventory and Manifest definitions."
            )
        );
    }

    #[test]
    fn configured_mutation_advertises_mutation_aware_instructions() {
        let (server, _read, _mutation) = mutation_server(RecordingService::new());
        let info = ServerHandler::get_info(&server);
        let instructions = info
            .instructions
            .as_deref()
            .expect("a composed server advertises instructions");

        // A mutation-configured server must not describe itself as read-only.
        assert!(
            !instructions.contains("Read-only"),
            "a mutable server must not claim to be read-only: {instructions}"
        );
        // The advertised capability matches the discovered mutable tool set.
        assert!(
            instructions.contains(MOVE_ITEM_TOOL),
            "a mutable server must name the mutation it exposes: {instructions}"
        );
        assert_ne!(
            instructions,
            "Read-only, bounded access to the signed-in Destiny inventory and Manifest definitions."
        );
    }

    #[test]
    fn get_tool_returns_only_the_discovered_tools() {
        let (server, _handle) = server(RecordingService::new());
        assert!(server.get_tool(LIST_CHARACTERS_TOOL).is_some());
        assert!(server.get_tool(SEARCH_INVENTORY_TOOL).is_some());
        assert!(server.get_tool("inspect_item").is_some());
        assert!(server.get_tool("get_subclass").is_some());
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
            ("get_subclass", json!({"character": 1})),
            ("get_subclass", json!({"unknown": true})),
            ("get_artifact", json!({"character": 1})),
            ("get_artifact", json!({"unknown": true})),
            ("get_loadouts", json!({"character": 1})),
            ("get_loadouts", json!({"unknown": true})),
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
    fn loadouts_dispatch_validates_before_service_and_preserves_errors() {
        let (server, handle) = server(RecordingService::new());
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "a".repeat(513),
            "bad\u{7}".to_owned(),
        ] {
            let result = server
                .dispatch("get_loadouts", args(json!({"character": value})))
                .unwrap();
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert!(handle.recorded().is_empty());
        let result = server
            .dispatch("get_loadouts", args(json!({"character": "hunter"})))
            .unwrap();
        assert_eq!(
            structured(&result),
            json!({"kind": "loadouts", "characters": []})
        );
        assert_eq!(handle.recorded(), ["get_loadouts"]);
        for category in [
            ErrorCategory::DefinitionsUnavailable,
            ErrorCategory::AuthorizationRequired,
            ErrorCategory::AmbiguousMatch,
        ] {
            let (server, _) = self::server(RecordingService::with_failure(category));
            let result = server
                .dispatch("get_loadouts", args(json!({"character": "hunter"})))
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(structured(&result)["code"], McpError::new(category).code());
        }
    }

    #[test]
    fn artifact_dispatch_validates_before_service_and_preserves_errors() {
        let (server, handle) = server(RecordingService::new());
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "a".repeat(513),
            "bad\u{7}".to_owned(),
        ] {
            let result = server
                .dispatch("get_artifact", args(json!({"character": value})))
                .unwrap();
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert!(handle.recorded().is_empty());
        let result = server
            .dispatch("get_artifact", args(json!({"character": "hunter"})))
            .unwrap();
        assert_eq!(
            structured(&result),
            json!({"kind": "artifact", "points": null, "characters": []})
        );
        assert_eq!(handle.recorded(), ["get_artifact"]);
        for category in [
            ErrorCategory::DefinitionsUnavailable,
            ErrorCategory::AuthorizationRequired,
            ErrorCategory::AmbiguousMatch,
        ] {
            let (server, _) = self::server(RecordingService::with_failure(category));
            let result = server
                .dispatch("get_artifact", args(json!({"character": "hunter"})))
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(structured(&result)["code"], McpError::new(category).code());
        }
    }

    #[test]
    fn subclass_options_dispatch_validates_filters_and_preserves_errors() {
        let (server, handle) = server(RecordingService::new());
        for value in ["".to_owned(), "a".repeat(513), "bad\u{7}".to_owned()] {
            let result = server
                .dispatch("get_subclass_options", args(json!({"character": value})))
                .unwrap();
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        for invalid in [
            json!({"locked": true}),
            json!({"element": "kinetic"}),
            json!({"character": 1}),
        ] {
            assert_eq!(
                server
                    .dispatch("get_subclass_options", args(invalid))
                    .unwrap_err()
                    .code
                    .0,
                -32602
            );
        }
        assert!(handle.recorded().is_empty());
        let request = json!({"character": "hunter", "element": "arc"});
        let result = server
            .dispatch("get_subclass_options", args(request.clone()))
            .unwrap();
        assert_eq!(
            structured(&result),
            json!({"kind": "subclass_options", "reference_character_id": null, "characters": []})
        );
        assert_eq!(handle.recorded(), ["get_subclass_options"]);
        let (server, _) = self::server(RecordingService::with_failure(
            ErrorCategory::DefinitionsUnavailable,
        ));
        assert_eq!(
            structured(
                &server
                    .dispatch("get_subclass_options", args(request))
                    .unwrap()
            )["code"],
            "definitions_unavailable"
        );
    }

    #[test]
    fn subclass_dispatch_validates_and_forwards_selector_and_errors() {
        let (server, handle) = server(RecordingService::new());
        for value in ["".to_owned(), "a".repeat(513), "bad\u{7}".to_owned()] {
            let result = server
                .dispatch("get_subclass", args(json!({"character": value})))
                .unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert!(handle.recorded().is_empty());
        let result = server
            .dispatch("get_subclass", args(json!({"character": "hunter"})))
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert_eq!(
            structured(&result),
            json!({"kind": "subclass", "characters": []})
        );
        assert_eq!(handle.recorded(), ["get_subclass"]);
        let (server, _) = self::server(RecordingService::with_failure(
            ErrorCategory::DefinitionsUnavailable,
        ));
        let result = server
            .dispatch("get_subclass", args(json!({"character": "hunter"})))
            .unwrap();
        assert_eq!(structured(&result)["code"], "definitions_unavailable");
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
    fn postmaster_arguments_are_rejected_before_service_calls() {
        let (server, handle) = server(RecordingService::new());
        for invalid in [
            json!({"quantity": 1}),
            json!({"character": 2}),
            json!({"membership": "secret"}),
        ] {
            assert!(server.dispatch("get_postmaster", args(invalid)).is_err());
        }
        for value in [
            "".to_owned(),
            " ".to_owned(),
            "a".repeat(513),
            "bad\u{7}".to_owned(),
        ] {
            let result = server
                .dispatch("get_postmaster", args(json!({"character": value})))
                .unwrap();
            assert_eq!(result.is_error, Some(true));
        }
        assert!(handle.recorded().is_empty());
        for valid in [
            json!({}),
            json!({"character": null}),
            json!({"character": "wArLoCk"}),
        ] {
            let result = server.dispatch("get_postmaster", args(valid)).unwrap();
            assert_eq!(result.is_error, Some(true));
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert_eq!(
            handle.recorded(),
            ["get_postmaster", "get_postmaster", "get_postmaster"]
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

    #[test]
    fn read_only_construction_exposes_exactly_the_four_reads() {
        let (server, _handle) = server(RecordingService::new());
        let tools = server.discover_tools().tools;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_postmaster",
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                "inspect_item",
                "get_subclass",
                "get_artifact",
                "get_loadouts",
                "weekly_reset",
                "weekly",
                "get_subclass_options"
            ]
        );
        assert!(
            server.get_tool(MOVE_ITEM_TOOL).is_none(),
            "a read-only server must not advertise move_item"
        );
    }

    #[test]
    fn read_only_construction_keeps_move_item_unknown() {
        let (server, read) = server(RecordingService::new());
        let error = server
            .dispatch(MOVE_ITEM_TOOL, args(exact_move_arguments()))
            .expect_err("move_item is unknown without a mutation seam");
        assert_eq!(error.code.0, -32601);
        assert_eq!(error.message, "unknown tool");
        let error = server
            .dispatch(
                crate::schema::EQUIP_ITEM_TOOL,
                args(json!({"item":"123","character":"warlock"})),
            )
            .expect_err("equip is unknown without mutation seam");
        assert_eq!(error.code.0, -32601);
        assert!(read.recorded().is_empty());
    }

    #[test]
    fn configured_mutation_exposes_the_four_reads_plus_move_item() {
        let (server, _read, _mutation) = mutation_server(RecordingService::new());
        let tools = server.discover_tools().tools;
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_postmaster",
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                "inspect_item",
                "get_subclass",
                "get_artifact",
                "get_loadouts",
                "weekly_reset",
                "weekly",
                "get_subclass_options",
                MOVE_ITEM_TOOL,
                crate::schema::EQUIP_ITEM_TOOL,
                crate::schema::SET_LOCK_STATE_TOOL,
                crate::schema::PULL_ITEM_TOOL,
                crate::schema::PULL_STACK_TOOL,
                crate::schema::APPLY_LOADOUT_TOOL,
                crate::schema::EQUIP_ITEMS_TOOL
            ]
        );
        let tool = server
            .get_tool(MOVE_ITEM_TOOL)
            .expect("move_item is advertised once composed");
        assert_eq!(tool.name, MOVE_ITEM_TOOL);
        assert!(tool.output_schema.is_some());
    }

    #[test]
    fn reads_still_dispatch_unchanged_when_a_mutation_seam_is_configured() {
        let (server, read, mutation) = mutation_server(RecordingService::new());
        let list = server
            .dispatch(LIST_CHARACTERS_TOOL, JsonObject::new())
            .expect("list dispatch succeeds");
        assert_eq!(list.is_error, Some(false));
        let inspect = server
            .dispatch("inspect_item", args(json!({"instance_id": "inst-a"})))
            .expect("inspect dispatch succeeds");
        assert_eq!(structured(&inspect), json!({"status": "missing"}));
        assert_eq!(read.recorded(), ["list_characters", "inspect_item"]);
        assert!(mutation.recorded().is_empty());
    }

    #[test]
    fn move_item_dispatches_through_the_mutation_seam() {
        let (server, read, mutation) = mutation_server(RecordingService::new());
        let result = server
            .dispatch(MOVE_ITEM_TOOL, args(exact_move_arguments()))
            .expect("move_item dispatch succeeds");
        assert_eq!(result.is_error, Some(false));
        assert_eq!(structured(&result)["status"], "applied");
        assert_eq!(structured(&result)["effects"]["item_id"], "inst-a");

        let recorded = mutation.recorded();
        assert_eq!(recorded.len(), 1);
        let expected = MoveItemArgs {
            instance_id: Some("inst-a".into()),
            item_text: None,
            to_vault: true,
            character_id: None,
            character_label: None,
        }
        .into_request()
        .expect("valid exact arguments");
        assert_eq!(recorded[0], expected);
        assert!(
            read.recorded().is_empty(),
            "a mutation must never reach the read seam"
        );
    }

    #[test]
    fn malformed_move_item_arguments_are_a_fixed_protocol_error() {
        let (server, _read, mutation) = mutation_server(RecordingService::new());
        for invalid in [
            json!({"unknown": true}),
            json!({"instance_id": 1, "to_vault": true}),
            json!({"to_vault": "yes"}),
        ] {
            let error = server
                .dispatch(MOVE_ITEM_TOOL, args(invalid.clone()))
                .expect_err("malformed arguments are a protocol error");
            assert_eq!(error.code.0, -32602, "must reject: {invalid}");
            assert_eq!(error.message, "invalid tool arguments");
        }
        assert!(mutation.recorded().is_empty());
    }

    #[test]
    fn ambiguous_move_item_selectors_return_a_sanitized_tool_error() {
        let (server, _read, mutation) = mutation_server(RecordingService::new());
        for rejected in [
            json!({"instance_id": "inst-a"}),
            json!({"to_vault": true}),
            json!({"instance_id": "inst-a", "item_text": "gjallar", "to_vault": true}),
            json!({"instance_id": "inst-a", "to_vault": true, "character_id": "warlock"}),
            json!({"instance_id": "", "to_vault": true}),
        ] {
            let result = server
                .dispatch(MOVE_ITEM_TOOL, args(rejected.clone()))
                .expect("a rejected selector is a tool-level error");
            assert_eq!(result.is_error, Some(true), "must reject: {rejected}");
            assert_eq!(structured(&result)["code"], "provider_validation");
        }
        assert!(
            mutation.recorded().is_empty(),
            "a rejected selector must never reach the mutation seam"
        );
    }

    #[test]
    fn mutation_service_failures_return_a_sanitized_tool_error() {
        let read = RecordingService::new();
        let mutation = RecordingMutationService::with_failure(ErrorCategory::StaleState);
        let server = ReadToolServer::with_mutation(Box::new(read), Box::new(mutation));
        let result = server
            .dispatch(MOVE_ITEM_TOOL, args(exact_move_arguments()))
            .expect("a mutation failure is a tool-level error");
        assert_eq!(result.is_error, Some(true));
        let rendered = serde_json::to_string(&structured(&result)).expect("serializes");
        assert!(rendered.contains("stale_state"));
        assert!(!rendered.contains("raw"));
    }

    #[test]
    fn equip_local_state_failure_is_a_tool_error() {
        struct LocalStateFailedEquip;
        impl MutationToolService for LocalStateFailedEquip {
            fn move_item(&mut self, _: &MoveItemRequest) -> Result<MoveItemOutput, McpError> {
                unreachable!()
            }
            fn equip_item(
                &mut self,
                _: &crate::schema::EquipItemArgs,
            ) -> Result<destiny_output::equip::EquipOutput, McpError> {
                let mut output = destiny_output::equip::EquipOutput::refused(
                    destiny_app::MutationError::MissingItem,
                );
                output.status = destiny_output::equip::EquipStatus::VerificationFailed;
                output.profile_refresh = destiny_output::equip::ProfileRefresh::Failed;
                Ok(output)
            }
        }
        let server = ReadToolServer::with_mutation(
            Box::new(RecordingService::new()),
            Box::new(LocalStateFailedEquip),
        );
        let result = server
            .dispatch(
                crate::schema::EQUIP_ITEM_TOOL,
                args(serde_json::json!({"item":"123", "character":"456"})),
            )
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert_eq!(structured(&result)["status"], "verification_failed");
    }

    #[test]
    fn concurrent_mutation_dispatch_is_serialized_by_the_shared_seam() {
        let delay = Duration::from_millis(5);
        let read = RecordingService::new();
        let mutation = RecordingMutationService::with_delay(delay);
        let mutation_handle = mutation.clone();
        let server = Arc::new(ReadToolServer::with_mutation(
            Box::new(read),
            Box::new(mutation),
        ));

        let mut threads = Vec::new();
        for _ in 0..8 {
            let server = Arc::clone(&server);
            threads.push(std::thread::spawn(move || {
                server
                    .dispatch(MOVE_ITEM_TOOL, args(exact_move_arguments()))
                    .expect("concurrent mutation dispatch succeeds")
            }));
        }
        for thread in threads {
            let result = thread.join().expect("thread completes");
            assert_eq!(result.is_error, Some(false));
        }

        assert_eq!(mutation_handle.recorded().len(), 8);
        assert_eq!(
            mutation_handle.max_concurrency(),
            1,
            "the mutation mutex must serialize seam access"
        );
    }
}
