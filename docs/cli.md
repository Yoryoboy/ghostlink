# Ghostlink CLI reference

The `ghostlink` executable (crate `destiny-cli`). Commands are shown both as
`ghostlink <command>` and, from a checkout, `cargo run -p destiny-cli -- <command>`.
See [configuration](configuration.md) for setup and authorization.

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

`destiny-cli` exposes three read commands over the shared WU-007 query
services:

```sh
cargo run -p destiny-cli -- list-characters
cargo run -p destiny-cli -- search-inventory --text gjallar --limit 5
cargo run -p destiny-cli -- inspect-item 700000000000000001
cargo run -p destiny-cli -- subclass --character hunter
cargo run -p destiny-cli -- subclass-options --character hunter --element strand --locked
```

Add the global `--json` flag to select machine output:

```sh
cargo run -p destiny-cli -- --json search-inventory --text gjallar --item-hash 1001
```

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
`status_reliable`.

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
used by `weekly-reset`, come from the Destiny 2 Manifest, which must be loaded
once before those commands can resolve them:

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
  `unchanged` and downloads nothing. Run it again after a Destiny patch.
- Opening the database with this version migrates it to schema v7; older
  binaries that do not support v7 refuse the database. Run `sync-manifest`
  once after upgrading to populate plug categories for perk-name searches.
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
(case-insensitive). Requires login and `ghostlink sync-manifest`.

`ghostlink --json subclass` returns `kind: "subclass"` and a `characters` array.
Each entry has `character_id`, `class` and `subclass` (null if none is equipped).
The subclass contains `instance_id`, `item_hash`, `name`, `element` and ordered
`slots`, with `kind`, `plug_hash`, `name` and `empty`. Slot kinds are `super`,
`aspect`, `fragment`, `grenade`, `melee`, `class_ability`, `movement` and
`transcendence`. Empty sockets remain explicit; unlocked options are not included.

### Seasonal artifact state

`ghostlink artifact [--character <id|class>]` shows live artifact points
from profile `pointProgression` and each character's equipped artifact and perks
from the freshness-synchronized inventory. Text shows points/cap and, below the
cap, progress toward the next point. Empty perk sockets appear as `(empty)`.
Requires login and `ghostlink sync-manifest`; live points are not persisted in
SQLite. Character selection follows the subclass rules above.

`ghostlink --json artifact` returns `kind: "artifact"`, nullable `points`
(`points`, `points_cap`, `progress_to_next_level`, `next_level_at`) and `characters`.
Each character has `character_id`, `class` and nullable `equipped` with
`instance_id`, `item_hash`, nullable `name` and ordered `slots`. Each slot has
`plug_hash`, nullable `name` and `empty`. Missing progression or equipped artifact
stays null. Legacy artifact tiers and power-bonus fields are not reported.

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
Cosmetics and stat trackers are omitted; unknown plugs remain visible.

### Inspect one owned item

`ghostlink inspect-item <INSTANCE_ID>` shows named socketed perks and mods,
the armor archetype, available perks/mods and named stats where present, using
the same enrichment as search hits. Raw perk component state is separate under
`perk states:`. CLI JSON exposes `perk_names`, `mod_names`, `mods` (the same
names as `mod_names`), `archetype`, `available_perks`, `available_mods` and
`stats` (`{name, value}` entries), alongside the definition and instance state.

### Weekly reset

```sh
ghostlink weekly-reset           # or: ghostlink --json weekly-reset
```

Shows the current public weekly rotation from Bungie `GetPublicMilestones`:
each milestone with its reset window (UTC), its activities, and their
modifiers. Like `sync-manifest`, it needs only `BUNGIE_API_KEY` and
`DESTINY_DATABASE_PATH`, never an OAuth login. Names are resolved against the
stored Manifest; a hash with no stored definition is shown as
`unresolved #<hash>` (JSON: `"resolved": false`, `"name": null`) and is never
guessed. Modifier descriptions may contain `{var:<hash>}` placeholders, which
are shown verbatim. Per-character weekly content and progress is covered by
`weekly` below.

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
  (`character_list`, `inventory_search`, or `item_inspection`) and a bounded
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

One invocation authorizes the intent. Fresh preflight precedes dispatch; every
attempted action requires a profile refresh and verification that the item is
on the target **and equipped**. Partial progress is reported without rollback
or multi-call atomicity. Never blindly retry an uncertain outcome: inspect fresh
state before deciding on another action. Refusal, incomplete execution and
verification failure return non-zero CLI exits.

`applied_pending_provider_refresh` (CLI exit 43) means the provider confirmed
every planned step, but the successful post-action refresh retained the
pre-dispatch revision/content and did not show the target state. Bungie's
profile can lag mutations by minutes; this is applied per provider, **not verified
success**. Do not retry; run `ghostlink sync` later to confirm. A changed profile
that contradicts the target remains `verification_failed` (42). Uncertain
steps, refusals and partial execution keep their existing outcomes.

The CLI JSON and MCP `equip_item` structured content use the same renderer:
`status`, `item`, `character`, `steps`, `reason`, `profile_refresh`, and `observed`.
A failed refresh never presents a stale observation as fresh. Deterministic
offline tests verify this pipeline and parity; they do **not** establish live
account success. Live equip smoke verification remains pending separate authorization.

