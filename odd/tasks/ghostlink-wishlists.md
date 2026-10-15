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

- [x] T1 (S5) core: parser + matcher, pure, fixtures. Route: inline. Commit: pending
      delivery choice.
- [ ] T2 (S1-S4) storage, voltron/local acquisition, sync seam, config key,
      sync output. Route: inline. Commit: —
- [ ] T3 (S5) inspect `wishlist` field (core/app/CLI/MCP) and
      `is:wishlist`/`is:trashlist`. Route: inline. Commit: —
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
