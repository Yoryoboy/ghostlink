//! Bounded MCP schemas, sanitized error shape, and tool descriptors.
//!
//! This module owns every wire-shaped type the MCP client sees. Inputs are
//! bounded before the read or mutation service runs, and outputs are finite
//! projections of the shared sanitized application results. The official SDK
//! types (`rmcp`) stay here and in the sibling [`crate::service`] module; no SDK
//! type reaches the application or core contracts, and no application or core
//! type leaks into an MCP error.
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
//! freshness metadata, or the sanitized effects, step evidence, final location,
//! and typed failure of one move. It never carries a credential, a membership
//! identifier, a raw provider payload, or an opaque provider location value: an
//! `ItemLocation::Other` instance is rendered as the opaque
//! [`LocationView::Other`] shape without its provider text.

use destiny_app::MAX_SEARCH_OFFSET;
use destiny_app::{
    AppError, ErrorCategory, MAX_HASH_FILTERS, MAX_SEARCH_LIMIT, MAX_SEARCH_TEXT_LENGTH,
    MAX_TAG_FILTERS,
};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// MCP output schemas must declare an object root, including tagged enums
/// whose generated schema expresses their object variants only through `oneOf`.
trait ObjectOutputSchema {
    fn with_object_output_schema<T: JsonSchema + 'static>(self) -> Self;
}

