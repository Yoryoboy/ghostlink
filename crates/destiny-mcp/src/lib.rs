//! MCP-facing contracts and stdio server for the Destiny semantic read tools.
//!
//! This crate owns the Model Context Protocol presentation for
//! `list_characters`, `search_inventory`, and `inspect_item`. It translates MCP
//! inputs and outputs against the shared application read services and keeps
//! every official SDK type (`rmcp`) inside this crate, so the application and
//! core contracts stay provider- and transport-neutral.
//!
//! [`server::ReadToolServer`] adapts the injectable read seam to
//! [`rmcp::ServerHandler`], and [`runtime::serve_stdio`] serves it over the
//! standard input/output transport. The production composition that names
//! concrete provider and platform adapters stays in the binary root; this
//! library names no adapter.
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
//! - No concrete adapter type appears in this library: the server, schemas, and
//!   service seam depend only on the application read service. The binary root
//!   is the single composition point that names adapters.

pub mod runtime;
pub mod schema;
pub mod server;
pub mod service;

pub use schema::{
    FoundInspection, InspectItemArgs, InspectItemOutput, InstanceQueryArgs, ListCharactersArgs,
    ListCharactersOutput, MAX_IDENTIFIER_LENGTH, McpError, SearchInventoryArgs,
    SearchInventoryOutput, structured_result, tool_specs,
};
pub use server::ReadToolServer;
pub use service::{
    InventoryReadService, ListCharactersRequest, QueryReadService, inspect_item_output,
    list_characters_output, search_inventory_output,
};

#[cfg(test)]
mod tests {
    /// The library must never name a concrete provider or platform adapter.
    ///
    /// Concrete adapters are a binary-composition concern. Keeping the library
    /// free of `destiny_adapters` references protects the presentation contracts
    /// from adapter churn and keeps the stdio server injectable with any
    /// application read service.
    #[test]
    fn the_library_never_names_concrete_adapters() {
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

    /// The binary composition root must be the place that names adapters.
    #[test]
    fn the_binary_root_owns_adapter_composition() {
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
}
