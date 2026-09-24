# Integration Assumptions

## Scope and date

This note records the external integration facts used to start the Destiny MVP architecture.
It covers Bungie OAuth and the official Rust MCP SDK only. It does not define the full
application architecture, endpoint model, token storage implementation, or deployment
mechanism.

- **Scope:** MVP authentication boundary and MCP transport/package baseline.
- **Evidence date:** 2026-04-17.
- **Audience:** DMA-003 architecture work.
- **Evidence policy:** facts below are limited to the primary-source material supplied
  for this task. Recommendations are explicitly marked as inference.

## Executive baseline

1. Bungie documents Authorization Code OAuth only, with a single case-sensitive,
   pre-registered redirect URL. Public and confidential clients have materially
   different token behavior.
2. The documented behavior does not establish PKCE, loopback wildcarding, dynamic
   ports, custom schemes, token revocation, or device-code authentication.
3. The official Rust SDK evidence supports `rmcp` 1.5.0 as a pinned 1.x baseline and
   documents a Tokio-based server with built-in stdio transport.
4. The native-client deployment choice remains unresolved: use a fixed callback with
   a protected confidential-client secret and backend, or accept repeated interactive
   authorization for a public client.

## Verified Bungie facts

### OAuth flow and registration

- Bungie supports a subset of OAuth 2.0 and documents only the Authorization Code
  grant.
- Clients are registered as either **Public** or **Confidential**.
- Exactly one redirect URL is registered for a client.
- A supplied `redirect_uri` must match the registered value case-sensitively.
- Scope is fixed at registration. `scope` must be omitted from authorization and
  token requests.
- The authorization endpoint is:
  `https://www.bungie.net/en/oauth/authorize`
- The token endpoint is:
  `https://www.bungie.net/platform/app/oauth/token/`
- Authorization requests require `response_type=code` and `client_id`.
- Clients should use a random, validated `state` value.

### Public and confidential clients

- A public client sends `client_id` and must not send `client_secret`.
- Bungie does not issue a client secret to a public client.
- Bungie does not issue refresh tokens to a public client under the documented
  behavior.
- A confidential client may refresh with `refresh_token`.
- The access-token response documents access-token expiry and refresh-token expiry.
- The documentation does not establish that a local executable can safely hold a
  confidential client secret.

### Requests and user interaction

- Protected resource calls require both `Authorization: Bearer` and the associated
  `X-API-Key`.
- Rejected or expired access tokens yield HTTP 401.
- The system browser should be used for authorization.
- Embedded web views are strongly discouraged and may be blocked.

### Explicit Bungie gaps

The supplied official OAuth page does **not** mention:

- PKCE;
- loopback URI wildcarding;
- dynamic ports;
- custom URI schemes;
- token revocation; or
- a device-code flow.

Therefore, these mechanisms must not be treated as Bungie guarantees. In particular,
exact single-value redirect registration means an ephemeral-port loopback callback
must not be assumed.

## Verified MCP Rust SDK facts

- The crates.io API recorded stable `rmcp` version **1.5.0**, published 2026-04-16.
- `rmcp` version 1.0.0 was published 2026-03-03.
- The Model Context Protocol Rust SDK repository describes RMCP as an official Rust
  MCP SDK using Tokio.
- The crate name is `rmcp`.
- The `rmcp-v1.5.0` snapshot targets MCP specification 2025-11-25.
- The SDK exposes server tools and macros.
- The SDK README shows a server served with `transport::stdio`.
- The crate README identifies `transport-io` as server-side stdio and `stdio` as the
  built-in server transport.
- The stable versioning policy says core crates at version 1.0 or greater follow
  SemVer, and `rmcp` and `rmcp-macros` are versioned together.

These version facts are a stability signal, not proof that every API is mature. The
repository `main` branch is moving relative to the time-bounded 1.5.0 snapshot, so
implementation must revalidate the selected APIs at implementation time.

## Architecture implications and recommendations (inference)

The following are design inferences from the verified facts, not provider guarantees.