impl ObjectOutputSchema for Tool {
    fn with_object_output_schema<T: JsonSchema + 'static>(self) -> Self {
        let mut tool = self.with_output_schema::<T>();
        if let Some(schema) = &mut tool.output_schema {
            std::sync::Arc::make_mut(schema)
                .entry("type")
                .or_insert_with(|| serde_json::json!("object"));
        }
        tool
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSelectionArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub id: Option<String>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub name: Option<String>,
}
impl BuildSelectionArgs {
    pub fn selector(&self) -> Result<destiny_app::builds::BuildSelector, McpError> {
        match (&self.id, &self.name) {
            (Some(id), None)
                if !id.is_empty()
                    && id.len() <= MAX_IDENTIFIER_LENGTH
                    && !id.chars().any(char::is_control) =>
            {
                Ok(destiny_app::builds::BuildSelector::Id(id.clone()))
            }
            (None, Some(name)) => {
                destiny_app::SearchTerm::new(name).map_err(McpError::from)?;
                Ok(destiny_app::builds::BuildSelector::Name(name.clone()))
            }
            _ => Err(McpError::new(ErrorCategory::ProviderValidation)),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildSourceArg {
    Import,
    Equipped,
    InGameLoadout,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SaveBuildArgs {
    pub source: BuildSourceArg,
    /// DIM JSON or full loadouts?loadout= URL; never a server file path.
    #[schemars(length(min = 1, max = 1048576))]
    pub input: Option<String>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub character: Option<String>,
    /// Required for equipped source; in-game source retains its original name.
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub name: Option<String>,
    #[schemars(range(min = 1, max = 12))]
    pub slot: Option<u8>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub loadout_name: Option<String>,
}
impl SaveBuildArgs {
    pub fn validate(&self) -> Result<(), McpError> {
        let invalid = || McpError::new(ErrorCategory::ProviderValidation);
        for text in [&self.character, &self.name, &self.loadout_name]
            .into_iter()
            .flatten()
        {
            destiny_app::SearchTerm::new(text).map_err(McpError::from)?;
        }
        match self.source {
            BuildSourceArg::Import
                if self.character.is_none()
                    && self.name.is_none()
                    && self.slot.is_none()
                    && self.loadout_name.is_none() =>
            {
                let input = self.input.as_ref().ok_or_else(invalid)?;
                if input.is_empty() || input.len() > 1048576 {
                    return Err(invalid());
                }
            }
            BuildSourceArg::Equipped
                if self.input.is_none()
                    && self.character.is_some()
                    && self.name.is_some()
                    && self.slot.is_none()
                    && self.loadout_name.is_none() => {}
            BuildSourceArg::InGameLoadout
                if self.input.is_none() && self.character.is_some() && self.name.is_none() =>
            {
                ApplyLoadoutArgs {
                    character: self.character.clone().unwrap(),
                    slot: self.slot,
                    name: self.loadout_name.clone(),
                }
                .request()?;
            }
            _ => return Err(invalid()),
        }
        Ok(())
    }
}
#[derive(JsonSchema)]
pub struct BuildOutputSchema {
    pub target_character: Option<String>,
    pub counts: Option<serde_json::Value>,
    pub items: Option<Vec<serde_json::Value>>,
    pub findings: Option<Vec<serde_json::Value>>,
    pub not_evaluated: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub kind: String,
    pub summary: Option<serde_json::Value>,
    pub builds: Option<Vec<serde_json::Value>>,
    pub dim_json: Option<String>,
    pub omissions: Option<Vec<String>>,
    pub unresolved_items: Option<Vec<String>>,
    pub deleted: Option<bool>,
    pub error: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnalyzeBuildArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub id: Option<String>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub name: Option<String>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub character: Option<String>,
}
impl AnalyzeBuildArgs {
    pub fn request(&self) -> Result<destiny_app::builds::BuildCheckRequest, McpError> {
        let selector = BuildSelectionArgs {
            id: self.id.clone(),
            name: self.name.clone(),
        }
        .selector()?;
        if let Some(character) = &self.character {
            destiny_app::SearchTerm::new(character).map_err(McpError::from)?;
        }
        Ok(destiny_app::builds::BuildCheckRequest {
            selector,
            character: self.character.as_ref().map(destiny_app::CharacterId::new),
        })
    }
}

pub fn build_tool_specs() -> Vec<Tool> {
    vec![
        Tool::new("list_builds", "List local builds for the committed profile membership; no synchronization.", JsonObject::new()).with_input_schema::<ListCharactersArgs>().with_object_output_schema::<BuildOutputSchema>().with_annotations(rmcp::model::ToolAnnotations::new().read_only(true)),
        Tool::new("get_build", "Show one local build by id or exact normalized name (exactly one). Includes DIM JSON export and known source omissions. Ambiguity returns candidates; never guess.", JsonObject::new()).with_input_schema::<BuildSelectionArgs>().with_object_output_schema::<BuildOutputSchema>().with_annotations(rmcp::model::ToolAnnotations::new().read_only(true)),
        Tool::new("save_build", "Save a local build. source=import: input is DIM JSON/loadouts?loadout= URL (dim.gg requires deferred online import). source=equipped: character and name. source=in_game_loadout: character and exactly one slot (1..12) or loadout_name. Inventory sources use ordinary freshness and report omissions/unresolved items. Never applies gear.", JsonObject::new()).with_input_schema::<SaveBuildArgs>().with_object_output_schema::<BuildOutputSchema>().with_annotations(rmcp::model::ToolAnnotations::new().read_only(false).destructive(false).idempotent(false)),
        Tool::new("delete_build", "Delete a local build by id or exact normalized name (exactly one). ID deletion is idempotent; name ambiguity returns candidates. Does not delete any Bungie/DIM loadout.", JsonObject::new()).with_input_schema::<BuildSelectionArgs>().with_object_output_schema::<BuildOutputSchema>().with_annotations(rmcp::model::ToolAnnotations::new().read_only(false).destructive(true)),
        Tool::new("analyze_build", "Check a stored build against synced inventory; exactly one id/name. Optional character ID/class; required if build class has multiple characters. Ordinary freshness. Reports resolved/missing items, counts, DIM findings and not_evaluated reasons; search matching is search_only. Never applies gear.", JsonObject::new()).with_input_schema::<AnalyzeBuildArgs>().with_object_output_schema::<BuildOutputSchema>().with_annotations(rmcp::model::ToolAnnotations::new().read_only(true)),
    ]
}

/// Stable tool name for the character listing read.
pub const LIST_CHARACTERS_TOOL: &str = "list_characters";

/// Stable tool name for the inventory search read.
pub const SEARCH_INVENTORY_TOOL: &str = "search_inventory";

/// Stable tool name for the exact instance inspection read.
pub const INSPECT_ITEM_TOOL: &str = "inspect_item";

/// Stable tool name for equipped subclass configuration.
pub const GET_SUBCLASS_TOOL: &str = "get_subclass";

/// Stable tool name for available subclass options.
pub const GET_SUBCLASS_OPTIONS_TOOL: &str = "get_subclass_options";

/// Stable tool name for live seasonal artifact state.
pub const GET_ARTIFACT_TOOL: &str = "get_artifact";

/// Stable tool name for saved in-game loadouts.
pub const GET_LOADOUTS_TOOL: &str = "get_loadouts";
pub const GET_LOADOUT_TOOL: &str = "get_loadout";
pub const APPLY_LOADOUT_TOOL: &str = "apply_loadout";

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyLoadoutArgs {
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub character: String,
    #[schemars(range(min = 1, max = 12))]
    pub slot: Option<u8>,
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub name: Option<String>,
}
impl ApplyLoadoutArgs {
    pub fn request(&self) -> Result<destiny_app::ApplyLoadoutRequest, McpError> {
        destiny_app::SearchTerm::new(&self.character).map_err(McpError::from)?;
        let loadout = match (&self.slot, &self.name) {
            (Some(slot), None) if (1..=12).contains(slot) => {
                destiny_app::LoadoutSelector::Slot(*slot)
            }
            (None, Some(name)) => {
                destiny_app::SearchTerm::new(name).map_err(McpError::from)?;
                destiny_app::LoadoutSelector::Name(name.clone())
            }
            _ => return Err(McpError::new(ErrorCategory::ProviderValidation)),
        };
        Ok(destiny_app::ApplyLoadoutRequest {
            character: destiny_app::CharacterId::new(&self.character),
            loadout,
            intent: destiny_app::MoveIntentAuthorization::Explicit,
        })
    }
}
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyLoadoutLocalStatusView {
    Updated,
    Partial,
    Failed,
    NotAttempted,
}
#[derive(JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplyLoadoutLocalReasonView {
    NotOwned,
    Ambiguous,
    MissingDefinition,
    DefinitionUnavailable,
    SubclassNotHeld,
    UnsupportedBucket,
    InvalidProjection,
    OnOtherCharacter,
    InPostmaster,
}
#[derive(JsonSchema)]
pub struct ApplyLoadoutLocalItemView {
    pub instance_id: String,
    pub item_hash: Option<i64>,
    pub bucket: Option<String>,
    pub name: Option<String>,
}
#[derive(JsonSchema)]
pub struct ApplyLoadoutLocalIssueView {
    pub item: ApplyLoadoutLocalItemView,
    pub reason: ApplyLoadoutLocalReasonView,
}
#[derive(JsonSchema)]
pub struct ApplyLoadoutLocalStateView {
    pub status: ApplyLoadoutLocalStatusView,
    pub unresolved_items: Vec<ApplyLoadoutLocalIssueView>,
    pub skipped: Vec<ApplyLoadoutLocalIssueView>,
    /// True includes cases with no projection effects to write.
    pub persisted: bool,
}
#[derive(JsonSchema)]
pub struct ApplyLoadoutOutputView {
    pub kind: String,
    pub character: Option<String>,
    pub slot: Option<u8>,
    pub name: Option<String>,
    pub outcome: String,
    pub receipt: Option<String>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub code: Option<String>,
    pub candidates: Option<Vec<ApplyLoadoutCandidateView>>,
    pub local_state: ApplyLoadoutLocalStateView,
}
#[derive(JsonSchema)]
pub struct ApplyLoadoutCandidateView {
    pub slot: u8,
    pub name: Option<String>,
}
pub fn apply_loadout_tool() -> Tool {
    let tool = Tool::new(
        APPLY_LOADOUT_TOOL,
        "Apply a saved loadout: character id/unique class and exactly one slot (1-based) or exact name; ambiguity returns candidates. Orbit/social space/offline only. Mods/plugs are not updated. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; incomplete, rejected or uncertain outcomes require refresh. Never retry an uncertain outcome. See local_state and docs/mcp.md for skips and persistence.",
        JsonObject::new(),
    ).with_input_schema::<ApplyLoadoutArgs>()
        .with_object_output_schema::<ApplyLoadoutOutputView>();
    // No top-level oneOf: the Anthropic API rejects tool input schemas that use
    // oneOf/anyOf/allOf at the root. Exactly one of slot or name is enforced at
    // runtime by `ApplyLoadoutArgs`.
    tool
}

/// Stable tool name for the single semantic mutation.
pub const MOVE_ITEM_TOOL: &str = "move_item";

/// Stable tool name for weekly predictions and current public milestones.
pub const WEEKLY_RESET_TOOL: &str = "weekly_reset";
pub const WEEKLY_TOOL: &str = "weekly";

const LIST_CHARACTERS_DESCRIPTION: &str = "List the signed-in account's characters with the freshness metadata of the committed profile projection.";

const SEARCH_INVENTORY_DESCRIPTION: &str = "Search definitions and inventory with bounded filters. Hits are compact tiles by default; detail=true returns full instance details. Continue owned hits with offset=next_offset; definition candidates are unchanged.";

const INSPECT_ITEM_DESCRIPTION: &str = "Inspect one owned item by instance_id or exact normalized name (exactly one required). Multiple owned namesakes return compact candidates; fuzzy matches are never inspected.";

const MOVE_ITEM_DESCRIPTION: &str = "Move exactly one instance_id/item_text to exactly one to_vault/character_id/character_label destination via ordered transfers. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty, incomplete effects or persistence failure require refresh. Never retry an uncertain outcome. verified_noop: local state already matches, no dispatch. See docs/mcp.md.";

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
            "get_postmaster",
            "Read committed Postmaster observations, not guaranteed live or complete. Omit character (or use null) for all owned characters; select an exact id or unique case-insensitive class. Anonymous stacks are not actionable. Component 103 currency coverage is independent of Postmaster coverage.",
            JsonObject::new(),
        )
        .with_input_schema::<GetPostmasterArgs>()
        .with_object_output_schema::<GetPostmasterOutput>()
        .with_annotations(rmcp::model::ToolAnnotations::new().read_only(true)),
        Tool::new(
            LIST_CHARACTERS_TOOL,
            LIST_CHARACTERS_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<ListCharactersArgs>()
        .with_object_output_schema::<ListCharactersOutput>(),
        Tool::new(
            SEARCH_INVENTORY_TOOL,
            SEARCH_INVENTORY_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<SearchInventoryArgs>()
        .with_object_output_schema::<SearchInventoryOutput>(),
        Tool::new(
            INSPECT_ITEM_TOOL,
            INSPECT_ITEM_DESCRIPTION,
            JsonObject::new(),
        )
        .with_input_schema::<InspectItemArgs>()
        .with_object_output_schema::<InspectItemOutput>(),
        Tool::new(
            GET_SUBCLASS_TOOL,
            "Return the equipped subclass super, abilities, aspects and fragments per character. Omit character for all characters, or select an exact id or unique hunter|titan|warlock class. Requires loaded Manifest definitions.",
            JsonObject::new(),
        )
        .with_input_schema::<GetSubclassArgs>()
        .with_object_output_schema::<GetSubclassOutput>(),
        Tool::new(
            GET_ARTIFACT_TOOL,
            "Return live artifact points and each character's equipped artifact and perks. Omit character for all characters, or select an exact id or unique hunter|titan|warlock class. Requires login and loaded Manifest definitions; reads live state.",
            JsonObject::new(),
        )
        .with_input_schema::<GetArtifactArgs>()
        .with_object_output_schema::<GetArtifactOutput>(),
        Tool::new(
            GET_LOADOUTS_TOOL,
            "List compact saved-loadout summaries: slot, name, subclass and equipped item names per slot. No plugs or insight text. Call get_loadouts first, then get_loadout for one loadout's full detail. Omit character for all characters, or select an exact id or unique hunter|titan|warlock class. Requires login and loaded Manifest definitions; reads live state.",
            JsonObject::new(),
        )
        .with_input_schema::<GetLoadoutsArgs>()
        .with_object_output_schema::<GetLoadoutsOutput>(),
        Tool::new(
            GET_LOADOUT_TOOL,
            "Read one saved loadout's full items, plugs, subclass and artifact perks; call get_loadouts first. loadout: one-based slot or normalized exact name. Optional character: exact id/unique class; omitted searches all. Unknown/ambiguous selectors return errors, never guesses. plug_insights deduplicates text; occurrences reference its index. Requires login and Manifest; reads live state. See docs/mcp.md.",
            JsonObject::new(),
        )
        .with_input_schema::<GetLoadoutArgs>()
        .with_object_output_schema::<GetLoadoutOutput>(),
        Tool::new(
            WEEKLY_RESET_TOOL,
            "Predict weekly calendar values from a community-derived rotation. Live milestones are observed data for the current week only. Requires login and `ghostlink sync-manifest` to load the required definitions. Dates use UTC, not the server's local zone.",
            JsonObject::new(),
        )
        .with_input_schema::<WeeklyResetArgs>()
        .with_object_output_schema::<WeeklyResetOutput>(),
        Tool::new(WEEKLY_TOOL, "Read per-character weekly content/progress. Requires login and sync-manifest. character: exact id/unique class; omitted selects highest light, deterministic first on ties. Reprise omitted. Vanguard alerts: GM, challenged Crucible/Vanguard Op variants and visible reward unions, not verified weapon assignments. See docs/mcp.md.", JsonObject::new())
            .with_input_schema::<WeeklyArgs>()
            .with_object_output_schema::<WeeklyOutput>(),
        Tool::new(
            GET_SUBCLASS_OPTIONS_TOOL,
            "Read subclass options (supers, abilities, aspects, fragments) with unlocked/locked/unknown status and raw can_insert/enabled. Status reliable only for the most recently played class. character: exact id/unique class; omit for all. Optional element filter. Requires Manifest. See docs/mcp.md.",
            JsonObject::new(),
        )
        .with_input_schema::<GetSubclassOptionsArgs>()
        .with_object_output_schema::<GetSubclassOptionsOutput>(),
    ]
}

/// The `move_item` descriptor, with its bounded input and structured output.
///
/// The descriptor is declared beside the read tools so the mutation contract is
/// reviewed with them, but it is deliberately not part of [`tool_specs`]: the
/// runtime advertises `move_item` only when a mutation seam is composed, and that
/// composition decision belongs to the binary root. Registering it changes no
/// input or output bound declared here.
pub fn move_item_tool() -> Tool {
    Tool::new(MOVE_ITEM_TOOL, MOVE_ITEM_DESCRIPTION, JsonObject::new())
        .with_input_schema::<MoveItemArgs>()
        .with_object_output_schema::<MoveItemOutput>()
}

pub const PULL_STACK_TOOL: &str = "pull_stack";

/// Exact non-instanced stack selection; provider identity is derived internally.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullStackArgs {
    #[schemars(length(min = 1, max = 10), regex(pattern = "^(0|[1-9][0-9]{0,9})$"))]
    pub item_hash: String,
    #[schemars(length(min = 1, max = 10), regex(pattern = "^[1-9][0-9]{0,9}$"))]
    pub amount: String,
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub character: String,
}
impl PullStackArgs {
    pub fn validate(&self) -> Result<(), McpError> {
        let invalid = || McpError::new(ErrorCategory::ProviderValidation);
        let hash = self.item_hash.parse::<u32>().map_err(|_| invalid())?;
        let amount = self.amount.parse::<i32>().map_err(|_| invalid())?;
        let character = self.character.parse::<i64>().map_err(|_| invalid())?;
        if hash.to_string() != self.item_hash
            || amount <= 0
            || amount.to_string() != self.amount
            || character <= 0
            || character.to_string() != self.character
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub fn pull_stack_tool() -> Tool {
    Tool::new(PULL_STACK_TOOL, "Pull a Postmaster stack: canonical uint32 item_hash, positive int32 amount/int64 character strings. Requires login/definitions; locally full refuses (1642). Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty or persistence failure require refresh. Never retry an uncertain outcome. Requested amount applies locally once, not as observed quantity. See docs/mcp.md.", JsonObject::new())
        .with_input_schema::<PullStackArgs>()
        .with_annotations(rmcp::model::ToolAnnotations::new().read_only(false).destructive(true).idempotent(false).open_world(true))
}

pub const PULL_ITEM_TOOL: &str = "pull_item";

/// One canonical genuine instance ID; every provider-bound value is derived.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PullItemArgs {
    #[schemars(length(min = 1, max = 19), regex(pattern = "^[1-9][0-9]{0,18}$"))]
    pub item: String,
}
impl PullItemArgs {
    pub fn instance_id(&self) -> Result<destiny_app::ItemInstanceId, McpError> {
        let invalid = || McpError::new(ErrorCategory::ProviderValidation);
        if self.item.is_empty() || self.item.len() > 19 {
            return Err(invalid());
        }
        let value = self.item.parse::<i64>().map_err(|_| invalid())?;
        if value <= 0 || value.to_string() != self.item {
            return Err(invalid());
        }
        Ok(destiny_app::ItemInstanceId::new(&self.item))
    }
}
pub fn pull_item_tool() -> Tool {
    Tool::new(PULL_ITEM_TOOL, "Pull genuine Postmaster item id to its owner's inventory, not equipped. Login/definitions required. Known full refuses; unknown occupancy permits dispatch; 1642 means full. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty or persistence failure require refresh. Never retry an uncertain outcome. No names, hashes, stacks or destination. See docs/mcp.md.", JsonObject::new())
        .with_input_schema::<PullItemArgs>()
        .with_annotations(rmcp::model::ToolAnnotations::new().read_only(false).destructive(true).idempotent(false).open_world(true))
}

pub const EQUIP_ITEMS_TOOL: &str = "equip_items";

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipItemsArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: String,
    #[schemars(length(min = 2, max = 17))]
    pub items: Vec<String>,
}
impl EquipItemsArgs {
    pub fn request(&self) -> Result<destiny_app::BatchEquipRequest, McpError> {
        if !(2..=17).contains(&self.items.len()) {
            return Err(McpError::new(ErrorCategory::ProviderValidation));
        }
        for item in &self.items {
            EquipItemArgs {
                item: item.clone(),
                character: self.character.clone(),
            }
            .validate()?;
        }
        Ok(destiny_app::BatchEquipRequest {
            character: self.character.clone(),
            items: self.items.clone(),
            intent: destiny_app::MoveIntentAuthorization::Explicit,
        })
    }
}
pub fn equip_items_tool() -> Tool {
    Tool::new(
        EQUIP_ITEMS_TOOL,
        "Equip 2–17 items on explicit character; validate slots/class/exotics, transfer via Vault, then one EquipItems call. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty, per-item failure, omitted results or persistence failure require refresh. Never retry an uncertain outcome. All-no-op: local state matches, no dispatch. See docs/mcp.md.",
        JsonObject::new(),
    )
    .with_input_schema::<EquipItemsArgs>()
    .with_annotations(
        rmcp::model::ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .idempotent(false)
            .open_world(true),
    )
}

pub const EQUIP_ITEM_TOOL: &str = "equip_item";

/// Both selectors are mandatory. An invocation authorizes exactly one intent.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EquipItemArgs {
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub item: String,
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: String,
}
impl EquipItemArgs {
    pub fn validate(&self) -> Result<(), McpError> {
        // Like CLI EquipArgs: validate all raw selectors as bounded semantic
        // text before I/O, even when resolution later chooses a numeric ID.
        destiny_app::ItemSelector::semantic(&self.item).map_err(McpError::from)?;
        for value in [&self.item, &self.character] {
            if value.trim().is_empty()
                || value.chars().count() > MAX_IDENTIFIER_LENGTH
                || value.chars().any(char::is_control)
            {
                return Err(McpError::new(ErrorCategory::ProviderValidation));
            }
        }
        Ok(())
    }
}
pub fn equip_item_tool() -> Tool {
    Tool::new(EQUIP_ITEM_TOOL, "Equip item on character; transfer if needed. Ambiguity refuses; partial progress is not rolled back. Orbit/social space/offline only. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty, incomplete effects or persistence failure require refresh. Never retry an uncertain outcome. verified_noop: local state matches, no dispatch. See docs/mcp.md.", JsonObject::new())
        .with_input_schema::<EquipItemArgs>()
}

