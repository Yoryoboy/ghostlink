# Ghostlink MCP server (`destiny-mcp`)

`destiny-mcp` serves bounded read tools plus eight default-enabled semantic
mutations over the Model Context Protocol stdio transport:

```text
get_postmaster    read committed Postmaster instances and anonymous stacks (no pull)
list_characters   list the signed-in account's characters
search_inventory  search Manifest definitions and concrete instances
inspect_item      inspect by instance identifier or normalized exact owned name
get_subclass      show equipped subclass configurations per character
get_subclass_options list available subclass options with unlock status
get_artifact      read live seasonal artifact state and per-character perks
get_loadouts      list compact saved-loadout summaries per character
get_loadout       read one saved loadout's full detail with deduplicated insights
weekly_reset      predict weekly rotations and observe current-week milestones
weekly            show one character's reliable weekly content and progress
move_item         move one concrete instance to the Vault or to a character
equip_item        transfer if necessary, equip on an explicit character, return acceptance
equip_items       equip 2–17 explicitly selected items on one character
set_lock_state    lock or unlock one explicitly selected owned item, return acceptance
pull_item         pull one genuine Postmaster equipment instance to its owner
pull_stack        pull an explicit amount of a non-instanced Postmaster stack
apply_loadout     apply one saved in-game loadout on an explicit character
apply_build       apply a stored build's moves and equips (or plan them with dry_run)
list_builds       list locally stored build definitions
get_build         show one local build, including DIM JSON export
save_build        import or capture equipped/in-game loadout as a local build
delete_build      delete one local build definition
analyze_build     check a local build against synced inventory
```

## Local build tools

Build tools other than `apply_build` never apply equipment or modify DIM/Bungie loadouts. They are
composed in the production server with membership-scoped SQLite storage.

| Tool | Arguments |
|---|---|
| `list_builds` | `{}` |
| `get_build` / `delete_build` | Exactly one `id` or normalized exact `name` |
| `analyze_build` | Exactly one `id`/`name`; optional `character` ID or unique class |
| `save_build` import | `source: "import"`, `input`: DIM JSON or full loadout URL |
| `save_build` equipped | `source: "equipped"`, `character`, `name` |
| `save_build` in-game | `source: "in_game_loadout"`, `character`, exactly one `slot` (1..12) or `loadout_name` |

