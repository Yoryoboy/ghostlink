# Build apply: planner, dry-run, executor, MCP tool, sockets, armor mods and fashion (BE-007)

`ghostlink build apply ... --dry-run` prints the plan from the pure equipment planner
and dispatches nothing (profile reads only; no mutation transport is composed).
Without `--dry-run` the same plan is executed (see "Executor" below). The MCP tool
`apply_build` does the same with a `dry_run` argument (see "MCP tool"). Subclass and
artifact socket overrides, armor mods and fashion are applied as plug insertions (see
"Sockets", "Armor mods" and "Fashion"); subclass and artifact items are equipped like
other build items (see "Subclass and artifact equips"). Existing move, equip,
batch equip and in-game apply-loadout commands are unchanged.

## Implemented boundary

- Migration 33 stores `equippingBlock.uniqueLabel` in a STRICT, cascading child
  table, refuses downgrade while populated, and invalidates Manifest freshness
  once on upgrade. The field lives beside existing equipping-block set metadata
  in `ArmorDefinition`. Missing and empty labels remain distinct. Run
  `ghostlink sync-manifest` after upgrading.
- Unique conflicts use the **label**, not Exotic rarity. DIM compares the same
  label on equipped items in different buckets (`item-move-service.ts:535-546`).
- `destiny_core::build_apply::plan_build_items` consumes resolved build items,
  inventory, definitions, characters and explicit capacity inputs. Resolution
  remains `build_analysis`'s responsibility; no same-hash replacements of build
  specifications are introduced.
- Ordered steps describe source replacement equips, Vault-mediated transfers,
  incidental space-making transfers and grouped arrival equips. Skips retain
  specification indices and typed reasons; already-satisfied items are separate.
  Socket overrides, armor mods and fashion become `InsertPlug` steps (see "Sockets",
  "Armor mods" and "Fashion"). What a plan does not apply is listed in `not_applied` with
  a reason: `socket_overrides_evidence_unavailable` (overrides but no socket catalogs)
  and the assignment reason when armor mods cannot be evaluated (for example
  `armor_energy_unavailable`).
- Planning clones its inventory and performs no repository or provider mutation.
  Failed preparation attempts discard their speculative local changes and steps.
  This is planning-time partial progress, **not** remote partial-failure handling.

## DIM evidence and adaptation

Evidence is from DIM commit `29c122e`, paths under `src/app/`:

| Decision | Evidence |
|---|---|
| Display-bucket order, source dequip before item moves, then arrival bulk equip | `loadout-drawer/loadout-apply.ts:355-604` |
| Exclude every resolved requested item from replacement/displacement choices | `loadout-drawer/loadout-apply.ts:417-436` |
| Prefer source character, then Vault, then other characters for replacements | `inventory/item-move-service.ts:216-257` |
| Prefer unconstrained label, matching type, rarity, then power; stable ties | `inventory/item-move-service.ts:1220-1279` |
| Prefer least-recently-played destination, preserve last Vault slot when possible | `inventory/item-move-service.ts:677-741` |
| Space-making candidate ranking | `inventory/item-move-service.ts:1138-1211` |
| Vault occupancy uses corresponding Vault bucket, not equipment-slot capacity | `inventory/stores-helpers.ts:65-66,101-107` |

Ghostlink has no DIM user-tag or manual-move data. Per owner decision, these
criteria are neutral: DIM's untagged item and no manual move. Definition search
`tags` are not DIM user tags and are never used as a substitute.

## BE-007a2 planner completion

| Decision | DIM evidence (`src/app/`) |
|---|---|
| Space-making ranking adds saved in-game loadout membership: displace owner's non-loadout items first, prefer destination characters that have the item in a loadout, then owner-cannot-use, destination-can-use | `inventory/item-move-service.ts:1138-1211`, `loadout/selectors.ts:109-120` |
| Lone dequip per owner happens inline with the move; several are replaced together per owner (replacements moved first, then one grouped equip) | `loadout-drawer/loadout-apply.ts:437-513` |
| Equips are counted before moving: one equip is inline with its move, several use one bulk equip afterwards | `loadout-drawer/loadout-apply.ts:424-433,526-540` |
| `clearWeapons`/`clearArmor`: only buckets present in the build, unequipped non-build and transferable items leave the character; Vault with one slot left prefers another character; reservations keep non-build items from refilling the bucket | `loadout-drawer/loadout-apply.ts:680-692,822-955`, `inventory/item-move-service.ts:775-790` |
| Postmaster items are pulled to their owner before any other move, even on the target character | `loadout-drawer/loadout-apply.ts:395-396`, `inventory/item-move-service.ts:1072-1083` |
| Vault slots are counted per corresponding Vault bucket, from the Vault vendor's accepted-item mapping and that bucket's capacity | `inventory/stores-helpers.ts:65-66,101-107`, `destiny2/d2-buckets.ts:60-66` |

