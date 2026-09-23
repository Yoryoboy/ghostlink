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

### Runtime configuration and authorization

A read composes the production runtime in one fail-fast order: the command line
is parsed and validated first, and only then does `destiny-cli` read the shared
runtime configuration, load the stored access credential, build the system
clock and the hardened HTTPS transport, open the durable SQLite store, and
dispatch the read through the shared `QueryService`. An invalid argument
therefore never reads configuration, a credential, a database, a clock, or the
network.

The read runtime requires exactly these variables (see `config.example.env`):

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
