//! Bounded MCP schemas, sanitized error shape, and tool descriptors.
//!
//! This module owns every wire-shaped type the MCP client sees. Inputs are
//! bounded before the read service runs, and outputs are finite projections of
//! the shared sanitized application results. The official SDK types (`rmcp`) stay
//! here and in the sibling [`crate::service`] module; no SDK type reaches the
//! application or core contracts, and no application or core type leaks into an
//! MCP error.
//!
//! # Bounds
//!
//! The numeric limits declared in the JSON Schema and enforced by
//! [`crate::service`] are the shared read-contract bounds
//! ([`MAX_SEARCH_LIMIT`], [`MAX_SEARCH_TEXT_LENGTH`], [`MAX_TAG_FILTERS`], and
//! [`MAX_HASH_FILTERS`]); this module introduces no second policy. An argument
//! that exceeds a bound is rejected with the sanitized
//! [`ErrorCategory::ProviderValidation`] category before any service call.
//!
//! # Sanitization
//!
//! A tool value carries only normalized domain reads plus their provenance and
//! freshness metadata. It never carries a credential, a membership identifier,
//! a raw provider payload, or an opaque provider location value: an
//! `ItemLocation::Other` instance is rendered as the opaque
//! [`LocationView::Other`] shape without its provider text.

use destiny_app::{
    AppError, ErrorCategory, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH,
    MAX_TAG_FILTERS,
};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable tool name for the character listing read.
pub const LIST_CHARACTERS_TOOL: &str = "list_characters";

/// Stable tool name for the inventory search read.
pub const SEARCH_INVENTORY_TOOL: &str = "search_inventory";

/// Stable tool name for the exact instance inspection read.
pub const INSPECT_ITEM_TOOL: &str = "inspect_item";

const LIST_CHARACTERS_DESCRIPTION: &str = "List the signed-in account's characters with the freshness metadata of the committed profile projection.";

const SEARCH_INVENTORY_DESCRIPTION: &str = "Search item definitions and, when requested, concrete inventory instances with bounded filters.";

const INSPECT_ITEM_DESCRIPTION: &str =
    "Inspect exactly one concrete inventory instance by its instance identifier.";

/// Maximum accepted MCP identifier length, in characters.
///
/// This is an MCP-local transport bound, not a shared domain rule: the shared
/// opaque `ItemInstanceId` and `CharacterId` types intentionally accept any
/// string, but a tool call must keep its JSON payload finite. The `character_id`
/// and `instance_id` arguments are therefore bounded to
/// `1..=MAX_IDENTIFIER_LENGTH` characters; empty identifiers are rejected
/// because the shared opaque types would otherwise accept them. The bound is
/// comfortably above any provider identifier while keeping every request
/// bounded, and it changes no core identifier type.
pub const MAX_IDENTIFIER_LENGTH: usize = 512;

/// Returns the exact semantic read tools this server exposes, in stable order.
///
/// The descriptor set is finite and deterministic: one tool per semantic read,
/// each with its bounded JSON input schema and its structured output schema. The
/// caller must not add, remove, or rename entries per request.
pub fn tool_specs() -> Vec<Tool> {
    vec![
        Tool::new(
            LIST_CHARACTERS_TOOL,
            LIST_CHARACTERS_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<ListCharactersArgs>()
        .with_output_schema::<ListCharactersOutput>(),
        Tool::new(
            SEARCH_INVENTORY_TOOL,
            SEARCH_INVENTORY_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<SearchInventoryArgs>()
        .with_output_schema::<SearchInventoryOutput>(),
        Tool::new(
            INSPECT_ITEM_TOOL,
            INSPECT_ITEM_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<InspectItemArgs>()
        .with_output_schema::<InspectItemOutput>(),
    ]
}

/// `list_characters` accepts no arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListCharactersArgs {}

/// Bounded `search_inventory` arguments.
///
/// The definition query always runs; the instance query is optional and absent
/// unless `instances` is present. `text`, `item_type`, `tags`, `limit`, and the
/// nested instance fields all use the shared read-contract bounds.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchInventoryArgs {
    /// Optional case-insensitive definition name substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_SEARCH_TEXT_LENGTH))]
    pub text: Option<String>,

    /// Optional exact definition item type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_SEARCH_TEXT_LENGTH))]
    pub item_type: Option<String>,

    /// Optional tag filters; every listed tag must be present.
    #[serde(default)]
    #[schemars(
        length(max = MAX_TAG_FILTERS),
        inner(length(max = MAX_SEARCH_TEXT_LENGTH))
    )]
    pub tags: Vec<String>,

    /// Maximum number of definition matches; defaults to the shared maximum.
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = MAX_SEARCH_LIMIT))]
    pub limit: usize,

    /// Optional concrete-instance query. Absent means no instance search runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<InstanceQueryArgs>,
}

