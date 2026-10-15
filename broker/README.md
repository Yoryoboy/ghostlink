# Ghostlink token broker

A stateless Cloudflare Worker that adds Bungie confidential-client authentication
without distributing the client secret. It exchanges codes and refresh tokens;
it stores nothing and never handles Destiny API traffic. Native clients only:
there is no CORS support, token persistence, or application logging. It also
serves a static `GET /callback` page so a phone browser can hand the callback URL
back to the CLI.

## Contract

`POST /token` accepts `application/x-www-form-urlencoded` (optional media-type
parameters), at most 4096 bytes, with exactly one of these ordered field sets:

- `grant_type=authorization_code`, `code`, `redirect_uri`
- `grant_type=refresh_token`, `refresh_token`

Field order is preserved when forwarding; any input order is accepted. Every
value must be nonempty, at most 2048 characters, and contain no control characters.
Duplicate/extra/missing fields, malformed encoding and callback mismatches are
rejected. `redirect_uri` must exactly match one comma-separated entry of
`ALLOWED_REDIRECT_URIS` (whitespace around entries is trimmed, empty entries are
ignored). A missing or empty list is `server_misconfigured`, like the other
required settings.
Clients send neither `client_id`, `client_secret`, nor an Authorization header.
The Worker form-encodes its id and secret separately, joins with `:`, then base64
encodes them for upstream Basic authentication, matching the Rust adapter.
The forwarded body contains only the grant fields.

The owner-configured upstream must use HTTPS without URL credentials or a fragment;
redirects are not followed. The default is Bungie's token endpoint. A 10-second
AbortSignal deadline covers both fetch and body consumption. Only responses with
`application/json` media type and at most 8192 bytes are relayed, preserving status
and body (including Bungie 400/401 errors), not provider headers. JSON is not
parsed or rewritten. All responses have `Content-Type: application/json`,
`Cache-Control: no-store`, and `Pragma: no-cache`.

| Broker failure | Status / JSON `error` |
| --- | --- |
| Invalid form or callback | 400 / `invalid_request` |
| Unsupported grant | 400 / `unsupported_grant_type` |
| Other path | 404 / `invalid_request` |
| Wrong method | 405 / `invalid_request` (405 includes `Allow`) |
| Request too large / wrong media type | 413 / 415 / `invalid_request` |
| Rate limited | 429 / `rate_limited` |
| Missing required configuration | 500 / `server_misconfigured` |
| Invalid upstream response, network or limiter failure | 502 / `bad_gateway` |
| Upstream deadline exceeded | 504 / `gateway_timeout` |

`RATE_LIMITER` allows 30 requests per 60 seconds per `CF-Connecting-IP`, with
`unknown` as the fallback key. Cloudflare's limit is approximate and local to
its serving location, not a global hard quota. A missing binding permits requests
for local development; a failing binding fails closed. Production must retain it.

## Callback page

`GET /callback` (and `HEAD`) returns a static, self-contained HTML page used after
Bungie approval. It responds without any configuration and makes no upstream
request, so it is not covered by `RATE_LIMITER`: keeping the token-exchange budget
for `/token` alone means a throttled exchange cannot also make the browser page
fail to load.

The page sets `Content-Type: text/html; charset=utf-8`, `Cache-Control: no-store`,
`Pragma: no-cache`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`,
`X-Frame-Options: DENY` and a strict `Content-Security-Policy`
(`default-src 'none'`, a per-response nonce for the inline script and style only,
`frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'none'`). There are no
external resources.

The server never parses, stores, logs or echoes `code`/`state`; the page script
reads `window.location` in the browser. It shows the full callback URL in a
read-only field with a Copy button and instructs the user to run
`ghostlink login complete` on the machine where `ghostlink login start` ran. An
"Open in Ghostlink on this computer" link points at
`ghostlink://oauth/callback` plus the same query string, with no automatic
redirect (a phone without the handler shows an error dialog).

Client-side messaging distinguishes the outcomes: an `error` query parameter
reports that the authorization was not approved (still showing the URL so the
CLI can report the denial), and a query with neither `code` nor `error` reports
that no authorization result is present.

Any other method on `/callback` is `405 invalid_request` with `Allow: GET, HEAD`.

## Local development

From `broker/`, using Node.js LTS:

```sh
npm ci
npm run typecheck
npm test
cp .dev.vars.example .dev.vars
npm run dev
```

Tests run in plain Node with injected fetch and dummy credentials: no live Bungie
requests or Workers pool. `.dev.vars` is ignored. Do not copy production secrets
into it. `wrangler dev` uses the configured upstream, so dummy credentials cannot
complete a real token exchange. The `handleRequest(request, env, deps)` seam allows
an injected fetch and shorter deadline for deterministic validation.

## Owner-only deployment

These are manual operator steps, not part of tests or CI:

1. Replace the public `BUNGIE_CLIENT_ID` placeholder in `wrangler.toml`; verify the
   `ALLOWED_REDIRECT_URIS` entries exactly match the registered Bungie application
   (both the HTTPS `/callback` URL and `ghostlink://oauth/callback` during the
   rollout). Verify the rate limit namespace id is suitable for your account.
2. Authenticate Wrangler using the owner's Cloudflare account, then set the
   secret through its interactive prompt (never a command-line argument):
   `npx wrangler secret put BUNGIE_CLIENT_SECRET`.
3. Run `npm run deploy`. Configure the published `/token` URL in the Rust client
   in PR-003; that adapter change is not included here.

Trust only an owner-controlled upstream: its bounded JSON body is relayed verbatim.
Do not enable body capture, tracing, or logs containing codes, tokens, secrets,
Authorization headers or upstream bodies. Observability is disabled in the config;
exception text never reaches clients. No deployment or live credentials are needed
for the checks above.
