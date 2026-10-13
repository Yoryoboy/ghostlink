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
entry. Unsafe/ambiguous handler skips preserve the listed handler files but do
**not** block receipt-owned binary/updater/receipt removal; the result is partial
(exit 41), with completed paths in `removed`. Handler mutation errors, agent
filesystem failures or changed snapshots still retain remaining binaries/receipt. Individual completed removals/edits are **not transactional**;
there is no rollback (including for a failure in the final receipt/CLI pair).

Configuration, databases, credentials, runtime sockets, unknown files, backups,
external overrides and custom paths remain untouched. Reports list default
`HOME/.config/ghostlink` and `HOME/.local/share/ghostlink` locations; actual custom
config/data/runtime paths are not resolved because config is never loaded. XDG
locations remain preserved except exact previewed integration files. A completed
binary uninstall does **not** mean all integrations are gone.

When handler cleanup is skipped, `manual_followups` gives concrete MIME, desktop
entry and record paths (respecting XDG overrides). Select an installed replacement
with `xdg-mime default <other>.desktop x-scheme-handler/ghostlink`, replacing
`<other>.desktop` with its desktop ID; verify using
`xdg-mime query default x-scheme-handler/ghostlink`. Alternatively remove Ghostlink
from scheme lines in the reported `mimeapps.list`, deleting a whole line only if
no foreign associations remain. KDE/other desktops may select through a
higher-priority desktop-specific MIME file or applications `mimeapps.list`;
inspect those if Ghostlink remains selected. Once no longer selected, verify
ownership before manually deleting the listed desktop entry and previous-handler
record. Never delete foreign/modified files merely because they are listed.

JSON emits one result object on stdout for exit 0, or one object on stderr for
nonzero exits. Preview/result fields include `kind: uninstall`, `status`,
`planned_removals`, `removed`, `changed`, `agents` (per-path outcomes), `skipped`,
`handler_actions`, `handler_skips`, `errors`, `manual_followups`, `preserved` and
`preserved_state`; agent execution also includes `retained_temps`. Partial results
include both the report and `error` in that **same object**, never a second object.
Human output renders the complete structured preview/report. JSON or non-TTY
mutation without `--yes` is refused without prompting; `--dry-run` needs no consent.
A preview with handler skips includes `expected_status: "partial"` and `warnings`
in both JSON and human output, while remaining read-only with status `preview`
and exit 0. This predicts preserved handlers, not guaranteed successful binary removal.

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
ghostlink search-inventory 'is:weapon -is:locked' --limit 5
ghostlink search-inventory --filters
ghostlink inspect-item 700000000000000001
ghostlink inspect-item --name "Ace of Spades"
ghostlink subclass --character hunter
ghostlink subclass-options --character hunter --element strand --locked
```

Add the global `--json` flag to select machine output:

```sh
ghostlink --json search-inventory --text gjallar --item-hash 1001
```

`ghostlink search-inventory --character` restricts owned item hits to an exact
character ID or a unique case-insensitive `hunter|titan|warlock` class. Exact IDs
take precedence; unknown selectors return `invalid_request`, and duplicate
classes return `ambiguous_match`, rather than an empty successful search. Without
`--character`, searches retain their existing account-wide scope. MCP
`search_inventory.character_id` remains an exact-ID filter.

```sh
ghostlink search-inventory --character hunter --json
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

A sync never moves the projection back in time. Bungie can serve an older cached
profile for minutes after an action; when the response's `responseMintedTimestamp`
is not newer than the stored one, nothing is published, the local projection
(including acknowledged action effects) is kept, and the command still exits 0
with `publication: "discarded_not_newer"` and "not published: Bungie returned data
that is not newer than the local projection, which was kept". Retry later for
newer data.

JSON reports `kind: "profile_sync"`, `fetched: true`, `publication`
(`"committed"` or `"discarded_not_newer"`), `revision`, `previous_revision`,
`revision_unchanged`, `checked_at` (acquisition Unix seconds), and
`profile_minted_at` (Bungie's minted instant of the stored projection, Unix
seconds with millisecond precision, or null when Bungie sent none). Human output
adds "Bungie minted at". The first publication has null previous revision and comparison.
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

