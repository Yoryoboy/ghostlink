# Ghostlink

Ghostlink connects AI assistants to your Destiny 2 account. It reads your real
characters, inventory, builds and weekly activities through the official Bungie
API, and can act on them safely: move items between the Vault and characters,
and equip gear with verification.

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

The installer puts **`ghostlink`** (CLI) and **`destiny-mcp`** (AI-agent server)
side by side. Follow its PATH instructions if needed. Run `ghostlink-update` to
update both together.

No Bungie developer account or config file is needed for the official release.
Linux needs a running Secret Service keyring (GNOME Keyring or KWallet with
Secret Service enabled) and `libdbus-1`, normally present on desktop distros.

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
ghostlink search-inventory --type "Hand Cannon" --element solar --sort Range
ghostlink weekly --character hunter
ghostlink equip "Ace of Spades" --character hunter
```

Add `--json` for machine-readable output. Search by perks, mods, armor stats and
more; inspect items, subclasses, the seasonal artifact and saved loadouts. See
the [CLI reference](docs/cli.md) for all commands.

Actions use fresh preflight checks and post-action verification. Bungie can lag
in reflecting a change: an "applied but pending refresh" outcome is not verified
success. Do not blindly retry; run `ghostlink sync` later to check.

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
The CLI and `destiny-mcp` share the OS keyring; MCP needs no separate login and
no tokens pass through the agent.

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
See the [MCP reference](docs/mcp.md) for tools and safety contracts.

### Option B: CLI skill

Agents with shell access can drive the CLI through the bundled
[skill](skills/ghostlink/SKILL.md), which maps intents to commands and sets the
safety rules for `equip`. Install the version-matched skill with:

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

Tokens stay in the OS keyring, not in a config file or agent prompt. The official
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