Ghostlink adaptations:

- A Postmaster pull follows the existing pull policy: side effects `Some(true)` and
  unknown (`None`) are typed skips (`postmaster_side_effects`,
  `postmaster_side_effects_unknown`).
- Stackable definitions (`max_stack_size > 1` or non-instanced) are the typed skip
  `stack_unsupported`; stack amounts are not planned. DIM moves amounts
  (`loadout-apply.ts:752-800`). This is a follow-up.
- Character occupancy counts only items of the destination bucket; an occupant
  with no definition or bucket makes occupancy `occupancy_unknown`.
- Saved-loadout membership comes from the local profile projection. When absent it
  is listed in `unknown` as `in_game_loadout_membership`, and every item ranks as
  outside any loadout.
- Vault evidence (`VaultEvidence`: equipment bucket to Vault bucket plus capacity) is
  projected by migration 34 and supplied to the planner by `build apply`; see
  "Vault evidence" below. When it is not stored, `unknown` lists `vault_capacity` and
  Vault-staged moves are skipped as `vault_capacity_unavailable`; nothing is guessed.

## Vault evidence (BE-007a2b)

Migration 34 adds three STRICT tables: `inventory_buckets` (bucket hash, `itemCount`),
`vault_vendors` and `vault_accepted_items` (ordered `acceptedItems` of the Vault
vendor, cascading from the vendor row). Integers are range-checked, downgrade is
refused while any of them holds rows, and upgrade invalidates Manifest freshness once.
Run `ghostlink sync-manifest` after upgrading; until then Vault-staged moves keep
skipping as `vault_capacity_unavailable`.

| Decision | DIM evidence (`src/app/`) |
|---|---|
| Vault bucket correspondence comes from vendor `1037843411` `acceptedItems` | `destiny2/d2-buckets.ts:59-61`, `search/d2-known-values.ts:343` |
| Later entries override earlier ones; a destination of 0 means no Vault bucket; an unknown destination bucket resolves to none | `destiny2/d2-buckets.ts:59-66` |
| Capacity is the Vault bucket's `itemCount` (1300 for the general bucket) | `destiny2/d2-buckets.ts:47`, `inventory/stores-helpers.ts:65-66` |
| A bucket with no Vault bucket has no Vault space | `inventory/stores-helpers.ts:65-66,101-107` |

Manifest cost: two more world requests (13 -> 15). Measured against Manifest
`244213.26.06.29.2000-1-bnet.65864`: `DestinyInventoryBucketDefinition` is 19,171 bytes
(60 records) and `DestinyVendorDefinition` is **38,887,425 bytes** (2,170 records) on
every Manifest refresh. DIM reads the same vendor table, so no cheaper source exists.
The vendor table is streamed and every vendor except the Vault vendor is skipped
without being stored (the Vault record is 2,576 bytes, 14 entries). Ceilings:
buckets 10,000 records / 4 MiB; vendors 20,000 records scanned / 128 MiB, 256 accepted
entries kept. A table without the Vault vendor yields no evidence, not an error.

## Executor (BE-007a2c)

`destiny_app::build_apply_exec::execute_plan` runs the plan's steps in order through
the existing adapters: `TransferGateway` (moves), `EquipGateway` (a lone equip),
`EquipItemsGateway` (a planned grouped equip) and `InstancedPullGateway` (Postmaster
pulls). Optimistic semantics are unchanged: a Bungie acknowledgement is success, the
intended result is applied to the local projection per succeeded step (the same
effects the single actions use), there is no post-action refresh and no retry. Requests
are paced 100 ms apart like batch equip; the adapters own the shared throttle, which
fails before dispatch when a wait would exceed 30 s. The plan is only valid for the
projection revision it was computed from; a changed revision is refused as
`stale_state` before anything is sent.

Partial failure follows DIM, adapted:

| Decision | DIM evidence (`src/app/`) |
|---|---|
| A failed step never aborts the plan; independent steps keep running | `loadout-drawer/loadout-apply.ts:527-578` (per-item try/catch inside the move loop) |
| The bulk equip takes only the items that arrived; failed or skipped ones are dropped | `loadout-drawer/loadout-apply.ts:580-593` |
| A failed grouped dequip (replacement move or equip) fails every item of that group | `loadout-drawer/loadout-apply.ts:471-523` |
| Bulk-equip partial failure is per item, by Bungie error code | `loadout-drawer/loadout-apply.ts:595-611` |
| A lone dequip, Postmaster pull or move-aside is part of the move it prepares, so its failure fails that move | `loadout-drawer/loadout-apply.ts:456-466,527-578`, `inventory/item-move-service.ts:677-741,1072-1083` |

Ghostlink adaptations (DIM does not model these):

- **Dependencies are explicit.** Planner steps carry `needs`, the earlier steps whose
  success they rely on (dequip, pull, space-making and earlier legs of the same
  move; a grouped dequip equip needs every replacement move). A step with a failed
  need is `skipped` with `dependency_failed` and the step it waited for. Equips also
  wait per item on the last step that touched that item.
- **Uncertain is not success.** A dispatched step with an unknown effect is `failed`
  (`uncertain`), is not projected, is not retried, and its dependents are skipped. DIM
  has no equivalent because it re-reads inventory.
- **Session-wide refusals stop the rest.** Rate-limit (including the throttle
  fail-fast), authentication, authorization or Bungie-unavailable refusals mark the
  remaining steps `skipped` with `provider_stopped`, because they would hit the same
  gate. DIM retries throttles with backoff; Ghostlink never retries.
- **Equip-not-possible** (DIM `checkEquipNotPossible`, `loadout-apply.ts:1584-1591`)
  is only a UI hint in DIM; here it is an ordinary typed rejection
  (`not_in_social_space`, `unequippable`).

Results (`kind: build_apply`): per item `succeeded`, `failed` (`rejected` with a stable
`reason`, `bungie_code`, `not_reported`, `authentication_refused`, `transient`,
`uncertain`, `not_dispatched`) or `skipped`; `not_applied` lists what is not applied.
`result` is `applied`, `no_op`, `partial`, `failed` or `refused`. Exits reuse the action
contract with no new code: 0 applied/no-op, 41 partial or failed (also when some build
items could not be planned), 42 local state could not be updated, category exits when
nothing reached Bungie. Freshness is invalidated after any dispatched step that did not
succeed, so the next read re-syncs.

### Grouped equips above 17 items

DIM sends one `equipItems` request with every item and does not chunk
(`loadout-drawer/loadout-apply.ts:580-594`, `bungie-api/destiny2-api.ts:229-246`); a
build holds one item per slot, so DIM never nears a limit. Ghostlink's
`EquipItemsRequest` is capped at 17 items (`MAX_EQUIP_ITEMS`), so a larger group
is sent as consecutive requests of at most 17, in plan order. Each chunk reports per
item by Bungie code; a session-wide refusal on one chunk leaves later chunks unsent
(`provider_stopped`). A group of 17 or fewer is unchanged (one request; a lone item
stays an inline equip).

## MCP tool (BE-007a2d)