#### DIM-style query

Pass a positional query, quoted for your shell. It can start with `-` and combines
with structured flags using AND. An explicit empty query selects all owned items;
omitting the query and all filters still returns definition candidates only.
`--filters` lists syntax, examples and supported/unsupported filters from the
same registry used for validation, without reading inventory. It prints plain
text even with `--json`.

```sh
ghostlink search-inventory 'is:weapon (is:solar or is:arc) -is:locked'
ghostlink search-inventory 'type:"Hand Cannon" perk:dragonfly' --location vault
ghostlink search-inventory 'stat:Range:>=30 power:>=440' --sort Range
ghostlink search-inventory '-is:locked' --json
ghostlink search-inventory '' --limit 5
```

Precedence is **explicit `and` > `or` > whitespace AND**: `a b or c` means
`a and (b or c)`. Use parentheses to make grouping clear. `-` and `not` negate;
`not:locked` is shorthand for `-is:locked`. Quoted words are phrases, not exact
names; use `exactname:"Ace of Spades"` for normalized exact matching.
Comparisons accept unsigned integers with `=`, `<`, `<=`, `>`, `>=` (no operator
means equality): `power:>=440` / `light:>=440`, `stat:Range:>=30`.
Absent or zero power never matches a power comparison; plain negation such as
`-power:>440` also includes items with no power. Missing stats fail comparisons.

| Query filters | Meaning |
| --- | --- |
| `name:`, `exactname:`, `type:`, `keyword:`, bare words/phrases | Normalized name/type/text matching |
| `perk:`, `perkname:`, `exactperk:`, `perkhash:` | Weapon perk text, exact normalized perk name, or hash |
| `mod:`, `archetype:` | Armor mod or archetype name |
| `hash:`, `character:` | Definition hash; character name/class or identifier |
| `tag:` | Exact stored definition tag, **not** a DIM user annotation |
| `power:` / `light:`, `stat:` | Numeric comparisons |

Supported `is:` values: `locked`, `equipped`, `invault`,
`ininventory`, `inpostmaster`, `weapon`, `armor`, `kineticslot`, `energy`, `power`,
`helmet`, `gauntlets`, `chest`, `leg`, `classitem`, `kinetic`, `arc`, `solar`,
`void`, `stasis`, `strand`, `common`, `rare`, `legendary`, `exotic`, `hunter`,
`titan`, `warlock`. `is:kinetic` means damage; `is:kineticslot` means slot.

Unsupported DIM filters (notes, wishlists, crafting/patterns, duplicates, kill
trackers, loadout membership, base stats and stat arithmetic) fail explicitly;
unknown keys/values and unbalanced groups are errors, even in an OR branch.
Sorting stays in `--sort`, not `sort:` or `sortby:`. Queries are bounded to
1024 bytes, nesting depth 32 and 128 AST nodes. Never silently drop a filter.

#### Structured flags

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
| `--offset <N>` | 0–10000; skip ordered owned hits only |
| `--detail` | Restore full instance hits instead of compact tiles |
| `--sort <Stat>[:asc\|desc]` | Sort owned hits by stat; descending by default |

`--text`, `--type`, `--tag`, and the new definition filters also constrain owned
hits. Without a query or filters, search returns definition candidates only.
Location or equipped filters alone also search owned instances. Owned hits default
to compact tiles: `instance_id`, `item_hash`, `name`, `item_type`, `tier`,
`location`, `equipped`, `locked`, `perk_names`; optional `element`/`power`,
`quantity` when greater than one, nonempty `mods`, and only stats referenced by
query/flag filters or sort. Human output is one line per hit. `--detail` restores
the previous full hit shape; candidates and inspection are unchanged.

Pagination happens over owned matches in the existing order. JSON includes
`next_offset` only when more hits exist and that offset is valid (at most 10000).
Human output prints `More results: rerun with --offset N`. Reuse the same query,
flags, sort and limit; definition candidates are not paginated.

