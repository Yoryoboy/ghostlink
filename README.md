# Ghostlink Destiny Workspace

This repository is a Rust workspace for the Destiny application. Dependency
direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-core <- destiny-adapters <- destiny-cli (composition root only)
destiny-core <- destiny-adapters <- destiny-mcp (binary composition root)
destiny-app  <--------------------- destiny-mcp (library contracts + stdio server)
```

The `destiny-mcp` library holds the bounded MCP schemas, the injectable
`InventoryReadService` and `MutationToolService` seams, the RMCP tools-only
server, and the stdio runtime; it never names a concrete adapter. Only the
`destiny-mcp` binary composition root loads the native credential store, the
HTTP profile transport, the SQLite projection store, and the transfer gateway.

The five crates are intentionally minimal.

## Configuration

Configuration is a Bungie public-client API key, public OAuth client ID,
custom-scheme callback URI (`ghostlink://oauth/callback`), and a separately
configured registered callback URI. The two callback values must match exactly,
including case. An optional `BUNGIE_CLIENT_SECRET` selects confidential mode.

The shared read composition roots (the CLI and the MCP server) additionally read
the database path, Manifest definition version, and a positive freshness TTL:

```text
BUNGIE_API_KEY                Bungie API key (also used by the read roots)
DESTINY_DATABASE_PATH         SQLite projection database path
DESTINY_DEFINITION_VERSION    Manifest definition version to persist and compare
DESTINY_FRESHNESS_TTL_SECONDS Positive freshness TTL in seconds (max 2592000)
```

### Configuration file

Every binary reads its configuration itself, so secrets never need to be
exported in the shell that launches it (or in an agent session that runs it).
For each requested key, a value present in the process environment wins;
otherwise the value comes from the per-user configuration file, resolved in
this order:

1. `$GHOSTLINK_CONFIG_FILE`, when set and non-empty;
2. `$XDG_CONFIG_HOME/ghostlink/config.env`, when `XDG_CONFIG_HOME` is absolute;
3. `$HOME/.config/ghostlink/config.env`.

A missing file is not an error; the environment alone is then used. The file
holds `KEY=value` lines (blank lines, `#` comments, an optional `export`
prefix, and one pair of surrounding quotes are accepted) and is limited to
64 KiB. On Unix it must not be accessible by group or others: a file with any
group/other permission bit is refused. Every failure is the sanitized
`configuration_error`; no line, value, or key content is ever printed.

Create it from the template and fill in the values locally, never in a shared
session:

```sh
mkdir -p ~/.config/ghostlink
install -m 600 config.example.env ~/.config/ghostlink/config.env
$EDITOR ~/.config/ghostlink/config.env
```

Do not commit credentials. Startup diagnostics are typed and sanitized.

### Confidential and public mode

Without `BUNGIE_CLIENT_SECRET`, Ghostlink uses public mode and requires browser
reauthorization when the access token expires. For persistent sessions, edit
your app in the Bungie Developer Portal, change its OAuth client type to
Confidential, and save the change. Put the issued `BUNGIE_CLIENT_SECRET` only in
the `0600` per-user configuration file above, never in shell exports or agent
sessions. Keep `BUNGIE_PUBLIC_CLIENT_ID` unchanged: when a secret is configured,
that client ID is required for reads as well as login. A present but empty or
invalid secret fails configuration validation; remove the key to use public mode.

Confidential mode enables refresh tokens lasting approximately 90 days, stored
alongside the access credential in the OS keyring rather than in the config
file. Login and automatic-refresh wiring follow in the persistent-session
implementation; configuring the secret alone does not yet enable refresh.

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

### Manifest definitions

Item names, types and tiers come from the Destiny 2 Manifest, which must be
loaded once before `search-inventory` or `inspect-item` can resolve them:

```sh
ghostlink sync-manifest          # or: ghostlink --json sync-manifest
```

- The Manifest is public: the command needs only `BUNGIE_API_KEY` and
  `DESTINY_DATABASE_PATH`, never an OAuth login, the keyring, or a client
  secret.
- It downloads the English `DestinyInventoryItemDefinition` table (about 10 MB
  gzip, about 200 MB uncompressed, decoded as a stream) and replaces the
  stored catalog atomically. Records without a display name or marked
  redacted are skipped and counted.
- Re-running it fetches only the small Manifest index; when the live version
  equals the stored one it reports `unchanged` and downloads nothing. Run it
  again after a Destiny patch.
- Until a catalog is loaded, `search-inventory` and `inspect-item` fail with
  `definitions not loaded; run ghostlink sync-manifest` (CLI and MCP) instead
  of returning an empty result.

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

### Runtime configuration and authorization

A read composes the production runtime in one fail-fast order: the command line
is parsed and validated first, and only then does `destiny-cli` read the shared
runtime configuration, load the stored access credential, build the system
clock and the hardened HTTPS transport, open the durable SQLite store, and
dispatch the read through the shared `QueryService`. An invalid argument
therefore never reads configuration, a credential, a database, a clock, or the
network.

The read runtime always requires these variables (see `config.example.env`);
confidential mode additionally requires the client ID paired with the secret:

| Variable | Meaning |
| --- | --- |
| `BUNGIE_API_KEY` | Bungie public-client API key |
| `DESTINY_DATABASE_PATH` | Path to the durable SQLite projection |
| `DESTINY_DEFINITION_VERSION` | Manifest definition version the projection was built from |
| `DESTINY_FRESHNESS_TTL_SECONDS` | Positive freshness TTL in seconds (maximum `2592000`) |

A read also requires an already-authorized account. `destiny-cli` loads a
complete access credential from the operating system's native credential store
(service `dev.ghostlink.destiny`, account `destiny-access-credential`) and never
degrades to a plaintext or filesystem fallback. When no credential is stored,
the read exits with the typed `authorization_required` outcome (exit 10); a
missing or invalid read configuration exits with `configuration_error` (exit
52). The interactive browser authorization flow that writes that credential
remains an application/adapter concern outside this read-only unit.

Automated checks are offline: no live Bungie account or credential is used, and
no live read is claimed. The journey in
`crates/destiny-adapters/tests/query_journey.rs` drives the real CLI runner over
the real `QueryService` / `ProfileSyncService` / `BungieProfileGateway` /
`SqliteStore` with a scripted transport and clock; it proves behavior without a
live account and is not a live-success claim.

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
