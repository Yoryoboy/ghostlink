# Ghostlink CLI reference

Install the `ghostlink` executable from a release; see the
[README](../README.md) for installation and setup. From a checkout, use
`cargo run -p ghostlink --bin ghostlink -- <command>` (the library is still named
`destiny_cli`). See [configuration](configuration.md) for optional overrides.

## Update

```sh
ghostlink update                       # updater defaults to latest stable
ghostlink update --version 0.1.0        # request a specific release version
ghostlink update --json
```

**Linux x86_64 GNU only.** This command validates the official installation and
runs its sibling `ghostlink-update` directly, never an updater found on PATH.
It needs no Bungie configuration, login, database initialization or keyring
access, and does not elevate privileges. Configuration, data, credentials and
agent registrations are not removed. Restart MCP hosts afterward so they load
the new `destiny-mcp` executable.

The supported receipt is cargo-dist **0.32.0**, `cargo-home` layout: products in
`install_prefix/bin`, exactly `ghostlink` and `destiny-mcp`, no libraries or
aliases, and the official GitHub source `Yoryoboy/ghostlink`. The running CLI
must agree with that root. Symlinked paths, ambiguous paths, missing siblings,
other layouts and receipts from other providers are refused. This is a local
ownership check, not cryptographic authentication of the updater or protection
against another local process replacing files after validation.

Receipt lookup follows axoupdater 0.10.0: a Unicode
`AXOUPDATER_CONFIG_WORKING_DIR` variable (even empty or `0`) selects the working
directory; otherwise `AXOUPDATER_CONFIG_PATH` selects a **directory**. Without
these, lookup tries the existing `XDG_CONFIG_HOME/ghostlink` directory, then
`HOME/.config/ghostlink` (OS home fallback if HOME is unset). The first existing
`ghostlink-receipt.json` is selected; invalid receipts do not trigger fallback.
The child is pinned to that receipt. App-name/custom-installer and installer
URL/directory overrides are scrubbed; `AXOUPDATER_GITHUB_TOKEN` remains allowed
and is never printed. Installer PATH modifications are disabled for this update.
Custom receipt lookup overrides do not change where the upstream installer
writes its new receipt; check an overridden receipt before a later update.

Success means **the updater completed**, not that a particular version was
installed: a current installation may need no change. JSON emits one result
object with `kind: update`, `status: updater_completed`, `requested_version`
(null for latest), and `installed_version: null`. Child stdout/stderr are drained
concurrently with at most 4 KiB retained per stream and never echoed, including
on failure; truncation flags appear on success. No atomic paired replacement,
rollback or unconditional integrity verification is promised. A failed updater
may have changed files; inspect the installation before retrying.

| Exit | Code | Meaning / action |
|---|---|---|
| 0 | updater_completed | Restart MCP hosts; check the CLI version if needed. |
| 2 | invalid_request | Check the command or requested version. |
| 40 | unsupported_installation / updater_missing / unsupported_operation | Use the original installation method or official release installer. |
| 41 | updater_failed | Check connectivity, release availability and permissions; files may have changed. |
| 43 | lifecycle_busy | Wait for the other lifecycle command; inspect unsafe lock files manually. |
| 51 | local_storage | Check file ownership and permissions without automatic elevation. |

A persistent `.ghostlink-lifecycle.lock` in the binary directory uses a kernel
lock to serialize lifecycle commands for that installation. Process exit releases
the lock; the file is not automatically deleted, including foreign lock files.
Standalone updater invocations are outside this serialization.

**Windows:** `ghostlink update` returns exit 40 without starting or deferring any
process. Close Ghostlink, stop `destiny-mcp` in MCP hosts, then run the official
`.\ghostlink-update.exe` (optionally `--version <VERSION>`) from its installation
directory in a separate PowerShell session. If absent, use the official Windows
release installer; restart MCP hosts afterward.

For Cargo builds, rebuild/reinstall with Cargo. For package-manager installs,
use that manager. For manual copies, obtain an official release or installer.
A refusal does not claim to have detected which installation method you used.

## Uninstall

```sh
ghostlink uninstall --dry-run        # read-only preview; creates no lock/temp files
ghostlink uninstall                  # full preview, then default-no TTY confirmation
ghostlink uninstall --yes --json     # explicit authorization; never prompts
```

**Linux x86_64 GNU only; no purge.** Stop MCP hosts and finish active login
first. The command does not prove processes are inactive, terminate them, elevate,
load Bungie configuration/dotenv, initialize SQLite or access the keyring.
Windows refuses before lookup or launch: close Ghostlink, stop MCP hosts and active
login, then use the original package manager or inspect the official receipt and
remove only owned binaries/receipt from a separate PowerShell session. Remove
verified integrations manually and preserve configuration, databases and credentials.
Other platforms also refuse safely. Source/manual/unknown receipts require the
original installation method, not an arbitrary target directory.

Uninstall uses the same receipt lookup and product ownership validation as update,
but a missing updater is optional. An unsafe optional updater is preserved and
reported. Only exact known regular files are removed: `destiny-mcp`, optional
`ghostlink-update`, the receipt, and finally `ghostlink`. Shared installation/Cargo
directories and PATH/shell configuration are never recursively removed or edited.
The lifecycle lock is acquired **after consent**, then installation identities and
integration snapshots are rechecked. Execution cannot expand the previewed scope;
files/symlinks are rechecked before each removal. The lock file remains deliberately:
unlinking a live lock inode could split serialization between processes.

Scoped integrations are previewed first. Exact bundled skill content is eligible
for cleanup, but content identity is **not installation provenance**; older,
modified or unknown copies, symlinks and backups remain. Only an OpenCode local
MCP entry whose command is exactly this installation's absolute `destiny-mcp`
path can be edited. Name-only Claude/Codex/Pi registrations always need manual
inspection and removal; these followups alone are not filesystem failures.
URI cleanup requires exact desktop ownership, preserves shared MIME files and
foreign choices, and clears an active Ghostlink selection before deleting its
entry. Unsafe/ambiguous handler skips or handler errors conservatively retain all
binaries and receipt. Agent filesystem failures or changed snapshots also retain
binaries/receipt. Individual completed removals/edits are **not transactional**;
there is no rollback (including for a failure in the final receipt/CLI pair).