`analyze_build` shares `ghostlink build check` output and ordinary freshness.
Without a character selector the build class must have one owned character;
otherwise resolve ambiguity with an explicit ID. Results contain item resolution,
location/owner/equipped/class compatibility, counts (no percentage), and every DIM
finding kind. `present: null` with `evaluation: not_evaluated` is not a clean bill
of health. Search exclusion is `search_only`; optimizer-dependent suppression is
not evaluated. The additive `mod_fit` object shares the CLI's bounded DIM
armor-mod assignment across all categories: nullable `invalid_mods`, `unassigned_mods` (after
allowed energy upgrades, duplicates retained), and `needs_upgrades_for_mods`.
Artifice prefers current placement then nonexotics; tuning uses canonical slot
order and DIM's exotic request-count rule. Missing raw tuning membership stays
unknown. General/activity Heap candidates rank by unassigned count, cumulative
lexicographic material cost, then assignment changes. `item_mod_assignments` gives
`{instance_id, mods}` proposals; `upgrade_costs` contains positive `{material_hash,
amount}` energy/artifice costs, not affordability or a provider execution plan.
`uses_seasonal_mods` includes expensive-variant re-fitting; missing re-fit inputs
leave that field/finding null with a reason without erasing independently proven fit.
Migration 32 supplies these material inputs; no new acquisition is added.
Legacy hashes and seasonal costs resolve
like DIM using enabled account/target-character unlocks (missing plug sets mean
no unlocks). Missing/non-pluggable definitions use the deprecated placeholder;
missing placeholder data stays unknown. See [build mod fitting](build-mods.md).
The additive `stats` object matches CLI JSON: six `{stat_hash, as_is, assumed}`
values, `satisfies_constraints`, `needs_upgrades_for_stats`, evaluation and reason.
DIM minima-only checks include assigned/unassigned mods, subclass overrides and
optional runtime Font effects. Maxima and better-stats optimization are not evaluated.
The global upgrade finding combines mod and stat requirements; missing evidence
remains null, never a successful constraint check. `empty_fragment_slots` and `too_many_fragments` now
compare saved fragment overrides with saved aspect capacity, never current aspects.
Only nonzero overrides at existing fragment socket indices count (even explicit
empty plugs). No selected aspects means zero capacity. Missing subclass sockets
or selected aspect definitions keep findings unknown with explicit reasons.
Socket application and optimization remain deferred. No gear is applied. See [scope and DIM evidence](build-mods.md). The additive `equipped_set_bonuses` object reports set hash/name,
`equipped_count`, and nullable `perks` with `sandbox_perk_hash`,
`required_set_count`, name/icon and nullable `active`. It uses build-equipped
resolved armor and current inserted event selectors, not future mod assignment.
`evaluation: partial` and `reasons` preserve unknown contributors/catalogs; counts
are lower bounds, and null activation is not inactive. Native sets precede event
selectors, which require visible sockets and enabled plugs. See
[CLI build checking](cli.md#check-a-build-against-inventory).

Source-inapplicable/unknown fields are rejected before service dispatch. MCP
imports accept content, never server file paths or stdin. Input is bounded to
1 MiB; `dim.gg` fails with `unsupported_operation` and typed reason
`share_link_requires_online_import` (BE-009), without fetching a share.

Local operations use the committed profile membership without synchronization;
run profile sync first if unavailable. Inventory captures require stored
Manifest, exact character ID or unique class, and ordinary profile freshness.
Server exclusion covers conversion and storage; build sync does not reenter
the same guard. List returns `kind: builds`; get/save return `kind: build`,
`summary`, `dim_json` (DIM export string), `omissions`, `unresolved_items`.
Equipped/in-game capture includes `craftedDate` in DIM JSON when component-309
objectives and Manifest styles establish complete crafting information.
`analyze_build` resolves by instance ID first, then nonzero crafted date (DIM's
last match, without hash/owner restrictions). Missing evidence stays unknown;
report capture omissions rather than promising complete socket/artifact data.
Equipped saves capture subclass/artifact `socketOverrides` (not legacy
`artifactUnlocks`) with DIM empty/default rules, positional fragment gating and
membership validation, without unlock/enabled filtering. In-game saves classify
armor mods/fashion by socket category and retain empty overrides, removing only
unset zero hashes. Get/save add `equipped_socket_overrides`; analysis retains
`spec.socket_overrides` but still reports socket application checks as
`not_evaluated`. Capture diagnostics are not stored: later get reports
`source_capture_evidence_not_persisted`. Unknown inputs are not substituted.
Per-capture missing crafting evidence is not separately retained on later get.
Unresolved instances are reported during conversion; stored DIM does not retain
those discarded entries (`unresolved_items: null` on later get). Ambiguity is an
error with candidates; ask for an ID
or slot rather than guessing. ID deletion is idempotent (`deleted: false` when
absent). These shapes match [CLI builds](cli.md#local-builds).

Mutation is composed by default in the product binary: its profile gateway and
durable projection are shared with the read path, and one explicit `move_item`
request is the whole intent authorization. There is no preview tool, confirmation
token or value, approval ceremony, or second MCP call. The single call plans the
move from the committed local projection, uses ordinary sync only when missing,
stale or invalidated, re-resolves and validates the revision-bound plan locally,
and issues only the ordered transfer steps. Bungie is the final validator. It returns `applied` with `isError=false` when every step is acknowledged.
It does not perform post-action verification; acknowledged effects persist locally
with freshness retained (see [Equip one item](cli.md#equip-one-item));
membership and credentials are resolved internally and are never accepted or
emitted through MCP.

`equip_item` accepts exactly `{"item":"Ace of Spades","character":"hunter"}`
(or exact instance/character ids). Both fields are required; unknown fields and
control characters are rejected. One call is the complete intent authorization,
with no separate token or approval ceremony. It uses the same shared equip
pipeline and JSON as `ghostlink equip`; see the [equip safety limits](cli.md#equip-one-item).
### Shared action contract

Move, equip, batch equip, lock, pull, stack pull and apply-loadout use the same
contract: Plans from Ghostlink's local projection (synced only when missing, stale or
invalidated). Bungie is the final validator. Acknowledged actions update local
state immediately; rejections and uncertain outcomes mark it for refresh.
Never retry an uncertain outcome. Incomplete effects or persistence failures
also require refresh.

Except `apply_loadout` (typed object below), their CLI JSON and MCP structured
output share a string `local_state`: `updated` means
acknowledged effects persisted and freshness retained; `unchanged` means no-op
or refusal before dispatch; `refresh_pending` means invalidated state that the
next ordinary read re-syncs from Bungie. If Bungie has nothing newer yet, that read keeps the local state and reports
`freshness: stale`, never `fresh`, until a newer profile is published. Human output adds a short refresh-pending
phrase only for that last value. This replaces `profile_refresh` and
`local_state_updated`. Compatibility status `verified_noop` means local state
(including Ghostlink's acknowledged actions) already matches; nothing was sent
to Bungie. It is not Bungie verification; invalidated projections re-sync before
planning or deciding a no-op.

Both `equip_item` and `move_item` return `applied` on full acknowledgement,
without claiming an observed destination/equipped state. Both dispatch from the
local projection under the ordinary freshness gate. Already-satisfied local
state remains `verified_noop`. Uncertain, refused and partial results remain
errors with no blind retry. Incomplete move `expected_location` is derived from
receipts, not an observation.
`move_item` also has the `ghostlink move` CLI surface.

Move JSON is not currently identical across transports. MCP retains its typed
`effects` (source/destination/item ID/via-vault), flat transfer step evidence,
and variant-specific failure/expected-location fields. CLI JSON instead has
`item` display metadata, labeled `destination`, action-tagged steps, completed
and uncompleted counts, `verification`, `reason`, and `observed`. Both share
acknowledgement semantics and `local_state`, but their top-level status vocabulary
also differs (`verified_no_op` in MCP versus `verified_noop` in CLI).
Unifying this is a schema migration, not a presentation-only change; consumers
must not assume move parity. Pull and stack-pull JSON remain unchanged.

`set_lock_state` accepts exactly `{"item":"Ace of Spades","locked":true}`;
use `false` to unlock. Both fields are required, `locked` must be a boolean, and
unknown fields, controls, and overlong selectors are rejected before I/O.
Item resolution matches move/equip; ambiguous names require an instance ID.
One call is explicit intent for ordinary sync and committed-projection validation
with at most one dispatch. A fresh projection requires no profile acquisition;
Bungie remains the final validator. Acknowledgement returns `applied` (CLI 0), not
observed lock state, with no post-action profile request. The acknowledged lock
state updates the local projection and advances its revision, retaining freshness.
Refused or uncertain attempts and local persistence failures invalidate freshness
so the next ordinary read syncs. Already satisfied according to local state is `verified_noop`, with no
provider mutation. Vault items use one
owned character for the provider call; live validation remains pending.

Lock JSON matches `ghostlink lock` / `ghostlink unlock`: `status`, `item`,
requested `locked`, sanitized `attempt` and `reason`, `local_state`, and
`observed` lock state (only local validation for no-op, otherwise null).
`applied` and `verified_noop` have `isError=false`. Refusals and incomplete
attempts (CLI 41), including auth refusal and transient/uncertain transport,
have `isError=true`. Local persistence failure after acknowledgement keeps the
receipt, invalidates freshness without retrying Bungie, and fails closed
(CLI 42): "local state could not be updated after the action". Provider-lag
verification exits 42/43 are no longer emitted. Never blindly retry; sync later
to confirm. See [Lock or unlock one item](cli.md#lock-or-unlock-one-item).
Fresh projection reads run concurrently on four round-robin read slots with
separate SQLite connections; they do not wait for a mutation. Mutations and
profile syncs retain one exclusive guard. Read-triggered syncs are single-flight:
freshness is rechecked after acquiring the guard. This serialization is local
to the server process, not other processes or in-game activity.

`apply_loadout` accepts `{"character":"hunter","slot":3}` or
`{"character":"hunter","name":"PvE"}`: character is required (exact owned id
or unique case-insensitive class), and exactly one of slot/name is required.
Slots are one-based (1–12) as shown by `get_loadouts`; names use normalized exact
matching within that character. Unknown/ambiguous names return typed errors and
candidates without guessing; empty/unavailable slots refuse. Unknown fields and
invalid selectors are rejected before service I/O. Tool input schemas never use
top-level `oneOf`, `anyOf` or `allOf`; slot/name exclusivity is enforced at runtime
(as is instance-id/name exclusivity for `inspect_item`). Requires login and loaded
Manifest definitions; orbit, a social space, or an offline character is required.
Location refusal says "go to orbit or a social space and retry".

One call uses ordinary sync and cached slot/name resolution, then exactly one
EquipLoadout action. A fresh cached projection needs zero profile/206 reads;
legacy projections without the cache use one 206 read. Acknowledged saved items
found locally in the Vault or on the target are **projected locally** onto the
character, equipped, with same-bucket replacements unequipped. Bungie does not
confirm per-item effects. No cross-character preparation runs before EquipLoadout.
Complete local persistence retains freshness, including subclass switches:
the saved subclass must already be held on the target character and replaces
other equipped subclass-bucket items. Missing items, subclasses not held on the
target, or rejected local effects apply the rest and invalidate freshness;
non-applied receipts and persistence failures also invalidate it. The next read
then syncs. Mods/plugs, including subclass aspects/fragments, remain unchanged
(DIM/Bungie API limitation). Single-item equip validation is unchanged.
JSON matches `ghostlink apply-loadout --json`: `kind: "apply_loadout"`,
`character`, one-based `slot`, `name`, `outcome`, bounded `receipt`, `reason`, and
`local_state` is an object (formerly a string):

```json
{"status":"updated","unresolved_items":[],"skipped":[],"persisted":true}
```

`status` is `updated` for complete projection (the seasonal artifact skip may be
listed), `partial` for unresolved items or other skipped items, `failed` for projection persistence or invalidation failure, or
`not_attempted` without an acknowledged receipt/local projection attempt.
Each issue in `unresolved_items` or `skipped` is
`{item: {instance_id, item_hash, bucket, name}, reason}`; hash, bucket and name
may be null. Reason codes: `not_owned`, `ambiguous`, `missing_definition`,
`definition_unavailable`, `subclass_not_held`, `unsupported_bucket`,
`invalid_projection`, `on_other_character`, `in_postmaster`. EquipLoadout does not
pull from another character or the Postmaster, so such items are reported as
unresolved and never projected. `persisted` is false only when writing projected effects
failed; true can also mean no effects were written.

`applied` is acknowledgement (`isError=false`, CLI 0), not observed state.
Refusal/auth/transient/uncertain attempts are errors (CLI 41). Applied receipts
with incomplete local effects or persistence failure fail closed (CLI 42),
retaining the receipt: `partial` reports "some loadout items were not applied; see
local_state", `failed` reports "local state could not be updated after the action". No post-action request
or retry; process-local loadout attempts are paced at one per second. No global
cross-process throttle guarantee. Never blindly repeat an uncertain action.

Install both binaries from a release and run `ghostlink` for setup (or
`ghostlink login` for sign-in only). Register `destiny-mcp` in your agent using
the wizard or the [manual instructions](../README.md#use-it-from-an-ai-agent).
No config file or environment exports are needed with official release defaults;
CLI and MCP share one [credential file](configuration.md#credential-file) and automatically renew access tokens
in broker or confidential mode. See [configuration](configuration.md) for overrides.

To launch the stdio server directly:

```sh
destiny-mcp
# From a source checkout:
cargo run -p ghostlink --bin destiny-mcp
```

The process speaks newline-delimited JSON-RPC on stdout only. Every startup and
runtime failure is a sanitized message written to stderr; nothing else is written
to stdout. A missing stored credential fails closed with `authorization is
required`, a legacy raw-material credential with `reauthorization is required`,
and neither path guesses an expiry or falls back to an environment value.

## Apply a stored build

`apply_build` takes `{"id":"..."}` or `{"name":"Raid"}` (exactly one), an optional
`character` (owned ID or unique class, as for `analyze_build`) and an optional
`dry_run` boolean. Unknown fields and invalid selectors are rejected before service
I/O; the input schema has no top-level `oneOf`/`anyOf`/`allOf`, and id/name
exclusivity is enforced at runtime. The description is within the 500-byte budget.

It runs the same planner and executor as `ghostlink build apply` and returns the same
JSON (`kind: "build_apply"`, or `build_apply_plan` with `dispatched: 0` for
`dry_run: true`; see [build apply](build-apply.md)). `isError` is true for every
non-zero CLI exit: partial or failed results (41), a failed local write (42) and
refusals (category exits). `result` is `applied`, `no_op`, `partial`, `failed` or
`refused`; each step carries per-item `succeeded`, `failed` or `skipped` outcomes with
typed reasons, and `local_state` is one of `unchanged`, `updated`, `refresh_pending`
or `failed` (with `reason`). Bungie acknowledgements are not re-read and nothing is
retried: never repeat a partial, failed or uncertain apply blindly. Sockets, mods and
subclass are not applied (`not_applied`).

When Bungie acknowledged actions but the projection write failed, the result keeps
its per-step outcomes, reports `local_state: "failed"` and the reason "local state
could not be updated after the action" (CLI 42), and freshness is invalidated so the
next read re-syncs. The tool runs under the same mutation lock as the other
mutations, so concurrent mutations are serialized; projection reads keep using their
own connections and are not blocked, and requests are paced 100 ms apart.

## Bungie read errors and pacing

Body `ErrorCode` decides read failures: 35/36/37/54/55/56/57/1672 return
`provider_rate_limit`; 5 returns `provider_unavailable` (maintenance);
99/22/2111/2123/2124 return `reauthorization_required`; 2101/2102 return
`configuration_error` (missing/invalid API key). Errors are sanitized; status
text alone does not classify them. See [CLI exits](cli.md#bungie-read-errors-and-pacing).

Reads and actions share process-wide adaptive pre-request pacing (OAuth excluded):
`2^n * 500 ms` after throttling, with success halving `n`. Computed cooldown caps
at five minutes; a longer `ThrottleSeconds` hint may extend it. Waits over 30
seconds fail before dispatch with `retry in about N seconds`; a caller-driven
probe after cooldown can recover. Existing bounded read retries remain. Actions
refused before dispatch are definitive, and actions are never automatically
retried. Do not loop or blindly repeat a mutation.

## Bungie action rejections

All action adapters share exact numeric rejection classification; provider
status text never decides. See the [canonical reason table](../skills/ghostlink/references/exit-codes.md#definitive-bungie-action-rejections)
for stable snake_case reasons and user actions. Rejected receipts/steps are
errors (`isError=true`), not uncertain outcomes. They invalidate freshness so
the next action/read syncs, without automatic retry or rollback of earlier
acknowledged transfer legs. Unknown codes remain uncertain. New typed reasons
include sanitized actionable `message` text alongside `reason`.

## Batch equip

`equip_items` accepts exactly `{"character":"hunter","items":["123456789","234567890"]}`
with 2–17 items. Character is an explicit unique class or owned ID; item ID/name
resolution matches CLI `ghostlink equip <ITEM>... --character <class|id>`.
The existing single-item `equip_item` tool is unchanged.

Committed-projection validation (synced only when missing, stale or invalidated,
like `equip_item`) checks the combined equipment state: unique items and
slots, class compatibility, equippability, and at most one exotic weapon and
one exotic armor including equipped slots not replaced. Invalid batches dispatch
nothing. Required transfers use Vault-mediated routing, stop on first failure,
and precede exactly one EquipItems call; actions are paced at 0.1s minimum.
Already-equipped requested items are per-item no-ops; all-no-op batches are
already satisfied according to local state without dispatch. Only every per-item success (or no-op) is MCP success.
Failures, omitted `not_reported` results and envelope refusal/auth/transient/
uncertainty remain errors. Structured output lists item, slot, status and numeric
code, matching CLI JSON. Acknowledged effects persist locally with freshness retained; incomplete attempts require refresh;
storage failure is fail-closed with "local state could not be updated after the
action". Restricted location says "go to orbit or a social space and retry".
Never blindly retry; read state and reassess user intent.

## Postmaster observations

Read-only `get_postmaster` uses the same account scope, ordinary freshness policy
and committed snapshot as `ghostlink postmaster --json`. Omit `character` or use
`null` for all owned characters, including empty groups. A selector resolves an
exact ID first, then a unique case-insensitive class; ambiguity is an error.
Names are optional and do not require a loaded Manifest catalog.

| Input | Contract |
|---|---|
| Arguments | Object; `additionalProperties: false` |
| `character` | Optional string or null; string length 1–512 characters |
| Refused strings | Empty, whitespace-only, control-containing or oversized |
| Other fields | Rejected before service calls; no quantity, pull or membership input |

Example MCP tool call:

```json
{"name":"get_postmaster","arguments":{"character":"hunter"}}
```

`structuredContent` matches the CLI logical JSON exactly:

```json
{
  "kind": "postmaster",
  "metadata": {"checked_at": 10000, "freshness": "fresh", "revision": 2, "source": "profile"},
  "profile_currencies_coverage": "not_observed",
  "characters": [{
    "character_id": "123", "class": "Hunter",
    "instances": [{"instance_id": "456", "item_hash": 1, "name": null, "quantity": 1}],
    "stacks": [{"item_hash": 900, "name": null, "bucket_hash": 215593132, "quantity": 7}]
  }]
}
```

IDs are strings; hashes, quantities, timestamps and revisions are numbers.
Missing names/classes are null. Equal stack rows remain separate; do not sum
potentially overlapping sources. Stack buckets are actual runtime observations;
gear has no inferred bucket. Anonymous stacks have no instance ID, ordinal or
stable action selector. This tool exposes no pull action.

Metadata reports committed observations, not guaranteed live holdings or full
Postmaster coverage. Currency coverage is `not_observed`, `unavailable` or
`available` and qualifies component 103 only, independently of Postmaster coverage.
A failed refresh returns a sanitized categorized MCP error, never stale holdings.
Selector, storage and provider errors retain their shared application categories.

## Pull one Postmaster equipment instance

With explicit user intent, call:

```json
{"name":"pull_item","arguments":{"item":"700000000000000001"}}
```

`item` is the only field, required as a canonical positive int64 string
(1–9223372036854775807, no leading zeros). Unknown fields, numeric JSON IDs,
names, hashes, anonymous stacks, quantity and destination are unsupported.
Invalid input is rejected before account/service work. Use the genuine
`instance_id` from `get_postmaster.instances`, never a synthesized stack selector.

One call resolves authenticated membership and uses ordinary sync and the committed
projection to derive the Postmaster owner and qualify non-destructive equipment.
Fresh projections need no profile acquisition or Manifest-index capacity fetch.
Pinned audited capacity counts character-held instances (including equipped) by
stored definition bucket. A known full bucket refuses locally; unknown occupant
definitions permit a blind attempt with Bungie as final validator. Error code 1642
returns a typed destination-full refusal; provider text alone never does. It shares production gateway/repository clones and the existing
operation guard with publishing reads and sibling mutations, not a separate store.
No inventory clearing, arbitrary destination, preview, rollback or redispatch.

`structuredContent` is the same shared JSON as `ghostlink pull --json`:
`kind: "instanced_pull"`, `disposition`, `plan_id`, `dispatch`, independent
`receipt` and `failure`. Bungie acknowledgement yields `disposition: "applied"`
with `isError=false` (CLI 0), not observed arrival. There is at most one action,
no post-action profile request or wait, and no `observations` field. Acknowledged
pulls update character inventory locally (not equipped), advance revision and retain
freshness. Incomplete attempts invalidate freshness so the next ordinary read syncs. `uncertain` and received `refused` remain
errors (CLI 41). Local persistence or invalidation failure returns `local_state_failed` with
`isError=true` (CLI 42), retaining the receipt and reporting "local state could
not be updated after the action". Pre-action refusals retain category exits.

Never blindly retry. Re-read state and reassess explicit intent first. Local
serialization excludes neither other processes nor game activity; acknowledgement
is not an atomic or observed-state guarantee. Non-instanced stacks use `pull_stack`;
live account validation is still pending.

## Pull a non-instanced Postmaster stack

With explicit hash, amount and owner intent, call:

```json
{"name":"pull_stack","arguments":{"item_hash":"7","amount":"3","character":"2"}}
```

All three fields are required decimal strings: `item_hash` is canonical uint32
(0–4294967295), `amount` positive int32 (1–2147483647), and `character` positive
int64 (1–9223372036854775807). No leading zeros (except hash `"0"`), signs,
whitespace, classes, numeric JSON values or extra fields. Invalid inputs are
rejected before I/O. Use committed stack observations, never synthesize instance
IDs or choose a destination.

Only qualified non-instanced account-wide Consumables/Modifications are supported.
Ordinary sync reuses the committed projection for planning and execution: fresh
state needs no profile acquisition or Manifest capacity fetch. Pinned audited
capacities do not require Manifest-version/artifact qualification; local fullness
refuses, with unchanged unique-stack and capacity arithmetic. Duplicate Postmaster
rows bound the amount by the smallest matching stack, not their sum. The adapter
uses the existing operation guard and production gateway/repository clones.

`structuredContent` is the shared `ghostlink pull-stack --json` shape: `kind:
"stack_pull"`, `disposition`, `plan_id`, `dispatch`, independent `receipt` and
`failure`; dispatch retains pre-action totals, not invented post quantities.
Acknowledgement returns `applied`, with `isError=false` (CLI 0), not observed
quantity arrival. There is no post-action request, wait or `observations` field.
Acknowledged pulls apply the requested quantity locally once: decrement a
Postmaster stack, fill/split profile inventory stacks, advance revision and retain
freshness. Following plans see the updated capacity. Bungie is the final validator;
numeric 1642 is a typed destination-full refusal, unknown codes stay uncertain.
Incomplete receipts or persistence failures invalidate. Transport uncertainty and
received refusals remain errors (CLI 41); persistence or invalidation failure returns
`local_state_failed` (CLI 42 / `isError=true`) with the exact receipt and "local
state could not be updated after the action". Pre-action refusals retain category
exits. There are no provider-lag failures or exit 43. Never blindly retry: re-read
and reassess explicit hash/amount/owner intent. Acceptance is not an atomic or
observed-state guarantee. Live account validation remains pending.

## Inventory search and inspection

Pass optional `query` for the [DIM-style language](cli.md#dim-style-query), ANDed
with structured fields. An explicit empty query selects all owned items.
Quotes are phrases (`exactname:` is exact); explicit `and` binds tighter than
`or`, which binds tighter than whitespace AND. Unknown/unsupported filters fail;
never silently remove them. CLI `ghostlink search-inventory --filters` lists
the registry vocabulary and syntax.

```json
{"name":"search_inventory","arguments":{"query":"is:weapon (is:solar or is:arc) -is:locked","location":"vault","limit":5}}
```

Hits default to compact tiles: `instance_id`, `item_hash`, `name`, `item_type`,
`tier`, `location`, `equipped`, `locked`, `perk_names`; optional element/power,
quantity when greater than one, nonempty `mods`, and only query/flag-filter or
sort-referenced stats. `detail: true` restores the previous full hit shape.
Candidates and inspection are unchanged.

`offset` defaults to 0 and accepts 0–10000, skipping ordered owned hits only.
Continue with returned `next_offset`, preserving query/fields/sort/limit;
`next_offset` is omitted unless more hits exist and the next offset is valid.
Definition candidates are unaffected.

```json
{"name":"search_inventory","arguments":{"query":"is:armor","limit":5,"offset":5,"detail":true}}
```

Invalid queries return MCP error `invalid_query` with `kind`, byte `span`
(`start`, `end`) and `message`; `supported_filters` is included only for
`unknown_filter`. Query text is not echoed. For example, an unknown key returns
`kind: "unknown_filter"`; unsupported DIM notes/wishlists/crafting/loadout
membership or stat arithmetic return explicit errors instead of relaxed results.

`inspect_item` requires exactly one of `instance_id` or `name`, enforced at
runtime without top-level schema combinators. `name` uses normalized exact owned
matching, never fuzzy; ambiguity returns `ambiguous_match` with compact
`candidates`. Ask which instance to inspect, then use its `instance_id`.

```json
{"name":"inspect_item","arguments":{"name":"Ace of Spades"}}
```

`search_inventory` accepts top-level `perk_names` and `mod_names` arrays in
addition to the existing singular `perk_name` and `mod_name`. Terms combine
with AND, including the singular field, with a shared cap of eight terms per
kind. Each term resolves independently; any resolved name within it matches.
It also accepts `tier` (exact stored tier, case-insensitive), `location`
(`"vault"` or `"character"`), `equipped` (boolean), and `sort`
(`"Stat[:asc|desc]"`, descending by default). Sorting and name resolution follow
the [CLI rules](cli.md#name-matching-rules). Both `limit` and `instances.limit` default to 100 and
accept 1–1000. Search output includes `resolved.perk_terms`,
`resolved.mod_terms` and, when sorting, `resolved.sort`.

`get_subclass_options` accepts optional bounded `character` and `element`
(arc|solar|void|stasis|strand|prismatic), but no `locked` input. For example:
`{"character":"hunter","element":"strand"}`. Output matches
`ghostlink --json subclass-options --character hunter --element strand` exactly.
The status caveat and open `can_insert` question in the [CLI reference](cli.md#available-subclass-options) apply to this tool too;
raw `can_insert`/`enabled` flags are included. Each option includes optional
`description` (official Manifest tooltip), omitted when unavailable.

`get_subclass` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json subclass` exactly, including socket order and empty slots.
Each slot includes optional `description` (official Manifest tooltip), omitted
when unavailable. Descriptions prefer the item's own valid text, falling back to
its displayable sandbox perk effects in reference order, deduplicated and joined
with blank lines. Missing references are ignored; blank, overlong (2048 characters)
or invalid composed text is omitted. Sandbox acquisition is required and atomic
with Manifest publication; sync failure preserves the prior cache. Schema v14
invalidates v13 freshness so `sync-manifest` backfills unchanged upstream versions.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

`get_artifact` accepts an optional `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json artifact` exactly, including points and equipped artifact sockets.
Each perk slot includes optional `description` (official Manifest tooltip), omitted
when unavailable.
For example: `{"character": "hunter"}`. Requires login and loaded Manifest definitions.

Call `get_loadouts` first for compact discovery, then `get_loadout` for detail.
Both use ordinary sync and saved loadouts cached with the profile (component 206);
a live 206 read is only a legacy fallback when the cache is unavailable.
`get_loadouts` accepts an optional bounded `character` (exact id or unique class),
with omission selecting all characters. Its structured output matches
`ghostlink --json loadouts` exactly: `kind: "loadouts"`, character ID/class, and
saved loadouts with one-based `slot`, `name`, nullable `subclass` (name/element),
and `items` (equipment slot/name). It contains no plugs, descriptions or Clarity.
For example: `{"character":"hunter"}`.

`get_loadout` requires `loadout`: a saved slot number (1–12), decimal slot string,
or normalized exact name. Optional `character` uses the same selector rules.
Omission searches all characters, but must resolve exactly one loadout; specify
character when ambiguous. Examples: `{"character":"hunter","loadout":3}` or
`{"character":"hunter","loadout":"PvE"}`. Unknown/ambiguous names return typed
`unknown_loadout_name` / `ambiguous_loadout_name` errors and candidates, matching
apply-loadout resolution; unavailable slots return `empty_slot`.

Its structured output matches `ghostlink --json loadouts 3 --character hunter`
exactly: `kind: "loadout"`, character ID/class, slot/name/presentation hashes,
full `items` including subclass configuration and artifact perks, and
`plug_insights`. Each plug retains `plug_hash`, `name`, `subclass_kind`,
`socket_index` and `empty`. Optional `insight` is a zero-based index into the
loadout-level `plug_insights` array, whose entries contain optional `description`,
`clarity`, and `clarity_match`. Entries are deduplicated by full content equality
in deterministic first-occurrence order, not by hash: parent-specific Clarity
variants survive, and identical insights across different hashes share an entry.
Plugs without insight text omit `insight`. Text remains verbatim and multiline.
Cosmetic filtering is unchanged; unknown plugs and missing instances are retained.
Both tools require login and loaded Manifest definitions; run
`ghostlink sync-manifest` once after upgrading for loadout names. The existing
identical text content plus `structuredContent` transport is unchanged.

Subclass slots, subclass options, artifact slots, loadout detail's referenced
`plug_insights` and inspect `plug_details` also include optional `clarity`: community research by
[Clarity](https://d2clarity.com), shown verbatim, including whitespace and
newlines. CLI JSON and MCP expose identical plug fields. Missing insights are
omitted. Exact hashes win; name fallback is restricted to the same semantic
plug kind, and unsupported kinds omit name fallback (exact hashes still work).
Unambiguous same-name, same-enhancement matches have no marker; an enhanced/base
fallback adds `clarity_match: "variant"`. Parent-item matches are preferred when
known, and ambiguous differing texts are never guessed. Official `description`
remains independent and unchanged.

When explaining effects, distinguish the official Bungie tooltip (`description`)
from community research (`clarity`), and cite Clarity when using its numbers.
Never invent values absent from both fields; missing `clarity` does not mean no
effect. Explicitly qualify variant text as an enhanced/base fallback, not exact
values for that variant, and never infer improved numbers.

Definition versions always come from the Manifest stored in SQLite; an old
`DESTINY_DEFINITION_VERSION` line or environment variable is ignored.
MCP never downloads a Manifest inside a tool call (downloads can exceed client
timeouts). With no stored catalog, definition-dependent tools return the typed
`definitions_unavailable` error: `definitions not loaded; run ghostlink sync-manifest`.
Run that CLI command, or `ghostlink login` (which syncs an absent catalog after
successful authorization). The server can observe a subsequent local sync
without restarting.

Armor `inspect_item` results also include the same `armor_model` object as CLI
JSON: investment-derived `base_stats`, `total_base`, plug-derived `masterwork_tier`,
raw `gear_tier`, `effective_gear_tier` (DIM's missing-tier fallback of 0), and
`energy` (capacity, used, unused, type and type hash). Stat entries include `base`,
`current`, assumed `base_masterworked`, and `maximum_value`. Missing data is
`null`, never an invented roll; non-armor results omit the object. Existing
`stats` are unchanged. No extra Manifest acquisition is performed for this model.
`tuning` adds the visible socket index, current tuning plug hash and exact
`tuned_stat_hash` from raw reusable membership (flags do not gate membership).
`reusable_membership_known` is false for legacy/unobserved rows: the stat stays null
with `raw_reusable_plugs_not_projected` until profile resync. Known membership with
no unique directional tuner stays null with `no_unique_directional_tuner`;
a resolved stat has a null reason. `armor_set` adds set hash/name and perk hashes
with required-piece thresholds and optional `name`/`icon` from sandbox definitions;
no active-bonus claims. Migration 30 forces one same-version Manifest refresh.
Missing catalogs retain the set hash
with null name/perks. Subclass results add `fragment_capacity: {"capacity":
number|null}`, derived from inserted aspects; non-subclasses omit it.
See [DIM evidence and deliberate unknowns](armor-model.md).

`inspect_item` returns the same enriched names as CLI inspection:
`perk_names`, `mod_names`, `mods`, `archetype`, `available_perks`,
`available_mods` and `stats`, alongside the definition, instance and metadata.
It also adds `plug_details` entries with `hash`, `name`, `role` and optional
official `description`, community `clarity` and `clarity_match` (multiline text
preserved). Roles are `intrinsic`,
`perk`, `mod`, `available_perk`, `available_mod` and `archetype`. Every resolved
socketed plug with category `intrinsics` appears as `intrinsic` for weapons and
armor, instead of a `perk` detail for that plug. Other details come from the
existing name fields. Repeated equipped plugs are deduplicated by hash/role;
available names exclude equipped names and are unique by normalized name.
Intrinsic entries come first in socket order, then other equipped entries in
socket order, the first archetype, and available entries in socket order.
Missing descriptions and an empty `plug_details` list are omitted. Existing
name arrays are unchanged. `search_inventory` hits do not include descriptions;
call `inspect_item` for that detail.

## Weekly reset

`weekly_reset` accepts an optional `date` in `YYYY-MM-DD` format, for example
`{"date":"2026-10-13"}`. Omit it for the current week. A date selects the
weekly period in effect at the end of that day in **UTC**, unlike the CLI's
local-time rule. Requires login.

The calendar predicts featured raids, dungeons, Pantheon Reprise/Encore, and
Grandmaster Vanguard Alert from a community-derived rotation; these are not
confirmed values. Dates before each rotation's verified coverage return
**no data** with the reason. Live Bungie milestones are observed data for the
current week only.

Structured output matches CLI JSON, including the additive `current_week`
boolean, `calendar` (`source: "prediction"`, RFC 3339 UTC `start` and `end`,
and `rotations`), and `milestones_source: "observed"`. Rotations include `key`,
`title`, `status` (`predicted` or `no_data`) and `activities` (`name`, `hash`),
with `coverage_start` and `coverage_note` for no data. Non-current weeks retain
`milestones: []`.

## Offline stdio integration test

The real child-process stdio test is gated behind the non-default
`offline-fixtures` feature. It spawns a clearly test-only `offline-fixture`
binary that composes deterministic in-memory application test doubles, then
drives complete MCP sessions (initialize, discovery, the original three-read
fixture coverage—not the current tool inventory—a
protocol failure, a sanitized tool failure, cancellation, and shutdown) and the
one-call mutation journey: one `move_item` request performs semantic planning, a
local validation under ordinary sync and the minimum transfer calls, returning acknowledgement without
post-action verification or a second mutation call. It also proves a repeated request
fails closed without replay, an already-satisfied destination is a verified
no-op, a changed committed revision is a typed stale failure, a refused step is an honest
partial execution, and stdout is protocol-only with no secret sentinel present.
Pull scenarios additionally prove canonical validation, deterministic actual-bucket
arrival, receipt/disposition separation, pending/uncertain/refused outcomes and
failure without redispatch. The fixture's separate seeded read store is not proof
of production repository sharing:


```sh
cargo test -p destiny-mcp --all-features --test stdio
```

The fixture binary is never reachable from the product `destiny-mcp` binary and
requires no credential, network, durable database, or credential file.
Equip child-process sessions additionally compare complete CLI/MCP JSON for
applied, no-op, refusal, partial progress and uncertainty, including profiles that
would lag or fail on a subsequent read, with equipped non-equipment neighbors. These
are offline observations, not a live-provider verification claim.
Postmaster child sessions additionally refresh stale synthetic seeds through the
real query service and committed in-memory repository, preserving owned gear,
duplicate material/consumable stacks and runtime buckets. They exercise filtering,
all currency-coverage states, empty groups and refresh refusal with protocol-only,
secret-canary-checked streams. No user database or live account is accessed.