pub const SET_LOCK_STATE_TOOL: &str = "set_lock_state";

/// One explicit intent to set a concrete owned item's lock state.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetLockStateArgs {
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub item: String,
    pub locked: bool,
}
impl SetLockStateArgs {
    pub fn validate(&self) -> Result<(), McpError> {
        destiny_app::ItemSelector::semantic(&self.item).map_err(McpError::from)?;
        if self.item.trim().is_empty() || self.item.chars().any(char::is_control) {
            return Err(McpError::new(ErrorCategory::ProviderValidation));
        }
        Ok(())
    }
}
pub fn set_lock_state_tool() -> Tool {
    Tool::new(SET_LOCK_STATE_TOOL, "Set owned item lock via locked boolean; ambiguity refuses, at most one dispatch. Vault uses an owned character. Plans from Ghostlink's local projection; sync only when stale/invalidated. Bungie is the final validator. Acknowledged actions update local state immediately; rejection, uncertainty or persistence failure require refresh. Never retry an uncertain outcome. verified_noop: local state matches, no dispatch. See docs/mcp.md.", JsonObject::new())
        .with_input_schema::<SetLockStateArgs>()
}

/// `list_characters` accepts no arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListCharactersArgs {}

/// `weekly_reset` optionally selects a UTC calendar date.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyResetArgs {
    /// YYYY-MM-DD: the weekly period in effect at the end of that UTC day; omit for the current week.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 10, max = 10), regex(pattern = r"^\d{4}-\d{2}-\d{2}$"))]
    pub date: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "weekly")]
