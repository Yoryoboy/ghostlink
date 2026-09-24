# Ghostlink CLI Login (WU-015)

## Objective and scope
Expose `ghostlink login` using existing public-client OAuth, loopback callback, native credential store and browser/headless adapters. The human owns Bungie application registration and browser authorization. Never collect credentials in the agent session or run live auth. Package a local CLI binary and supply a manual integration checklist; no push, PR, merge or release.

## Constraints and acceptance
- Keep `destiny-cli` as the internal crate; install the product executable as `ghostlink`.
- Exact callback `http://127.0.0.1:43127/oauth/callback`, matching portal and local configuration. Bind before opening browser; cryptographic state; reject mismatches/replay/timeout; sanitized diagnostics; no plaintext credential fallback.
- Offline tests for command and composed OAuth flow; build and help; no claim of live Bungie/keyring success. Bungie public clients receive no refresh token and must reauthorize on expiry.
- TDD mode: not configured in prior WU; ordinary checks. Runner: `cargo test --workspace --all-features`; also fmt, clippy, build and help.
- Delivery: ask-on-risk; forecast about 400-700 authored lines, monitor slices before commits. Route multi-file writes to bounded worker and verification to verifier. RDD switch off in this session; use native assessment after writer.

## Tasks
- [x] **GCL-001 — Establish recovery tracker and branch.** Route inline mechanical; local file and Engram observation `6037` written, branch confirmed. Commit evidence: see next progress update.
- [ ] **GCL-002 — Wire safe product login and executable name.** Route delegated writer; integrate existing OAuth service/adapters and config, production secure random, command dispatch, safe outcome display. Offline focused tests and fmt/clippy. Commit evidence pending.
- [ ] **GCL-003 — Validate packaged CLI and publish human-run checklist locally.** Route delegated verifier for build/test and local documentation; user owns live steps. Commit evidence pending.

## Progress
Started from clean `main` at `7e5fef6`; created `feature/ghostlink-cli-login`. Read-only mapping found existing OAuth state machine, browser, loopback callback, token transport and native store but no production SecureRandom or CLI login composition. Official Bungie OAuth documentation confirms Public client, fixed redirect, portal-configured scope and no public-client refresh token. Live authorization pending human action.

## Next step
Mirror tracker; implement GCL-002 without handling live credentials.
