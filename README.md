# Ghostlink Destiny Workspace

This repository is a Rust workspace for the Destiny application. Dependency
direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-core <- destiny-adapters <- destiny-mcp (binary composition root)
destiny-app  <--------------------- destiny-mcp (library contracts + stdio server)
```

The `destiny-mcp` library holds the bounded MCP schemas, the injectable
`InventoryReadService` and `MutationToolService` seams, the RMCP tools-only
server, and the stdio runtime; it never names a concrete adapter. Only the
`destiny-mcp` binary composition root loads the native credential store, the
HTTP profile transport, the SQLite projection store, and the transfer gateway.

## Configuration

Configuration is a Bungie public-client API key, public OAuth client ID, HTTP
loopback callback URI, and a separately configured registered callback URI. The
two callback values must match exactly, including case. There is no
client-secret configuration.

The shared read composition roots (the CLI and the MCP server) additionally read
the database path, Manifest definition version, and a positive freshness TTL:

```text
BUNGIE_API_KEY                Bungie API key (also used by the read roots)
DESTINY_DATABASE_PATH         SQLite projection database path
DESTINY_DEFINITION_VERSION    Manifest definition version to persist and compare
DESTINY_FRESHNESS_TTL_SECONDS Positive freshness TTL in seconds (max 2592000)
```

Copy `config.example.env` and replace its obvious placeholders locally; do not
commit credentials. Startup diagnostics are typed and sanitized.

## destiny-mcp

`destiny-mcp` serves the three read tools plus one default-enabled semantic
mutation over the Model Context Protocol stdio transport:

```text
list_characters   list the signed-in account's characters
search_inventory  search Manifest definitions and concrete instances
inspect_item      inspect exactly one concrete instance by instance identifier
move_item         move one concrete instance to the Vault or to a character
```

Mutation is composed by default in the product binary: its profile gateway and
durable projection are shared with the read path, and one explicit `move_item`
request is the whole intent authorization. There is no preview tool, confirmation
token or value, approval ceremony, or second MCP call. The single call plans the
move, forces a fresh revision-bound preflight, issues only the ordered transfer
steps, refreshes the projection after the action, and verifies the destination;
membership and credentials are resolved internally and are never accepted or
emitted through MCP.

Run it with the shared read configuration present in the environment:

```sh
cargo run -p destiny-mcp
```

The process speaks newline-delimited JSON-RPC on stdout only. Every startup and
runtime failure is a sanitized message written to stderr; nothing else is written
to stdout. A missing stored credential fails closed with `authorization is
required`, a legacy raw-material credential with `reauthorization is required`,
and neither path guesses an expiry or falls back to an environment value.

### Offline stdio integration test

The real child-process stdio test is gated behind the non-default
`offline-fixtures` feature. It spawns a clearly test-only `offline-fixture`
binary that composes deterministic in-memory application test doubles, then
drives complete MCP sessions (initialize, discovery, the three reads, a
protocol failure, a sanitized tool failure, cancellation, and shutdown) and the
one-call mutation journey: one `move_item` request performs semantic planning, a
fresh preflight, the minimum transfer calls, a post-action refresh, and
destination verification with no second call. It also proves a repeated request
fails closed without replay, an already-satisfied destination is a verified
no-op, a changed preflight is a typed stale failure, a refused step is an honest
partial execution, and stdout is protocol-only with no secret sentinel present:

```sh
cargo test -p destiny-mcp --all-features --test stdio
```

The fixture binary is never reachable from the product `destiny-mcp` binary and
requires no credential, network, database, or native keyring.

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