/// Bounded concrete-instance search arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceQueryArgs {
    /// Absent matches every instance; present matches only the listed definition
    /// hashes, and an empty list matches nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_HASH_FILTERS))]
    pub item_hashes: Option<Vec<i64>>,

    /// Optional exact perk hash the instance must carry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perk_hash: Option<u32>,

    /// Optional character the instance must be located on; the vault never matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character_id: Option<String>,

    /// Maximum number of instance matches; defaults to the shared maximum.
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = MAX_SEARCH_LIMIT))]
    pub limit: usize,
}

/// `inspect_item` arguments: exactly one concrete instance identifier.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectItemArgs {
    /// Concrete instance identifier; never a definition hash or display name.
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub instance_id: String,
}

fn default_search_limit() -> usize {
    MAX_SEARCH_LIMIT
}

/// The current state of one character in the committed projection.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CharacterView {
    pub id: String,
    pub name: Option<String>,
    pub class: Option<String>,
    pub race: Option<String>,
    pub gender: Option<String>,
    pub light: Option<u16>,
}

/// One summarized perk on a concrete instance.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PerkView {
    pub perk_hash: u32,
    pub active: bool,
    pub visible: bool,
}

/// A bounded, sanitized instance location.
///
/// The opaque provider value behind [`ItemLocation::Other`] is deliberately not
/// carried: the MCP client sees that the location is unknown without receiving
/// provider text.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocationView {
    Vault,
    Character { character_id: String },
    Other,
}

/// A Manifest definition, distinct from any concrete instance.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct DefinitionView {
    pub hash: i64,
    pub bucket_hash: Option<i64>,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    pub element: Option<String>,
    pub tags: Vec<String>,
    pub definition_version: String,
}

/// A concrete instance, distinct from the definition it references.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct InstanceView {
    pub instance_id: String,
    pub item_hash: i64,
    pub location: LocationView,
    pub quantity: u32,
    pub equipped: bool,
    pub locked: bool,
    pub power: Option<u16>,
    pub perks: Vec<PerkView>,
}

/// The committed projection metadata and freshness behind one answer.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct QueryMetadataView {
    pub dataset: String,
    pub source: String,
    pub revision: u64,
    pub checked_at: u64,
    pub definition_version: String,
    pub freshness: String,
}

/// Structured result of `list_characters`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ListCharactersOutput {
    pub characters: Vec<CharacterView>,
    pub metadata: QueryMetadataView,
}

/// One instance match paired with the exact definition it references.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct InventorySearchHitView {
    pub definition: DefinitionView,
    pub instance: InstanceView,
}

/// Structured result of `search_inventory`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SearchInventoryOutput {
    pub candidates: Vec<DefinitionView>,
    pub hits: Vec<InventorySearchHitView>,
    pub metadata: QueryMetadataView,
}

/// A found inspection: one exact definition beside one concrete instance.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct FoundInspection {
    pub definition: DefinitionView,
    pub instance: InstanceView,
    pub metadata: QueryMetadataView,
}

/// Structured result of `inspect_item`.
///
/// `Missing` is an explicit, successful outcome of the shared resolution
/// contract; it is not an error and is never confused with a concrete instance.
/// The found case is boxed so the two variants stay a comparable size.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InspectItemOutput {
    Found(Box<FoundInspection>),
    Missing,
}