```sh
ghostlink search-inventory 'is:armor' --limit 5 --json
ghostlink search-inventory 'is:armor' --limit 5 --offset 5 --json
ghostlink search-inventory 'is:armor' --detail --json
```

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
Compact search hits include only filter/sort-referenced stats; use `--detail`
or inspect the chosen item for all named stats.
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

`ghostlink loadouts [<slot-or-name>] [--character <id|class>] [--detail]` shows saved loadout
names, weapons, armor, subclass configuration and artifact perks per character.
Omit `--character` for all characters; selection follows the subclass rules above.
The compact view summarizes equipment; `--detail` adds each item's perks and mods.
Requires login and Manifest definitions. Ordinary sync reads saved loadouts cached
with the profile (component 206); a live 206 read is only a legacy fallback when
the cache is unavailable. Run `ghostlink sync-manifest` once after upgrading to
populate loadout names.

Start with `ghostlink --json loadouts`: it returns compact `kind: "loadouts"`
summaries, regardless of `--detail`. Per character, each saved loadout includes
`slot`, `name`, nullable `subclass` (name/element), and `items` (slot/name).
There are no plugs, descriptions or Clarity texts in this discovery output.

Select one saved loadout positionally for full detail:

```sh
ghostlink loadouts 3 --character hunter --json
ghostlink loadouts "PvE" --character hunter --json
```

The detail envelope has `kind: "loadout"`, character ID/class, presentation hashes,
full items/plugs (including subclass configuration and artifact perks), and a
`plug_insights` array. Each plug with text has an `insight` index into that array;
entries contain optional official `description`, community `clarity`, and
`clarity_match`. Full content equality deduplicates entries in deterministic
first-occurrence order, preserving parent-specific variants even for the same
plug hash. Plugs without insights omit the index. Slot/socket gaps, missing
instances (`missing: true`), unknown plugs and verbatim multiline text are retained;
cosmetics and stat trackers remain omitted. JSON matches MCP `get_loadouts` and
`get_loadout` respectively.

Names use the same normalized exact resolution as apply-loadout; unknown or
ambiguous names return typed errors and candidates. Omitted character searches
all characters but must resolve uniquely; specify character if ambiguous.
Without `--json`, a positional selector prints the existing detailed text for
that one loadout. No-selector human output is unchanged: the compact text view
shows subclass/artifact descriptions; `--detail` also shows each item's perks/mods.

### Apply a saved loadout

```sh
ghostlink apply-loadout 3 --character hunter
ghostlink apply-loadout --slot 2 --character hunter
ghostlink apply-loadout --name "PvP" --character hunter
ghostlink apply-loadout "PvE" --character hunter --json
```

`--character` is required: exact owned id or unique case-insensitive class, with
no default. LOADOUT is the one-based slot shown by `ghostlink loadouts` (1–12),
or a normalized exact name within that character (trim/collapse whitespace,
case-insensitive). Use exactly one selector: positional LOADOUT, `--slot N`, or
`--name NAME`; combining selectors is an argument error. `--name` always means a
name, including numeric names. Unknown/ambiguous names return typed errors with available
candidates; choose a slot, never guess. Empty/unavailable slots refuse before action.
Requires login and loaded Manifest definitions. Go to orbit or a social space
first; offline characters are also permitted. A location refusal says
"go to orbit or a social space and retry".

Ordinary sync resolves the saved slot/name from the committed cache: a fresh
cached projection needs zero profile or component-206 reads. Legacy projections
without the cache use one 206 read. Exactly one official EquipLoadout call follows;
no item-by-item provider emulation, post-action request, or automatic retry.
Acknowledged saved items found locally in the Vault or on the target are projected
locally onto the character, as equipped, with same-bucket replacements unequipped.
These are not per-item Bungie confirmations. EquipLoadout does not pull an item
from another character or the Postmaster, and no cross-character preparation is
done: such an item is reported as unresolved (`on_other_character`,
`in_postmaster`) and never projected. Complete persistence retains freshness;
missing or unreachable items, subclasses not held on the target, or rejected local
effects apply the rest and invalidate freshness so the next ordinary read syncs.
The seasonal artifact every saved loadout carries is listed in `skipped` as
`unsupported_bucket` without making the projection partial. Mods/plugs remain
unchanged, including subclass aspects/fragments, matching DIM's Bungie API
limitation. Held subclasses switch locally and unequip the previous subclass;
this loadout-only path does not change single-item equip validation.
JSON (`kind: "apply_loadout"`) shows `character`, one-based `slot`, `name`,
`outcome`, `receipt`, `reason`, and a typed `local_state` object:

