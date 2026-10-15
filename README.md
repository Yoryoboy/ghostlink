# Ghostlink

Ghostlink connects AI assistants to your Destiny 2 account. It reads your real
characters, inventory, builds and weekly activities through the official Bungie
API, and can act on them safely: move items between the Vault and characters,
equip gear, and lock or unlock items with acknowledgement-based success.

Instead of digging through Vaults, filters and perk lists, you ask:

> "Which shotguns do I have with One-Two Punch?"
> "Move The Last Word to my Warlock."
> "What's featured this week?"

## Install

**Linux** (x86_64):

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Yoryoboy/ghostlink/releases/latest/download/ghostlink-installer.sh | sh
```

**Windows** (x86_64, PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Yoryoboy/ghostlink/releases/latest/download/ghostlink-installer.ps1 | iex"
```

These commands install the latest stable release. GitHub never treats a
prerelease as "latest", so to install a release candidate use its versioned URL
instead, for example
`https://github.com/Yoryoboy/ghostlink/releases/download/v0.1.0-rc.3/ghostlink-installer.sh`
(or `ghostlink-installer.ps1` on Windows).

The installer puts **`ghostlink`** (CLI) and **`destiny-mcp`** (AI-agent server)
side by side in `$CARGO_HOME/bin` (normally `~/.cargo/bin`) and adds that directory to your shell profile.
The shell you installed from keeps its old `PATH`: open a new terminal (or run
`source ~/.cargo/env` on Linux) before running `ghostlink`. On Linux, run
`ghostlink update` (or `ghostlink update --version 0.1.0`) to invoke the validated
sibling updater. Restart MCP hosts afterward. On Windows, close Ghostlink and
stop its MCP server first, then run `.\ghostlink-update.exe` from the installation
directory in a separate PowerShell session. See [update limitations](docs/cli.md#update).

To remove a validated official Linux installation, stop MCP hosts and active login,
then preview with `ghostlink uninstall --dry-run`. Run `ghostlink uninstall` for
full preview and default-no confirmation, or explicitly authorize with `--yes`.
Configuration, databases, credentials and unknown files remain; there is no purge.
Some integrations require manual followup, and partial cleanup is not rolled back.
Unsafe handler selections preserve handler files, not receipt-owned binaries: removal
ends partial (exit 41), with concrete manual steps. Dry-run warns of this outcome.
Windows self-uninstall safely refuses and provides manual guidance.
See [uninstall safety and outcomes](docs/cli.md#uninstall).

No Bungie developer account or config file is needed for the official release.
No keyring or D-Bus session is needed either, so headless servers work: sign-in
stores tokens in a private plain-text file (see [credential file](docs/configuration.md#credential-file)).

> Release preparation is still in progress: this checkout's official application
> defaults await owner provisioning, and release installation/live sign-in await
> validation. See [configuration](docs/configuration.md) for the current defaults.

## Set up

Run `ghostlink` in an interactive terminal. The first run opens the setup wizard;
run `ghostlink setup` to repeat it any time.

1. Pick detected agents: **Claude Code, Codex, OpenCode or Pi**. Use space to
   toggle selections and enter to confirm.
2. Choose **skill + MCP** (recommended), **MCP only**, or **skill only**.
3. Sign in to Bungie in your browser.

One sign-in serves both the CLI and the MCP server. Access tokens renew
automatically; expect to sign in again after roughly 90 days. On Linux, login
can register the `ghostlink://` handler with your permission to return to the
app. On Windows, paste the final URL back into the terminal; a Windows handler
is coming in a later release. Game definitions (the Manifest) sync automatically
when missing after login or on a CLI read.

## Use it

```sh
ghostlink list-characters
ghostlink postmaster --character hunter --json
ghostlink search-inventory 'is:weapon (is:solar or is:arc) -is:locked' --sort Range
ghostlink search-inventory --filters
ghostlink search-inventory 'type:"Hand Cannon"' --detail --json
ghostlink search-inventory '' --limit 5 --offset 5
ghostlink weekly --character hunter
ghostlink move "Ace of Spades" --vault
ghostlink equip "Ace of Spades" --character hunter
# Equip several items in one batch (one item keeps the single-item path):
ghostlink equip 123456789 234567890 --character hunter --json
ghostlink lock "Ace of Spades" --json
ghostlink unlock "Ace of Spades" --json
ghostlink apply-loadout 3 --character hunter --json
```

Add `--json` for machine-readable output. DIM-style queries combine with flags
using AND; quote the whole query, including leading `-` negation. Search hits are
compact tiles by default; `--detail` restores full hits. Continue owned hits with
`--offset` using the returned `next_offset` (definition candidates are unchanged).
Use `ghostlink inspect-item --name "Ace of Spades"` for normalized exact-name
inspection; ambiguity returns candidates, never a fuzzy choice. See the
[CLI reference](docs/cli.md) for syntax, supported filters and all commands.

`ghostlink postmaster` lists committed Postmaster instances and anonymous stacks,
including empty character groups, with ordinary freshness synchronization. Currency
coverage qualifies currency data only, not Postmaster completeness. This read
never pulls items or exposes synthetic instance IDs. For explicit recovery intent,
use `ghostlink pull <INSTANCE_ID>` or `ghostlink pull-stack <ITEM_HASH> --amount <N>
--character <OWNER_ID>` (MCP `pull_item` / `pull_stack`). Instanced pulls use
ordinary local sync and advisory definition-bucket capacity; known full refuses,
unknown occupancy lets Bungie decide. Stack pulls also use ordinary local sync,
with the unchanged stack-capacity arithmetic and pinned audited bucket limits;
no per-action Manifest capacity fetch or version/artifact qualification.
Acknowledgement returns `applied` (CLI 0 / MCP success),
not observed arrival or quantities. Refused/transport-uncertain dispatched attempts
exit 41; local invalidation storage failure fails closed (42), retaining the receipt
and reporting "local state could not be updated after the action". There are no
post-action profile requests or waits. Acknowledged instanced pulls update local
inventory without equipping; acknowledged stack pulls decrement a Postmaster row
and fill/split profile inventory stacks once. Both advance revision and retain
freshness. Incomplete attempts or persistence failures invalidate.
Never blindly retry.

Move/equip validate the committed local projection and return `applied` (exit 0) when Bungie
acknowledges every step, without post-action verification. Plans from Ghostlink's
local projection (synced only when missing, stale or invalidated). Bungie is the final
validator. Acknowledged actions update local state immediately; rejections and
uncertain outcomes mark it for refresh. Never retry an uncertain outcome.
Incomplete effects and persistence failures also require refresh.
Actions other than apply-loadout report a string `local_state`: `updated`,
`unchanged` (no-op or refusal before dispatch), or `refresh_pending` (the next
ordinary read re-syncs).
`verified_noop` means local state, including Ghostlink's acknowledged actions,
already matches; nothing was sent to Bungie. It is not Bungie verification.

Apply a saved in-game loadout with `ghostlink apply-loadout <slot|name> --character
<class|id>` (MCP `apply_loadout`). Slots are one-based as shown by `loadouts`;
names use normalized exact matching within the selected character, never guesses.
Requires login, Manifest definitions, and orbit, a social space, or an offline
character. Ordinary sync resolves cached loadouts before one EquipLoadout action.
Acknowledgement returns `applied` (0 / MCP success); refusal/auth/transient/uncertain
attempts fail (41), and local invalidation storage failure fails closed (42) with
the receipt retained. No post-action request or retry; complete local persistence
retains freshness, while incomplete effects or failures require refresh.
Its `local_state` is now an object: `status` (`updated`, `partial`, `failed`,
`not_attempted`), `unresolved_items`, `skipped`, and `persisted`. Item effects are
**projected locally**, not individually confirmed by Bungie. EquipLoadout pulls from
the Vault but not from another character or the Postmaster; such items are reported
as unresolved, never projected. See the [next release draft](docs/release-notes/next.md).

Fresh MCP projection reads use four concurrent slots; mutations and profile syncs
remain exclusive, and read-triggered syncs are single-flight. Bungie throttling
uses process-wide adaptive pacing; long waits fail fast with a retry time rather
than holding a tool call indefinitely. Never blindly retry an action.

## Use it from an AI agent

The wizard handles integration for you. The following are **manual alternatives**;
pick one or both. You can also manage the bundled integrations without the wizard:

```sh
ghostlink agents install --agent claude codex opencode pi  # skill + MCP
ghostlink agents status --json
ghostlink agents uninstall --agent pi --mcp
```

Use `--skill` or `--mcp` on install/uninstall to select just one integration.
Either way, sign in once with `ghostlink login` if you skipped wizard sign-in.
The CLI and `destiny-mcp` share one credential file; MCP needs no separate login
and no tokens pass through the agent.

### Option A: MCP server

Register `destiny-mcp` as a stdio MCP server. The commands below assume it is
on your `PATH`; otherwise use its absolute installed path.

```sh
# Claude Code (user scope, available in every project)
claude mcp add --scope user ghostlink -- destiny-mcp

# Codex
codex mcp add ghostlink -- destiny-mcp

# Pi (global ~/.pi/agent/mcp.json; add -l for the current project only)
pi mcp add ghostlink -- destiny-mcp
```

OpenCode: add the server to `~/.config/opencode/opencode.json` (or run
`opencode mcp add` and follow the prompts):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "ghostlink": { "type": "local", "command": ["destiny-mcp"], "enabled": true }
  }
}
```

Other MCP clients usually accept the common `mcpServers` shape:

```json
{
  "mcpServers": {
    "ghostlink": { "command": "destiny-mcp" }
  }
}
```

Restart your agent if needed, then ask the questions at the top of this page.
Use read-only `get_postmaster` with `{}` for all owned characters or
`{"character":"hunter"}` for one unique class (an exact ID also works).
It matches CLI JSON, including empty groups and duplicate anonymous stacks;
these are committed observations, not guaranteed live or complete holdings.
See the [MCP reference](docs/mcp.md#postmaster-observations) for its schema,
currency-coverage limits and the other tools' safety contracts.

### Option B: CLI skill

Agents with shell access can drive the CLI through the bundled
[skill](skills/ghostlink/SKILL.md), which maps intents to commands and sets the
safety rules for `move` and `equip`. Both require explicit item and destination
intent and never blindly retry uncertain outcomes. Move requires exactly one of
`--character` or `--vault`; an item already at that destination during planning
is refused (exit 40). Lock/unlock require explicit item and requested-state intent;
Vault items use one owned character for the provider call. Never blindly retry.
See the [CLI reference](docs/cli.md#lock-or-unlock-one-item). Install the version-matched skill with:

```sh
ghostlink agents install --agent claude codex opencode pi --skill
```

Alternatively, use the [`skills`](https://github.com/vercel-labs/skills) CLI:

```sh
npx skills add Yoryoboy/ghostlink --skill ghostlink -g
```

Or copy or symlink `skills/ghostlink` into the agent's skills directory:

| Agent | Skills directory |
|---|---|
| Claude Code | `~/.claude/skills/` |
| Codex | `~/.codex/skills/` |
| OpenCode | `~/.config/opencode/skills/` |
| Pi | `~/.agents/skills/` |

When Ghostlink MCP tools are available in the session, prefer them over shelling
out to the CLI.

## Privacy and security

Tokens stay in `~/.config/ghostlink/credentials.json` (or `$XDG_CONFIG_HOME/ghostlink/credentials.json`, mode `0600`), never in an
agent prompt. The file is **not encrypted**: any process running as your user can
read it, so keep it out of dotfile repositories and backups you share. The official
[token broker](broker/README.md) only exchanges and refreshes OAuth tokens; it
stores nothing and never sees game API traffic. Game requests go directly to
Bungie.

## For developers

For building from source, architecture and offline checks, see
[development](docs/development.md). Optional `config.env` overrides, public /
broker / direct confidential OAuth modes and database paths are documented in
[configuration](docs/configuration.md). To self-host a token broker, see the
[broker runbook](broker/README.md); ordinary users do not need their own Bungie
application.

Contributions are welcome. Open an issue to discuss larger changes first, keep
pull requests focused, and run the development checks before submitting.

## Credits

Perk insights come from **[Clarity](https://d2clarity.com)**, community-researched
and maintained by the Clarity team, not by Ghostlink. Thanks for sharing that
careful work! Feedback about those insights belongs in
[Clarity's Discord](https://d2clarity.com/discord).

Weekly rotation predictions use **[ShatteredVault](https://shatteredvault.com)**'s
algorithm and lists, verified against historical weekly data from
**[Kyber's Corner](https://kyberscorner.com)**. Thanks for sharing that work!
Ghostlink corrects one dungeon slot: Sundered Doctrine pairs with Spire of the
Watcher instead of Grasp of Avarice, following Kyber's historical data.

Weapon wish lists default to `voltron.txt` from
**[48klocs/dim-wish-list-sources](https://github.com/48klocs/dim-wish-list-sources)**
(MIT License, Copyright (c) 2019 Dave Solomon), which compiles roll
recommendations written by many community curators. Thanks to 48klocs and every
curator for that work! Ghostlink downloads the list at sync time and never
bundles it; the opinions in it belong to their curators, not to Ghostlink. See
[wish-list configuration](docs/configuration.md#wish-lists) to add your own lists
or turn them off.

Official item data comes from Bungie's Destiny 2 API.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

Ghostlink is not affiliated with or endorsed by Bungie. Destiny is a trademark
of Bungie, Inc.
