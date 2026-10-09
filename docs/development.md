# Development

End users should use the [release installers](../README.md#install). Building
from source requires a recent stable Rust toolchain; on Linux, also install
`libdbus-1-dev` and `pkg-config` (Debian/Ubuntu names).

```sh
cargo build -p ghostlink --release
./target/release/ghostlink --help
./target/release/destiny-mcp --help
```

Both executables are built side by side. For interactive use, run
`./target/release/ghostlink` for setup. This checkout's public application
defaults still await owner provisioning; see [configuration](configuration.md)
for optional `config.env` overrides and direct confidential mode. Never distribute
a client secret. Broker self-hosting is covered in the [broker runbook](../broker/README.md).

## Workspace layout

This repository is a Rust workspace for the Destiny application. Dependency
direction points inward:

```text
destiny-core <- destiny-app <- destiny-cli
destiny-core <- destiny-adapters
destiny-core <- destiny-adapters <- destiny-cli (composition root only)
destiny-core <- destiny-adapters <- destiny-mcp (private process composition)
destiny-app  <--------------------- destiny-mcp (library contracts + stdio server)
destiny-output <------------------- destiny-cli, destiny-mcp (shared presentation)
```

The `destiny-mcp` library holds the bounded MCP schemas, the injectable
`InventoryReadService` and `MutationToolService` seams, the RMCP tools-only
server, and the stdio runtime. These presentation modules never name a concrete
adapter. The private process module (`src/main.rs`, retained at its original
path) loads the native credential store, HTTP transports, SQLite projection
store, and transfer gateway. Its public `destiny_mcp::run()` entry point is
called by the tiny binary wrapper in the `ghostlink` package.

The package in `crates/destiny-cli` is named `ghostlink`; its library retains the
`destiny_cli` name, so Rust imports are unchanged. This package owns both release
executables, `ghostlink` and `destiny-mcp`. The standalone `destiny-mcp` package
remains a workspace member for development and feature-gated offline tests.

`destiny-output` holds transport-neutral presentation contracts shared by the
CLI and the MCP server, so both surfaces render identical JSON.

The crates are intentionally minimal.

Design background lives in [mvp-architecture.md](mvp-architecture.md),
[mvp-implementation-plan.md](mvp-implementation-plan.md) and
[integration-assumptions.md](integration-assumptions.md).

## Agent skill

`skills/ghostlink/SKILL.md` prefers Ghostlink MCP tools when available, with the
CLI as the shell-capable fallback. It holds judgment (intent to command, output
contract, `equip` safety) and defers
flags to `ghostlink <cmd> --help`. `crates/destiny-cli/tests/skill_drift.rs`
fails when the skill names a missing command or flag, or omits a public
command, so update the skill together with any CLI surface change.

## Lifecycle update tests

`installation.rs` validates the narrowly supported official cargo-home receipt;
`update.rs` invokes only its validated sibling updater. The runner routes update
before application runtime construction. Windows returns standalone instructions,
never a helper launch. See [update](cli.md#update) for the receipt/exit contract.

```sh
cargo test -p ghostlink --test update
cargo test -p ghostlink --test agents --test handler_parsing
cargo fmt --all --check
```

Update tests use isolated temporary receipts, fake product files and shell
updaters, not installed binaries, live releases or keyrings. They exercise version
forwarding, bounded concurrent stream draining, refusal boundaries, receipt
lookup/child environment, lock contention and panic-on-access runtime factories.
Linux-only execution tests do not verify Windows runtime behavior or real release
installation. Fixture directories are retained in the OS temporary directory.

## Lifecycle uninstall tests

`uninstall.rs` dispatches before runtime/API/dotenv/SQLite/keyring factories.
`Installation::validate_uninstall` shares receipt/product validation but does not
require an updater. `run_at` injects fixture paths, confirmation and a
`LinuxUriHandlerRegistrar` with a fake command runner; production uses only
`current_exe`, never a user-selected install root. Scoped agent and handler APIs
bind execution to their read-only previews. The kernel lock is acquired after
consent and snapshots rechecked. Handler skips preserve handler files while allowing
binary/receipt removal with exit 41; mutation errors and changed snapshots still
retain CLI/receipt. Tests cover active defaults absent from the owned MIME file,
missing MIME files, recorded defaults, foreign entries and read-only partial warnings
in both JSON and human previews.
No purge, elevation, recursive installation deletion or process termination exists.
See [uninstall](cli.md#uninstall) for reporting/exit semantics and limitations.

```sh
cargo test -p ghostlink --test uninstall --test update --test skill_drift --test agents --test handler_parsing
cargo test -p destiny-adapters --all-features uri_handler
cargo fmt --all --check
cargo clippy -p ghostlink --all-targets -- -D warnings
```

Uninstall tests use only retained private temporary fixture trees, dummy installed
binaries and fake `xdg-mime` queries, never actual HOME, credentials or integrations.
They cover preview no-writes, consent/cancellation, ownership/symlinks, optional
updater, lock contention, changed snapshots, active-handler unsafe skips and partial
filesystem failures, JSON shape, Windows guidance and panic-on-access factories.
The handler regression models a higher-priority active Ghostlink association with
an unrelated user MIME file: without an owned clearing rewrite, desktop/record stay.
Real desktop behavior, hostile concurrent local replacement, Windows execution and
live installation/removal are not proven by these fixtures.

## Baseline checks

CI (`.github/workflows/ci.yml`) runs fmt, clippy, build, and the all-features
tests on Linux and Windows. The Windows callback handler is a later phase;
macOS is out of scope.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --workspace --all-features
cargo run -p ghostlink --bin ghostlink -- --help
cargo run -p ghostlink --bin destiny-mcp -- --help
```

Application tests require no credentials and do not call live services. Cargo
may fetch crates when they are not cached. Linux builds require `libdbus-1-dev`
and `pkg-config` (Debian/Ubuntu package names).

The `destiny-mcp` package is a library (plus a test-only `offline-fixture`
binary); the `destiny-mcp` executable is built by the `ghostlink` package, next
to `ghostlink`.

## Release packaging

Keep user-facing changes in the [next release-notes draft](release-notes/next.md) before tagging.

`dist-workspace.toml` pins cargo-dist 0.32.0. Install that version with
`cargo install cargo-dist --version 0.32.0 --locked`. After changing packaging
configuration, regenerate `.github/workflows/release.yml` with `dist generate`;
`dist generate --check` checks that the workflow matches the configuration.

The generated workflow plans on pull requests and publishes on version-tag
pushes, such as `v0.1.0`, matching the workspace version. Only an authorized
maintainer should create/push release tags. Its Linux build matrix installs
`libdbus-1-dev` and `pkg-config` from `[dist.dependencies.apt]`; Windows uses MSVC.
No macOS builds or package-manager installers are configured.

One archive per platform contains **both** `ghostlink` and `destiny-mcp`:

- `ghostlink-x86_64-unknown-linux-gnu.tar.xz`
- `ghostlink-x86_64-pc-windows-msvc.zip`

One installer per platform places both executables in the **same directory**:
`$CARGO_HOME/bin`, falling back to `~/.cargo/bin` (PowerShell uses the equivalent
Windows paths). This is required by `ghostlink agents install`, which finds MCP
beside the running CLI. The standalone MCP package has `dist = false`; libraries
and the feature-gated `offline-fixture` are not shipped as executable artifacts.
Release builds use the `dist` profile without test-only features.

After the maintainer publishes a release, the one-line installers are:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/Yoryoboy/ghostlink/releases/latest/download/ghostlink-installer.sh | sh
```

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/Yoryoboy/ghostlink/releases/latest/download/ghostlink-installer.ps1 | iex"
```

The installers also install `ghostlink-update` (`ghostlink-update.exe` on
Windows). Run `ghostlink-update` to update both product executables together;
`ghostlink-update --version 0.1.0` selects a version. The updater uses the
installer receipt; it is not a `ghostlink` subcommand and is a separate release
asset, not an executable inside the product archive.

### Linux runtime

The executables dynamically link `libdbus-1.so.3`, normally already present on
virtually all desktop Linux distributions. Minimal systems need the `libdbus-1-3`
Debian/Ubuntu runtime package (or their distribution's equivalent). Authentication
also requires a running D-Bus session and Secret Service keyring provider.

The keyring store supports `vendored`, forwarded through `dbus-secret-service`
and `dbus` to `libdbus-sys`, which statically builds bundled libdbus. We retain
system linkage: desktop systems already provide it, system security updates
remain effective, and bundling does not remove the session/keyring requirement.

### Local verification (no publication)

```sh
dist plan
dist build --artifacts=local --target x86_64-unknown-linux-gnu
```

Artifacts are written under this worktree's `target/distrib/`. Inspect/extract the
archive there and run only `ghostlink --version` and `destiny-mcp --help` for
credential-free smoke checks. Local installer generation is available with
`dist build --artifacts=global`; this builds files only, without uploading.
Never run installers against a developer's real home during verification.