```json
{"status":"updated","unresolved_items":[],"skipped":[],"persisted":true}
```

`status` is `updated` (complete projection; may list the artifact skip), `partial`
(unresolved items or skipped items other than the artifact),
`failed` (projection persistence or invalidation failed), or `not_attempted`
(no acknowledged receipt/local projection attempt). Each issue in
`unresolved_items` or `skipped` has `{item: {instance_id, item_hash, bucket, name},
reason}`; hash, bucket and name may be null. Reasons are `not_owned`, `ambiguous`,
`missing_definition`, `definition_unavailable`, `subclass_not_held`,
`unsupported_bucket`, `invalid_projection`, `on_other_character`, `in_postmaster`.
`persisted` is false only when
writing projected effects failed; true can also mean there were no effects.

`applied` is acknowledgement, not observed
game state (exit 0). Refusal/auth/transient/uncertain attempts invalidate freshness
and fail (41). An applied receipt with incomplete local effects or persistence
failure is `local_state_failed` (42), preserving the receipt. A `partial`
projection reports "some loadout items were not applied; see local_state"; a
`failed` one reports "local state could not be updated after the action". Persistence failure also
attempts invalidation and never retries. Nonzero action output is on stderr; startup/argument errors retain the
ordinary error envelope. No provider-lag exit 43. Never blindly retry uncertainty.
MCP `apply_loadout` uses the same service and output.

### Inspect one owned item

Use exactly one of `ghostlink inspect-item <INSTANCE_ID>` or
`ghostlink inspect-item --name "Ace of Spades"`. Name lookup is normalized exact
matching (case/whitespace), never fuzzy. Multiple owned matches return
`ambiguous_match` (exit 31) with compact candidates; choose an `instance_id`.
A missing match returns `item_not_found` (32).

Inspection shows named socketed perks and mods,
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
- `inspect-item` accepts exactly one instance identifier or `--name` exact-name
  selector; missing matches return `item_not_found` (32), ambiguity returns
  compact candidates with `ambiguous_match` (31).
- Failures write a sanitized diagnostic to stderr (human) or one sanitized
  `{"error":{"code":"...","message":"..."}}` object (JSON) and never echo a
  credential, header, raw provider response, or membership ID. Query syntax
  errors are an exception to argument non-echoing: exit 2 (`invalid_request`)
  includes the error kind, byte span, bounded sanitized excerpt and caret.
  Other invalid arguments keep code `invalid_request` and exit 2;
  the message names the argument and reason, for example
  `invalid --limit: must be between 1 and 1000`, without echoing its value.
- Argument and query syntax validation happens before any service call;
  catalog-based name/stat resolution happens during the read. Exit codes follow the stable matrix in
  `odd/tasks/destiny-cli-read-journey.md`.


### Bungie read errors and pacing

Read responses classify body `ErrorCode`, not provider status text:

| Bungie code | CLI code / exit | Next step |
| --- | --- | --- |
| 35, 36, 37, 54, 55, 56, 57, 1672 | `provider_rate_limit` / 20 | Wait for the reported retry time; do not loop |
| 5 | `provider_unavailable` / 21 | Maintenance; try a read later |
| 99, 22, 2111, 2123, 2124 | `reauthorization_required` / 11 | Run `ghostlink login` |
| 2101, 2102 | `configuration_error` / 52 | Fix the missing/invalid API key setup |

Bungie reads and actions share process-wide adaptive pacing (OAuth excluded).
After throttling, pre-request delay is `2^n * 500 ms`; success halves `n`.
The computed cooldown caps at five minutes; a longer positive `ThrottleSeconds`
hint may extend it. Waits above 30 seconds fail before dispatch with
`retry in about N seconds`; a later caller-driven probe can recover. Existing
bounded read retries remain; actions are never automatically retried, and an
action refused before dispatch is definitive (nothing sent). This is not a
cross-process throttle guarantee.

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
committed original Postmaster owner's character inventory.

