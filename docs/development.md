# Development

## Workspace layout

This repository is a Rust workspace for the Destiny application. Dependency
direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-core <- destiny-adapters <- destiny-cli (composition root only)
destiny-core <- destiny-adapters <- destiny-mcp (binary composition root)
destiny-app  <--------------------- destiny-mcp (library contracts + stdio server)
destiny-output <------------------- destiny-cli, destiny-mcp (shared presentation)
```

The `destiny-mcp` library holds the bounded MCP schemas, the injectable
`InventoryReadService` and `MutationToolService` seams, the RMCP tools-only
server, and the stdio runtime; it never names a concrete adapter. Only the
`destiny-mcp` binary composition root loads the native credential store, the
HTTP profile transport, the SQLite projection store, and the transfer gateway.

`destiny-output` holds transport-neutral presentation contracts shared by the
CLI and the MCP server, so both surfaces render identical JSON.

The crates are intentionally minimal.

Design background lives in [mvp-architecture.md](mvp-architecture.md),
[mvp-implementation-plan.md](mvp-implementation-plan.md) and
[integration-assumptions.md](integration-assumptions.md).

## Agent skill

`skills/ghostlink/SKILL.md` teaches shell-capable agents to drive the CLI. It
holds judgment (intent to command, output contract, `equip` safety) and defers
flags to `ghostlink <cmd> --help`. `crates/destiny-cli/tests/skill_drift.rs`
fails when the skill names a missing command or flag, or omits a public
command, so update the skill together with any CLI surface change.

## Baseline checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo run -p destiny-cli -- --help
cargo run -p destiny-mcp -- --help
```

All baseline commands are offline and require no credentials.