`apply_build` (`{id | name, character?, dry_run?}`) is composed outermost in the
mutation chain with its own build-store connection and action gateways over the shared
credential session. It calls `BuildService::plan_apply` for `dry_run: true` and
`BuildService::apply` otherwise, and returns `BuildOutput`, the same value the CLI
prints, so JSON and the failure class match. `isError` is set for every non-zero CLI
exit. Local-state persistence failure (acknowledged actions whose projection write
failed) is covered by dedicated tests for the executor, the CLI (exit 42) and the MCP
service (the same output). See [MCP](mcp.md#apply-a-stored-build).

## Sockets (BE-007b1: subclass and artifact overrides)

Builds store `socketOverrides` for the subclass (super, abilities, aspects, fragments)
and the artifact (BE-005d2). `plan_socket_overrides` (`destiny_core::build_sockets`)
ports DIM's socket phase; each changed socket becomes one `insert_plug` step in phase
`sockets`, after the equips and before clear-space, like DIM. Items are judged where
the plan leaves them (an item the plan moves to the target counts as there). A step
`needs` every earlier move, pull or equip that handled its item.

| Decision | DIM evidence (`src/app/`) |
|---|---|
| Overrides apply to every build item that has them, except for the Vault and class-incompatible items; nothing else is checked locally (equipped state included, Bungie decides) | `loadout-drawer/loadout-apply.ts:272-299` |
| Subclass ability/super sockets with no override get their default choice (`singleInitialItemHash`, else the first plug-set entry) | `loadout-apply.ts:283-299`, `utils/socket-utils.ts:244-250` |
| Aspects and fragments are interchangeable between their sockets: plugs already present stay, remaining overrides fill the excess sockets from the end of the list, leftover sockets go back to their empty plug; with no aspect/fragment override the category is not touched | `loadout-apply.ts:1142-1192` |
| Only the active fragment sockets count: capacity is the sum of the loadout's own aspects, else of the plugged aspects | `loadout-apply.ts:1196-1199`, `loadout-drawer/loadout-utils.ts:745-771` |
| Other sockets (super, abilities) take their override directly | `loadout-apply.ts:1201-1213` |
| Artifact: a plug already in some socket of the loadout is kept and its override remapped; if the intended plug cannot move to that socket the socket is emptied first | `loadout-apply.ts:1010-1131` |
| A plug already in its socket needs no request; one failed plug never stops the next, even on the same item | `loadout-apply.ts:1303-1366` |
| Only free insertions: reusable or randomized plug set, no material cost on the new plug, none on the one it replaces; DIM never uses the paid AWA endpoint in releases (`$featureFlags.awa` is dev-only) | `inventory/advanced-write-actions.ts:41-103`, `config/feature-flags.ts:29` |
| Request: `InsertSocketPlugFree` with `socketArrayType` Default for the item's owner | `inventory/advanced-write-actions.ts:196-208` |

Ghostlink adaptations and limits:

- **Endpoint and scope.** `POST /Destiny2/Actions/Items/InsertSocketPlugFree/`
  (bungie-api-ts 5.10.0 `insertSocketPlugFree`, body `DestinyInsertPlugsFreeActionRequest`:
  `plug {socketIndex, socketArrayType: 0, plugItemHash}`, `itemId`, `characterId`,
  `membershipType`). It works for characters in orbit, a social space or offline and
  needs at least 0.5 s between insertions; the gateway enforces that spacing itself and
  never retries. Ghostlink's authorization URL carries no `scope` parameter (the
  scopes are those of the registered Bungie application); the other item actions
  already require `MoveEquipDestinyItems`, which Bungie also lists for plug
  insertion, so no new scope and **no re-login** are expected. This could not be
  verified offline: a rejected live call (`authentication_refused`) is the signal.
- **Local state.** DIM replaces the item with the one Bungie returns after each
  insertion (`advanced-write-actions.ts:243-258`). Ghostlink's projection derives
  perks, stats and energy from component data and cannot rebuild them, so an
  acknowledged insertion does not edit the projection: freshness is invalidated and
  `local_state` is `refresh_pending`; the next read re-syncs.
- **Evidence.** Socket types, plug sets and material requirement sets come from the
  stored Manifest catalogs (and the definitions of the plugs the sockets can hold). A
  plug with a missing definition or cost set, an unknown empty plug or fragment
  capacity is skipped with a typed reason, never guessed. `socket_skipped` lists them
  by override index; reasons: `unresolved`, `missing_definition`, `class_mismatch`,
  `not_on_target_character`, `socket_state_unavailable`, `plug_definition_unavailable`,
  `empty_plug_unknown`, `ability_default_unknown`, `fragment_capacity_unknown`,
  `not_free`, `insertion_cost_unavailable`. `socket_already_there` counts plugs that
  needed no request.
- **Not checked locally.** The core settings lists of protected plugs and blocked
  socket types (`canInsertForFree`) are not fetched; Bungie rejects those. DIM's
  irreversible-shader guard and transmog swap concern fashion (BE-007b3).
- **Subclass equip.** See "Subclass and artifact equips". An empty `socketOverrides`
  object is stored as no overrides, so DIM's defaults-only patch for `{}` does not run.
- **Pacing.** The executor's 100 ms pacing also runs before each insertion; the gateway
  adds the 0.5 s spacing.

## Armor mods (BE-007b2)

`plan_armor_mods` (`destiny_core::build_mod_apply`) ports DIM's mod phase. It runs after
the socket overrides and before clear-space, as `insert_plug` steps in phase `mods`.
Mods go to the build's armor marked equip, else the piece equipped on the target, one
per armor slot; like DIM (`loadout-apply.ts:1282-1296`) they are not limited to the
build's own armor. Items are judged where the plan leaves them.