Configuration, databases, credentials, runtime sockets, unknown files, backups,
external overrides and custom paths remain untouched. Reports list default
`HOME/.config/ghostlink` and `HOME/.local/share/ghostlink` locations; actual custom
config/data/runtime paths are not resolved because config is never loaded. XDG
locations remain preserved except exact previewed integration files. A completed
binary uninstall does **not** mean all integrations are gone.

JSON emits one result object on stdout for exit 0, or one object on stderr for
nonzero exits. Preview/result fields include `kind: uninstall`, `status`,
`planned_removals`, `removed`, `changed`, `agents` (per-path outcomes), `skipped`,
`handler_actions`, `handler_skips`, `errors`, `manual_followups`, `preserved` and
`preserved_state`; agent execution also includes `retained_temps`. Partial results
include both the report and `error` in that **same object**, never a second object.
Human output renders the complete structured preview/report. JSON or non-TTY
mutation without `--yes` is refused without prompting; `--dry-run` needs no consent.

| Exit | Code / status | Meaning |
|---:|---|---|
| 0 | preview / cancelled / removed_with_manual_followups | Read-only preview, no-consent cancellation, or completed known-file removal; inspect manual followups. |
| 2 | invalid_request / confirmation_required | Invalid syntax (purge is not accepted), or use `--yes` after preview. |
| 40 | unsupported_installation / unsupported_operation | Ownership/layout or platform refused; no cleanup performed. |
| 41 | partial_failure | Snapshot change, unsafe handler skip, or cleanup failure; inspect per-path outcomes, do not blindly retry. Lock metadata may exist even when cleanup did not start. |
| 43 | lifecycle_busy | Concurrent lifecycle command or unsafe lock; never automatically unlink it. |
| 51 | local_storage | Preview/local access failed; no elevation. |

## First-run setup

Run `ghostlink` in an interactive terminal for the first-run wizard, or run
`ghostlink setup` to repeat it at any time. Both stdin and stdout must be TTYs.
Bare `ghostlink` otherwise prints the normal help, including with `--json` or
when setup has already completed. Explicit `setup` without both TTYs, or with
`--json`, returns a typed `invalid_request` error (exit 2), never a prompt.

The wizard offers only detected Claude Code, Codex, OpenCode and Pi agents,
all preselected. Use space to toggle and enter to confirm. Selecting none skips
installation; if no agents are detected, the wizard says so and proceeds to
login. Choose skill and MCP server (recommended), MCP only, or CLI skill only.
Installation uses the same engine and per-agent outcomes described below,
including the manual configuration snippet when OpenCode cannot be merged.

Sign-in defaults to yes and runs the existing `ghostlink login --callback auto`
flow; successful login auto-syncs missing Manifest definitions. A Manifest sync
failure is non-fatal and includes a `ghostlink sync-manifest` hint. Login may be
skipped. Esc or Ctrl+C at a wizard prompt cancels with exit 130 and keeps any
already-completed installations; no completion marker is written on abort.

At the end, the wizard writes `setup-complete` beside the **default** database:
`$XDG_DATA_HOME/ghostlink/` (absolute XDG path), otherwise
`~/.local/share/ghostlink/`, or `%LOCALAPPDATA%\\ghostlink\\` on Windows.
`DESTINY_DATABASE_PATH` does not relocate this marker. Reaching the end writes
the marker even when login was skipped or installation/login reported errors;
those errors retain a non-zero exit and can be retried with `ghostlink setup`.
Failure to write the marker returns a non-zero exit.

For automation, use the explicit non-interactive equivalents:

```sh
ghostlink agents install --agent claude pi       # both skill and MCP
# Or add --skill / --mcp to install only one integration.
ghostlink login                                # callback auto; may use paste
```

## AI agent integrations (non-interactive)

```sh
ghostlink agents status --json
ghostlink agents install --agent claude codex opencode pi
ghostlink agents uninstall --agent pi --mcp
```

Only these four agents are supported. Repeated `--agent` is also accepted;
install/uninstall require an explicit selection. `--skill` or `--mcp` selects
one step; neither flag means both. No wizard or confirmation is involved.
Detection resolves PATH without execution or checks the agent's config directory.
Status inspects the skill and uses `mcp list` for CLI-managed registrations.

The embedded, version-matched skill installs into each agent's skill directory
(Pi uses `~/.agents/skills`). Foreign skills and symlinked paths are refused.
Uninstall requires the `name: ghostlink` marker and preserves unknown files.
MCP install requires an absolute `destiny-mcp` sibling of this `ghostlink` binary
(`destiny-mcp.exe` on Windows); there is no PATH fallback. Claude uses user scope.
CLI registration retries only an explicit `already exists` diagnostic, by
removing the named Ghostlink registration and adding it again. Commands use
argv, captured bounded output, and a 15-second timeout, never a shell.

OpenCode merges only `mcp.ghostlink` into strict JSON, preserving other keys.
Each adapter run backs up the pre-write file to `opencode.json.ghostlink-backup`
once, then writes via a same-directory temporary file and rename. JSONC,
invalid JSON, or incompatible object shapes remain untouched and produce
`manual_step_required` with a snippet. A failed write may leave its temporary
file for recovery. Uninstall removes only `mcp.ghostlink`.

JSON has `kind: "agents"` and an `agents` array with `agent`, `detected`, `skill`
and `mcp`; each selected step has `status`, plus `snippet` or a typed `error`
when applicable. Unselected steps are null. Outcomes are `installed`,
`already_installed`, `removed`, `not_installed`, `manual_step_required`, `failed`.
All agents and steps are attempted independently. Exit 51 (`local_storage`)
means at least one failed step; the complete report is on stderr, otherwise
stdout and exit 0 (including manual steps). Invalid arguments exit 2.
Integration bootstrap is operator-facing; it is not required in the embedded
game-operation skill's command-coverage checklist.

## Login

`ghostlink login` prints the authorization URL and opens a local browser when
available. Callback delivery defaults to `--callback auto`; use
`--callback paste` for terminal paste or `--callback handler` to require the
installed Linux handler explicitly.

