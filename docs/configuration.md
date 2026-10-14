# Configuration

Official releases are designed to need no config file or Bungie developer
account: install, run `ghostlink` for the setup wizard, and sign in. See
[the README](../README.md) for the quick path. The current source checkout still
awaits owner provisioning of public application defaults, as noted below.

The CLI and MCP server resolve configuration themselves and share one
[credential file](#credential-file). Overrides are mainly for development or self-hosting. OAuth uses
`BUNGIE_PUBLIC_CLIENT_ID`, `BUNGIE_CALLBACK_URI` and
`BUNGIE_REGISTERED_CALLBACK_URI`; the two callback values must match exactly,
including case. Optional `BUNGIE_CLIENT_SECRET` and
`GHOSTLINK_TOKEN_BROKER_URL` select the OAuth mode described below.

Shared runtime settings:

```text
BUNGIE_API_KEY                Bungie API key (also used by the read roots)
DESTINY_DATABASE_PATH         Optional SQLite projection database path override
DESTINY_FRESHNESS_TTL_SECONDS Positive freshness TTL in seconds (max 2592000)
```

## Configuration file

Every binary reads its configuration itself, so secrets never need to be
exported in the shell that launches it (or in an agent session that runs it).
For each requested key, precedence is **process environment > `config.env` >
compiled default**. Present empty or invalid values fail validation rather than
falling back. `config.env` is optional; its path is resolved in this order:

On Linux and other Unix systems:

1. `$GHOSTLINK_CONFIG_FILE`, when set and non-empty;
2. `$XDG_CONFIG_HOME/ghostlink/config.env`, when `XDG_CONFIG_HOME` is absolute;
3. `$HOME/.config/ghostlink/config.env`.

On Windows, `$GHOSTLINK_CONFIG_FILE` still wins; otherwise the file is
`%APPDATA%\ghostlink\config.env` (APPDATA must be non-empty and absolute).
Windows does not apply the Unix permission-bit check; protect private values
with the operating system's access controls.

A missing file is not an error; environment values and compiled defaults are
then used. The file
holds `KEY=value` lines (blank lines, `#` comments, an optional `export`
prefix, and one pair of surrounding quotes are accepted) and is limited to
64 KiB. On Unix it must not be accessible by group or others: a file with any
group/other permission bit is refused. Every failure is the sanitized
`configuration_error`; no line, value, or key content is ever printed.

For overrides or self-hosting, optionally create it from the template. Remove
unused placeholder lines: they override compiled defaults. Fill any private
values locally, never in a shared session:

```sh
mkdir -m 700 -p ~/.config/ghostlink
install -m 600 config.example.env ~/.config/ghostlink/config.env
$EDITOR ~/.config/ghostlink/config.env
```

Never commit a client secret or account tokens. Official public application
values (API key, public client ID, callback URI, broker URL) are safe to commit
and are extractable from release binaries. Their single owner-maintained block
is `crates/destiny-app/src/defaults.rs`; every value remains overridable.

Callbacks default to `ghostlink://oauth/callback` and freshness TTL to `900`
seconds. API key, public client ID, and broker URL currently have `None` defaults
pending owner provisioning; absent required values still produce the same
sanitized configuration error. Thus this checkout is not yet fully zero-config.
The database path defaults to the OS location below. The definition version is
always read from committed Manifest metadata in that database; an old
`DESTINY_DEFINITION_VERSION` line or environment variable is ignored. Existing
snapshots remain readable; run `ghostlink sync` to re-stamp an old profile with
its stored Manifest version. This can advance the next profile revision once,
while identical re-syncs retain it. No
schema migration is needed for this change. Client secrets never have a compiled
default. Startup diagnostics are typed and sanitized.

## Database location

`DESTINY_DATABASE_PATH` is optional: **process environment > `config.env` >
OS default**. Explicit empty or invalid overrides fail rather than falling back.

| OS | Default database |
| --- | --- |
| Linux / other Unix | `$XDG_DATA_HOME/ghostlink/ghostlink.db` when XDG_DATA_HOME is non-empty and absolute; otherwise `$HOME/.local/share/ghostlink/ghostlink.db` (HOME must be non-empty and absolute) |
| Windows | `%LOCALAPPDATA%\ghostlink\ghostlink.db` (LOCALAPPDATA must be non-empty and absolute) |

If no base directory can be resolved and no override is supplied, startup
returns the same sanitized `configuration_error`. Every database open creates
missing parent directories, including nested parents. New directories use
`0700` on Unix; existing directory permissions are never changed. Creation or
open failures return sanitized `local_storage` errors without paths.

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
broker URL, including the compiled broker default. An environment or file
broker URL overrides the compiled one. Present but empty or invalid values fail sanitized configuration
validation, including an invalid broker URL when a secret is present.

Both renewable modes require `BUNGIE_PUBLIC_CLIENT_ID` for reads as well as
login. Login stores refresh tokens (approximately 90 days) alongside the access
credential in the [credential file](#credential-file). CLI and MCP automatically
refresh expiring access credentials and persist rotated refresh tokens. Login reports renewable status
when the exchange returns a refresh grant. Public mode remains unchanged and
requires browser reauthorization when its access token expires.

## Credential file

Sign-in stores the access token, its expiry and, in renewable modes, the refresh
token in one plain-text JSON file. No keyring, Secret Service or D-Bus session is
used, so authentication and renewal work the same on desktops and headless
servers.

| Platform | Default location |
| --- | --- |
| Linux and other Unix | `$XDG_CONFIG_HOME/ghostlink/credentials.json`, else `~/.config/ghostlink/credentials.json` |
| Windows | `%APPDATA%\ghostlink\credentials.json` |

`GHOSTLINK_CREDENTIALS_FILE` overrides the location (process environment only).
The file is separate from `config.env` and from the inventory database.

- **Not encrypted.** Any process running as the same user can read it. Do not
  commit it to a dotfiles repository or copy it into shared backups.
- **Private permissions.** On Unix a missing directory is created `0700` and the
  file is always written `0600`. An already existing directory keeps its
  permissions; the `0600` file is what protects the tokens. Tighten the
  directory too with `chmod 700 ~/.config/ghostlink`. A file that group or others can access is
  refused with `credential storage is unavailable`; fix it with
  `chmod 600 ~/.config/ghostlink/credentials.json`. On Windows the per-user
  profile's access control applies.
- **Atomic writes.** Saves replace the file through a temporary sibling and a
  rename, so readers never see a partial file.
- **One refresh at a time.** CLI and MCP share the file. Bungie rotates the
  refresh token on every renewal, so a renewal holds an exclusive lock on the
  empty sibling `credentials.json.lock` and rereads the file first; a process
  whose token another process already rotated adopts the new one.
- **Never rendered.** Tokens never appear in logs, errors, MCP output or agent
  prompts.

Earlier releases kept tokens in the OS keyring. Those entries are not read or
migrated: run `ghostlink login` once after upgrading. Deleting
`credentials.json` signs out new processes; a running MCP server keeps its
cached token until it next needs to renew, so restart the MCP host.


## Runtime configuration and authorization

A read composes the production runtime in one fail-fast order: the command line
is parsed and validated first, and only then does `destiny-cli` read the shared
runtime configuration, load the stored access credential, build the system
clock and the hardened HTTPS transport, open the durable SQLite store, and
dispatch the read through the shared `QueryService`. An invalid argument
therefore never reads configuration, a credential or a database, and never
accesses a clock or the network.

The read runtime resolves these settings (see `config.example.env`). Only the
API key must be supplied if it is absent from compiled defaults; paths, version
and TTL resolve automatically. Confidential and broker modes also require the
client ID:

| Variable | Meaning |
| --- | --- |
| `BUNGIE_API_KEY` | Bungie public-client API key |
| `DESTINY_DATABASE_PATH` | Optional override for the OS-default durable SQLite projection path |
| `DESTINY_FRESHNESS_TTL_SECONDS` | Positive freshness TTL in seconds (maximum `2592000`) |

A read also requires an already-authorized account. `destiny-cli` loads a
complete access credential from the [credential file](#credential-file). When no credential is stored,
the read exits with the typed `authorization_required` outcome (exit 10); a
missing or invalid read configuration exits with `configuration_error` (exit
52). The interactive browser authorization flow that writes that credential
is available through `ghostlink` (setup wizard) or `ghostlink login`. Linux can
return via the `ghostlink://` handler; Windows uses terminal paste for this
release. After login, missing Manifest definitions sync automatically; CLI reads
also sync absent catalogs. MCP tools never download a Manifest during a call:
if definitions are missing, run `ghostlink sync-manifest` once. Existing catalogs
can be refreshed with the same command after game patches.

Automated checks are offline: no live Bungie account or credential is used, and
no live read is claimed. The journey in
`crates/destiny-adapters/tests/query_journey.rs` drives the real CLI runner over
the real `QueryService` / `ProfileSyncService` / `BungieProfileGateway` /
`SqliteStore` with a scripted transport and clock; it proves behavior without a
live account and is not a live-success claim.