- **Use the system browser for OAuth.** The authorization UX should launch the system
  browser rather than embedding a web view.
- **Treat redirect registration as a deployment contract.** The architecture should
  model one fixed, exact redirect URI per registered client and should not design
  around an assumed random port or wildcard.
- **Keep deployment choice open.** DMA-003 should compare a fixed callback suitable
  for a confidential client whose secret is protected by a backend against a public
  client flow that requires repeated interactive authorization. It must not claim
  that a local executable safely stores a confidential secret.
- **Make refresh conditional.** Refresh-token handling belongs only on the confidential
  path documented by Bungie. The public path needs an explicit reauthorization state.
- **Keep API credentials outside MCP data.** Core integration code should attach the
  Bungie API key and bearer token to protected calls without exposing either to MCP
  clients, model context, logs, or SQLite, consistent with the product constraint.
- **Handle 401 as an authentication boundary.** A rejected or expired access token
  should produce a typed authentication outcome that can trigger the applicable
  reauthorization or refresh path; the precise retry policy remains an architecture
  decision.
- **Pin the MCP SDK.** Start with an exact compatible `rmcp` 1.x version, likely the
  verified 1.5.0 snapshot if implementation-time revalidation confirms compatibility,
  and keep `rmcp-macros` aligned. Do not track repository `main` directly.
- **Keep MCP thin and stdio-oriented for the MVP.** The documented server-side stdio
  transport fits the vision's thin MCP adapter over one Rust core. Tool behavior and
  Destiny logic should remain in the core rather than in MCP handlers.
- **Revalidate before implementation.** Because `main` has moved since the 1.5.0
  evidence snapshot, verify package versions, APIs, and transport feature names again
  before scaffolding.

## Unresolved gaps for DMA-003

1. Which deployment model is approved: a fixed callback with a protected confidential
   client/backend, or a public client with repeated interactive authorization?
2. If the public path is selected, what local UX and state machine represent required
   reauthorization, cancellation, and expired access tokens?
3. If a backend is selected, what exact trust boundary owns the confidential secret and
   callback, and how does the native client communicate with it?
4. What fixed redirect URI is registered for the selected deployment, and how is its
   case-sensitive value configured without creating environment drift?
5. Which implementation-time `rmcp` 1.x version and feature set pass compatibility
   checks, given movement on the repository `main` branch?
6. What exact retry, refresh, and 401 error semantics should the shared Rust core
   expose to CLI and MCP adapters?
7. What secure local mechanism, if any, will hold public-client access state without
   placing secrets in MCP context, logs, or SQLite?

## Implementation revalidation (2026-09-21)

The DOF-004 public-client token-exchange slice revalidated the official Bungie OAuth
page on 2026-09-21 before implementation. Within the bounded scope of that page, the
revalidation confirms:

- The token endpoint is `https://www.bungie.net/platform/app/oauth/token/`.
- The token request is an `application/x-www-form-urlencoded` POST.
- The public-client token request sends `grant_type=authorization_code`, `code`, and
  `client_id`, and must not send `client_secret`.
- `redirect_uri` is optional on the token request but must match the registered value
  exactly when supplied; this implementation sends the same exact configured callback
  used for authorization.
- `scope` is omitted from token requests.
- The public-client path receives no refresh token, so no refresh grant or
  refresh-token storage is implemented.

This note adds no claim beyond the official page. The explicit gaps and unresolved
questions recorded below are unchanged.

## Credential storage decision (WU-013 / DHS-006, 2026-09-24)

This section resolves unresolved gap 7 above and records the MVP credential policy
decided on 2026-09-24. It adds no claim about Bungie or the MCP SDK.

- **Fail closed only, with no fallback.** The access credential is held in the
  operating system's native secure store through the `keyring-core` client layer.
  The MVP offers no plaintext, filesystem, or environment-variable fallback, and no
  opt-in gate selects one. A missing entry is a typed `AuthorizationRequired`
  failure, a legacy raw-material entry is `ReauthorizationRequired`, and an
  unusable or unsupported store is `CredentialStoreUnavailable`; none of these paths
  degrades to a weaker store.