### Callback handler

```sh
ghostlink handler status       # inspect registration and known desktop pitfalls
ghostlink handler install      # explicitly install this executable's handler
ghostlink handler uninstall    # remove it and restore the recorded association
```

These commands support global `--json`. Auto uses the handler when `ghostlink://`
already points at this executable. Otherwise, with a local browser, terminal
stdin and stdout, and no `--json`, it shows the exact planned changes and asks
`[y/N]`. Confirmation installs the handler and uses it; declining, unavailable
registration, or a failed setup falls back to terminal paste with a notice on
stderr. Headless auto uses paste. Non-interactive and JSON auto never prompt or
install; they use paste unless the handler is already registered and a local
browser is available. Nothing is installed without confirmation or the explicit
install command.

Handler management is Linux-only; other platforms report `unsupported_operation`
and auto falls back to paste. Status reports contradictory `mimeapps.list`
sections and remembered portal choices with remedies, but does not repair them.
See the [callback handler runbook](../packaging/ghostlink-callback-handler.md)
for installation details, manual fallback, and security limits.

## Read commands

`ghostlink` exposes read commands over the shared query services:

```sh
ghostlink list-characters
ghostlink postmaster --character hunter --json
ghostlink search-inventory --text gjallar --limit 5
ghostlink inspect-item 700000000000000001
ghostlink subclass --character hunter
ghostlink subclass-options --character hunter --element strand --locked
```

Add the global `--json` flag to select machine output:

```sh
ghostlink --json search-inventory --text gjallar --item-hash 1001
```

### Postmaster observations

`ghostlink postmaster [--character ID_OR_CLASS] [--json]` lists committed
Postmaster holdings. Omit the selector for all owned characters, including empty
groups; use an exact ID or a unique case-insensitive class such as `hunter`.
Empty/control-bearing selectors, selectors longer than 256 characters, duplicate
selectors, positional extras and mutation flags are rejected before dispatch.

```sh
ghostlink postmaster
ghostlink postmaster --character hunter --json
ghostlink --json postmaster --character 700000000000000001
```

This read uses ordinary profile freshness synchronization, not an unconditional
live lookup. Refresh failures refuse stale fallback. Text separates instances
and stacks, shows quantities and actual stack runtime buckets, and includes a
revision/time/freshness footer. Missing names fall back to hashes. Instance runtime
buckets are not persisted and are never inferred from definition buckets.

JSON is one stdout object with `kind: "postmaster"`, `metadata` (the existing
`checked_at`, `freshness`, `revision`, `source` fields),
`profile_currencies_coverage` and `characters`. Each character has `character_id`
(string), nullable `class`, and separate `instances` and `stacks` arrays. Instances
have string `instance_id`, numeric `item_hash`/`quantity` and nullable `name`;
stacks have numeric `item_hash`/`bucket_hash`/`quantity` and nullable `name`.
Duplicate rows remain separate, without totals or synthetic IDs. Errors retain
the sanitized stderr/exit contract and leave stdout empty.

Currency coverage is `not_observed`, `unavailable` or `available` and qualifies
only profile currency component 103. Even `available` currencies or an empty
group do not prove Postmaster completeness or live holdings. Anonymous stack
rows have no stable pull selector. This read command never pulls an item.

### Explicit profile sync

Run `ghostlink sync` (or `ghostlink --json sync`) to force authenticated profile
acquisition even when the local projection is fresh. It takes no arguments and
publishes the local projection before reporting success; it makes no game mutation.
Failures use the existing sanitized errors and nonzero exits.

JSON reports `kind: "profile_sync"`, `fetched: true`, `publication: "committed"`,
`revision`, `previous_revision`, `revision_unchanged`, and `checked_at` (acquisition
Unix seconds). The first publication has null previous revision and comparison.
An unchanged revision means repository deduplication, not unchanged game state.
`fetched` means acquisition completed through the production HTTP gateway; it
promises neither origin freshness, no-cache behavior, a request count, nor mutation
verification. Existing read retries remain unchanged.

### Available subclass options

`ghostlink subclass-options [--character ID_OR_CLASS] [--element ELEMENT] [--locked]`
lists supers, abilities, aspects and fragments. Omit `--character` for all
characters, or select an exact id or unique hunter|titan|warlock class.
Elements are arc, solar, void, stasis, strand and prismatic.
`--locked` shows only locked options in text and cannot be combined with `--json`.
JSON always includes all statuses (`unlocked`, `locked`, `unknown`) and raw
`can_insert`/`enabled` flags, plus `reference_character_id` and per-character
`status_reliable`. Each option also includes optional `description`: the official
Manifest tooltip, omitted when unavailable. Text shows it indented beneath its
plug name; `--locked` filters descriptions along with options.

Status is reliable only for the most recently played character's **class**;
other classes report `unknown`, including shared options. With no last-played
reference, every class is unknown. **Open question:** `can_insert` may not match
in-game unlocks even for the reference class (Hunter Strand Whirling Maelstrom
and Threaded Specter need in-game verification). Treat status as provisional.
Requires login, loaded Manifest definitions and a profile sync.

### Inventory filters

Filter Manifest candidates and owned hits by definition attributes and power:

```sh
ghostlink search-inventory --type "Hand Cannon" --element solar --min-power 440
ghostlink search-inventory --slot helmet --class hunter
ghostlink search-inventory --perk Rampage
ghostlink search-inventory --mod "Harmonic Siphon"
ghostlink --json search-inventory --element arc --slot energy
```

Names are case-insensitive:

| Flag | Accepted values |
| --- | --- |
| `--element <NAME>` | kinetic, arc, solar, void, stasis, strand |
| `--slot <NAME>` | kinetic, energy, power, helmet, gauntlets, chest, legs, classitem, other, unknown |
| `--class <NAME>` | titan, hunter, warlock, any, unknown |
| `--min-power <N>` | 0–65535 accepted; Edge of Fate power scale is 10–550 (soft cap 440); instances without power never match |
| `--tier <NAME>` | Exact stored tier, e.g. Exotic; unknown names match nothing |
| `--location <LOCATION>` | vault, character; combines with `--character` using AND |
| `--equipped <BOOL>` | true, false; filters owned instances |
| `--limit <N>` | 1–1000; default 100, bounds definition candidates and instance hits |
| `--sort <Stat>[:asc\|desc]` | Sort owned hits by stat; descending by default |

