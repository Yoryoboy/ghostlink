# Destiny OAuth Foundations — COMPLETE

## Objective

Implement WU-003 from `docs/mvp-implementation-plan.md`: secure public-client Bungie authorization, exact callback handling, desktop and headless UX, explicit reauthorization, and multiplatform native credential storage.

## Problem

The application has provider-neutral authentication ports but no authorization state machine, callback listener, code exchange, browser boundary, or production credential store. Bungie documents Authorization Code only, one exact redirect URI, no public-client secret, and no public-client refresh token.

## Why

Authentication must work locally and on SSH/headless hosts without inventing an unsupported device flow or exposing authorization codes, tokens, API keys, callback values, or credential contents.

## Scope

- Revalidate and record Bungie's current official OAuth contract.
- Implement a public-client authorization state machine with one-time random state, expiry, denial, cancellation, mismatch, replay rejection, exchange failure, logout, and reauthorization outcomes.
- Implement one exact fixed loopback callback listener and a provider-neutral token exchange boundary.
- Support desktop automatic browser launch and headless URL output with an exact SSH local-forward instruction.
- Implement multiplatform native credential storage for macOS, Windows, and Linux/*nix through explicitly selected keyring backends, failing closed when unavailable.
- Keep all automated tests offline and credential-free.

## Constraints

- No client secret, confidential-client path, refresh token, PKCE, device-code flow, wildcard callback, dynamic callback port, embedded webview, or manual authorization-code paste.
- Headless mode cannot create the browser-side SSH tunnel from the remote process. It generates the exact command/URL and automatically continues after the forwarded callback arrives.
- The callback URI remains an exact case-sensitive deployment contract.
- Secrets never enter logs, errors, CLI/MCP results, SQLite, fixtures, or model context.
- OS credential-store unavailability fails closed; no plaintext fallback.
- Technical artifacts are written in English.

## Evidence and decisions

- Bungie official OAuth documentation, revalidated on 2026-09-21, establishes Authorization Code, form-encoded token POSTs, required public-client `client_id`, optional exact `redirect_uri`, no public `client_secret`, no public refresh token, random validated `state`, and system-browser authorization.
- No official evidence establishes device-code/headless OAuth. Headless support therefore reuses the exact browser/callback flow through SSH local forwarding.
- The Rust `keyring` ecosystem documents native secure-store support across macOS, Windows, and *nix and recommends selecting explicit backend crates for production applications.
- User decision: support desktop browser launch plus headless URL/SSH-tunnel guidance, and multiplatform native credential storage.

## Delivery

- Strategy: `ask-on-risk`.
- Forecast: approximately 800–1,200 authored changed lines across contracts, state machine, adapters, tests, and documentation.
- Chain strategy: `feature-branch-chain`.
- Engram mirror topic: `odd/destiny-oauth-foundations/tasks`.
- Branch: `feature/destiny-oauth-foundations`, chained from completed WU-002.

## Effective checks

- TDD mode: not configured; use ordinary functional checks without claiming strict RED/GREEN evidence.
- Baseline: fmt, clippy all targets/features, workspace tests, all-features tests, CLI/MCP help.
- Focused: authorization state-machine tests, callback listener tests, fake browser/exchange/store contracts, secret-sentinel leak tests, and platform compile checks where available.
- Live Bungie OAuth and real OS-store checks are separate manual gates and cannot make offline CI fail.

## Tasks

- [x] **DOF-001 — Establish evidence, feature boundary, and UX contract**
  - Route: parent primary-source research and tracking.
  - Outcome: recorded current official evidence, desktop/headless behavior, OS-store scope, exclusions, and six bounded slices.
  - Check: direct official-source fetch succeeded; unsupported-flow absence remains explicitly bounded to the official page; tracker and branch were clean after commit.
  - Evidence: commit `1760aff` (`chore: start Destiny OAuth foundations`).

- [x] **DOF-002 — Implement authorization state machine and boundaries**
  - Route: delegated multi-file implementation followed by high-risk independent verification and a bounded invariant correction.
  - Outcome: added opaque/redacted authorization values, provider-neutral browser/callback/random/exchange ports, and a deterministic authorization service covering pending state, one-time callback consumption, expiry, replay, denial, cancellation, mismatch, exchange/storage failure, authorized/reauthorization states, provider rejection, and atomic logout. Unsafe begin re-entry, missing behavioral coverage, ambiguous store failure, and status/state disagreement were corrected.
  - Check: fmt, clippy, workspace/all-features/core/app test-support checks passed; independent correction verification passed with 40 all-features and 25 app test-support tests and no findings.
  - Evidence: implementation commit `2e23da0`; correction commit `8470683` (`fix: enforce Destiny authorization invariants`); re-verification passed.

- [x] **DOF-003 — Implement exact callback and desktop/headless UX**
  - Route: delegated multi-file implementation followed by repeated independent security verification and bounded corrections.
  - Outcome: added exact fixed loopback callback handling, strict and bounded HTTP parsing with one request-wide deadline, IPv4/IPv6/localhost behavior, safe system-browser argv, OAuth URL construction, SSH-first auto detection, and shell-safe headless forwarding guidance. Corrections closed SSH injection, port-zero, dual-stack, parser ambiguity, socket timeout, test TOCTOU, and slow-drip risks.
  - Check: final independent verification passed fmt, clippy, two 62-test all-features runs, 18 adapter tests, 29 app tests, both help commands, and diff check with no skips or findings.
  - Evidence: commit `aa6433c` (`feat: add Destiny OAuth callback UX`); final independent verdict commit-ready.

- [x] **DOF-004 — Implement public-client token exchange and reauthorization**
  - Route: delegated multi-file implementation after dependency/API revalidation; deterministic parent-readback gaps received one bounded delegated correction; final independent verification ran under ordinary policy after the user explicitly disabled RDD for this clone.
  - Outcome: implemented a bounded form-encoded authorization-code exchange with `client_id` and exact `redirect_uri`; no secret/scope/refresh flow; hardened TLS/timeout/redirect/proxy/response-size/content-type handling; strict token decoding; typed 401/403 reauthorization; expiry and logout coordination.
  - Check: fmt and clippy passed; workspace and all-features tests passed; 39 adapter tests and 32 app tests passed; CLI/MCP help and diff check passed; focused token suite passed 21 tests. The independent verifier classified the intermittent callback timeout test as a pre-existing environment-dependent DOF-003 flake after 9/10 isolated passes; it found no DOF-004 correctness or security blocker. Live OAuth remains N/A.
  - Evidence: implementation commit `000cc31` (`feat: add Destiny public token exchange`); ordinary independent verification passed and declared the candidate commit-ready. Native review was explicitly bypassed for this clone after the provider defect reported at `Gentleman-Programming/gentle-ai#4370` comment `5764097110`; no receipt exists.

- [x] **DOF-005 — Implement multiplatform native credential store**
  - Route: parent primary-source backend/version revalidation, delegated multi-file implementation, parent readback, and independent verification under ordinary policy.
  - Outcome: added `NativeCredentialStore` in destiny-adapters implementing the core `CredentialStore` port over `keyring-core` 1.0.0 with explicit target-gated production backends: `apple-native-keyring-store` 1.0.2 (`keychain`+`protected`) on macOS, `windows-native-keyring-store` 1.1.0 on Windows, and `dbus-secret-service-keyring-store` 1.0.1 with `crypto-rust` on Linux/FreeBSD. Binary secrets use `set_secret`/`get_secret` (no UTF-8 restriction) under fixed deployment-contract specifiers `CREDENTIAL_SERVICE`/`CREDENTIAL_USER`; unsupported targets and every backend failure map fail-closed to `CredentialStoreUnavailable` with no plaintext, filesystem, or in-memory fallback; `NoEntry` loads as `None` and delete is an idempotent no-op; keyring error payloads (which can carry secret bytes) are discarded whole and never formatted; tests inject the keyring-core mock store and never touch a real OS store.
  - Check: fmt, clippy `-D warnings`, workspace/all-features tests (94 passed), focused adapters tests (47 passed), CLI/MCP help, and diff-scope check all passed. Independent verifier confirmed fail-closed, redaction (sentinel tests over `BadEncoding`/`BadDataFormat`/`PlatformFailure`), key-steering resistance, and offline test isolation against the vendored 1.0.0 sources, and declared the candidate commit-ready. Linux backend compiled and exercised via mock; macOS/Windows construction calls match published APIs but their compilation is deferred to first cross-platform CI (documented follow-up). Real OS-store checks remain a separate manual gate.
  - Evidence: commit `93118a3` (`feat: add native credential store adapter`).

- [x] **DOF-006 — Verify and close WU-003**
  - Route: native read-only risk assessment plus delegated independent verification under clone-disabled RDD policy; parent reran a final diff/status spot check.
  - Outcome: WU-003 is complete at the committed branch state. Final review found no deterministic correctness or security blockers across authorization lifecycle, callback parsing/deadlines, desktop/headless boundaries, public-client exchange, redaction, logout/reauthorization, native credential storage, dependency features, or offline isolation. The known callback timeout test remains low-severity DOF-003 test-isolation debt rather than a product defect.
  - Check: assessment classified the full 17-file/4,982-line base diff high risk and required an independent verifier. `git diff --check`, fmt, clippy `-D warnings`, workspace tests (75 passed), all-features tests (94 passed on exact retry), adapters tests (47 passed), app test-support tests (32 passed), CLI/MCP help, dependency-duplicate review, source security review, and clean-status checks passed. The intermittent callback timeout test failed once in the parallel all-features run and once during repeated adapter runs, then passed on exact retries; it is unchanged since `aa6433c` and already tracked. Live Bungie OAuth, real OS-store checks, and first macOS/Windows backend compilation remain explicit manual/follow-up gates.
  - Evidence: independent final verdict `passed`; no native review or receipt exists because RDD is disabled for this clone; closure commit recorded in the final evidence update.

## Acceptance criteria

- Desktop mode launches the system browser when supported.
- The headless UX boundary supplies a safe authorization URL and exact SSH local-forward guidance, then waits for the same exact callback; it never claims to create the client-side tunnel. CLI presentation wiring remains WU-008 scope.
- Pending state is random, short-lived, single-use, and rejects mismatch/replay.
- Public token exchange sends no client secret, scope, refresh grant, or credential-bearing diagnostics.
- Expiry and 401 produce explicit reauthorization; logout removes stored credentials.
- Native credential stores are multiplatform and fail closed when unavailable.
- CLI/MCP never receive raw secrets or callback query values.
- All baseline automated checks remain offline and credential-free.

## Progress

- WU-002 completed at `d477e4d`; this feature branch contains 10 cohesive commits before its closure evidence update.
- DOF-001 established the evidence-backed UX contract in `1760aff`.
- DOF-002 implemented the authorization state machine in `2e23da0`, corrected invariants in `8470683`, and recorded evidence in `4f3b2cb`.
- DOF-003 implemented and hardened exact callback/browser/headless boundaries in `aa6433c`, with evidence in `3023b27`.
- DOF-004 implemented the hardened public-client exchange in `000cc31`, with evidence in `1b2bf64`. It uses constrained `reqwest` 0.13.5 blocking/rustls features, disables redirects/proxies, bounds deadlines and response size, and rejects secret/scope/refresh behavior.
- DOF-005 implemented fail-closed native credential storage in `93118a3`, with evidence in `c5a2c77`, using `keyring-core` 1.0.0 plus explicit target-gated store crates.
- DOF-006 independently verified the complete `feature/destiny-domain-foundations...HEAD` feature range: 17 files, 4,953 insertions, and 29 deletions before closure documentation. The high-risk assessment required independent verification; all required checks passed, with only the known pre-existing callback timeout test-isolation flake observed and reproduced.
- RDD remains disabled for this clone by explicit user decision after the reported native selection defect; WU-003 therefore closes under ordinary independent verification with no native receipt.
- Manual/follow-up gates remain honest and separate: live Bungie OAuth, real OS credential stores, and first macOS/Windows CI compilation of target-gated backends.

## Verification evidence

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace --locked`: 75 passed.
- `cargo test --workspace --all-features --locked`: 94 passed on exact retry after the known callback timeout flake.
- `cargo test -p destiny-adapters --all-features --locked`: 47 passed; repeated runs reproduced the same known test-only flake once.
- `cargo test -p destiny-app --features test-support --locked`: 32 passed.
- CLI/MCP help, dependency-duplicate review, source security review, diff check, and final clean-status checks passed.
- Rollback boundary: reset this feature branch to `feature/destiny-domain-foundations` (`d477e4d`) to remove only WU-003 authorization, callback, token, credential-store, dependency, test, and feature-document changes.

## Next step

Start WU-004 — Manifest acquisition, validation, normalization, and fake repository — on a new feature branch chained from this completed branch.
