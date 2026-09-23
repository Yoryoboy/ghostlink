//! MCP-facing contracts for the Destiny semantic read tools.
//!
//! This crate owns the Model Context Protocol presentation for
//! `list_characters`, `search_inventory`, and `inspect_item`. It translates MCP
//! inputs and outputs against the shared application read services and keeps
//! every official SDK type (`rmcp`) inside this crate, so the application and
//! core contracts stay provider- and transport-neutral.
//!
//! The current slice is contract-only: it pins the SDK dependency, defines the
//! bounded MCP schemas, and proves the translation binds to the shared query
//! behavior. The stdio server runtime and its production composition are a
//! separate follow-on step; this crate exposes a narrow, injectable read-service
//! seam for that runtime to call.
//!
//! # Boundaries
//!
//! - Every read delegates to the shared application query service; this crate
//!   invents no freshness, search, storage, provider, or authorization policy.
//! - MCP arguments are bounded before the service runs, and results are bounded,
//!   deterministic projections of the shared sanitized results.
//! - Failures expose stable sanitized categories and messages only. Raw provider
//!   payloads, credentials, and unbounded upstream records never cross this
//!   boundary.

pub mod schema;
pub mod service;

pub use schema::{
    FoundInspection, InspectItemArgs, InspectItemOutput, InstanceQueryArgs, ListCharactersArgs,
    ListCharactersOutput, MAX_IDENTIFIER_LENGTH, McpError, SearchInventoryArgs,
    SearchInventoryOutput, structured_result, tool_specs,
};
pub use service::{
    InventoryReadService, ListCharactersRequest, QueryReadService, inspect_item_output,
    list_characters_output, search_inventory_output,
};