`--text`, `--type`, `--tag`, and the new definition filters also constrain owned
hits. An unfiltered search returns definition candidates only. Text and JSON
output include element, slot, and class where available. Location or equipped
filters alone also search owned instances.

Repeat `--perk` or `--mod` up to eight times per kind. Every term must match
(AND); within one term, any resolved name matches. For example:

```sh
ghostlink search-inventory --perk "Kill Clip" --perk "Ricochet Rounds" --type "Auto Rifle"
ghostlink search-inventory --type "Hand Cannon" --location vault --sort Range --limit 5
ghostlink search-inventory --tier Exotic --location character
ghostlink search-inventory --equipped true
```

Sorting happens before the limit. Items missing the stat sort last in either
direction; ties use instance identifier order. Ambiguous stat names are rejected.

MCP `search_inventory` accepts `element`, `slot`, `class_type`, and `min_power`
at the top level. Its optional `instances` object also accepts `min_power`;
if both minimums are supplied, the higher one applies.

**After upgrading, run `ghostlink sync-manifest` once** to populate the new
attributes, even if the live Manifest version has not changed.

`--perk NAME` searches owned instances for a socketed weapon perk (traits,
barrels, magazines, intrinsics and similar weapon-perk categories), not armor
mods or cosmetics. Names use the catalog matching rules below; an exact
`Rampage` query does not also match longer names. Unknown plug
definitions never match. Hits include resolved weapon-perk names in socket
order in text, JSON (`perk_names`) and MCP output. MCP accepts the same filter
as `perk_name`.

`--perk` matches only weapons; `--mod NAME` matches only armor (helmet,
gauntlets, chest, legs and class item). Mod matching uses the same catalog
rules. Empty-socket placeholders are excluded. Included armor-mod categories
are `enhancements.v2_*`, `enhancements.universal`, `enhancements.season_*`
and `enhancements.raid_*` (standard, universal, seasonal and raid mods).
Masterworks, ghost mods, armor-tier tuning plugs and cosmetics do not count.
Armor hits show socket-ordered mod names with a `mods:` label in text and a
`mods` array in JSON and MCP output; weapon perks remain separate. MCP accepts
`mod_name`. Existing databases pick up armor mods after the next profile sync
(`ghostlink sync`); no migration is needed.

### Selectable perks and mods

By default, perk and mod filters match only socketed plugs. Opt into socketed
**or available** alternatives with `--include-available` (MCP: `include_available`).

```sh
ghostlink search-inventory --perk "Kill Clip" --include-available
```

Hits show `available perks:` for weapons and `available mods:` for armor;
JSON and MCP expose `available_perks` and `available_mods`. Names are unique,
in socket order, and exclude already socketed names. Unknown definitions never
match or display. Alternatives require a profile refresh (`ghostlink sync`)
after the automatic schema-v10 migration; archetype matching remains socketed-only.

### Name matching rules

`--perk`, `--mod`, `--archetype` and names in `--stat` and `--sort` resolve
against the loaded Manifest catalog **before** searching inventory. Each repeated
term resolves independently. Matching ignores case and trims/collapses whitespace:

1. An exact name wins, without including longer names.
2. Otherwise, a substring selects all distinct matching names of that kind
   (at most 10). A broader query fails and asks for a more specific name.
3. No match is an input error with up to three closest spelling suggestions;
   suggestions are never silently selected.

```sh
ghostlink search-inventory --perk ramp    # selects valid weapon-perk names containing ramp
ghostlink search-inventory --perk rampge  # error: unknown perk name "rampge"; did you mean: Rampage, ...
```

Text reports one `matched perk: Rampage` (or a comma-separated list) per term,
and similarly `matched mod:`. CLI JSON and MCP both include `resolved`, with
flat `perk`, `mod`, `archetype` arrays and a `stats` map from normalized input
to matched names, for example `{"perk":["Rampage"],"stats":{"cla":["Class"]}}`.
`resolved.perk_terms` and `resolved.mod_terms` preserve each term's resolved
alternatives as arrays in input order. Items must carry **any** resolved name
within each term, and **every** term and separate stat condition must hold.
`--sort` uses the same resolution but requires one distinct display name;
JSON/MCP add `resolved.sort` with `name` and `order`, and text reports
`sorted by: Range (desc)`. Catalogs are kind-specific: shaders never become
perk or archetype matches.

### Armor archetypes (`--archetype`)

Search owned armor by its socketed archetype:

```sh
ghostlink search-inventory --archetype Specialist
ghostlink search-inventory --slot helmet --archetype Specialist --stat "Class>=30"
```

| Archetype | Primary stat | Secondary stat |
| --- | --- | --- |
| Brawler | Melee | Health |
| Gunner | Weapons | Grenade |
| Specialist | Class | Weapons |
| Grenadier | Grenade | Super |
| Paragon | Super | — |
| Bulwark | Health | — |

Matching follows the catalog rules above and is armor-only,
using the Manifest category `armor_archetypes` (a shader named Paragon does
not match). Armor hits expose `archetype:` in text and `archetype` in JSON
and MCP output; if multiple archetype plugs exist, the first in socket order
is displayed. MCP accepts the top-level `archetype` field; its closed schema
rejects unknown fields such as `archetype_name`. Archetypes appear after the
next profile sync (`ghostlink sync`); no migration is needed.

### Manifest definitions

Item names, types and tiers, and the activity, milestone and modifier names
used by `weekly-reset`, come from the Destiny 2 Manifest. Missing catalogs sync
automatically after login or on a CLI read. To refresh them explicitly:

```sh
ghostlink sync-manifest          # or: ghostlink --json sync-manifest
```

- The Manifest is public: the command needs only `BUNGIE_API_KEY` and
  `DESTINY_DATABASE_PATH`, never an OAuth login, the keyring, or a client
  secret.