Planning and execution use ordinary sync: a fresh projection needs no profile
acquisition or Manifest-index capacity fetch. Local advisory capacity counts
character-held instances (including equipped) by definition bucket against the
pinned audited constants. Known full buckets refuse before dispatch; unknown
occupant definitions allow a blind attempt. Bungie is the final validator:
numeric error 1642 means destination full; free a slot and retry after reassessing. No freeing space, redemption, redispatch, reminting or rollback.
Bungie acknowledgement returns `applied` without post-action profile requests
or waits. Acknowledged pulls update the committed character inventory (not equipped),
advance revision and retain freshness and real acquisition time. Incomplete attempts
or persistence failures invalidate freshness; the next ordinary read syncs.

JSON reuses the shared `kind: "instanced_pull"` projection: independent
`disposition`, `receipt`, `dispatch`, `failure` and `plan_id`. There is no
post-action `observations` field. Typed pre-action failures have null receipt.

| Disposition | Exit | Meaning |
| --- | ---: | --- |
| `applied` | 0 | Bungie acknowledged the action; not observed arrival. |
| `uncertain` | 41 | Dispatched attempt without a clear acknowledgement; never success. |
| `refused` | 41 / varies | Received refusal exits 41; pre-action refusals retain category exits. |
| `local_state_failed` | 42 | Local persistence or invalidation failed after dispatch; receipt retained. |

Only applied output goes to stdout; failures go to stderr. Storage failure reports
"local state could not be updated after the action" and fails closed even after
acknowledgement. Pull no longer emits provider-lag exit 43. Planning missing target
is 32, ambiguity 31, missing definition 33; local revision drift/expiry is 30, other
pre-action failures retain category exits. Never blindly retry: sync, inspect and
reassess explicit intent first. Local serialization excludes neither game nor
other-process activity. Anonymous stacks use `pull-stack` below; live validation
and full Postmaster support remain pending.

Pull refusals in human output are actionable sentences, not raw JSON. Definition
version mismatches name snapshot and stored versions and suggest `ghostlink sync`.
JSON refusal codes and fields are unchanged.

## Pull a non-instanced Postmaster stack

```sh
ghostlink pull-stack 123456789 --amount 3 --character 700000000000000001 --json
```

`ITEM_HASH` is canonical uint32 decimal (0–4294967295), `--amount` is
canonical positive int32 decimal (1–2147483647), and `--character` is the
canonical positive int64 decimal Postmaster owner ID (1–9223372036854775807)
or a unique hunter|titan|warlock class (case-insensitive).
All three are required. The hash is positional; `--item-hash HASH` is also accepted,
but cannot be combined with the positional hash. No signs, leading zeros,
whitespace, item names, instance IDs, membership overrides or alternative destinations. Invalid input
fails before authentication, storage or service access.

Requires login with `MoveEquipDestinyItems` and qualified stored Manifest
metadata. One invocation-owned SQLite connection serializes local inventory
publication and definition use, with authenticated membership and ordinary sync
for planning and execution. Fresh committed state needs no profile acquisition or
Manifest capacity fetch. Only pinned audited account-wide Consumables and
Modifications buckets are supported, without Manifest-version/artifact qualification.
Unknown metadata, side effects, instanced items or locally full destinations refuse.
Duplicate anonymous rows remain significant: amount must not exceed the smallest
matching owner Postmaster stack, not their sum. The owner selects the source;
the destination is the qualified normal account-wide ProfileInventory bucket,
not an arbitrary character or Vault.

