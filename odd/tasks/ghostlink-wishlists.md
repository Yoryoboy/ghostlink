# Ghostlink Weapon Wish Lists

Load DIM-format weapon wish lists (voltron by default, plus the user's local
lists) and expose, per weapon instance, which curated roll it matches, so AI
agents can answer "should I keep this drop?" with cited community opinion.

## Specs

- S1 — Default on. User: "si deberia ser por defecto". Voltron
  (`https://raw.githubusercontent.com/48klocs/dim-wish-list-sources/master/voltron.txt`)
  loads with no configuration.
- S2 — Source. User chose "Solo voltron (recomendado)": "Sin el trash de
  choosy de 2021; cualquier trash propio entra como lista local." No
  `choosy_voltron` preset.
- S3 — Local lists. User: "soportariamos listas locales mias". The same DIM
  format, read from local files the user configures.
- S4 — License. User: "hay que investigar tema olicencia". Download at run
  time (never embed list data in the binary); README Credits name 48klocs, the
  voltron curators, and the license facts in the Log.
- S5 — Agent value. User: "si, pensandolo asi, si aporta mucho valor entonces",
  accepting the agent-oriented shape: a bounded per-instance `wishlist` result
  on inspect (CLI JSON/human and MCP) with the matched roll's notes,
  source/section and normalized tags; `is:wishlist` and `is:trashlist` search
  filters; a skill rule to cite the curator and the opinion's age, and to treat
  "no match" as "no opinion", never as "bad".

## Decisions (parent, from the accepted proposal)

- DIM format (wiki "Creating Wish Lists"): `dimwishlist:item=H&perks=a,b`,
  optional `#notes:`; `//notes:` block notes (closed by the first unrecognized
  line or a new block note; line notes win); `title:`/`description:`; `//`
  comments; `item=-H` trash (perks optional); `item=-69420` wildcard (any
  item, never trash). Weapons only. First matching roll in file order wins;
  lists apply in configured order.