- It downloads the English `DestinyInventoryItemDefinition` table (about 10 MB
  gzip, about 200 MB uncompressed, decoded as a stream) plus the
  `DestinyMilestoneDefinition`, `DestinyActivityDefinition` and
  `DestinyActivityModifierDefinition` tables, and replaces all four stored
  catalogs in one transaction. Records without a display name or marked
  redacted are skipped and counted.
- Re-running it fetches only the small Manifest index; when the live version
  equals the stored one and all four catalogs are populated it reports
  `unchanged` and downloads no Manifest components. Run it again after a Destiny patch.
- After the Manifest succeeds, the same sync path checks Clarity's community
  perk-data version, downloading descriptions only when that version changes.
  The report adds `clarity: updated|unchanged|unavailable`, version, stored and
  skipped entry counts (JSON: a `clarity` object with `status`, `version`,
  `entries_stored`, `entries_skipped`). Clarity failures leave both the published
  Manifest and previous insights intact and never change the Manifest exit code.
  Automatic CLI sync reports Clarity status on stderr.
- Opening the database applies supported schema migrations automatically;
  older binaries can refuse a newer database. After an upgrade that adds catalog
  attributes, run `sync-manifest` to populate them even if the version is unchanged.
- On first CLI read or profile sync, an absent catalog is synced automatically
  once using the same path as `sync-manifest`. The progress notice
  `Definitions not loaded; syncing manifest...` goes to stderr only, including
  with `--json`; stdout JSON shapes and exit codes are unchanged. Sync failures
  remain typed errors, not empty results. Existing catalogs are not automatically
  updated: run `sync-manifest` after a patch or an upgrade needing new catalogs.
- After a successful `ghostlink login`, an absent catalog is also synced. Failure
  does not fail the login; stderr suggests running `ghostlink sync-manifest`.
- MCP does not download definitions during a tool call. An absent catalog returns
  `definitions not loaded; run ghostlink sync-manifest` instead.

### Filter owned items by stats (`--stat`)

```sh
ghostlink search-inventory --slot helmet --stat "Weapons>=30"
ghostlink search-inventory --slot helmet --class hunter --stat "Weapons>=30" --stat "Health>=20"
ghostlink --json search-inventory --stat "Class<=35" --stat "Super=0"
```

Repeat `--stat "<Name><op><N>"` up to eight times; every condition must hold.
Operators are `>=` (at least), `<=` (at most), `>` (strictly greater), `<`
(strictly less), and `=` (equals), with signed integer values. Names use the
catalog matching rules above. Quote comparisons to protect them from shell
redirection. A stat filter alone searches owned items; an item missing that
stat does not match.
Names come from the Manifest, and an unknown name is an input error. If stat
definitions are not loaded, run `ghostlink sync-manifest`.

Stat names are **not unique**: Class and Melee can each refer to multiple
hashes. Any matching hash may satisfy each condition. Hits show named stats
in stable name-then-hash order (unnamed stats are omitted), as `stats: Class
35, Health 20, ...` in text and a `stats` array of `{name, value}` in JSON.
MCP `search_inventory` accepts the same comparisons in its `stats` array.
Stats appear after the next profile sync (the next stale inventory read
refreshes the stored profile projection).

### Equipped subclass configuration

`ghostlink subclass [--character <id|class>]` shows the equipped subclass super,
abilities, aspects and fragments. Omit `--character` for all characters, or use
an exact character id or unique `hunter`, `titan` or `warlock` class
(case-insensitive). Requires login; missing definitions sync automatically.

`ghostlink --json subclass` returns `kind: "subclass"` and a `characters` array.
Each entry has `character_id`, `class` and `subclass` (null if none is equipped).
The subclass contains `instance_id`, `item_hash`, `name`, `element` and ordered
`slots`, with `kind`, `plug_hash`, `name` and `empty`. Slot kinds are `super`,
`aspect`, `fragment`, `grenade`, `melee`, `class_ability`, `movement` and
`transcendence`. Slots include optional `description` (official Manifest tooltip),
omitted when unavailable. Text shows descriptions directly beneath single-plug
rows without repeating the name. Multi-plug rows use one indented named block per
plug, including aspects and fragments; multiline text is preserved. The same
layout applies to artifact, loadout, subclass-option and inspection descriptions.
Empty sockets remain explicit; unlocked options are not included.

