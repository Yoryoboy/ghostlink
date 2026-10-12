# Build apply: planner and dry-run (BE-007a, incomplete)

`ghostlink build apply ... --dry-run` prints the plan from the pure equipment
planner and dispatches nothing (profile reads only; no mutation transport is
composed). There is **no executor and no MCP apply tool yet**: `build apply`
without `--dry-run` exits 40 (`unsupported_operation`). Existing move, equip,
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
  Sockets, mods and subclass application are explicitly `not_applied` with
  `sockets_mods_subclass_deferred_to_be_007b`.
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
  a pure planner input. Production supplies none, so `unknown` lists
  `vault_capacity` and Vault-staged moves are skipped as
  `vault_capacity_unavailable`. DIM derives it from the Vault vendor `acceptedItems`
  and `DestinyInventoryBucketDefinition.itemCount`, which Ghostlink does not
  project or download. Projecting them (a migration 34 plus new sync components)
  could not be audited offline, so no capacity is guessed.

## Remaining work

Vault bucket/capacity projection; stack amounts; the executor (reusing the move,
equip, batch equip and Postmaster pull adapters, throttle and error mapping, with
DIM's per-item partial failure adapted to optimistic semantics: continue
independent items, track dependency failures, never blindly retry, never claim
provider-observed success); a no-dispatch test that fails any mutation transport;
and the MCP mutation tool. BE-007b (sockets, mods, subclass) remains separate.
