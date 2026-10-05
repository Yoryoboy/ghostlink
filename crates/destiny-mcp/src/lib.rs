//! MCP-facing contracts and stdio server for the Destiny semantic tools.
//!
//! This crate owns the Model Context Protocol presentation for
//! `list_characters`, `search_inventory`, `inspect_item`, `weekly_reset`, and the single
//! intent-authorized `move_item` mutation. It translates MCP inputs and outputs
//! against the shared application read and mutation services and keeps every
//! official SDK type (`rmcp`) inside this crate, so the application and core
//! contracts stay provider- and transport-neutral.
//!
//! [`server::ReadToolServer`] adapts the injectable read seam to
//! [`rmcp::ServerHandler`], and [`runtime::serve_stdio`] serves it over the
//! standard input/output transport. [`service::ComposedMutationService`] is the
//! adapter-free mutation seam: it composes the shared planner/executor and
//! derives the provider membership from the committed projection, so a caller
//! never supplies a membership, a credential, or a plan. The production
//! composition that names concrete provider and platform adapters stays in a
//! private process module, shared by the development and distribution binaries.
//!
//! # Boundaries
//!
//! - Every read and mutation delegates to a shared application service; this
//!   crate invents no freshness, search, storage, provider, planning, execution,
//!   verification, or authorization policy.
//! - MCP arguments are bounded before a service runs, and results are bounded,
//!   deterministic projections of the shared sanitized results.
//! - Failures expose stable sanitized categories and messages only. Raw provider
//!   payloads, credentials, membership identifiers, and unbounded upstream
//!   records never cross this boundary.
//! - The server, schemas, and service seams depend only on application services.
//!   The private process root is the single composition point that names adapters;
//!   its only public surface is [`run`].

#[path = "main.rs"]
mod process;
pub use process::run;

pub mod runtime;
pub mod schema;
pub mod server;
pub mod service;

pub use schema::{
    FoundInspection, IncompleteMoveView, InspectItemArgs, InspectItemOutput, InstanceQueryArgs,
    ListCharactersArgs, ListCharactersOutput, MAX_IDENTIFIER_LENGTH, MOVE_ITEM_TOOL, McpError,
    MoveEffectsView, MoveItemArgs, MoveItemOutput, MoveSuccessView, SearchInventoryArgs,
    SearchInventoryOutput, StepEvidenceView, StepOutcomeView, TransferRejectionView,
    VerificationFailureReasonView, VerificationFailureView, move_item_tool, structured_result,
    tool_specs,
};
pub use schema::{
    WEEKLY_RESET_TOOL, WeeklyActivityView, WeeklyMilestoneView, WeeklyModifierView,
    WeeklyResetArgs, WeeklyResetOutput,
};
pub use schema::{WEEKLY_TOOL, WeeklyArgs, WeeklyOutput};
pub use server::ReadToolServer;
pub use service::{
    ComposedMutationService, InventoryReadService, ListCharactersRequest, MoveItemRequest,
    MutationToolService, QueryReadService, inspect_item_output, list_characters_output,
    move_item_output, search_inventory_output,
};
pub use service::{
    PersonalWeeklyReadService, WeeklyReadService, weekly_output, weekly_reset_output,
};

#[cfg(test)]
mod tests {
    /// Presentation modules must never name concrete provider/platform adapters.
    ///
    /// Adapters stay in the private process composition module. Keeping the
    /// presentation contracts adapter-free keeps the stdio server injectable.
    #[test]
    fn the_presentation_modules_never_name_concrete_adapters() {
        for relative in [
            "src/lib.rs",
            "src/schema.rs",
            "src/service.rs",
            "src/server.rs",
            "src/runtime.rs",
        ] {
            let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), relative);
            let source = std::fs::read_to_string(&path).expect("library source is readable");
            // Ignore this crate's own test code, which necessarily mentions the
            // adapter crate name to assert its absence from production code.
            let production = source
                .split("#[cfg(test)]")
                .next()
                .expect("the file has a production prefix");
            assert!(
                !production.contains("destiny_adapters"),
                "library source {relative} names a concrete adapter"
            );
        }
    }

    /// The shared process composition root must be the place that names adapters.
    #[test]
    fn the_process_root_owns_adapter_composition() {
        let path = format!("{}/src/main.rs", env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(&path).expect("binary source is readable");
        assert!(
            source.contains("destiny_adapters"),
            "the binary composition root must name the concrete adapters"
        );
    }

    /// The product binary must never reach the offline fixture composition.
    ///
    /// The fixture is a separate, feature-gated binary; the product root has no
    /// fixture path, flag, or test-support dependency.
    #[test]
    fn the_product_binary_has_no_fixture_path_or_flag() {
        let path = format!("{}/src/main.rs", env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(&path).expect("binary source is readable");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("the file has a production prefix");
        for forbidden in ["test_support", "offline_fixture", "offline-fixtures"] {
            assert!(
                !production.contains(forbidden),
                "the product binary references the offline fixture via {forbidden}"
            );
        }
    }

    /// The presentation library must not depend on the core contract directly.
    ///
    /// `destiny-mcp` translates MCP values against the shared application
    /// services only, so a core type can reach it only through the additive
    /// `destiny-app` re-exports. A direct `destiny-core` manifest dependency
    /// would let the presentation layer bound itself to core types that the
    /// application boundary never sanctioned.
    #[test]
    fn the_mcp_manifest_has_no_direct_core_dependency() {
        let path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR"));
        let manifest = std::fs::read_to_string(&path).expect("crate manifest is readable");
        assert!(
            !manifest
                .lines()
                .any(|line| line.trim_start().starts_with("destiny-core")),
            "destiny-mcp must not depend directly on destiny-core"
        );
    }
}