One consumed plan dispatches at most once. Acknowledgement returns `applied`
(exit 0 / stdout), not observed quantity arrival, without post-action requests or
waits. Acknowledged pulls apply the requested quantity locally once: decrement a
Postmaster stack, fill/split profile inventory stacks, advance revision and retain
freshness. Capacity and the next plan see those updated stacks. Bungie is the final
validator; numeric 1642 is a typed destination-full refusal, unknown codes uncertain.
Incomplete receipts or persistence failures invalidate. No automatic definition
download, recompose, freeing space, reminting, redispatch or rollback.
JSON uses shared `kind: "stack_pull"`, independent `disposition`, `receipt`,
`dispatch`, `failure` and `plan_id`; dispatch includes pre-action quantity totals,
not invented post quantities. There is no post-action `observations` field.
Received refusals remain `refused`; transport uncertainty remains `uncertain`:
both exit 41 on stderr. Persistence or invalidation failure is `local_state_failed` (42),
retaining the receipt and reporting "local state could not be updated after the
action". There is no provider-lag failure or exit 43.
Missing definition is 33, ambiguous definition 31, local revision drift/expiry 30;
other pre-action failures use existing category exits.

Never blindly retry. Sync and re-read `postmaster`, then reassess explicit
hash/amount/owner intent. Acceptance is not an atomic or observed-state guarantee;
local serialization does not cover the game or other processes. Offline tests
are not live-account or full Postmaster readiness claims.

## Bungie action rejections