pub struct WeeklyOutput {
    pub character: WeeklyCharacterView,
    pub definition_version: String,
    pub vanguard_alerts: VanguardAlertsView,
    pub featured_raids: Vec<PersonalGroupView>,
    pub featured_dungeons: Vec<PersonalGroupView>,

    pub expedition: Option<PersonalGroupView>,
    #[serde(rename = "override")]
    pub override_activity: Option<PersonalGroupView>,

    pub milestones: Vec<PersonalEntityView>,
    pub unclassified_unresolved: usize,
    pub unclassified: Vec<UnclassifiedActivityView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct VanguardAlertsView {
    pub grandmaster: Option<PersonalGroupView>,
    pub crucible: Vec<PersonalGroupView>,
    pub quickplay: Vec<QuickplayAlertView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyCharacterView {
    pub id: String,
    pub name: Option<String>,
    pub class: Option<String>,
    pub race: Option<String>,
    pub gender: Option<String>,
    pub light: Option<u16>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalEntityView {
    pub hash: u32,
    pub name: Option<String>,
    pub resolved: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalGroupView {
    pub root: String,
    pub reward_names: Vec<String>,
    pub activity_type: Option<PersonalEntityView>,
    pub variants: Vec<PersonalActivityView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalActivityView {
    #[serde(flatten)]
    pub entity: PersonalEntityView,
    pub activity_type: Option<PersonalEntityView>,
    pub rewards: Vec<PersonalEntityView>,
    pub modifiers: Vec<PersonalModifierView>,
    pub challenge_objectives: Vec<PersonalEntityView>,
    pub challenges: Vec<PersonalChallengeView>,
    pub phases: Option<PersonalPhaseView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct QuickplayAlertView {
    #[serde(flatten)]
    pub group: PersonalGroupView,
    pub pools: Vec<QuickplayPoolView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct QuickplayPoolView {
    pub objectives: Vec<PersonalChallengeView>,
    pub rewards: Vec<String>,
    pub variants: Vec<PersonalActivityView>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct UnclassifiedActivityView {
    pub hash: u32,
    pub name: Option<String>,
    pub reason: UnclassifiedReasonView,
    pub source: UnclassifiedSourceView,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnclassifiedReasonView {
    ActivityDefinitionMissing,
    ActivityTypeMissing,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnclassifiedSourceView {
    PublicMilestones,
    CharacterActivities,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalModifierView {
    #[serde(flatten)]
    pub entity: PersonalEntityView,
    /// Derived, name-based English presentation hint, not an API taxonomy.
    pub category: ModifierCategoryView,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierCategoryView {
    Surge,
    Overcharge,
    Champions,
    Other,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalChallengeView {
    #[serde(flatten)]
    pub entity: PersonalEntityView,
    pub progress: i64,
    pub completion_value: i64,
    pub complete: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PersonalPhaseView {
    pub complete: usize,
    pub total: usize,
}
impl WeeklyOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Structured public rotation, matching the CLI JSON contract.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "weekly_reset")]
pub struct WeeklyResetOutput {
    pub definition_version: String,
    pub current_week: bool,
    pub calendar: WeeklyCalendarView,
    pub milestones_source: String,
    #[schemars(length(max = 64))]
    pub milestones: Vec<WeeklyMilestoneView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyCalendarView {
    pub source: String,
    /// RFC3339 UTC start of the resolved weekly period.
    pub start: Option<String>,
    /// RFC3339 UTC end of the resolved weekly period (exclusive).
    pub end: Option<String>,
    pub rotations: Vec<WeeklyRotationView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyRotationView {
    pub key: String,
    pub title: String,
    #[serde(flatten)]
    pub outcome: WeeklyRotationOutcomeView,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WeeklyRotationOutcomeView {
    Predicted {
        activities: Vec<CalendarActivityView>,
    },
    NoData {
        activities: Vec<CalendarActivityView>,
        coverage_start: Option<String>,
        coverage_note: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CalendarActivityView {
    pub name: String,
    pub hash: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyMilestoneView {
    pub hash: u32,
    pub order: u32,
    #[schemars(length(max = 256))]
    pub name: Option<String>,
    #[schemars(length(max = 2048))]
    pub description: Option<String>,
    pub resolved: bool,
    /// RFC3339 UTC instant, or null when unavailable.
    pub start: Option<String>,
    /// RFC3339 UTC instant, or null when unavailable.
    pub end: Option<String>,
    #[schemars(length(max = 64))]
    pub activities: Vec<WeeklyActivityView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyActivityView {
    pub hash: u32,
    #[schemars(length(max = 256))]
    pub name: Option<String>,
    #[schemars(length(max = 2048))]
    pub description: Option<String>,
    pub resolved: bool,
    #[schemars(length(max = 64))]
    pub modifiers: Vec<WeeklyModifierView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct WeeklyModifierView {
    pub hash: u32,
    #[schemars(length(max = 256))]
    pub name: Option<String>,
    #[schemars(length(max = 2048))]
    pub description: Option<String>,
    pub resolved: bool,
}

impl WeeklyResetOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Bounded `search_inventory` arguments.
///
/// The definition query always runs. Any definition filter, minimum power,
/// socketed perk/mod name, or explicit `instances` query also searches owned items using those filters.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchInventoryArgs {
    /// DIM query: "is:weapon -is:locked", "perk:rampage", "power:>=2000 or is:exotic".
    /// ANDed with other arguments; empty string searches all owned items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Return full instance details instead of compact tiles (default false).
    #[serde(default)]
    pub detail: bool,
    /// Match socketed or selectable perks and mods; default is socketed only.
    #[serde(default)]
    pub include_available: bool,
    /// Optional case-insensitive definition name substring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_SEARCH_TEXT_LENGTH))]
    pub text: Option<String>,

    /// Optional exact definition item type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_SEARCH_TEXT_LENGTH))]
    pub item_type: Option<String>,

    /// Exact stored definition tier (case-insensitive); unknown names match nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_SEARCH_TEXT_LENGTH))]
    pub tier: Option<String>,

    /// Instance location class; combined with character restrictions using AND.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<LocationSchema>")]
    pub location: Option<String>,

    /// Instance equipped state; supplying false also enables instance search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equipped: Option<bool>,

    /// Optional tag filters; every listed tag must be present.
    #[serde(default)]
    #[schemars(
        length(max = MAX_TAG_FILTERS),
        inner(length(max = MAX_SEARCH_TEXT_LENGTH))
    )]
    pub tags: Vec<String>,

    /// Damage type (case-insensitive): kinetic, arc, solar, void, stasis, strand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,

    /// Slot (case-insensitive): kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,

    /// Class (case-insensitive): titan, hunter, warlock, any, unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_type: Option<String>,

    /// Socketed weapon-perk name: exact first, then substring; typos get suggestions.
    /// Supplying this field searches owned instances even without other filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub perk_name: Option<String>,

    /// Socketed armor-mod name: exact first, then substring; typos get suggestions.
    /// Supplying this field searches owned instances even without other filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub mod_name: Option<String>,

    /// Weapon-perk terms, AND across terms (also combined with perk_name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = destiny_app::MAX_NAME_TERMS), inner(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH)))]
    pub perk_names: Vec<String>,

    /// Armor-mod terms, AND across terms (also combined with mod_name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(length(max = destiny_app::MAX_NAME_TERMS), inner(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH)))]
    pub mod_names: Vec<String>,

    /// Socketed armor-archetype name: exact first, then substring; searches owned armor.
    /// Use `archetype`, not `archetype_name`; unknown fields are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub archetype: Option<String>,

    /// Stat comparisons: Name>=N, Name<=N, Name=N. All must hold; names are
    /// exact and case-insensitive, and any matching hash may satisfy a name.
    /// A stat-only filter searches owned instances.
    #[serde(default)]
    #[schemars(length(max = destiny_app::MAX_STAT_CONDITIONS), inner(length(min = 1, max = 288)))]
    pub stats: Vec<String>,

    /// Sort owned hits by one resolved stat: Stat[:asc|desc], descending by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 261))]
    pub sort: Option<String>,

    /// Minimum owned instance power (0..65535); items without power do not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_power: Option<u16>,

    /// Maximum number of definition matches; defaults to 100.
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = MAX_SEARCH_LIMIT))]
    pub limit: usize,

    /// Skip ordered owned hits (0..10000); definition candidates are unchanged.
    #[serde(default)]
    #[schemars(range(min = 0, max = MAX_SEARCH_OFFSET))]
    pub offset: usize,

    /// Optional concrete-instance restrictions, combined with definition filters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<InstanceQueryArgs>,
}

#[derive(JsonSchema)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
enum LocationSchema {
    Vault,
    Character,
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

    /// Minimum instance power (0..65535); items without power do not match.
    /// When both power filters are supplied, the higher minimum applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_power: Option<u16>,

    /// Maximum number of instance matches; defaults to 100.
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = MAX_SEARCH_LIMIT))]
    pub limit: usize,
}

/// `inspect_item` arguments: exactly one concrete instance identifier or name.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InspectItemArgs {
    /// Concrete instance identifier; mutually exclusive with name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub instance_id: Option<String>,
    /// Exact normalized owned item name; mutually exclusive with instance_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String", length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub name: Option<String>,
}

/// Optional Postmaster selector; omitted or null returns all owned characters.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetPostmasterArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "postmaster")]
pub struct GetPostmasterOutput {
    pub metadata: PostmasterMetadataView,
    pub profile_currencies_coverage: PostmasterCurrencyCoverageView,
    pub characters: Vec<PostmasterCharacterView>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PostmasterCurrencyCoverageView {
    NotObserved,
    Unavailable,
    Available,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PostmasterMetadataView {
    pub checked_at: u64,
    pub freshness: String,
    pub revision: u64,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PostmasterCharacterView {
    pub character_id: String,
    pub class: Option<String>,
    pub instances: Vec<PostmasterInstanceView>,
    pub stacks: Vec<PostmasterStackView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PostmasterInstanceView {
    pub instance_id: String,
    pub item_hash: i64,
    pub name: Option<String>,
    pub quantity: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct PostmasterStackView {
    pub item_hash: i64,
    pub name: Option<String>,
    pub bucket_hash: i64,
    pub quantity: i32,
}

impl GetPostmasterOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Optional character selector; omitted returns all characters.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetSubclassArgs {
    /// Exact character id or unique hunter|titan|warlock class (case-insensitive).
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
}

/// Optional bounded character selector for the live artifact read.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetArtifactArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "artifact")]
pub struct GetArtifactOutput {
    pub points: Option<ArtifactPointsView>,
    pub characters: Vec<CharacterArtifactView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ArtifactPointsView {
    pub points: u32,
    pub points_cap: u32,
    pub progress_to_next_level: u32,
    pub next_level_at: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CharacterArtifactView {
    pub character_id: String,
    pub class: Option<String>,
    pub equipped: Option<EquippedArtifactView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct EquippedArtifactView {
    pub instance_id: String,
    pub item_hash: i64,
    pub name: Option<String>,
    pub slots: Vec<ArtifactSlotView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ArtifactSlotView {
    pub plug_hash: i64,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity_match: Option<String>,
    pub empty: bool,
}

impl GetArtifactOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Optional bounded character selector for saved in-game loadouts.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetLoadoutsArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LoadoutSelectorArg {
    Slot(#[schemars(range(min = 1, max = 12))] u8),
    Name(#[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))] String),
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetLoadoutArgs {
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
    pub loadout: LoadoutSelectorArg,
}

/// Compact saved loadouts envelope, matching the CLI JSON projection.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "loadouts")]
pub struct GetLoadoutsOutput {
    pub characters: Vec<CharacterLoadoutsView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CharacterLoadoutsView {
    pub character_id: String,
    pub class: Option<String>,
    pub loadouts: Vec<LoadoutSummaryView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutSummaryView {
    pub slot: u8,
    pub name: Option<String>,
    pub subclass: Option<LoadoutSubclassSummaryView>,
    pub items: Vec<LoadoutItemSummaryView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutSubclassSummaryView {
    pub name: Option<String>,
    pub element: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutItemSummaryView {
    pub slot: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "loadout")]
pub struct GetLoadoutOutput {
    pub character_id: String,
    pub class: Option<String>,
    pub slot: u8,
    pub name: Option<String>,
    pub name_hash: u32,
    pub color_hash: u32,
    pub icon_hash: u32,
    pub items: Vec<LoadoutItemView>,
    pub plug_insights: Vec<LoadoutInsightView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutItemView {
    pub instance_id: String,
    pub item_hash: Option<i64>,
    pub name: Option<String>,
    pub item_type: Option<String>,
    pub bucket_hash: Option<u32>,
    pub element: Option<String>,
    pub missing: bool,
    pub plugs: Vec<LoadoutPlugView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutPlugView {
    pub socket_index: u8,
    pub plug_hash: i64,
    pub name: Option<String>,
    pub subclass_kind: Option<SubclassPlugKindView>,
    pub empty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insight: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct LoadoutInsightView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity_match: Option<String>,
}

impl GetLoadoutOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

impl GetLoadoutsOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Optional bounded character and element selectors; no status filtering.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GetSubclassOptionsArgs {
    /// Exact character id or unique hunter|titan|warlock class (case-insensitive).
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character: Option<String>,
    pub element: Option<SubclassElementView>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubclassElementView {
    Arc,
    Solar,
    Void,
    Stasis,
    Strand,
    Prismatic,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionStatusView {
    Unlocked,
    Locked,
    Unknown,
}

/// Full CLI-compatible options envelope, including raw provider flags.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "subclass_options")]
pub struct GetSubclassOptionsOutput {
    pub reference_character_id: Option<String>,
    pub characters: Vec<CharacterSubclassOptionsView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CharacterSubclassOptionsView {
    pub character_id: String,
    pub class: Option<String>,
    pub status_reliable: bool,
    pub elements: Vec<ElementOptionsView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ElementOptionsView {
    pub element: Option<SubclassElementView>,
    pub groups: Vec<OptionGroupView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct OptionGroupView {
    pub kind: SubclassPlugKindView,
    pub options: Vec<SubclassOptionView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SubclassOptionView {
    pub plug_hash: i64,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity_match: Option<String>,
    pub status: OptionStatusView,
    pub can_insert: bool,
    pub enabled: bool,
}

impl GetSubclassOptionsOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

/// Equipped subclass configuration, matching the CLI JSON envelope.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename = "subclass")]
pub struct GetSubclassOutput {
    pub characters: Vec<CharacterSubclassView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct CharacterSubclassView {
    pub character_id: String,
    pub class: Option<String>,
    pub subclass: Option<SubclassConfigView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SubclassConfigView {
    pub instance_id: String,
    pub item_hash: i64,
    pub name: String,
    pub element: Option<String>,
    pub slots: Vec<SubclassSlotView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SubclassSlotView {
    pub kind: SubclassPlugKindView,
    pub plug_hash: i64,
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity_match: Option<String>,
    pub empty: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubclassPlugKindView {
    Super,
    Aspect,
    Fragment,
    Grenade,
    Melee,
    ClassAbility,
    Movement,
    Transcendence,
}

impl GetSubclassOutput {
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

fn default_search_limit() -> usize {
    100
}

/// Bounded `move_item` arguments.
///
/// A caller names exactly one item and exactly one destination, and nothing
/// else. Membership, credentials, revisions, and plans are not accepted at all:
/// unknown arguments are rejected, so a caller cannot smuggle a provider
/// identity into the call. The adapter bounds are enforced before any service
/// call, and the shared selector constructors validate the constructed values
/// again.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MoveItemArgs {
    /// Exact instance identifier; exactly one of `instance_id` or `item_text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub instance_id: Option<String>,

    /// Semantic definition term; exactly one of `instance_id` or `item_text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub item_text: Option<String>,

    /// Explicit Vault destination; exclusive with any character destination.
    #[serde(default)]
    pub to_vault: bool,

    /// Exact character identifier destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_IDENTIFIER_LENGTH))]
    pub character_id: Option<String>,

    /// Character class or name label destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = MAX_SEARCH_TEXT_LENGTH))]
    pub character_label: Option<String>,
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
    Postmaster { character_id: String },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_type: Option<String>,
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
    pub available_perks: Vec<String>,
    pub available_mods: Vec<String>,
    pub definition: DefinitionView,
    pub instance: InstanceView,
    /// Resolved weapon-perk names in socket order, never plug hashes.
    pub perk_names: Vec<String>,
    /// Resolved armor-mod names in socket order, never plug hashes.
    pub mods: Vec<String>,
    /// First armor archetype in socket order; absent for other slots.
    pub archetype: Option<String>,
    pub stats: Vec<NamedStatView>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct NamedStatView {
    pub name: String,
    pub value: i32,
}

/// Wire schema for compact hits; projection is shared in destiny-output.
#[allow(dead_code)]
#[derive(JsonSchema)]
pub struct CompactSearchHitSchema {
    pub instance_id: String,
    pub item_hash: i64,
    pub name: String,
    pub item_type: Option<String>,
    pub tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<u16>,
    pub location: LocationView,
    pub equipped: bool,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<u32>,
    pub perk_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mods: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<NamedStatView>,
}

#[allow(dead_code)]
#[derive(JsonSchema)]
#[serde(untagged)]
enum SearchHitSchema {
    Compact(Box<CompactSearchHitSchema>),
    Detail(Box<InventorySearchHitView>),
}

/// Structured result of `search_inventory`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SearchInventoryOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub resolved: ResolvedNamesView,
    pub candidates: Vec<DefinitionView>,
    #[schemars(with = "Vec<SearchHitSchema>")]
    pub hits: Vec<InventorySearchHitView>,
    pub metadata: QueryMetadataView,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ResolvedNamesView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<ResolvedSortView>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub perk: Vec<String>,
    #[serde(rename = "mod", skip_serializing_if = "Vec::is_empty", default)]
    pub mods: Vec<String>,
    /// Each term's resolved alternatives, in input order.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub perk_terms: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub mod_terms: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub archetype: Vec<String>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty", default)]
    pub stats: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ResolvedSortView {
    pub name: String,
    pub order: String,
}

/// Inspection-only plug roles, matching CLI JSON spellings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionPlugRoleView {
    Intrinsic,
    Perk,
    Mod,
    AvailablePerk,
    AvailableMod,
    Archetype,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct InspectionPlugView {
    pub hash: i64,
    pub name: String,
    pub role: InspectionPlugRoleView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clarity_match: Option<String>,
}

/// A found inspection: one exact definition beside one concrete instance.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct FoundInspection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plug_details: Vec<InspectionPlugView>,
    pub perk_names: Vec<String>,
    pub mod_names: Vec<String>,
    /// Search-compatible spelling of `mod_names`.
    pub mods: Vec<String>,
    pub archetype: Option<String>,
    pub available_perks: Vec<String>,
    pub available_mods: Vec<String>,
    pub stats: Vec<NamedStatView>,
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

/// The sanitized expected postcondition of one move.
///
/// It records only what a successful execution must make observable: the
/// concrete item leaves `source` and arrives at `destination`, using the Vault
/// exactly once when `via_vault` is true. It carries no credential, raw provider
/// payload, membership identifier, revision, or plan identifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct MoveEffectsView {
    pub item_id: String,
    pub via_vault: bool,
    pub source: LocationView,
    pub destination: LocationView,
}

/// One planned step with the bounded outcome observed for it.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct StepEvidenceView {
    pub ordinal: u8,
    pub from: LocationView,
    pub to: LocationView,
    pub outcome: StepOutcomeView,
}

/// The sanitized outcome observed for one planned step.
///
/// `Rejected`, `Transient`, and `Uncertain` stay distinct so a caller never has
/// to collapse an unknown step effect into a success or a failure.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepOutcomeView {
    Unattempted,
    Completed,
    Rejected {
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        reason: TransferRejectionView,
    },
    EquipRejected {
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        reason: EquipRejectionView,
    },
    /// Received authentication refusal: attempted, not applied.
    AuthenticationRefused,
    Transient,
    Uncertain,
}

/// The sanitized reason a provider deterministically refused one step.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferRejectionView {
    ItemNotFound,
    AlreadyInInventory,
    NotInSocialSpace,
    Unequippable,
    ExoticLimit,
    UniquenessViolation,
    ActionForbidden,
    Throttled,
    NotTransferable,
    Equipped,
    DestinationFull,
    InvalidRequest,
}

/// The sanitized reason an equip step was refused.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipRejectionView {
    AlreadyInInventory,
    DestinationFull,
    UniquenessViolation,
    Equipped,
    NotTransferable,
    ActionForbidden,
    Throttled,
    NotInSocialSpace,
    ExoticLimit,
    Unequippable,
    ItemNotFound,
    CharacterNotFound,
    InvalidRequest,
}

/// Persisted action state shared with CLI JSON.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStateView {
    Updated,
    Unchanged,
    RefreshPending,
}

/// Local state already matches; no action was sent to Bungie.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct MoveSuccessView {
    pub local_state: LocalStateView,
    pub effects: MoveEffectsView,
    pub observed: LocationView,
    pub steps: Vec<StepEvidenceView>,
}

/// An honest incomplete execution: a dispatched sequence stopped short.
///
/// `partial` distinguishes an attempt that left at least one applied step behind
/// from one that applied nothing; both are incomplete rather than successful, and
/// neither claims a rollback. `failure` carries the sanitized pre-dispatch error
/// only when the sequence stopped on an error rather than on a classified step.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct IncompleteMoveView {
    pub local_state: LocalStateView,
    pub effects: MoveEffectsView,
    /// Receipt-derived expected location, not an observation.
    pub expected_location: LocationView,
    pub partial: bool,
    pub steps: Vec<StepEvidenceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<McpError>,
}

/// Why local state could not be updated after a dispatched sequence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailureReasonView {
    RefreshFailed,
}

/// A local-state update failure retaining bounded dispatch evidence.
/// The legacy `observed` field is not populated; `failure` is sanitized.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct VerificationFailureView {
    pub local_state: LocalStateView,
    pub effects: MoveEffectsView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<LocationView>,
    pub reason: VerificationFailureReasonView,
    pub steps: Vec<StepEvidenceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<McpError>,
}

/// Structured result of `move_item`.
///
/// `Applied` acknowledges receipts; `VerifiedNoOp` means local state (including Ghostlink's acknowledged actions)
/// already matches; nothing was sent to Bungie, not Bungie verification.
/// `Incomplete` and `VerificationFailed` preserve evidence without claiming success.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MoveItemOutput {
    /// Bungie acknowledged all steps; the resulting profile was not re-read.
    Applied(AppliedMoveView),
    VerifiedNoOp(MoveSuccessView),
    Incomplete(IncompleteMoveView),
    VerificationFailed(VerificationFailureView),
}

/// Acknowledged move with no claimed post-action observation.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct AppliedMoveView {
    pub local_state: LocalStateView,
    pub effects: MoveEffectsView,
    pub steps: Vec<StepEvidenceView>,
    pub reason: String,
}

/// A sanitized, stable MCP tool failure.
///
/// `code` is a stable machine token derived from the shared
/// [`ErrorCategory`], and `message` is the shared sanitized category message.
/// The type carries no provider payload, credential, or configuration value. The
/// same shape is reused inside a non-success `move_item` output, so one
/// sanitized failure contract covers both a tool-level error and an honest
/// partial result.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct McpError {
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    span: Option<QueryErrorSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    supported_filters: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct QueryErrorSpan {
    start: usize,
    end: usize,
}

impl McpError {
    /// Rich diagnostics without echoing the query or expanding tool discovery.
    pub fn invalid_query(error: destiny_app::search_query::QueryParseError) -> Self {
        Self {
            code: "invalid_query".to_owned(),
            message: error.message,
            kind: Some(error.kind.as_str().to_owned()),
            span: Some(QueryErrorSpan {
                start: error.span.start,
                end: error.span.end,
            }),
            supported_filters: (error.kind
                == destiny_app::search_query::QueryParseErrorKind::UnknownFilter)
                .then(|| {
                    destiny_app::search_query::list_filters()
                        .iter()
                        .filter(|spec| {
                            spec.format != destiny_app::search_query::FilterFormat::Unsupported
                        })
                        .flat_map(|spec| spec.keys.iter().map(|key| (*key).to_owned()))
                        .collect()
                }),
        }
    }

    /// Builds the sanitized MCP error for one shared application category.
    pub fn new(category: ErrorCategory) -> Self {
        Self {
            code: error_code(category).to_owned(),
            message: AppError::new(category).to_string(),
            kind: None,
            span: None,
            supported_filters: None,
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
        Self {
            code: error_code(error.category()).to_owned(),
            message: error.to_string(),
            kind: None,
            span: None,
            supported_filters: None,
        }
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
    /// Renders compact inventory tiles by default.
    pub fn into_call_tool_result(self) -> CallToolResult {
        match serde_json::to_value(self) {
            Ok(value) => structured_result(destiny_output::search::compact_search(value)),
            Err(_) => {
                CallToolResult::error(vec![ContentBlock::text("result serialization failed")])
            }
        }
    }

    /// Renders the unchanged full hit shape for explicit detail requests.
    pub fn into_detail_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

impl InspectItemOutput {
    /// Renders this output as a structured tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        structured_result(self)
    }
}

impl MoveItemOutput {
    /// Renders this output as a structured tool result.
    pub fn into_call_tool_result(self) -> CallToolResult {
        let success = matches!(self, Self::Applied(_) | Self::VerifiedNoOp(_));
        let mut result = structured_result(self);
        result.is_error = Some(!success);
        result
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
        ErrorCategory::DefinitionsUnavailable => "definitions_unavailable",
        ErrorCategory::UnknownStatName => "unknown_stat_name",
        ErrorCategory::UnknownSearchName => "unknown_search_name",
        ErrorCategory::AmbiguousSearchName => "ambiguous_search_name",
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
        MAX_TAG_FILTERS, MOVE_ITEM_TOOL, McpError, MoveItemArgs, SEARCH_INVENTORY_TOOL,
        SearchInventoryArgs, move_item_tool, tool_specs,
    };
    use destiny_app::{AppError, ErrorCategory};
    use serde_json::{Value, json};

    #[test]
    fn every_tool_description_stays_within_byte_budget() {
        let mut tools = tool_specs();
        tools.extend(super::build_tool_specs());
        tools.extend([
            move_item_tool(),
            super::equip_item_tool(),
            super::equip_items_tool(),
            super::set_lock_state_tool(),
            super::pull_item_tool(),
            super::pull_stack_tool(),
            super::apply_loadout_tool(),
        ]);
        let total: usize = tools
            .iter()
            .map(|tool| tool.description.as_ref().expect("description").len())
            .sum();
        println!("{} tool descriptions: {total} bytes", tools.len());
        for tool in tools {
            let bytes = tool.description.as_ref().expect("description").len();
            assert!(bytes <= 500, "{}: {bytes} bytes exceeds 500", tool.name);
        }
    }

    fn schema_json(tool_name: &str) -> Value {
        tool_specs()
            .into_iter()
            .find(|tool| tool.name == tool_name)
            .expect("tool is exposed")
            .schema_as_json_value()
    }

    fn move_item_schema() -> Value {
        move_item_tool().schema_as_json_value()
    }

    #[test]
    fn every_tool_output_schema_declares_an_object_root() {
        let mut tools = tool_specs();
        tools.extend(super::build_tool_specs());
        tools.extend([
            move_item_tool(),
            super::equip_item_tool(),
            super::set_lock_state_tool(),
            super::pull_item_tool(),
            super::pull_stack_tool(),
            super::apply_loadout_tool(),
            super::equip_items_tool(),
        ]);
        let invalid: Vec<_> = tools
            .iter()
            .filter(|tool| {
                tool.output_schema
                    .as_ref()
                    .is_some_and(|schema| schema.get("type") != Some(&json!("object")))
            })
            .map(|tool| tool.name.as_ref())
            .collect();
        assert!(
            invalid.is_empty(),
            "outputSchema.type must be object: {invalid:?}"
        );
    }

    #[test]
    fn object_output_schema_preserves_generated_enum_variants() {
        for (actual, generated) in [
            (
                tool_specs()
                    .into_iter()
                    .find(|tool| tool.name == INSPECT_ITEM_TOOL)
                    .expect("inspect_item is exposed"),
                rmcp::model::Tool::new("inspect", "", rmcp::model::JsonObject::new())
                    .with_output_schema::<super::InspectItemOutput>(),
            ),
            (
                move_item_tool(),
                rmcp::model::Tool::new("move", "", rmcp::model::JsonObject::new())
                    .with_output_schema::<super::MoveItemOutput>(),
            ),
        ] {
            let actual = actual.output_schema.expect("output schema");
            let mut expected = (*generated.output_schema.expect("generated schema")).clone();
            assert!(expected.get("oneOf").is_some_and(Value::is_array));
            expected.insert("type".into(), json!("object"));
            assert_eq!(*actual, expected);
        }
    }

    #[test]
    fn postmaster_is_discoverable_with_strict_optional_selector() {
        let tool = tool_specs()
            .into_iter()
            .find(|tool| tool.name == "get_postmaster")
            .expect("Postmaster read must be discoverable");
        let schema = tool.schema_as_json_value();
        assert_eq!(schema["additionalProperties"], false);
        assert!(
            schema["properties"]["character"]
                .to_string()
                .contains("512")
        );
    }

    #[test]
    fn tool_specs_expose_semantic_reads_in_stable_order() {
        let tools = tool_specs();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_postmaster",
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                INSPECT_ITEM_TOOL,
                super::GET_SUBCLASS_TOOL,
                super::GET_ARTIFACT_TOOL,
                super::GET_LOADOUTS_TOOL,
                super::GET_LOADOUT_TOOL,
                super::WEEKLY_RESET_TOOL,
                super::WEEKLY_TOOL,
                super::GET_SUBCLASS_OPTIONS_TOOL
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
        for field in ["perk_names", "mod_names"] {
            let property = &schema["properties"][field];
            assert_eq!(property["type"], "array");
            assert_eq!(property["maxItems"], destiny_app::MAX_NAME_TERMS);
            assert_eq!(property["items"]["minLength"], 1);
            assert_eq!(property["items"]["maxLength"], MAX_SEARCH_TEXT_LENGTH);
        }

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
        assert!(serde_json::from_value::<MoveItemArgs>(json!({"unknown": 1})).is_err());
        assert!(serde_json::from_value::<super::WeeklyResetArgs>(json!({"unknown": 1})).is_err());
        assert_eq!(
            schema_json(super::WEEKLY_RESET_TOOL)["additionalProperties"],
            false
        );
    }

    #[test]
    fn move_item_descriptor_is_declared_bounded_and_not_registered_by_tool_specs() {
        let tool = move_item_tool();
        assert_eq!(tool.name, MOVE_ITEM_TOOL);
        assert_eq!(tool.name, "move_item");
        assert!(
            tool.description
                .as_deref()
                .is_some_and(|value| !value.is_empty()),
            "move_item needs a stable description"
        );
        assert!(
            tool.output_schema.is_some(),
            "move_item needs a structured output schema"
        );

        // The mutation tool is declared here but only advertised once a
        // mutation seam is composed, so the read descriptor set is unchanged.
        let declared = tool_specs();
        let registered: Vec<&str> = declared.iter().map(|tool| tool.name.as_ref()).collect();
        assert!(!registered.contains(&MOVE_ITEM_TOOL));

        let rendered = serde_json::to_string(&move_item_schema()).expect("schema serializes");
        assert_eq!(
            move_item_schema().get("additionalProperties"),
            Some(&Value::Bool(false)),
            "unknown arguments must stay rejected"
        );
        assert!(
            rendered.contains(&format!("\"maxLength\":{MAX_IDENTIFIER_LENGTH}")),
            "identifiers must be bounded: {rendered}"
        );
        assert!(
            rendered.contains(&format!("\"maxLength\":{MAX_SEARCH_TEXT_LENGTH}")),
            "semantic text must be bounded: {rendered}"
        );
        assert!(
            rendered.contains("\"minLength\":1"),
            "empty selectors must be rejected: {rendered}"
        );
    }

    #[test]
    fn move_item_arguments_accept_no_provider_identity_or_plan() {
        // The declared input is a closed object with exactly the five bounded
        // semantic fields a caller may name.
        let input = move_item_schema();
        let mut declared: Vec<&str> = input
            .get("properties")
            .and_then(Value::as_object)
            .expect("the input schema declares properties")
            .keys()
            .map(String::as_str)
            .collect();
        declared.sort_unstable();
        assert_eq!(
            declared,
            [
                "character_id",
                "character_label",
                "instance_id",
                "item_text",
                "to_vault"
            ]
        );

        // No declared object property anywhere in the input or the output
        // schema is a provider identity, a credential, or a plan field.
        let output_schema = move_item_tool()
            .output_schema
            .expect("structured output schema");
        let output = serde_json::to_value(&*output_schema).expect("output schema serializes");
        for schema in [&input, &output] {
            for name in declared_property_names(schema) {
                for forbidden in [
                    "membership",
                    "membership_id",
                    "credential",
                    "revision",
                    "plan",
                    "plan_id",
                    "access_token",
                ] {
                    assert_ne!(
                        name, forbidden,
                        "move_item must not declare a caller-supplied {forbidden} field"
                    );
                }
            }
        }

        // A caller that tries to supply one is rejected by the closed object.
        for smuggled in [
            json!({"instance_id": "inst", "to_vault": true, "membership_id": "4611686018484639825"}),
            json!({"instance_id": "inst", "to_vault": true, "credential": "secret"}),
            json!({"instance_id": "inst", "to_vault": true, "revision": 7}),
            json!({"instance_id": "inst", "to_vault": true, "plan_id": "plan-1"}),
        ] {
            assert!(
                serde_json::from_value::<MoveItemArgs>(smuggled.clone()).is_err(),
                "smuggled provider identity must be rejected: {smuggled}"
            );
        }
    }

    /// Every object property name declared anywhere in one JSON schema.
    fn declared_property_names(schema: &Value) -> Vec<String> {
        let mut names = Vec::new();
        collect_property_names(&mut names, schema);
        names
    }

    fn collect_property_names(names: &mut Vec<String>, value: &Value) {
        match value {
            Value::Object(object) => {
                if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                    names.extend(properties.keys().cloned());
                }
                for nested in object.values() {
                    collect_property_names(names, nested);
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect_property_names(names, item);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    #[test]
    fn search_filter_schema_documents_names_and_rejects_unknown_fields() {
        let schema = serde_json::to_value(schemars::schema_for!(SearchInventoryArgs)).unwrap();
        for (field, names) in [
            (
                "element",
                vec!["kinetic", "arc", "solar", "void", "stasis", "strand"],
            ),
            (
                "slot",
                vec![
                    "kinetic",
                    "energy",
                    "power",
                    "helmet",
                    "gauntlets",
                    "chest",
                    "legs",
                    "classitem",
                    "other",
                    "unknown",
                ],
            ),
            (
                "class_type",
                vec!["titan", "hunter", "warlock", "any", "unknown"],
            ),
        ] {
            let description = schema["properties"][field]["description"].as_str().unwrap();
            for name in names {
                assert!(description.contains(name), "{field}: {name}");
            }
        }
        assert!(schema["properties"].get("min_power").is_some());
        assert_eq!(schema["additionalProperties"], false);
        assert!(
            serde_json::from_value::<SearchInventoryArgs>(json!({"min_power": 65536})).is_err()
        );
        assert!(
            serde_json::from_value::<SearchInventoryArgs>(json!({"instances": {"unknown": 1}}))
                .is_err()
        );
    }

    #[test]
    fn inspection_schema_has_no_top_level_combinators() {
        // The Anthropic API rejects tool input schemas with a top-level
        // oneOf/anyOf/allOf; the exactly-one-selector rule is enforced at runtime.
        let schema = serde_json::to_value(schemars::schema_for!(InspectItemArgs)).unwrap();
        for keyword in ["oneOf", "anyOf", "allOf"] {
            assert!(schema.get(keyword).is_none(), "{keyword}");
        }
    }

    #[test]
    fn omitted_limits_default_to_100() {
        let args: SearchInventoryArgs = serde_json::from_value(json!({})).expect("empty args");
        assert_eq!(args.limit, 100);
        assert_eq!(args.tags, Vec::<String>::new());
        assert_eq!(args.instances, None);
    }

    #[test]
    fn applied_move_schema_is_success_without_observation() {
        use super::{AppliedMoveView, LocationView, MoveEffectsView, MoveItemOutput};
        let output = MoveItemOutput::Applied(AppliedMoveView {
            local_state: super::LocalStateView::Updated,
            effects: MoveEffectsView {
                item_id: "1".into(),
                via_vault: false,
                source: LocationView::Vault,
                destination: LocationView::Character {
                    character_id: "2".into(),
                },
            },
            steps: vec![],
            reason: "Bungie accepted the action; not re-read".into(),
        });
        let value = serde_json::to_value(&output).unwrap();
        assert_eq!(value["status"], "applied");
        assert_eq!(value["local_state"], "updated");
        assert!(value.get("profile_refresh").is_none());
        assert!(value.get("observed").is_none());
        assert_eq!(
            serde_json::from_value::<MoveItemOutput>(value).unwrap(),
            output
        );
        assert_eq!(output.into_call_tool_result().is_error, Some(false));
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
