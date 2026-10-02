# Ghostlink Config File (WU-016)

## Objective and scope
Let `ghostlink` (CLI and MCP composition roots) load its configuration from a per-user file so secrets never pass through the invoking shell or an agent session. The binary reads the file itself; process environment variables keep precedence. The human writes the file and its secret values; the agent never reads or writes secrets.

## Constraints and acceptance
- Location: `$GHOSTLINK_CONFIG_FILE` if set, else `$XDG_CONFIG_HOME/ghostlink/config.env`, else `$HOME/.config/ghostlink/config.env`. A missing file is not an error (environment only).
- Format: `KEY=value` lines; blank lines and `#` comments ignored; malformed lines rejected with a value-free error (line number only). Size bounded.
- Precedence: a key present in the process environment wins over the file. Only the exact requested keys are read from either source.
- Permissions (Unix): refuse a file that is group- or world-accessible (fail closed, value-free error naming the problem, not the contents).
- No configuration value, path content, or secret is ever rendered in errors, `Debug`, or output.
- Hook point: the existing `exact_key_values(names, lookup)` seam in `crates/destiny-app/src/config.rs`, so every `from_env()` inherits it.
- Test-first: deterministic unit tests for parsing, precedence, missing file, permissions, and sanitization (observe RED, then GREEN).
- Checks: `cargo fmt --all -- --check`, `cargo test --workspace --all-features` (after `cargo build -p destiny-cli`), `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- Delivery: work-unit commits on `feature/ghostlink-config-file`; push/PR/merge are user decisions.

## Tasks
- [x] **GCF-001 — Establish tracker and branch.** Branch `feature/ghostlink-config-file` from `main` at `bc6f895`. Commit evidence: `3a2241b`.
- [x] **GCF-002 — Load configuration from the per-user file with environment precedence.** Delegated writer, test-first: RED was unresolved `load`/`parse`/`resolve_path` and missing `exact_source_values`; GREEN 212 destiny-app tests. New private module `crates/destiny-app/src/config/file.rs`; `exact_env_values` composes env-first per key over the file. Real-binary tests are unaffected (leak journey clears env; MCP fixture reads no config). Workspace verification: fmt, build, 820 passed/19 suites, clippy, diff check. Commit evidence: `36da03c`.
- [x] **GCF-003 — Document the config file and migration.** README configuration-file section and `config.example.env` template (also corrected stale loopback callback to `ghostlink://oauth/callback`). Commit evidence: `9d36947`.
- [ ] **GCF-004 — Live read validation.** Human creates the file; agent runs read-only `ghostlink` commands with no secrets in its shell, reporting sanitized outcomes.

## Progress
Started from `main` at `bc6f895` after PR #1 merge. Exploration: `ReadRuntimeConfig::from_env`, `LoginRuntimeConfig::from_env`, and `Config::from_env` all funnel through `exact_env_values` -> `exact_key_values(names, lookup)`; consumers are `destiny-cli` (`runtime.rs`, `login.rs`), `destiny-mcp/src/main.rs`, and `destiny-app/src/lib.rs`.

## Next step
GCF-004: human creates `~/.config/ghostlink/config.env` (0600) with values from `.env.fish`; agent then runs read-only commands in a shell without secrets.