/// A sanitized, stable MCP tool failure.
///
/// `code` is a stable machine token derived from the shared
/// [`ErrorCategory`], and `message` is the shared sanitized category message.
/// The type carries no provider payload, credential, or configuration value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpError {
    code: String,
    message: String,
}

impl McpError {
    /// Builds the sanitized MCP error for one shared application category.
    pub fn new(category: ErrorCategory) -> Self {
        Self {
            code: error_code(category).to_owned(),
            message: AppError::new(category).to_string(),
        }
    }

    /// The stable machine-readable error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The sanitized human-readable category message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Renders this failure as a tool-level error result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        match serde_json::to_value(&self) {
            Ok(value) => CallToolResult::structured_error(value),
            Err(_) => CallToolResult::error(vec![ContentBlock::text("tool error")]),
        }
    }
}

impl From<AppError> for McpError {
    fn from(error: AppError) -> Self {
        Self::new(error.category())
    }
}

impl std::fmt::Display for McpError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for McpError {}

/// Serializes one structured MCP output without ever panicking.
///
/// A serialization failure of a bounded, plain-data output is an internal
/// invariant violation; it degrades to a tool-level error result instead of
/// exposing a panic or a raw diagnostic.
pub fn structured_result<T: Serialize>(value: T) -> CallToolResult {
    match serde_json::to_value(value) {
        Ok(value) => CallToolResult::structured(value),
        Err(_) => CallToolResult::error(vec![ContentBlock::text("result serialization failed")]),
    }
}

impl ListCharactersOutput {
    /// Renders this output as a structured tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

impl SearchInventoryOutput {
    /// Renders this output as a structured tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

impl InspectItemOutput {
    /// Renders this output as a structured tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// The stable code for one shared error category.
fn error_code(category: ErrorCategory) -> &'static str {
    match category {
        ErrorCategory::AuthorizationRequired => "authorization_required",
        ErrorCategory::ReauthorizationRequired => "reauthorization_required",
        ErrorCategory::AlreadyAuthorizing => "already_authorizing",
        ErrorCategory::AlreadyAuthorized => "already_authorized",
        ErrorCategory::AuthorizationDenied => "authorization_denied",
        ErrorCategory::CsrfStateMismatch => "csrf_state_mismatch",
        ErrorCategory::CredentialStoreUnavailable => "credential_store_unavailable",
        ErrorCategory::ProviderAuthentication => "provider_authentication",
        ErrorCategory::ProviderRateLimit => "provider_rate_limit",
        ErrorCategory::ProviderUnavailable => "provider_unavailable",
        ErrorCategory::ProviderValidation => "provider_validation",
        ErrorCategory::StaleState => "stale_state",
        ErrorCategory::AmbiguousMatch => "ambiguous_match",
        ErrorCategory::UnsupportedOperation => "unsupported_operation",
        ErrorCategory::PartialFailure => "partial_failure",
        ErrorCategory::VerificationFailed => "verification_failed",
        ErrorCategory::LocalStorage => "local_storage",
        ErrorCategory::ConfigurationError => "configuration_error",
        ErrorCategory::CallbackTimeout => "callback_timeout",
        ErrorCategory::ClockOverflow => "clock_overflow",
        ErrorCategory::PlanIdExhausted => "plan_id_exhausted",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        INSPECT_ITEM_TOOL, InspectItemArgs, LIST_CHARACTERS_TOOL, ListCharactersArgs,
        MAX_HASH_FILTERS, MAX_IDENTIFIER_LENGTH, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH,
        MAX_TAG_FILTERS, McpError, SEARCH_INVENTORY_TOOL, SearchInventoryArgs, tool_specs,
    };
    use destiny_app::{AppError, ErrorCategory};
    use serde_json::{Value, json};

    fn schema_json(tool_name: &str) -> Value {
        tool_specs()
            .into_iter()
            .find(|tool| tool.name == tool_name)
            .expect("tool is exposed")
            .schema_as_json_value()
    }