| Decision | DIM evidence (`src/app/`) |
|---|---|
| The mod phase runs when the build has mods, fashion or `clearMods`; after socket overrides, before clear-space | `loadout-drawer/loadout-apply.ts:629-672` |
| A mod that is missing or not unlocked on the target character is dropped before fitting (the resolved variant is checked) | `loadout-apply.ts:306-328` |
| If every requested mod is already on the armor (each plugged copy consumed once) and `clearMods` is off, nothing is sent | `loadout-apply.ts:1302-1310,1410-1458` |
| Mods are fitted with the existing assignment (`fitMostMods` with the in-game rules: no energy upgrades, capacity floor 1); mods that do not fit are reported and never sent | `loadout-apply.ts:1317-1330`, `loadout/mod-assignment-utils.ts:286` |
| `pickPlugPositions`: keep a mod where it already is, else replace a plug of the same exclusion group, else the first fitting socket; mods that fit fewer sockets go first; sockets ordered by plug-set size; leftover sockets return to their empty plug, requested only with `clearMods`; artifice sockets count even when hidden | `loadout/mod-assignment-utils.ts:576-614,689-770` |
| `createPluggingStrategy`: regains (free energy, no new exclusion group) first, spends next, optional empty-plug regains only when a spend lacks energy or an exclusion group; a swap that both needs and releases something is split through the empty plug | `loadout/mod-assignment-utils.ts:780-1000` |
| Only required steps are sent, in order; a failed step never stops the next | `loadout-apply.ts:1383-1405,1461-1546` |
| Only free insertions, as for socket overrides | `inventory/advanced-write-actions.ts:41-103` |

Ghostlink adaptations and limits:

- **Evidence.** The assignment needs the resolved armor pieces with socket state, energy
  and rarity, the armor-mod socket types and plug sets, and plug definitions with material
  requirement sets (the `build check` inputs). Otherwise the phase is not planned and the
  assignment reason is listed in `not_applied`. DIM assigns mods to fewer than five
  armor pieces and so does the apply (see "Armor set"); `build check` still needs all
  five, as DIM's analysis does.
- **Projected unlocks.** Profile sync stores profile and character plug-set entries only
  for plugs with a known definition that is a subclass option, an armor mod (plug category
  `enhancements.*`, masterwork plugs excluded) or fashion (item sub type shader 20 or
  ornament 21, DIM `isFashionPlug`). DIM reads every plug set
  (`inventory/selectors.ts:327-345`); other cosmetics and unknown definitions are dropped.
  Storage stays bounded by decode (4096 sets x 512 plugs) and one row per scope and plug.
  The table is still named `subclass_plug_options`; it is not renamed.
- **Skips** (`mod_skipped`, one entry per mod hash): `mod_not_unlocked`,
  `invalid_for_armor`, `unassigned`, `not_on_target_character`,
  `item_energy_unavailable`, `no_socket_for_mod`, `not_enough_energy`, the
  assignment's own socket-state reasons, and the free-insertion reasons of the socket
  phase (`not_free`, `plug_definition_unavailable`, `insertion_cost_unavailable`).
  `mods_in_place` counts requested mods that needed no request.
- **Items elsewhere.** DIM inserts into armor wherever it is; Ghostlink plans mods only
  for armor on the target character after the plan (the step is sent with the target).
- **Dependencies.** A mod step `needs` every earlier move, pull or equip of its item.
  Steps of one item are not chained to each other (DIM continues after a failed mod).

### Armor set

DIM collects the build's armor marked equip, else the equipped piece, per slot, and
fits mods to whatever it found (`loadout-apply.ts:1282-1330`; `fitMostMods` takes any
number of pieces and leaves a bucket mod without a piece unassigned,
`mod-assignment-utils.ts:286-354`). The apply does the same with the
in-game rules: with fewer than five pieces the mods of a slot that has no piece are
`unassigned` (or `invalid_for_armor` when no piece admits them), and nothing is guessed.
Only the stat/check analysis (`build check`, DIM `analysis.ts:454`) still requires the
full set.

## Subclass and artifact equips (BE-007b3)

| Decision | DIM evidence (`src/app/`) |
|---|---|
| Subclass and Artifacts are ordinary build items, sorted after weapons and armor | `destiny2/d2-bucket-categories.ts:21-23`, `loadout-drawer/loadout-apply.ts:118-129,261-271` |
| A move is requested only for items that are not on the target and are not `notransfer`; an equip only when `equip` is set and the item is not equipped | `loadout-apply.ts:386-405` |
| Items are equipped with the same single or bulk equip as other items | `loadout-apply.ts:424-433,526-540,580-594` |

