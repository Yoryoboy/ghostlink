# Configuration

Ghostlink reads its configuration itself; see [the README](../README.md) for a quick start.

Configuration is a Bungie public-client API key, public OAuth client ID,
custom-scheme callback URI (`ghostlink://oauth/callback`), and a separately
configured registered callback URI. The two callback values must match exactly,
including case. Optional `BUNGIE_CLIENT_SECRET` and
`GHOSTLINK_TOKEN_BROKER_URL` select the OAuth mode described below.

The shared read composition roots (the CLI and the MCP server) additionally read
the database path, Manifest definition version, and a positive freshness TTL:

```text
BUNGIE_API_KEY                Bungie API key (also used by the read roots)
DESTINY_DATABASE_PATH         SQLite projection database path
DESTINY_DEFINITION_VERSION    Manifest definition version to persist and compare
DESTINY_FRESHNESS_TTL_SECONDS Positive freshness TTL in seconds (max 2592000)
```

## Configuration file

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

## OAuth modes

Selection is identical for login, CLI reads, and MCP, in this order:

| Configuration | Mode | Token exchange and refresh |
| --- | --- | --- |
| `BUNGIE_CLIENT_SECRET` present | Direct confidential (development/self-hosting) | Bungie, with Basic authentication |
| No secret, `GHOSTLINK_TOKEN_BROKER_URL` present | Broker | Trusted broker, without client authentication |
| Neither key present | Public | Bungie code exchange only; no refresh |

Broker mode keeps the client secret on the broker, never in Ghostlink. The
stateless broker handles only authorization-code and refresh grants, not Destiny
API traffic. Configure its **base URL**, for example
`GHOSTLINK_TOKEN_BROKER_URL=https://YOUR_BROKER.workers.dev`; Ghostlink posts to
`https://YOUR_BROKER.workers.dev/token`. A trailing slash is optional; a path
prefix is treated as a directory and preserved. The URL must be absolute HTTPS,
no longer than 2048 bytes, with no userinfo, query, fragment, control characters,
or surrounding whitespace. Only use a trusted broker: it receives codes and
refresh tokens. Its registered client and callback must match your login config.

For direct confidential mode, change your Bungie application's OAuth client type
to Confidential and store the issued secret only in the `0600` per-user file,
never in shell exports or agent sessions. A secret takes precedence over a
broker URL. Present but empty or invalid values fail sanitized configuration
validation, including an invalid broker URL when a secret is present.

Both renewable modes require `BUNGIE_PUBLIC_CLIENT_ID` for reads as well as
login. Login stores refresh tokens (approximately 90 days) alongside the access
credential in the OS keyring. CLI and MCP automatically refresh expiring access
credentials and persist rotated refresh tokens. Login reports renewable status
when the exchange returns a refresh grant. Public mode remains unchanged and
requires browser reauthorization when its access token expires.


## Runtime configuration and authorization

A read composes the production runtime in one fail-fast order: the command line
is parsed and validated first, and only then does `destiny-cli` read the shared
runtime configuration, load the stored access credential, build the system
clock and the hardened HTTPS transport, open the durable SQLite store, and
dispatch the read through the shared `QueryService`. An invalid argument
therefore never reads configuration, a credential, a database, a clock, or the
network.

The read runtime always requires these variables (see `config.example.env`);
confidential and broker modes additionally require the client ID:

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

