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

/// Stable tool name for the single semantic mutation.
pub const MOVE_ITEM_TOOL: &str = "move_item";

/// Stable tool name for the credential-free public rotation.
pub const WEEKLY_RESET_TOOL: &str = "weekly_reset";
pub const WEEKLY_TOOL: &str = "weekly";

const LIST_CHARACTERS_DESCRIPTION: &str = "List the signed-in account's characters with the freshness metadata of the committed profile projection.";

const SEARCH_INVENTORY_DESCRIPTION: &str = "Search item definitions and, when requested, concrete inventory instances with bounded filters.";

const INSPECT_ITEM_DESCRIPTION: &str =
    "Inspect exactly one concrete inventory instance by its instance identifier.";

const MOVE_ITEM_DESCRIPTION: &str = "Move one concrete inventory instance to the Vault or to a character. One call plans the move, forces a fresh preflight, issues only the ordered steps, refreshes the projection after the action, and verifies the destination.";

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
        Tool::new(
            WEEKLY_RESET_TOOL,
            "Read the current week's public rotation without account credentials. Run `ghostlink sync-manifest` first to load the required definitions.",
            JsonObject::new(),
        )
        .with_input_schema::<WeeklyResetArgs>()
        .with_output_schema::<WeeklyResetOutput>(),
        Tool::new(WEEKLY_TOOL, "Read reliable per-character weekly content and progress. Requires login and `ghostlink sync-manifest`. Reprise is omitted. Vanguard alerts report GM, challenged Crucible, and challenged Vanguard Op variants with visible reward unions; these are not verified in-game weapon assignments. Character accepts an exact id or unique hunter|titan|warlock class; omitted selects strictly highest light, ties use deterministic first.", JsonObject::new())
            .with_input_schema::<WeeklyArgs>()
            .with_output_schema::<WeeklyOutput>(),
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
        .with_output_schema::<MoveItemOutput>()
}

/// `list_characters` accepts no arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ListCharactersArgs {}

/// `weekly_reset` accepts no arguments.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WeeklyResetArgs {}

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
    #[schemars(length(max = 64))]
    pub milestones: Vec<WeeklyMilestoneView>,
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

    /// Minimum owned instance power (0..65535); items without power do not match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_power: Option<u16>,

    /// Maximum number of definition matches; defaults to the shared maximum.
    #[serde(default = "default_search_limit")]
    #[schemars(range(min = 1, max = MAX_SEARCH_LIMIT))]
    pub limit: usize,

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

/// Structured result of `search_inventory`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct SearchInventoryOutput {
    pub resolved: ResolvedNamesView,
    pub candidates: Vec<DefinitionView>,
    pub hits: Vec<InventorySearchHitView>,
    pub metadata: QueryMetadataView,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct ResolvedNamesView {
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
    Rejected { reason: TransferRejectionView },
    Transient,
    Uncertain,
}

/// The sanitized reason a provider deterministically refused one step.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferRejectionView {
    NotTransferable,
    Equipped,
    DestinationFull,
    InvalidRequest,
}

/// A completed attempt whose destination the refreshed projection proves.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct MoveSuccessView {
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
    pub effects: MoveEffectsView,
    pub observed: LocationView,
    pub partial: bool,
    pub steps: Vec<StepEvidenceView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<McpError>,
}

/// Why the destination could not be verified after a dispatched sequence.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailureReasonView {
    RefreshFailed,
    Unresolved,
    Elsewhere,
}

/// A dispatched sequence whose destination could not be verified.
///
/// The bounded evidence survives, `observed` is the concrete location the
/// refreshed projection reported when there was one, and `failure` carries the
/// sanitized refresh or pre-dispatch error when one applies.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct VerificationFailureView {
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
/// `VerifiedSuccess` and `VerifiedNoOp` are proven end states; `Incomplete` and
/// `VerificationFailed` are honest non-success outcomes that keep their evidence
/// instead of collapsing into a bare error.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MoveItemOutput {
    VerifiedSuccess(MoveSuccessView),
    VerifiedNoOp(MoveSuccessView),
    Incomplete(IncompleteMoveView),
    VerificationFailed(VerificationFailureView),
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
        Self {
            code: error_code(error.category()).to_owned(),
            message: error.to_string(),
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

impl MoveItemOutput {
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
    fn tool_specs_expose_only_the_four_semantic_reads_in_stable_order() {
        let tools = tool_specs();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                LIST_CHARACTERS_TOOL,
                SEARCH_INVENTORY_TOOL,
                INSPECT_ITEM_TOOL,
                super::WEEKLY_RESET_TOOL,
                super::WEEKLY_TOOL
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