    #[test]
    fn tool_specs_expose_only_the_three_semantic_reads_in_stable_order() {
        let tools = tool_specs();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                INSPECT_ITEM_TOOL
            ]
        );
        for tool in &tools {
            assert!(
                tool.description
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()),
                "tool {} needs a stable description",
                tool.name
            );
            assert!(
                tool.output_schema.is_some(),
                "tool {} needs a structured output schema",
                tool.name
            );
        }
    }

    #[test]
    fn search_inventory_schema_declares_the_shared_bounds() {
        let schema = schema_json(SEARCH_INVENTORY_TOOL);
        let rendered = serde_json::to_string(&schema).expect("schema serializes");

        assert_eq!(
            schema.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "unknown arguments must stay rejected"
        );
        assert!(
            rendered.contains(&format!("\"maximum\":{MAX_SEARCH_LIMIT}")),
            "definition limit must be bounded: {rendered}"
        );
        assert!(
            rendered.contains("\"minimum\":1"),
            "limit must have a lower bound: {rendered}"
        );
        assert!(
            rendered.contains(&format!("\"maxItems\":{MAX_TAG_FILTERS}")),
            "tag filters must be bounded: {rendered}"
        );
        assert!(
            rendered.contains(&format!("\"maxItems\":{MAX_HASH_FILTERS}")),
            "hash candidates must be bounded: {rendered}"
        );
        assert!(
            rendered.contains(&format!("\"maxLength\":{MAX_SEARCH_TEXT_LENGTH}")),
            "text inputs must be bounded: {rendered}"
        );
    }

    #[test]
    fn identifier_schemas_declare_the_mcp_transport_bound() {
        let search = serde_json::to_string(&schema_json(SEARCH_INVENTORY_TOOL)).expect("schema");
        assert!(
            search.contains(&format!("\"maxLength\":{MAX_IDENTIFIER_LENGTH}")),
            "character_id must be bounded: {search}"
        );
        assert!(
            search.contains("\"minLength\":1"),
            "character_id must reject empty values: {search}"
        );

        let inspect = serde_json::to_string(&schema_json(INSPECT_ITEM_TOOL)).expect("schema");
        assert!(
            inspect.contains(&format!("\"maxLength\":{MAX_IDENTIFIER_LENGTH}")),
            "instance_id must be bounded: {inspect}"
        );
        assert!(
            inspect.contains("\"minLength\":1"),
            "instance_id must reject empty values: {inspect}"
        );
    }

    #[test]
    fn list_and_inspect_schemas_are_closed_objects() {
        assert_eq!(
            schema_json(LIST_CHARACTERS_TOOL).get("additionalProperties"),
            Some(&Value::Bool(false))
        );
        assert_eq!(
            schema_json(INSPECT_ITEM_TOOL).get("additionalProperties"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn unknown_arguments_are_rejected_by_every_input_type() {
        assert!(serde_json::from_value::<SearchInventoryArgs>(json!({"unknown": 1})).is_err());
        assert!(serde_json::from_value::<ListCharactersArgs>(json!({"unknown": 1})).is_err());
        assert!(
            serde_json::from_value::<InspectItemArgs>(json!({"instance_id": "inst", "unknown": 1}))
                .is_err()
        );
    }

    #[test]
    fn omitted_limits_default_to_the_shared_maximum() {
        let args: SearchInventoryArgs = serde_json::from_value(json!({})).expect("empty args");
        assert_eq!(args.limit, MAX_SEARCH_LIMIT);
        assert_eq!(args.tags, Vec::<String>::new());
        assert_eq!(args.instances, None);
    }

    #[test]
    fn error_codes_are_stable_and_messages_are_sanitized() {
        let error = McpError::from(AppError::new(ErrorCategory::ProviderValidation));
        assert_eq!(error.code(), "provider_validation");
        assert_eq!(
            error.message(),
            AppError::new(ErrorCategory::ProviderValidation).to_string()
        );
        assert_eq!(
            McpError::new(ErrorCategory::LocalStorage).code(),
            "local_storage"
        );
        assert_eq!(
            McpError::new(ErrorCategory::ProviderUnavailable).code(),
            "provider_unavailable"
        );

        let rendered = serde_json::to_string(&error).expect("error serializes");
        assert!(rendered.contains("provider_validation"));
        assert!(!rendered.contains("BUNGIE_API_KEY"));
    }
}