- Matching: every listed perk hash must be among the instance's socketed or
  available plug hashes; an enhanced option matches a base wish-list perk with
  the same name (DIM: "Enhanced versions will be matched in your wishlist
  automatically").
- Sync inside `sync-manifest` (and its automatic first-run sync), after
  Clarity, like Clarity: failure is reported, never fatal, never rolls back the
  Manifest. Voltron uses a conditional GET (ETag) plus a content SHA-256;
  unchanged content is not re-parsed. Local files are re-read each sync.
- Config key `GHOSTLINK_WISHLISTS`: comma-separated entries; `voltron` is the
  built-in source, other entries are absolute local file paths; unset means
  `voltron`; `none` disables wish lists. No arbitrary remote URLs.
- Own storage (new schema version): sources, deduplicated notes, rolls with
  file ordinal and section title. Notes stored verbatim; invalid lines are
  skipped and counted, never edited.
- Inspect `wishlist`: `verdict` (`wishlist`|`trash`|`none`) from the first
  match, up to 3 distinct matches in file order (`source`, `section`, `perks`
  names, `notes`, `tags`), `matches_total`; absent when no list is loaded.
  Tags parsed from `|tags:` in notes, lower-cased; `god-pve` → `pve-god`,
  `god-pvp` → `pvp-god`, `m+kb` → `mkb`.

## Tasks

- [x] T1 (S5) core: parser + matcher, pure, fixtures. Route: inline. Commit: 38b9b2a
      Native review review-b47ca34247689872 (base 2efa39b): approved,
      acknowledged; 4 readability SUGGESTIONs, no blockers.
- [x] T2 (S1-S4) storage, voltron/local acquisition, sync seam, config key,
      sync output. Route: inline. Commits: 71bb55d, 8383086, 9dce82b,
      1cd892a (+ style fixup).
- [x] T3a (S5) inspect `wishlist` field (core/app/CLI/MCP). Route: inline.
      Commit: f311348.
- [ ] T3b (S5) `is:wishlist`/`is:trashlist`. Route: inline. Commit: —
- [ ] T4 (S4-S5) docs (cli, mcp, configuration), skill rule, README Credits.
      Route: inline. Commit: —

Forecast: ~1,800 authored changed lines (T1 ~500, T2 ~700, T3 ~450, T4 ~150).
Delivery: ask-on-risk; user chose chain_strategy=feature-branch-chain
(slices: A core T1, B data T2, C surfaces T3, D docs T4).

## Log

- L1 (user, 2026-10-15): "dale revisa. si deberia ser por defecto. aun no me
  decido entre voltron y choosy, quiero entender culaes serian las
  diferenicacon sue cases relaes, soportariamos listas locales mias. hay que
  investigar tema olicencia"
- L2 (user): "si, pensandolo asi, si aporta mucho valor entonces"; chose
  "Armar el documento de la feature y arrancar" and "Solo voltron
  (recomendado)".
- L3 (research, 2026-10-15): voltron.txt 28.5 MB, 310,561 lines, 274,677
  rolls, 1,248 items, 0 trash, 0 wildcard; last commit 2026-09-17.
  choosy_voltron = voltron + 53 item-level trash lines from 2021-11-01;
  29 of the 53 also have voltron wish rolls. Licenses:
  48klocs/dim-wish-list-sources MIT (c) 2019 Dave Solomon;
  Adamsdown-Boy/destiny-rolls CC0-1.0; ThisIsMyPseudonym/sharditkeepit no
  license. Ghostlink already stores socketed plugs and component-310 reusable
  plugs per instance (`profile.rs` decode_available_plugs).
- L4 (T1 evidence): RED = compile error (unresolved `destiny_core::wishlist`);
  GREEN 11/11 `cargo test -p destiny-core --test wishlist`; clippy and fmt
  clean. Scratch parse of the real files (release): voltron 274,677 rolls,
  0 skipped, 5,207 distinct notes, 275 sections, ~200 ms; choosy 51 trash
  after accepting DIM's bare `#notes` marker (2 lines).
- L5 (user, 2026-10-15): "cuanod termines t1, avisame antes de empezar t2";
  then "lo hacemos en la procima sesion, guarda el progreso hasta ahora".
  T2 not started (read-only exploration only). Resume notes for T2: schema
  LATEST_VERSION is 36 (`crates/destiny-adapters/src/sqlite/migrations.rs:46`)
  so wish lists take v37; follow Clarity seams: transport/decode
  `crates/destiny-adapters/src/clarity.rs`, storage
  `crates/destiny-adapters/src/sqlite/clarity.rs`, sync orchestration
  `crates/destiny-app/src/clarity.rs` (`sync_manifest_and_clarity`), CLI
  composition `crates/destiny-cli/src/runtime.rs:456-525`, sync output
  `crates/destiny-cli/src/output.rs:576` (`render_manifest_with_clarity`),
  config keys and defaults `crates/destiny-app/src/config.rs:16-33,433-466`.
- L6 (review, 2026-10-15): lineage review-efb27d9573371a93 (base 59a651e)
  stuck on a gentle-ai admission defect (proof path with spaces truncated);
  user chose "Abandonar y revisar solo el slice A". Abandoned
  (operator_disposition, no captured work), restarted with base 2efa39b as
  review-b47ca34247689872: readability + reliability approved, authority
  burned. Advisory, non-blocking (wishlist.rs:57, 74, 101-102, 124): struct
  instead of anonymous ParsedRoll tuple, clarify title:/@title: branch,
  simplify sign parsing, name the 10-digit cap. Fix in T2: `notes_text`
  cuts at the last `|tags:` while `notes_tags` reads from the last bare
  `tags:`, so `foo tags:pve` disagrees between them.
- L7 (review, 2026-10-15): hook-required review review-f1fc94cb27f88830 of
  the full branch (base 59a651e, HEAD 49fc64d) approved and acknowledged;
  both lenses repeat the notes/tags delimiter WARNING (wishlist.rs:131-140).
- L8 (user): "dale empcemos con T2".
- L9 (T2 decisions, parent): the content fingerprint is FNV-1a 64 plus byte
  length in core, not SHA-256, to avoid a new dependency (high-risk item 5);
  it only detects change and voltron's ETag stays the first gate. Notes/tags
  fix: voltron has 442 lines with a bare ` tags:` (no `|`), so `notes_text`
  now cuts at the same last `tags:` that `notes_tags` reads.
  `GHOSTLINK_WISHLISTS` is validated only by the sync (`invalid_config`
  report, stored lists untouched), never by `ReadRuntimeConfig::from_values`,
  so a typo cannot block reads. Sync statuses: overall
  `ok|partial|unavailable|disabled|invalid_config`; per source
  `updated|unchanged|unavailable`, with the rolls still stored. A failing
  source keeps its rolls; unconfigured sources are deleted even offline.
- L10 (T2 evidence): RED then GREEN per unit: core 15/15
  (`--test wishlist`), storage 4/4 (`wishlist_store`), gateway 3/3 + 2 unit,
  sync 8/8 (`wishlist_sync`), config 1 new, CLI output 2/2
  (`wishlist_output`) + 1 runtime composition test; the exact-output test
  `manifest_foundations_render_both_outcomes_without_credentials` now pins
  the wish-list section. Isolated end-to-end (copy of the real database in
  `mktemp -d`, real GitHub voltron): first sync `updated` 274,677 rolls,
  0 skipped, 5,482 texts (~21 s including a Manifest refresh); second sync
  `unchanged` in 0.9 s via ETag; local list added `updated`; relative path
  `invalid_config` with stored lists intact, exit 0; missing local file +
  reorder `partial`; `none` `disabled` with 0 rows and 0 texts left. Storage
  measured with dbstat: 47.6 MB, cut to 36.7 MB by dropping the UNIQUE text
  index (dedup stays in Rust per publication). WITHOUT ROWID rolls saved
  ~2 MB more but broke the `bridge_rows` test convention that every table
  has a rowid, so it was reverted.
- L11 (review, 2026-10-15): hook-required review review-9bd540ce1f588b49
  (full branch, lenses risk/resilience/reliability) approved and
  acknowledged with three advisory WARNINGs. User: "Vayamos con tu
  recomendación" (fix them before T3).
- L12 (follow-up evidence): R4 an empty voltron parse is refused as
  `unavailable` (RED: it replaced stored rolls); it is refused even on a first
  sync, since storing it with its ETag would pin the empty list behind 304s.
  An emptied local list still publishes empty (user intent). R3 a missing
  voltron HTTP client no longer blocks local lists (RED: the local list was
  `unavailable`; runtime now falls back to `UnavailableVoltron`). R1 notes and
  sections are shared `Arc<str>` (RED: `Arc::ptr_eq`); measured 204 MB of
  per-roll copies for 4.8 MB of distinct text, and a differential run of the
  real voltron sync went from 322 MB to 102 MB peak RSS (2f8cc0a vs new).
- L13 (T3 split, parent): T3 became two units: T3a the inspect `wishlist`
  field (core view, app builder, CLI human/JSON, MCP view) and T3b the
  `is:wishlist`/`is:trashlist` filters. T3b needs new plumbing: no existing
  filter reads the repository inside eval, and `evaluate_wishlist` needs a
  perk-hash-to-name map for enhanced variants. The explorer also found
  `is:wishlist` currently `Unsupported` in
  `crates/destiny-core/src/search_query/registry.rs:158`, `is:trashlist` must
  be inserted after `is:titan` (registry.rs:153) for the ascending-key test,
  `ItemView` literal sites in core/adapters/MCP tests need updating, and
  `crates/destiny-mcp/tests/stdio.rs:224-234` pins bare `wishlist:` as
  unsupported, so the bare key must stay unsupported.
- L14 (T3a decisions): the field is `Option<WishlistInspection>` on
  `ItemInspection` (absent for non-weapons and with no list loaded). Access
  follows the Clarity precedent: a defaulted `WorldDefinitionRepository::
  lookup_wishlist_rolls` that returns `None` when no source is stored, so no
  new bound and no fixture churn; the SQLite override uses a cheap
  `SELECT EXISTS` presence check. Enhanced perks match by name because the
  app resolves the wish-list perk hashes through the plug definitions. Views
  are shared by CLI and MCP through `destiny-output/src/wishlist.rs`.
  Evidence: core 18/18 (`--test wishlist`, includes `Arc::ptr_eq`,
  inspection matching and the weapon-only rule), inspect integration 3/3
  (`wishlist_inspect`, incl. CLI/MCP JSON parity and field absence).
- L15 (review, 2026-10-15 14:24): the RDD hook asked for a review of the T3a
  candidate (whole branch since 59a651e; assess said reviewDue false,
  under_budget, medium). Started review-4d850975733946c8 with the
  readability and reliability lenses; its two reviewer slots are still
  offered and unconsumed because the reviewer run failed on the provider
  quota (Claude five-hour limit, resets 17:40 America/Argentina/Buenos_Aires).
  Nothing was mutated. Retry the same bound slots after the reset; never
  replay the failed capture from memory. The lineage stays `reviewing`, so the
  candidate is frozen at tree 7cdb1eff for T3a.
- L16 (review, 2026-10-15): review-4d850975733946c8 approved but could not be
  acknowledged because the doc commit 88b7c1b moved the candidate; a fresh
  lineage review-b9d1ecf0d85e870e (tree f8c18321) approved and was burned
  with 6 advisory findings. Never commit between a review START and its
  acknowledgement.
- L17 (T3a follow-ups): two lists sharing a section and notes collapsed into
  one cited source (RED: `lists_sharing_section_and_notes_are_cited_separately`
  cited only `/mine.txt`). Matches now group by source, section and notes, and
  the source travels with the roll through a shared `evaluate_keyed`, so the
  `std::ptr::eq` lookup is gone. Suggestions applied: `MAX_NUMBER_DIGITS`,
  a clearer `title:`/`@title:` branch, the sync JSON is built on a
  `serde_json::Value` instead of re-parsing rendered text, and the app method
  is `wishlist_inspection` so it no longer shadows core's `inspect_wishlist`.