- **The native-store record is versioned, not independently encrypted.**
  `NativeCredentialStore` persists a bounded, self-describing record (a leading
  marker plus expiry metadata) around the credential material in the OS secure
  store. The record includes the material verbatim; its confidentiality depends
  on the native store, not on this envelope. The record is not a filesystem or
  environment fallback. A raw-material-only entry written by an older build is
  detected and rejected rather than silently reinterpreted with a guessed expiry.
- **No diagnostics sink exists.** The MVP introduces no logging framework in WU-013.
  Process diagnostics are limited to sanitized `AppError` values on stderr, and
  stdout carries MCP protocol frames only. WU-013 records and tests that
  no-log/no-leak process-stream contract instead of inventing a runtime logger; any
  future diagnostics sink must preserve it.
- **Correlation identity is a contract, not yet propagation.** `CorrelationId` is a
  bounded, non-secret, visible-ASCII request identifier whose `Display`/`Debug`
  intentionally render the non-secret value. Carrying it through `TransferRequest`,
  the read/mutation services, and a future diagnostic sink is deliberately deferred:
  it would cross surfaces and many callers that WU-013 does not own, and there is no
  sink to receive it yet. Plan/step identifiers already remain visible for mutation
  execution without it.
- **Leak-harness limitation (reported, non-blocking).** The cross-boundary leak
  harness drives a real `destiny-cli` child process only through the sanitized
  failure and rejected-request paths. It cannot drive the CLI to the SQLite success
  path offline because the production composition loads the OS-keyring credential
  before it opens the store, so a live credential plus network would be required.
  The success-path process streams are instead proven through `destiny_cli::run`,
  which emits the exact bytes the process writes. See WU-013 in
  `docs/mvp-implementation-plan.md` and `odd/tasks/destiny-hardening.md` for the
  full harness record.

## Source register

| Source | Publisher | Retrieved | Confidence / freshness note |
|---|---|---:|---|
| [Bungie OAuth Documentation](https://github.com/Bungie-net/api/wiki/OAuth-Documentation) | Bungie-net | 2026-04-17 | High confidence for documented OAuth behavior; freshness is time-bounded to retrieval and silent on the explicit gaps listed above. |
| [Bungie OAuth Documentation](https://github.com/Bungie-net/api/wiki/OAuth-Documentation) | Bungie-net | 2026-09-21 | Implementation revalidation for the public-client token exchange: form POST fields, optional exact `redirect_uri`, `scope` omission, no public `client_secret`, and no public refresh token. Limited to the official page. |
| [`rmcp` crate metadata](https://crates.io/api/v1/crates/rmcp) | crates.io | 2026-04-17 | High confidence for the published-version record at retrieval; version availability may change. |
| [RMCP repository README at `rmcp-v1.5.0`](https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/rmcp-v1.5.0/README.md) | Model Context Protocol project | 2026-04-17 | High confidence for the tagged snapshot's SDK, Tokio, target-spec, tools/macros, and stdio claims; not a guarantee about later `main`. |
| [RMCP crate README at `rmcp-v1.5.0`](https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/rmcp-v1.5.0/crates/rmcp/README.md) | Model Context Protocol project | 2026-04-17 | High confidence for the tagged crate's transport names and server-side stdio description; revalidate against the pinned implementation version. |
| [RMCP versioning policy](https://github.com/modelcontextprotocol/rust-sdk/blob/main/VERSIONING.md) | Model Context Protocol project | 2026-04-17 | High confidence for the stated SemVer and paired-crate policy; `main` is moving, so this policy and the 1.5.0 snapshot should be rechecked at implementation time. |

## Handoff to DMA-003

Use this note as the evidence boundary. DMA-003 may turn the labeled inferences into
architecture decisions only after resolving the deployment and version questions above.
It should preserve the distinction between Bungie guarantees, SDK snapshot facts, and
project-owned recommendations.
