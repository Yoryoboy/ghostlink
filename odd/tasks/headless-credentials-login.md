# Headless credentials and login (#89, #90)

Make Ghostlink usable on a headless home server: plaintext credential file by default with no keyring or D-Bus (#89), then a non-interactive two-step login completed from a browser on another device (#90).
Delivery: #89 on `feat/file-credential-store` (from `main` `923eb15`); #90 on `feat/headless-login` stacked on it. Push, PR and merge are owner decisions.

## Specs

- **S1 — Default store is a local plaintext file (#89).** Owner: "quiero que por default sea guardar las credenicales en un archivo local en texto plano". Location chosen by the owner: `~/.config/ghostlink/credentials.json` (resolved like `config.env`: `$XDG_CONFIG_HOME/ghostlink/credentials.json`, else `$HOME/.config/ghostlink/credentials.json`; Windows `%APPDATA%\ghostlink\credentials.json`). JSON format.
- **S2 — Keyring removed (#89).** Owner chose "Eliminarlo": no keyring backend, no keyring/D-Bus dependency, no `libdbus` build requirement.
- **S3 — Existing keyring users re-login once (#89).** Owner chose "Re-login una vez": no migration; a missing credential file asks for `ghostlink login` as today, and never touches D-Bus.
- **S4 — #89 expected behavior, verbatim:**
  - "CLI and MCP share the same persistent credential store, including access and refresh tokens."
  - "Default authentication and token renewal work without a keyring or D-Bus session."
  - "Store credentials separately from inventory data and ordinary configuration, at a documented per-user location."
  - "Restrict directory/file permissions on Unix (0700/0600) and use appropriate user access controls on Windows."
  - "Use atomic writes and safe coordination between CLI and MCP during token rotation."
  - "Never expose tokens in logs, errors, MCP output or agent prompts."
  - "Document honestly that a plain local file is not encrypted and that processes with access as the same user may read it."
  - "Update documentation and tests that currently promise keyring-only storage and reject filesystem storage."
- **S5 — Non-interactive two-step headless login (#90).** Owner chose "Paste no interactivo": "`login start` imprime URL y `login complete` recibe la URL de callback. Sin infra nueva, pero el code viaja por donde lo pegues (WhatsApp)." `complete` also accepts the callback URL on stdin so it can stay out of shell history and the process list.
- **S6 — #90 constraints kept, verbatim:**
  - "Validate OAuth state and bind completion to the initiating login session. Enforce expiry, cancellation and replay protection"
  - "Provide clear status and sanitized errors for pending, completed, denied and expired authorization."
  - "Do not depend on a local browser, graphical environment, desktop URI handler or interactive wizard."
  - "Preserve existing desktop login behavior and document the headless command and operational requirements."
  - "Cover the flow with offline tests, and separately validate the real supported provider flow before claiming live compatibility."
  - Owner-accepted deviation: the authorization code may travel through chat (see S5). Document the residual risk.

## Tasks

- [x] T1 — S1, S4: file credential store adapter (JSON, 0700 dir / 0600 file, atomic temp+rename, no token in Debug/errors). Inline, test-first: RED (missing `FileCredentialStore`), GREEN 13 tests. Assess: medium, deferred to slice. Commit: `dba71e8`
- [x] T2 — S4: cross-process coordination: `CredentialStore::lock_for_refresh` (default no-op), exclusive lock on `credentials.json.lock`, reload inside the lock and adopt a rotated credential; a removed file requires authorization. Save failure keeps the existing best-effort semantics (not surfaced; keeps a usable rotated token in-process). Test-first: RED 5 failures, GREEN session 16 + store 15. Assess: medium. Commit: `ab9572f`
- [x] T3 — S1-S3: `destiny_app::credentials_file()` (`GHOSTLINK_CREDENTIALS_FILE`, `$XDG_CONFIG_HOME`, `~/.config`, `%APPDATA%`; path test); CLI read/login and MCP use `FileCredentialStore`; keyring adapter and 4 deps removed (Cargo.lock -352); `libdbus-1-dev` dropped from CI and dist, `pkg-config` kept (aws-lc-sys build dep). Public-binary check: no file exit 10, 0644 exit 50, expired exit 11, lock 0600. Workspace: 2049 passed, fmt/clippy clean. Assess: high (CI shell) → independent verify + native review offered. Commit: `97c66b2`
- [x] T4 — S4: README, configuration (new `## Credential file`), development, cli, mcp, integration-assumptions (superseded note), release notes. Commit: `a047525`
- [ ] T5 — S5, S6: pending login session + `login start` / `login complete`. Branch `feat/headless-login`. Inline, test-first. Commit: —
- [ ] T6 — S6: headless login docs and release notes. Commit: —
- [ ] T7 — S6: independent verification (security + delivery risk) and owner-run live validation on the home server. —

## Log

- **L1 (2026-10-14, owner, verbatim):** "quiero hacer las 89 y 90. me importan mas ahorita ya que donde estoy hacienod olas pruebas son en en mi home server. me gustaria. por ejemplo, con respecto a las credenciales quiero que por default sea guardar las credenicales en un archivo local en texto plano, tipo en un .ghostlink? dime tu. y la 90 es importante impleemnterlo por l oque te digo"
- **L2 (owner decisions):** path `~/.config/ghostlink/credentials.json`; keyring "Eliminarlo"; migration "Re-login una vez (Recomendado)"; #90 "Paste no interactivo".
- **L3 (exploration):** keyring adapter `crates/destiny-adapters/src/credential_store.rs` (binary `GLCR` v2, no file fallback by design); refresh write-back `crates/destiny-app/src/session.rs:136-139` ignores save errors and there is no cross-process lock while Bungie rotates refresh tokens; interactive paste login already exists (`login.rs`, `oauth.rs` `ManualPaste*`); Bungie has no device grant (`odd/tasks/destiny-oauth-foundations.md:36`); broker is a stateless `POST /token` Worker.
- **L4 (design):** override `GHOSTLINK_CREDENTIALS_FILE` added for isolation/self-hosting, mirroring `GHOSTLINK_CONFIG_FILE`. Group/other-accessible file fails closed like `config.env`.