Ghostlink adaptations:

- The audited `EquipmentCapacityPolicy` still validates exactly the eight weapon/armor
  buckets (their capacity is proven against the Manifest bucket artifact); it was **not**
  widened. Subclass and artifact buckets are equip-only buckets of the planner: they
  are equipped where they already are. A subclass that is not on the target character is
  skipped as `not_transferable` when its definition says so (subclasses do not move), and
  as `unsupported_bucket` when it would need a move, because their character capacity is
  not audited.
- They are excluded from `clearWeapons`/`clearArmor` and never take part in Vault
  evidence.
- Equipping one replaces whatever occupies its bucket on the target in the local
  projection, like other equips. The single-item `equip` command still rejects them.
- Builds with a subclass no longer report `unsupported_bucket`, so a build whose other
  steps succeed is `applied`.

## Fashion (BE-007b3)

`plan_fashion` (`destiny_core::build_fashion_apply`) ports DIM's `modsByBucket` handling.
Fashion steps are `insert_plug` steps in phase `mods`, after every armor mod step, for the
same armor set as the mods.

| Decision | DIM evidence (`src/app/`) |
|---|---|
| A fashion plug is applied only if it exists and is unlocked in the plug sets, is an owned exotic ornament, or is the default shader or a default ornament | `loadout-drawer/loadout-apply.ts:308-320,329-338` |
| Each plug goes into the first socket of the armor piece it fits (empty plug, plug set member, or the item's own reusable plugs); otherwise it is unassigned | `loadout-apply.ts:1365-1380`, `utils/socket-utils.ts:228-237` |
| Mods are plugged for all armor first, then fashion (shaders, ornaments, armor-type plugs) | `loadout-apply.ts:1388-1405`, `loadout-drawer/loadout-utils.ts:857-864` |
| A plug already in its socket needs no request | `loadout-apply.ts:1502-1505` |
| Transmog swap: an ornament made from the same item is replaced by the socket's default plug | `loadout-apply.ts:1489-1500`, `inventory/advanced-write-actions.ts:144-151` |
| Only free insertions | `advanced-write-actions.ts:41-103` (shared with sockets and mods) |
| Irreversible-shader guard: a plugged shader other than the default shader or default ornaments is never overwritten unless it is unlocked in the socket's plug set or enabled among the item's reusable plugs | `advanced-write-actions.ts:105-135,165-175` |
| Exotic ornament unlocks are the `canInsert` plugs of the cosmetic sockets of any owned exotic armor, minus default ornaments | `inventory/selectors.ts:372-389` |

Ghostlink adaptations and limits:

- **Classification data (migration 35).** `itemType`, `itemSubType` and
  `itemCategoryHashes` are projected for plug definitions only (STRICT tables
  `definition_item_classes` and `definition_item_categories`, guarded downgrade, one
  freshness invalidation, same conventions as migrations 23-34). No new Manifest
  download: they come from the item table already fetched. Run `ghostlink sync-manifest`
  after upgrading; without it fashion plugs have no classification and stay unclassified
  (the irreversible-shader guard cannot recognize shaders, so a stale projection must be
  refreshed before applying fashion).
- **Evidence.** The cosmetic sockets' types and plug sets and the fashion plugs' insertion
  cost sets are loaded per request; plug-set members are not loaded as definitions.
- **Plug-set identity.** Plug-set entries are projected without their plug-set hash.
  DIM's guard asks whether the shader is unlocked in the *socket's* plug set; Ghostlink
  asks whether it is enabled in any profile or target-character plug set (or enabled on
  the item), and requires the socket to have a plug set. Universal ornament plug sets
  (which DIM reads through `canInsert`, `plugset-helpers.ts:26-36`) are approximated for
  requested ornaments only: an ornament reported insertable counts as unlocked.
- **Skips** (`mod_skipped`): `fashion_not_unlocked`, `fashion_unassigned`,
  `fashion_no_armor` (no armor piece for that bucket), `irreversible_shader`,
  `not_on_target_character`, `socket_state_unavailable` and the free-insertion reasons.
  Plugs already in place count in `mods_in_place`.
- **Core-settings lists.** Protected plug and blocked socket-type lists are not checked
  locally (Bungie rejects them), as for sockets.

## Remaining work

Stack amounts; per-plug-set unlock scoping and the universal ornament plug-set list;
live validation of plug insertion and the equip endpoints (BE-010).
