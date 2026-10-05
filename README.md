# Ghostlink

Ghostlink connects AI assistants to your Destiny 2 account. It reads your real
characters, inventory, builds and weekly activities through the official Bungie
API, and can act on them safely: move items between the Vault and characters,
and equip gear with verification.

Instead of digging through Vaults, filters and perk lists, you ask:

> "Which shotguns do I have with One-Two Punch?"
> "Move The Last Word to my Warlock."
> "What's featured this week?"

Ghostlink ships two front ends over the same core:

- **`ghostlink`** — a command-line tool for humans and scripts.
- **`destiny-mcp`** — a [Model Context Protocol](https://modelcontextprotocol.io)
  server that exposes the same capabilities as tools for AI agents.

## Features

- **Inventory search** by name, type, element, slot, class, power, tier, perks,
  mods, armor archetype and stats, with sorting and typo suggestions.
- **Item inspection** with resolved perks, mods, archetype and stats.
- **Build insight**: equipped subclass, available subclass options, seasonal
  artifact and saved in-game loadouts.
- **Weekly content**: the public weekly rotation and per-character featured
  activities with challenge progress.
- **Safe actions**: move items (MCP) and equip items (CLI and MCP), each with a
  fresh preflight and post-action verification. No blind retries, no guessing.
- **Local cache**: a SQLite projection of your profile plus the Destiny
  Manifest, refreshed on demand.
- **Credentials stay private**: tokens live in the OS keyring; secrets are
  never printed or accepted as tool input.

## Quick start

Requirements: a recent stable Rust toolchain and a
[Bungie developer application](https://www.bungie.net/en/Application) with
OAuth enabled and the callback URL `ghostlink://oauth/callback`.

```sh
# 1. Build
cargo build --release

# 2. Configure (owner-only file, never commit it)
mkdir -p ~/.config/ghostlink
install -m 600 config.example.env ~/.config/ghostlink/config.env
$EDITOR ~/.config/ghostlink/config.env

# 3. Authorize your Bungie account and load game definitions
ghostlink login          # offers to set up the ghostlink:// handler on Linux
ghostlink sync-manifest

# 4. Use it
ghostlink list-characters
ghostlink search-inventory --type "Hand Cannon" --element solar --sort Range
ghostlink weekly --character hunter
ghostlink equip "Ace of Spades" --character hunter
```

Add `--json` to any command for machine-readable output.

### Use it from an AI agent

There are two independent ways to connect an agent. Pick one or both.

Either way, sign in once with `ghostlink login`. The CLI and `destiny-mcp`
share the same configuration file and the same OS keyring entry, so the MCP
server needs no separate authentication step and no secrets pass through the
agent.

#### Option A: MCP server

Register `destiny-mcp` as a stdio MCP server. The commands below assume
`destiny-mcp` is on your `PATH`; otherwise use its absolute path (for example
`/path/to/target/release/destiny-mcp`).

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

#### Option B: CLI skill

Agents with shell access can drive the `ghostlink` CLI through the bundled
[skill](skills/ghostlink/SKILL.md), which maps intents to commands and sets the
safety rules for `equip`. Install it with the
[`skills`](https://github.com/vercel-labs/skills) CLI:

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

## Documentation

| Topic | Where |
| --- | --- |
| Configuration, public vs. confidential mode, authorization | [docs/configuration.md](docs/configuration.md) |
| Every CLI command, filter and output format | [docs/cli.md](docs/cli.md) |
| MCP tools and their contracts | [docs/mcp.md](docs/mcp.md) |
| Linux `ghostlink://` callback handler | [packaging/ghostlink-callback-handler.md](packaging/ghostlink-callback-handler.md) |
| Architecture, workspace layout and checks | [docs/development.md](docs/development.md) |

## Project status

Early development (`0.1.0`). Read features are stable; mutation paths are
verified by deterministic offline tests, while live-account verification is
still in progress. Bungie may lag in reflecting changes, so an action can be
reported as applied but pending refresh — run `ghostlink sync` later to confirm.

## Development

```sh
cargo test --workspace --all-features
```

All checks run offline and need no credentials. See
[docs/development.md](docs/development.md) for the full baseline.

## Contributing

Contributions are welcome. Open an issue to discuss larger changes first, keep
pull requests focused, and run the checks in
[docs/development.md](docs/development.md) before submitting.

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
