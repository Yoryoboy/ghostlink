# Ghostlink Persistent Session (WU-017)

## Objective and scope
Keep the user's Bungie session alive without repeated browser login. When `BUNGIE_CLIENT_SECRET` is configured, `ghostlink` acts as a Bungie confidential OAuth client: the token response carries a refresh token, which is stored in the native keyring alongside the access credential, and expiring access tokens are refreshed automatically in both the CLI and the long-lived MCP server (mid-session). Without a secret, behavior stays exactly as today (public client, reauthorize on expiry).

This WU explicitly supersedes the earlier non-goals "no client secret, confidential-client path, refresh token" recorded in `destiny-oauth-foundations.md`, `destiny-workspace-foundations.md`, and `ghostlink-cli-login.md`.

## Provider contract (Bungie OAuth wiki, verified 2026-10-01)
- Token endpoint `https://www.bungie.net/platform/app/oauth/token/` for both grants.
- Confidential client auth: `Authorization: Basic base64(client_id:client_secret)` (chosen), or `client_secret` in the body. Public clients must not send a secret.
- Code response adds `refresh_token` and `refresh_expires_in` (example 7776000 s = 90 days); `expires_in` 3600 s.
- Refresh: form body `grant_type=refresh_token&refresh_token=...`, no scope. The response may rotate the refresh token.
- Expired/invalid tokens return HTTP 401.

## Design decisions
- **Mode selection:** confidential iff `BUNGIE_CLIENT_SECRET` is present. `BUNGIE_PUBLIC_CLIENT_ID` stays the client id key (no breaking rename). Secret wrapped in a redacted newtype following the `ReadApiKey` pattern; loaded only through the existing config seam (env over `~/.config/ghostlink/config.env`, 0600 enforced).
- **Credential shape:** `AccessCredential` gains an optional refresh grant (opaque redacted `RefreshToken` + `refresh_expires_at`). Keyring record `GLCR` v2 carries it; v1 records stay readable (no refresh token).
- **Credential source:** new core port that yields a currently valid `AccessCredential` per request. Gateways (profile, transfer) consume it instead of a fixed credential. A static source keeps existing tests simple. The refreshing source (app layer) refreshes when the access token is within a safety margin (60 s) of expiry and a non-expired refresh token exists, persists the new credential (keeping the old refresh token if the response omits one), and serializes refreshes behind a lock so concurrent MCP calls refresh once.
- **Failure:** refresh rejected (400/401) or refresh token expired yields `ReauthorizationRequired`; stored credential is not silently deleted. No secret, token, or code ever reaches errors, `Debug`, logs, or output.

## Constraints and acceptance
- Test-first for every behavior task (token request/decode, record v2, config, refreshing source, gateway consumption): observe RED, then GREEN.
- Checks per task: `cargo fmt --all -- --check`, `cargo build -p destiny-cli`, `cargo test --workspace --all-features`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `git diff --check`.
- The agent never reads or writes secrets, never runs `ghostlink login`. Live validation is human-run with sanitized output.
- Review workload: forecast ~800-1200 authored lines; deliver as layered work-unit commits so a chained PR split stays possible.
- Delivery: work-unit commits on `feature/ghostlink-persistent-session`; push/PR/merge are user decisions.

## Tasks
- [ ] **PSR-001 — Establish tracker and branch.** Branch `feature/ghostlink-persistent-session` from `main` at `382c4f5`.
- [ ] **PSR-002 — Core types and ports.** `RefreshToken`, optional refresh grant on `AccessCredential`, token-refresh port, credential-source port with a static implementation.
- [ ] **PSR-003 — Confidential token adapter.** Basic client auth when a secret is configured; decode `refresh_token`/`refresh_expires_in` (still rejected for public mode); refresh-grant request and decode with rotation.
- [ ] **PSR-004 — Keyring record v2.** Persist the refresh grant; read v1 records unchanged.
- [ ] **PSR-005 — Configuration.** Optional `BUNGIE_CLIENT_SECRET`; read runtime gains client id and optional secret; `config.example.env` and README.
- [ ] **PSR-006 — Refreshing credential source and gateway consumption.** App-layer refresh policy (margin, rotation, single-flight lock, failure mapping); profile and transfer gateways request a credential per call.
- [ ] **PSR-007 — Wire CLI and MCP.** Login uses confidential exchange when configured; CLI and MCP compose the refreshing source; login output reports refresh availability accurately.
- [ ] **PSR-008 — Live validation and docs.** Human switches the Bungie app to Confidential, adds the secret, runs `ghostlink login`; verify refresh after access expiry (sanitized); close tracker.

## Progress
Exploration (read-only): no OAuth refresh code exists; `token.rs` decode rejects `refresh_token`; `CredentialStore` holds one credential (`GLCR` v1); gateways capture a fixed `AccessCredential` at composition (`destiny-cli/src/runtime.rs`, `destiny-mcp/src/main.rs`). User chose mid-session MCP refresh.

## Next step
PSR-002.
