# Live Validation — v0.1.0-rc.1 (Linux laptop) — PENDING

Validate the installed product, not the source tree, against the real account.
The game runs on the Windows PC; an agent on the Linux laptop drives the
installed `ghostlink` CLI (and MCP through an agent), and the owner confirms
each in-game result. Record every result in this file as it happens.

## Rules for the session

- Use only the installed binaries (`~/.cargo/bin/ghostlink`, `destiny-mcp`).
  Never `cargo run` or a workspace build.
- One action at a time. Before each mutation, the agent names the exact item,
  character and expected in-game result, then waits for the owner's in-game
  confirmation before the next step.
- Prefer non-exotic, unused, unlocked items. Never move or equip anything the
  owner did not agree to. Never dismantle (Ghostlink cannot).
- No blind retries. On exit 41/42 stop, record the output, and decide with the
  owner.
- Mutations need the character in orbit, a social space or offline.
- Results: `PASS`, `FAIL` (with output), `SKIP` (with reason), `N/A`.

## Expected outcome contract (since 2026-10-08)

- Acknowledged action: status `applied`, exit 0. Ghostlink does NOT re-read the
  profile after the action; the in-game screen is the source of truth.
- The next read (`search-inventory`, `inspect-item`, `loadouts`, ...) forces a
  provider sync. Bungie's profile may lag for some seconds or minutes, so a
  stale read right after an action is expected, not a failure.
- Already-satisfied requests: `verified_noop`, exit 0, zero dispatch.
- Refused, uncertain or partial: exit 41. Local state could not be updated:
  exit 42.

## 0. Preparation (before the session)

- [ ] Prerelease `v0.1.0-rc.1` exists with Linux tarball,
  `ghostlink-installer.sh` and `sha256.sum`.
- [ ] Laptop: `gh auth status` OK for `Yoryoboy/ghostlink` (private repo).
- [ ] Record any existing install: `which -a ghostlink destiny-mcp`,
  `ghostlink --version`. If a dev build lives in `~/.cargo/bin`, back it up
  (`cp ~/.cargo/bin/ghostlink ~/ghostlink.dev.bak`), because the installer
  replaces it.
- [ ] Note the existing config/database/credential state (do not delete it).
  Decide with the owner: keep the existing login, or test first-run with a
  fresh `XDG_CONFIG_HOME`/`XDG_DATA_HOME` in a throwaway directory.

## 1. Install from the prerelease (installer path)

The repo is private, so download with `gh` and point the official installer
at the local copy:

```bash
gh release download v0.1.0-rc.1 -R Yoryoboy/ghostlink -D ~/gl-rc1
cd ~/gl-rc1 && sha256sum -c sha256.sum --ignore-missing
GHOSTLINK_DOWNLOAD_URL="file://$HOME/gl-rc1" sh ~/gl-rc1/ghostlink-installer.sh
```

- [ ] Installer finishes; `ghostlink --version` prints `0.1.0-rc.1`.
- [ ] `destiny-mcp` and `ghostlink-update` are installed next to `ghostlink`.
- [ ] An install receipt exists (needed by `uninstall`).
- [ ] Alternative (optional): direct download with
  `GHOSTLINK_GITHUB_TOKEN="$(gh auth token)" sh ghostlink-installer.sh`;
  record whether private release URLs work with a bearer token.

## 2. First run, login and reads

- [ ] `ghostlink setup`: wizard flow, agent detection, cancel behavior.
- [ ] `ghostlink login` (default `--mode auto`; `--mode paste` if the handler is not
  available): authorization succeeds, no secrets printed.
- [ ] `ghostlink handler status` on the laptop desktop (record DE/browser).
- [ ] `ghostlink sync-manifest`, then `ghostlink sync`.
- [ ] Reads: `list-characters`, `search-inventory` (one perk filter),
  `inspect-item`, `subclass`, `artifact`, `loadouts`, `postmaster`,
  `weekly`, `weekly-reset`.
- [ ] `--json` on at least one read produces one bounded JSON object.

## 3. Inventory actions (IAX-009; owner confirms in game)

For each step record command, exit code, status and the in-game confirmation.

- [ ] **Move to Vault**: a non-equipped item from a character to the Vault
  (`ghostlink move <item> --vault`).
- [ ] **Move back** to a character (`--character <class>`).
- [ ] **Move character to character** (Vault-mediated, two steps).
- [ ] **Already there**: move to its current location → `verified_noop`.
- [ ] **Lock** a character item; **unlock** it.
- [ ] **Lock a Vault item** (routed through the first character); unlock it.
- [ ] **Lock already locked** → `verified_noop`.
- [ ] **Equip one item** already on the character.
- [ ] **Equip one item from the Vault** (transfer then equip). Regression
  check for the old EQ-007 case: expect `applied`, confirm in game.
- [ ] **Batch equip** 2–3 items (`ghostlink equip A B C --character <class>`),
  at least one coming from the Vault; per-item results all succeed.
- [ ] **Batch equip refusal**: two exotic weapons in one request → refused
  before any action (nothing moves in game).
- [ ] **Apply loadout by slot** (`ghostlink apply-loadout <n> --character
  <class>`); confirm the full loadout in game.
- [ ] **Apply loadout by name**.
- [ ] **Apply loadout outside orbit** (only if convenient: in an activity) →
  location refusal message, exit 41.
- [ ] **Postmaster pull** of a genuine instance (`ghostlink pull <instance-id>`),
  if the Postmaster has gear.
- [ ] **Postmaster pull-stack** (`ghostlink pull-stack <hash> --amount <n>
  --character <id>`), if the Postmaster has a stack.
- [ ] After the actions, run a read and confirm it reflects the game once
  Bungie catches up (invalidation forces sync).

## 4. MCP through an agent

- [ ] `ghostlink agents status`, then `ghostlink agents install` for the
  agent used on the laptop (Claude, Codex, OpenCode or Pi); record config
  changes and skill installation (PR-007/PR-008 native check).
- [ ] From the agent: one read tool, `move_item`, `equip_items`,
  `apply_loadout`, `set_lock_state`; owner confirms in game.
- [ ] Restart the agent and confirm the MCP server resolves the installed
  sibling binary and reports `0.1.0-rc.1`.
- [ ] Session renewal: if the session crosses access-token expiry, record
  whether reads keep working (persistent-session/broker refresh check).

## 5. Lifecycle (Linux)

- [ ] `ghostlink uninstall --dry-run`: lists only receipt-owned files, writes
  nothing.
- [ ] `ghostlink uninstall` and cancel at the confirmation: nothing removed.
- [ ] `ghostlink update` against the private repo: record the result. A
  download/authorization failure caused by the private repo is expected, not a
  product bug (a real update test needs `rc.2`).
- [ ] `ghostlink agents uninstall` for the agent installed in step 4.
- [ ] `ghostlink uninstall --yes`: removes only installed binaries/receipt;
  config, database and credentials are preserved.
- [ ] Restore the dev build backup if one was taken in step 0.

## Out of scope for this session

- Native Windows runtime (config in `%APPDATA%`, Windows credential store,
  SQLite locking, PowerShell installer, paste login on Windows).
- Update between two releases (needs `rc.2`), and lifecycle on a disposable VM.
- Clarity positive insight (needs a catalog match) and Clarity release
  partnership before any public release.
- Weekly calendar decisive check on 2026-12-01 (dungeon slot 5).

## Results

| Step | Result | Notes |
|---|---|---|