On subclass slots, subclass options, artifact slots, loadout plugs and inspect
`plug_details`, optional `clarity` adds community research by
[Clarity](https://d2clarity.com) next to the official `description`. Stored text
is shown verbatim in JSON, including whitespace and newlines; absent insights
are omitted. Exact hashes win; name fallback is restricted to the same semantic
plug kind, and unsupported kinds omit name fallback (exact hashes still work).
Unambiguous same-name, same-enhancement matches have no marker. An enhanced/base
fallback adds `clarity_match: "variant"`; ambiguous differing texts are never
guessed (a parent-item match is preferred when known). Human output places
`Clarity:` beneath the description, or `Clarity (variant):` for a fallback,
with indented text. It follows the same single/multiple-plug layout and
`--locked`/`--detail` visibility rules.

When explaining effects, distinguish the official Bungie tooltip (`description`)
from community research (`clarity`), and cite Clarity when using its numbers.
Never invent values absent from both fields; missing `clarity` does not mean no
effect. Explicitly qualify variant text as an enhanced/base fallback, not exact
values for that variant, and never infer improved numbers.

Official descriptions prefer the item's own valid tooltip. When absent or invalid,
sync joins its displayable sandbox perk effects in reference order, deduplicating
identical text with a blank line between effects. Missing references are ignored;
blank or invalid composed text is omitted (2048-character cap). Sandbox perks are
a required Manifest component: acquisition failure leaves the published cache
unchanged. Run `sync-manifest` after upgrading from schema v13 to backfill the
catalog even when Bungie's version is unchanged.

### Seasonal artifact state

`ghostlink artifact [--character <id|class>]` shows live artifact points
from profile `pointProgression` and each character's equipped artifact and perks
from the freshness-synchronized inventory. Text shows points/cap and, below the
cap, progress toward the next point. Empty perk sockets appear as `(empty)`.
Requires login; missing definitions sync automatically. Live points are not
persisted in SQLite. Character selection follows the subclass rules above.

`ghostlink --json artifact` returns `kind: "artifact"`, nullable `points`
(`points`, `points_cap`, `progress_to_next_level`, `next_level_at`) and `characters`.
Each character has `character_id`, `class` and nullable `equipped` with
`instance_id`, `item_hash`, nullable `name` and ordered `slots`. Each slot has
`plug_hash`, nullable `name` and `empty`. Missing progression or equipped artifact
stays null. Slots also include optional `description` (official Manifest tooltip),
omitted when unavailable; text shows descriptions beneath named non-empty perks.
Legacy artifact tiers and power-bonus fields are not reported.

### Saved in-game loadouts

`ghostlink loadouts [--character <id|class>] [--detail]` shows saved loadout
names, weapons, armor, subclass configuration and artifact perks per character.
Omit `--character` for all characters; selection follows the subclass rules above.
The compact view summarizes equipment; `--detail` adds each item's perks and mods.
Requires login and Manifest definitions. Run `ghostlink sync-manifest` once after
upgrading to populate loadout names.

`ghostlink --json loadouts` returns the full `kind: "loadouts"` envelope regardless
of `--detail`. Unused slots are omitted; saved slot and socket indices retain gaps.
Missing inventory instances are explicit (`missing: true`, nullable item fields).
Cosmetics and stat trackers are omitted; unknown plugs remain visible. Each plug
includes optional `description` (official Manifest tooltip), omitted when
unavailable. The compact text view shows subclass/artifact descriptions; `--detail`
also shows descriptions beneath each item's named perks and mods.

### Inspect one owned item

`ghostlink inspect-item <INSTANCE_ID>` shows named socketed perks and mods,
the armor archetype, available perks/mods and named stats where present, using
the same enrichment as search hits. Raw perk component state is separate under
`perk states:`. CLI JSON exposes `perk_names`, `mod_names`, `mods` (the same
names as `mod_names`), `archetype`, `available_perks`, `available_mods` and
`stats` (`{name, value}` entries), alongside the definition and instance state.
Inspection also adds `plug_details`: `{hash, name, role, description?, clarity?, clarity_match?}` entries.
Roles are `intrinsic`, `perk`, `mod`, `available_perk`, `available_mod` and
`archetype`. Every resolved socketed plug with category `intrinsics` appears as
`intrinsic` for weapons and armor, instead of a `perk` detail for that plug.
Other details come from the existing name fields, which remain unchanged.
Repeated equipped plugs are deduplicated by hash/role; available names still
exclude equipped names and are unique by normalized name. Intrinsic details
come first in socket order, then other equipped details in socket order, the
first archetype, and available details in socket order. Missing descriptions
are omitted; an empty `plug_details` list is omitted entirely. Official text
preserves newlines in JSON and appears indented under the corresponding names
in human output. Intrinsics appear on an `intrinsic:` line; a single plug's
description is indented directly beneath it without repeating its name.
Search hits stay compact and do not include descriptions:
inspect an item for detail.

### Weekly reset

```sh
ghostlink weekly-reset           # or: ghostlink --json weekly-reset
ghostlink weekly-reset --date 2026-10-13
```

Requires login. Omit `--date` for the current week. `--date YYYY-MM-DD`
selects the weekly period in effect at the end of that day in the system's
local time zone. In Australia at UTC+10 and later, Tuesday yields the previous
week because the reset is Wednesday locally. The resolved window is shown in
local time; JSON timestamps are RFC 3339 UTC.

The calendar section contains community-derived **predictions**, not confirmed
rotations: featured raids, dungeons, Pantheon Reprise/Encore, and Grandmaster
Vanguard Alert. Dates before each rotation's verified coverage show **no data**
with the reason. Live milestones from Bungie `GetPublicMilestones` are
**observed** data, available for the current week only, with activities and
modifiers; other weeks have no live milestones.

JSON adds `current_week` (whether the selected week is current),
`calendar` (`source: "prediction"`, `start`, `end`, and `rotations`), and
`milestones_source: "observed"`. Each rotation includes `key`, `title`,
`status` (`predicted` or `no_data`) and `activities` (`name`, `hash`), with
`coverage_start` and `coverage_note` for no data. Non-current weeks retain
`milestones: []`.

Live milestone names are resolved against the stored Manifest; a hash with no
stored definition is shown as `unresolved #<hash>` (JSON: `"resolved": false`,
`"name": null`) and is never guessed. Modifier descriptions may contain
`{var:<hash>}` placeholders, which are shown verbatim. Per-character weekly
content and progress is covered by `weekly` below.

### Weekly content for one character

```sh
ghostlink weekly                       # character with the highest light
ghostlink weekly --character hunter    # or titan, warlock, or an exact id
ghostlink weekly --details             # plus modifiers grouped by derived category
ghostlink weekly --debug               # plus hashes, raw entries, manifest, unclassified
ghostlink --json weekly
```

Requires a login (it reads profile components 202 and 204 for one character)
and a loaded Manifest. It shows only what the API supports reliably, each with
the character's challenge progress and raid phases:

- **Grandmaster, Crucible and Quickplay alerts**: activities of type Mission
  (strictly highest light), The Crucible and Vanguard Op that carry a weekly
  challenge, with every reward the API lists. Quickplay weapons are not
  assigned to Fireteam or Arena Ops.
- **Featured raids**: public milestone raids with a weekly challenge, minus
  content whose reward is the always-on "Weekly Reward".
- **Featured dungeons**: the character's dungeons with a weekly challenge,
  under the same exclusion.
- **Weekly Expedition and Override**: reported only when exactly one variant
  is offered.

Omitted on purpose because the API is ambiguous or contradicts the game:
Featured Reprise/Encore (Pantheon), Weekly Reclaim, and lifetime completion
flags. Activities whose definitions are missing are counted, never guessed
(`unclassified_unresolved`).

The human output reads as a dashboard: a `<Class> · Power <light>` header,
then GRANDMASTER, FEATURED RAIDS, FEATURED DUNGEONS, VANGUARD, CRUCIBLE and
SEASONAL, each shown only when it has content. Objectives use Bungie's text
(control characters replaced) with `○`/`✓` and progress; outside `--debug`,
rewards and modifiers are deduplicated by exact name; Quickplay shows one line
per distinct reward pool. Outside `--debug`, unclassified activities end the
output as a warning.

Output levels (`--details` and `--debug` are mutually exclusive, weekly only,
and rejected with `--json`):

- default: what is featured and what to do; no modifiers, and hashes only for
  unresolved entries (`unresolved #<hash>`).
- `--details`: adds modifiers grouped as Surge, Overcharge, Champions and
  Other modifiers. The groups are derived from modifier names, not from the
  API; unnamed modifiers are hidden behind a count line.
- `--debug`: adds the Manifest version, hashes, every entry of the classified
  activities without deduplication, and an UNCLASSIFIED section listing each
  unclassified activity with its reason and source.

`--json` stays the complete contract, including the derived modifier
`category`, Quickplay `pools`, and the `unclassified` list.

### Output contract

- Human success output is line-oriented text with a `revision` / `checked_at` /
  `freshness` metadata footer. Empty and missing outcomes are explicit.
- JSON success output is exactly one object on stdout with a stable `kind`
  (such as `character_list`, `inventory_search`, or `item_inspection`) and a bounded
  `metadata` block (`revision`, `checked_at`, `source`, `freshness`). Definition
  search candidates and concrete instance hits stay in separate, bounded arrays.
- `inspect-item` accepts only a concrete instance identifier; a missing instance
  is the typed `item_not_found` outcome (exit 32), never an empty success.
- Failures write exactly one sanitized line to stderr (human) or one sanitized
  `{"error":{"code":"...","message":"..."}}` object (JSON) and never echo a
  rejected argument value, credential, header, raw provider response, or
  membership ID. Invalid arguments keep code `invalid_request` and exit 2;
  the message names the argument and reason, for example
  `invalid --limit: must be between 1 and 1000`, without echoing its value.
- Validation happens before any service call, so invalid arguments make no
  provider or repository request. Exit codes follow the stable matrix in
  `odd/tasks/destiny-cli-read-journey.md`.


## Pull one Postmaster equipment instance

```sh
ghostlink pull 700000000000000001 --json
```

`ITEM` must be a genuine canonical positive int64 instance ID (1 through
9223372036854775807), obtained from the Postmaster `instances` array. Names,
hashes, leading zeros, anonymous stacks, quantity, destination and owner arguments
are not accepted. Requires login with `MoveEquipDestinyItems` authorization and
qualified stored Manifest definitions; run `ghostlink sync-manifest` explicitly
if missing. Membership comes from authentication, and the destination is the
freshly observed original Postmaster owner's character inventory.

One invocation forces fresh planning and preflight, checks explicit non-destructive
instanced-equipment metadata and target-bucket capacity, then dispatches at most
one action. The pinned audited capacity policy must match the freshly fetched
public Manifest index version and bucket artifact; changed or missing evidence
refuses the proof. No freeing space, redemption, redispatch, reminting or rollback.
After a received receipt, at most three forced observations run, with two seconds
between attempts, stopping at the first valid arrival or integrity/time failure.

JSON reuses the shared `kind: "instanced_pull"` projection: independent
`disposition`, `receipt`, `dispatch`, `failure`, `plan_id` and ordered
`observations`. Each observation retains classification AND failure, publication
metadata and selected target/bucket evidence. A timestamp failure may coexist
with `verified_arrival` without proving success. Confirmed refusal remains
`refused` even with arrival. Typed pre-action failures have null receipt and no
observations; no acknowledgement alone is success.

| Disposition | Exit | Meaning |
| --- | ---: | --- |
| `verified` | 0 | Exact instance observed at the owner's character in the normal destination bucket. |
| `not_yet_observed` | 43 | Acknowledged but arrival not yet observed; not proof of application. |
| `uncertain` | 42 | Unverified or invalid observation evidence. |
| `refused` | varies | Eligibility/preflight refusal or received refusal; 40 by default, auth 11, transient 21, typed infrastructure category exits. |

Only verified output goes to stdout; every other structured outcome goes to
stderr with a nonzero exit. Never blindly retry: sync, inspect the instance and
reassess explicit intent first. One invocation owns local inventory publication
and definition use throughout the pipeline, not game or other-process activity.
Observed arrival is neither causal proof nor an atomic guarantee. Anonymous
stacks use the separate bounded `pull-stack` command below; this is not full
Postmaster support or a live-readiness claim.

## Pull a non-instanced Postmaster stack

```sh
ghostlink pull-stack 123456789 --amount 3 --character 700000000000000001 --json
```

`ITEM_HASH` is canonical uint32 decimal (0–4294967295), `--amount` is
canonical positive int32 decimal (1–2147483647), and `--character` is the
canonical positive int64 decimal Postmaster owner ID (1–9223372036854775807).
All three are required. No signs, leading zeros, whitespace, names, class labels,
instance IDs, membership overrides or alternative destinations. Invalid input
fails before authentication, storage or service access.

Requires login with `MoveEquipDestinyItems` and qualified stored Manifest
metadata. One invocation-owned SQLite connection serializes local inventory
publication and definition use, with authenticated membership, fresh planning
and preflight, and fresh public Manifest index capacity evidence. Only pinned
audited account-wide Consumables and Modifications buckets are supported.
Unknown metadata, side effects, instanced items, capacity or policy drift refuse.
Duplicate anonymous rows remain significant: amount must not exceed the smallest
matching owner Postmaster stack, not their sum. The owner selects the source;
the destination is the qualified normal account-wide ProfileInventory bucket,
not an arbitrary character or Vault.

One consumed plan dispatches at most once; at most three forced observations
follow a received receipt, separated by two seconds. No automatic definition
download, recompose, freeing space, reminting, redispatch or rollback.
JSON uses shared `kind: "stack_pull"`, independent `disposition`, `receipt`,
`dispatch`, `failure`, `plan_id` and ordered `observations`. Text uses the shared
ordered sanitized renderer. Quantity evidence includes pre/post destination,
whole-profile and owner Postmaster totals; null means unknown, never zero.
Only `verified` exits 0 on stdout. Lag/uncertainty exits 42 on stderr; the current
stack service does not emit `not_yet_observed` (reserved mapping 43). Received
refusal stays refused even with arrival: auth 11, transient 21, otherwise 40.
Missing definition is 33, ambiguous definition 31, preflight drift/expiry 30;
other infrastructure failures use existing category exits. No new exit code.

Never blindly retry. Sync and re-read `postmaster` later, then reassess explicit
hash/amount/owner intent. Observed quantities are not causal or atomic proof;
local serialization does not cover the game or other processes. Offline tests
are not live-account or full Postmaster readiness claims.

## Move one item

```sh
ghostlink move "Ace of Spades" --character hunter
ghostlink move 123456789 --vault --json
```

Exactly one destination is required: `--character <id|class>` or `--vault`;
using both or neither exits 2. Exact instance and character IDs win over names
and class labels. Otherwise the item selector and character class must each
resolve uniquely; no defaults or guessed matches are used. Requires login and
loaded Manifest definitions; if missing, run `ghostlink sync-manifest` explicitly.

The CLI uses the same `plan_move` / `execute_move_verified` pipeline as MCP
`move_item`. It transfers character-to-character via the Vault, without
equipping, freeing space, or changing plugs. An item already at the destination
when planning is **refused**: `already in <destination>`, exit **40**
(`unsupported_operation`), not a verified no-op. If the item reaches the
requested destination between planning and mandatory fresh preflight, the
shared executor instead reports `verified_noop` (exit 0), with no transfer.
Other changed preflight locations are refused before dispatch (exit 30).
Equipped-item and capacity restrictions can be provider refusals after dispatch;
the move planner does not apply equip's class/exotic policy.

One invocation explicitly authorizes one item and destination. There are no
retries or rollback. Fully acknowledged steps return `applied` (exit 0), without
post-action refresh/verification. Inventory content, revision and acquisition time
remain unchanged; a durable invalidation flag forces the next ordinary read to sync. Never blindly retry an uncertain result: run
`ghostlink sync`, inspect the item, and reassess the user's intent first.

Move has its own transport-neutral text/JSON contract: `status`, `item`,
`destination`, ordered `steps`, `steps_completed`, `steps_uncompleted`,
`verification`, `reason`, `profile_refresh`, and `observed`. Completed counts
are provider-confirmed steps, not inferred completions from verification;
uncertain steps remain uncertain. Applied/incomplete results omit observations
rather than labeling pre-action state as fresh. Applied/no-op goes to stdout (0);
other results go to stderr. Incomplete execution exits 41. Move no longer emits
provider-lag verification exits 42/43. Do not retry blindly; read fresh state first. Missing item
exits 32, missing character 2, ambiguity 31; other refusals follow the existing
category exit mapping. Offline tests establish the pipeline, not live account
success.

## Lock or unlock one item

```sh
ghostlink lock "Ace of Spades" --json
ghostlink unlock 700000000000000001 --json
```

Each invocation explicitly authorizes the requested lock state of one owned
instance. Item selectors follow move/equip: concrete IDs win, numeric misses
remain exact IDs, and semantic names refuse ambiguity. No character or destination
flag is accepted. Vault items use one owned character for the provider call
(the first in committed projection order); live Vault routing validation remains
pending. Locking does not apply equip class, exotic, or equipped-state policy.

The shared service forces fresh preflight, returns a verified no-op when already
satisfied, otherwise dispatches once. Bungie acknowledgement returns `applied`
without post-action verification: accepted requested state, not observed in-game
state. Dispatched attempts invalidate local freshness; the next ordinary read
forces sync without writing guessed state. Refusal, authentication refusal and
transient/uncertain transport remain failures. Never blindly retry; check later
with `ghostlink sync` and `ghostlink inspect-item`, not repeat the action.

JSON includes `status`, `item` (name and instance ID), requested `locked`,
`attempt` (null when not dispatched), sanitized `reason`, `profile_refresh`, and
`observed` (fresh preflight state for a no-op, otherwise null). Exit 0 means
`applied` or `verified_noop`; 41 means incomplete. Local invalidation storage
failure after dispatch fails closed (42), with "local state could not be updated
after the action". Lock/unlock no longer emit provider-lag exits 42/43.
Refusals use category exits (missing item 32, ambiguity 31, stale state 30).
Success goes to stdout; refusals and unverified results go to stderr.

## Equip one item

```sh
ghostlink equip "Ace of Spades" --character hunter
ghostlink equip 123456789 --character 987654321 --json
```

Both item and character are required: use an exact instance id or an unambiguous
item selector, and an exact character id or unique class. There is no default
character or guessed match. The command automatically transfers from the Vault,
or via the Vault from another character, before equipping. Already equipped on
the target is a verified no-op.

The shared planner refuses ambiguous/missing selections, an item equipped on
another character, non-equipment, incompatible or unknown class, and unsafe
exotic conflicts or unknown equipment metadata. It does not unequip elsewhere,
free space, or change builds/plugs. Bungie requires the target to be in orbit,
a social space, or offline; there is no activity pre-check, so provider refusal
may occur after a transfer.

One invocation authorizes the intent. Fresh preflight precedes dispatch; full
acknowledgement returns `applied` (exit 0), without post-action refresh or claiming
the item was observed on the target and equipped. A durable invalidation flag
forces sync on the next ordinary read, preserving real acquisition metadata. Partial progress is reported without rollback
or multi-call atomicity. Never blindly retry an uncertain outcome: inspect fresh
state before deciding on another action. Refusal, incomplete execution and
verification failure return non-zero CLI exits.

Equip no longer emits provider-lag verification exits 42/43. Bungie's profile
can lag mutations; accepted steps are not proof of observed in-game state.
Uncertain steps, refusals and partial execution remain failures. Already-satisfied
fresh preflight remains `verified_noop` with no dispatch. Never retry blindly.

The CLI JSON and MCP `equip_item` structured content use the same renderer:
`status`, `item`, `character`, `steps`, `reason`, `profile_refresh`, and `observed`.
A failed refresh never presents a stale observation as fresh. Deterministic
offline tests verify this pipeline and parity; they do **not** establish live
account success. Live equip smoke verification remains pending separate authorization.

