# Ghostlink Destiny Workspace

This repository is a Rust workspace for the Destiny application. Dependency
direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-core <- destiny-adapters <- destiny-cli (composition root only)
destiny-core <- destiny-adapters <- destiny-mcp (binary composition root)
destiny-app  <--------------------- destiny-mcp (library contracts + stdio server)
```

The `destiny-mcp` library holds the bounded MCP schemas, the injectable
`InventoryReadService` and `MutationToolService` seams, the RMCP tools-only
server, and the stdio runtime; it never names a concrete adapter. Only the
`destiny-mcp` binary composition root loads the native credential store, the
HTTP profile transport, the SQLite projection store, and the transfer gateway.

The five crates are intentionally minimal.

## Configuration

Configuration is a Bungie public-client API key, public OAuth client ID,
custom-scheme callback URI (`ghostlink://oauth/callback`), and a separately
configured registered callback URI. The two callback values must match exactly,
including case. An optional `BUNGIE_CLIENT_SECRET` selects confidential mode.

The shared read composition roots (the CLI and the MCP server) additionally read
the database path, Manifest definition version, and a positive freshness TTL:

```text
BUNGIE_API_KEY                Bungie API key (also used by the read roots)
DESTINY_DATABASE_PATH         SQLite projection database path
DESTINY_DEFINITION_VERSION    Manifest definition version to persist and compare
DESTINY_FRESHNESS_TTL_SECONDS Positive freshness TTL in seconds (max 2592000)
```

### Configuration file

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

### Confidential and public mode

Without `BUNGIE_CLIENT_SECRET`, Ghostlink uses public mode and requires browser
reauthorization when the access token expires. For persistent sessions, edit
your app in the Bungie Developer Portal, change its OAuth client type to
Confidential, and save the change. Put the issued `BUNGIE_CLIENT_SECRET` only in
the `0600` per-user configuration file above, never in shell exports or agent
sessions. Keep `BUNGIE_PUBLIC_CLIENT_ID` unchanged: when a secret is configured,
that client ID is required for reads as well as login. A present but empty or
invalid secret fails configuration validation; remove the key to use public mode.

Confidential mode enables refresh tokens lasting approximately 90 days, stored
alongside the access credential in the OS keyring rather than in the config
file. Login and automatic-refresh wiring follow in the persistent-session
implementation; configuring the secret alone does not yet enable refresh.

## Destiny CLI (read-only)

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
- Until a catalog is loaded, `search-inventory`, `inspect-item` and `subclass` fail with
  `definitions not loaded; run ghostlink sync-manifest` (CLI and MCP) instead
  of returning an empty result.

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

### Runtime configuration and authorization

A read composes the production runtime in one fail-fast order: the command line
is parsed and validated first, and only then does `destiny-cli` read the shared
runtime configuration, load the stored access credential, build the system
clock and the hardened HTTPS transport, open the durable SQLite store, and
dispatch the read through the shared `QueryService`. An invalid argument
therefore never reads configuration, a credential, a database, a clock, or the
network.

The read runtime always requires these variables (see `config.example.env`);
confidential mode additionally requires the client ID paired with the secret:

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

## destiny-mcp

`destiny-mcp` serves five read tools plus one default-enabled semantic
mutation over the Model Context Protocol stdio transport:

```text
list_characters   list the signed-in account's characters
search_inventory  search Manifest definitions and concrete instances
inspect_item      inspect exactly one concrete instance by instance identifier
get_subclass      show equipped subclass configurations per character
get_subclass_options list available subclass options with unlock status
get_artifact      read live seasonal artifact state and per-character perks
get_loadouts      show saved in-game loadouts and configured equipment per character
weekly_reset      show the current public weekly rotation (same data as the CLI)
weekly            show one character's reliable weekly content and progress
move_item         move one concrete instance to the Vault or to a character
```

Mutation is composed by default in the product binary: its profile gateway and
durable projection are shared with the read path, and one explicit `move_item`
request is the whole intent authorization. There is no preview tool, confirmation
token or value, approval ceremony, or second MCP call. The single call plans the
move, forces a fresh revision-bound preflight, issues only the ordered transfer
steps, refreshes the projection after the action, and verifies the destination;
membership and credentials are resolved internally and are never accepted or
emitted through MCP.

Run it with the shared read configuration present in the environment:

```sh
cargo run -p destiny-mcp
```

The process speaks newline-delimited JSON-RPC on stdout only. Every startup and
runtime failure is a sanitized message written to stderr; nothing else is written
to stdout. A missing stored credential fails closed with `authorization is
required`, a legacy raw-material credential with `reauthorization is required`,
and neither path guesses an expiry or falls back to an environment value.

### Inventory search and inspection

`search_inventory` accepts top-level `perk_names` and `mod_names` arrays in
addition to the existing singular `perk_name` and `mod_name`. Terms combine
with AND, including the singular field, with a shared cap of eight terms per
kind. Each term resolves independently; any resolved name within it matches.
It also accepts `tier` (exact stored tier, case-insensitive), `location`
(`"vault"` or `"character"`), `equipped` (boolean), and `sort`
(`"Stat[:asc|desc]"`, descending by default). Sorting and name resolution follow
the CLI rules above. Both `limit` and `instances.limit` default to 100 and
accept 1–1000. Search output includes `resolved.perk_terms`,
`resolved.mod_terms` and, when sorting, `resolved.sort`.

`get_subclass_options` accepts optional bounded `character` and `element`
(arc|solar|void|stasis|strand|prismatic), but no `locked` input. For example:
`{"character":"hunter","element":"strand"}`. Output matches
`ghostlink --json subclass-options --character hunter --element strand` exactly.
The status caveat and open `can_insert` question above apply to this tool too;
raw `can_insert`/`enabled` flags are included.

`get_subclass` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json subclass` exactly, including socket order and empty slots.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

`get_artifact` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json artifact` exactly, including points and equipped artifact sockets.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

`get_loadouts` accepts an optional bounded `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json loadouts` exactly, including names, equipment, subclass
configuration, artifact perks and retained slot/socket indices. For example:
`{"character": "hunter"}`. Requires login and loaded Manifest definitions;
run `ghostlink sync-manifest` once after upgrading for loadout names.

`inspect_item` returns the same enriched names as CLI inspection:
`perk_names`, `mod_names`, `mods`, `archetype`, `available_perks`,
`available_mods` and `stats`, alongside the definition, instance and metadata.

### Offline stdio integration test

The real child-process stdio test is gated behind the non-default
`offline-fixtures` feature. It spawns a clearly test-only `offline-fixture`
binary that composes deterministic in-memory application test doubles, then
drives complete MCP sessions (initialize, discovery, the three reads, a
protocol failure, a sanitized tool failure, cancellation, and shutdown) and the
one-call mutation journey: one `move_item` request performs semantic planning, a
fresh preflight, the minimum transfer calls, a post-action refresh, and
destination verification with no second call. It also proves a repeated request
fails closed without replay, an already-satisfied destination is a verified
no-op, a changed preflight is a typed stale failure, a refused step is an honest
partial execution, and stdout is protocol-only with no secret sentinel present:

```sh
cargo test -p destiny-mcp --all-features --test stdio
```

The fixture binary is never reachable from the product `destiny-mcp` binary and
requires no credential, network, database, or native keyring.

## Baseline checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo run -p destiny-cli -- --help
cargo run -p destiny-mcp -- --help
```

All baseline commands are offline and require no credentials.