Every action classifies definitive rejections by exact numeric Bungie code,
never status text. Stable snake_case reasons and actionable messages are listed
once in the [shared reason table](../skills/ghostlink/references/exit-codes.md#definitive-bungie-action-rejections).
Received rejections retain the existing incomplete/refused exit 41, invalidate
freshness for the next action/read, and never retry automatically. Unknown codes
remain uncertain. Earlier acknowledged legs of a move remain applied.

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
requested destination between planning and committed-projection validation, the
shared executor instead reports `verified_noop` (exit 0), with no transfer.
Other changed local locations are refused before dispatch (exit 30).
Equipped-item and capacity restrictions can be provider refusals after dispatch;
the move planner does not apply equip's class/exotic policy.

One invocation explicitly authorizes one item and destination. There are no
retries or rollback. Fully acknowledged steps return `applied` (exit 0), without
post-action refresh/verification. Acknowledged effects update inventory and advance revision while retaining
acquisition time and freshness. Incomplete attempts require refresh. Never blindly retry an uncertain result: run
`ghostlink sync`, inspect the item, and reassess the user's intent first.

Move has its own transport-neutral text/JSON contract: `status`, `item`,
`destination`, ordered `steps`, `steps_completed`, `steps_uncompleted`,
`verification`, `reason`, `local_state`, and `observed`. Completed counts
are provider-confirmed steps, not inferred completions from verification;
uncertain steps remain uncertain. Applied/incomplete results omit observations
rather than labeling pre-action state as fresh. Applied/no-op goes to stdout (0);
other results go to stderr. Incomplete execution exits 41. Move no longer emits
provider-lag verification exits 42/43. Do not retry blindly; read fresh state first. Missing item
exits 32, missing character 2, ambiguity 31; other refusals follow the existing
category exit mapping. Offline tests establish the pipeline, not live account
success.

### Shared action state contract

Move, equip, batch equip, lock, pull, stack pull and apply-loadout share this contract:
Plans from Ghostlink's local projection (synced only when missing, stale or invalidated).
Bungie is the final validator. Acknowledged actions update local state immediately;
rejections and uncertain outcomes mark it for refresh. Never retry an uncertain outcome.
Incomplete effects and persistence failures also require refresh.

Except apply-loadout (the typed object described above), CLI JSON and MCP
structured output report a string `local_state`: `updated` means
acknowledged effects persisted with freshness retained; `unchanged` means no-op or
refusal before dispatch; `refresh_pending` means invalidated state requiring the
next ordinary read to re-sync. If Bungie has nothing newer yet, that read keeps the local state and reports
`freshness: stale`, never `fresh`, until a newer profile is published. This replaces `profile_refresh` and
`local_state_updated`. Human output adds “local state refresh pending” only for
that last value. Compatibility status `verified_noop` means local state (including
Ghostlink's acknowledged actions) already matches; nothing was sent to Bungie.
It is not verification against Bungie. Invalidated projections re-sync before
any no-op decision.

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

The shared service uses ordinary sync, returns `verified_noop` when local state
already matches (nothing sent to Bungie), otherwise dispatches once. Bungie acknowledgement returns `applied`
without post-action verification: accepted requested state, not observed in-game
state. Acknowledged effects persist locally with freshness retained; incomplete
attempts and persistence failures mark the projection for refresh. Refusal, authentication refusal and
transient/uncertain transport remain failures. Never blindly retry; check later
with `ghostlink sync` and `ghostlink inspect-item`, not repeat the action.

JSON includes `status`, `item` (name and instance ID), requested `locked`,
`attempt` (null when not dispatched), sanitized `reason`, `local_state`, and
`observed` (committed local state for a no-op, otherwise null). Exit 0 means
`applied` or `verified_noop`; 41 means incomplete. Local invalidation storage
failure after dispatch fails closed (42), with "local state could not be updated
after the action". Lock/unlock no longer emit provider-lag exits 42/43.
Refusals use category exits (missing item 32, ambiguity 31, stale state 30).
Success goes to stdout; refusals and unverified results go to stderr.

## Equip several items

```sh
ghostlink equip 123456789 234567890 --character hunter --json
```

`equip <ITEM>... --character <class|id>` accepts 1–17 selectors. Exactly one
item retains the existing single-item behavior and output below. Two or more
use combined-state validation of the committed local projection and one batch equip call after necessary
Vault-mediated transfers, paced at least 0.1s apart. Each selector uses the same
ID/name and ambiguity rules as single-item equip.

The entire batch is refused before dispatch for duplicates, repeated equipment
slots, wrong class, non-equippable items, or exotic conflicts (including equipped
items in slots not being replaced). A failed transfer stops the sequence without
equipping. Already-equipped requested items are per-item no-ops; an all-no-op
batch matches local state without dispatch. Only all successful per-item results produce
`applied` (0); failures, omitted `not_reported` results and uncertain/refused
attempts are incomplete (41). Human and JSON output list item, slot, status and
sanitized numeric code. Acknowledged effects persist locally with freshness retained; incomplete attempts require refresh;
storage failure exits 42: "local state could not be updated after the action".
Location refusal says "go to orbit or a social space and retry". Never blindly
retry a partial batch; read state and reassess intent first.

## Equip one item

```sh
ghostlink equip "Ace of Spades" --character hunter
ghostlink equip 123456789 --character 987654321 --json
```

Both item and character are required: use an exact instance id or an unambiguous
item selector, and an exact character id or unique class. There is no default
character or guessed match. The command automatically transfers from the Vault,
or via the Vault from another character, before equipping. Already equipped on
the target according to local state is `verified_noop`, with nothing sent to Bungie.

The shared planner refuses ambiguous/missing selections, an item equipped on
another character, non-equipment, incompatible or unknown class, and unsafe
exotic conflicts or unknown equipment metadata. It does not unequip elsewhere,
free space, or change builds/plugs. Bungie requires the target to be in orbit,
a social space, or offline; there is no activity pre-check, so provider refusal
may occur after a transfer.

One invocation authorizes the intent. Ordinary sync and local validation precede dispatch; full
acknowledgement returns `applied` (exit 0), without post-action refresh or claiming
the item was observed on the target and equipped. Acknowledged effects persist locally with freshness retained and real acquisition
metadata preserved; incomplete attempts require refresh. Partial progress is reported without rollback
or multi-call atomicity. Never blindly retry an uncertain outcome: inspect fresh
state before deciding on another action. Refusal, incomplete execution and
verification failure return non-zero CLI exits.

Equip no longer emits provider-lag verification exits 42/43. Bungie's profile
can lag mutations; accepted steps are not proof of observed in-game state.
Uncertain steps, refusals and partial execution remain failures. Already-satisfied
local state remains `verified_noop` with no dispatch. Never retry blindly.

The CLI JSON and MCP `equip_item` structured content use the same renderer:
`status`, `item`, `character`, `steps`, `reason`, `local_state`, and `observed`.
A failed refresh never presents a stale observation as fresh. Deterministic
offline tests verify this pipeline and parity; they do **not** establish live
account success. Live equip smoke verification remains pending separate authorization.

