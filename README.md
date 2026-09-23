# Ghostlink Destiny Workspace

This repository is a provider-free Rust workspace for the Destiny application.
Dependency direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-app <- destiny-mcp
```

The five crates are intentionally minimal. Runtime configuration is a Bungie
public-client API key, public OAuth client ID, HTTP loopback callback URI, and
a separately configured registered callback URI. The two callback values must
match exactly, including case. There is no client-secret configuration.
OAuth, HTTP calls, SQLite, MCP transport, and credential persistence are
planned for later work units.

Copy `config.example.env` and replace its obvious placeholders locally; do not
commit credentials. Startup diagnostics are typed and sanitized.

## Destiny CLI (read-only)

`destiny-cli` exposes three read commands over the shared WU-007 query
services:

```sh
cargo run -p destiny-cli -- list-characters
cargo run -p destiny-cli -- search-inventory --text gjallar --limit 5
cargo run -p destiny-cli -- inspect-item 700000000000000001
```

Add the global `--json` flag to select machine output:

```sh
cargo run -p destiny-cli -- --json search-inventory --text gjallar --item-hash 1001
```

### Output contract

- Human success output is line-oriented text with a `revision` / `checked_at` /
  `freshness` metadata footer. Empty and missing outcomes are explicit.
- JSON success output is exactly one object on stdout with a stable `kind`
  (`character_list`, `inventory_search`, or `item_inspection`) and a bounded
  `metadata` block (`revision`, `checked_at`, `source`, `freshness`). Definition
  search candidates and concrete instance hits stay in separate, bounded arrays.
- `inspect-item` accepts only a concrete instance identifier; a missing instance
  is the typed `item_not_found` outcome (exit 32), never an empty success.
- Failures write exactly one sanitized line to stderr (human) or one sanitized
  `{"error":{"code":"...","message":"..."}}` object (JSON) and never echo a
  rejected argument, credential, header, raw provider response, or membership
  ID.
- Validation happens before any service call, so invalid arguments make no
  provider or repository request. Exit codes follow the stable matrix in
  `odd/tasks/destiny-cli-read-journey.md`.

### Composition status

Normal production composition remains **blocked / pending** the authorized
foundational composition root: `destiny-app` does not yet expose a public
constructor that builds a `QueryService` from the production configuration and
adapters, so the shipped binary validates its arguments and, once startup
configuration is valid, reports the sanitized `unsupported_operation`/40
outcome for a read rather than fabricating an adapter or a production mode. The
offline journey in `crates/destiny-adapters/tests/query_journey.rs` drives the
real CLI runner over the real `QueryService` / `ProfileSyncService` /
`BungieProfileGateway` / `SqliteStore` with a scripted transport and clock; it
proves behavior without a live account and is **not** a live-success claim.

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
