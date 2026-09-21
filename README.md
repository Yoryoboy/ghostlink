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
