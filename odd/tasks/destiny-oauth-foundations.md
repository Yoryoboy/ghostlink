# Destiny OAuth Foundations

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

- Bungie official OAuth documentation currently establishes Authorization Code, a single registered redirect URL, exact matching, public-client `client_id`, no public `client_secret`, no public refresh token, random validated `state`, and system-browser authorization.
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

- [ ] **DOF-004 — Implement public-client token exchange and reauthorization**
  - Route: delegated multi-file implementation after dependency/API revalidation.
  - Outcome: form-encoded authorization-code exchange with `client_id`, no secret/scope/refresh flow, typed denial/exchange/401/expiry outcomes, and logout coordination.
  - Check: fake HTTP contract tests and request-shape assertions; no live credentials.
  - Evidence: pending.

- [ ] **DOF-005 — Implement multiplatform native credential store**
  - Route: delegated multi-file implementation after explicit backend/version validation.
  - Outcome: native secure-store adapters for supported desktop platforms with fail-closed capability detection and no plaintext fallback.
  - Check: backend selection/compile tests, restricted fake integration, redaction/leak tests; real OS-store checks documented separately.
  - Evidence: pending.

- [ ] **DOF-006 — Verify and close WU-003**
  - Route: native review/assessment and delegated verification under active RDD policy.
  - Outcome: verify full feature, correct blockers, document live/manual limitations, and close WU-003.
  - Check: baseline/focused commands, dependency/security review, repository status, and review outcome.
  - Evidence: pending.

## Acceptance criteria

- Desktop mode launches the system browser when supported.
- Headless mode prints a safe authorization URL and exact SSH local-forward guidance, then waits for the same exact callback; it never claims to create the client-side tunnel.
- Pending state is random, short-lived, single-use, and rejects mismatch/replay.
- Public token exchange sends no client secret, scope, refresh grant, or credential-bearing diagnostics.
- Expiry and 401 produce explicit reauthorization; logout removes stored credentials.
- Native credential stores are multiplatform and fail closed when unavailable.
- CLI/MCP never receive raw secrets or callback query values.
- All baseline automated checks remain offline and credential-free.

## Progress

- WU-002 completed at `d477e4d`.
- Official Bungie documentation was fetched directly; generic web search was unavailable, so absence claims are limited to the official page and prior evidence register.
- User selected URL plus SSH tunnel guidance for headless mode and multiplatform native credential storage.
- DOF-001 established the feature boundary and evidence-backed UX contract in `1760aff`.
- DOF-002 implemented the offline authorization state machine in `2e23da0`; high-risk verification findings were corrected in `8470683`, and re-verification passed.
- DOF-003 implemented and hardened exact callback/browser/headless UX in `aa6433c`; final independent verification passed after closing all security and flakiness findings.

## Next step

Implement DOF-004 public-client token exchange and reauthorization mapping.
