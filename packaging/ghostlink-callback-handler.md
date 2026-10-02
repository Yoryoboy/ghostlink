# Ghostlink callback handler (Linux)

This is the operator runbook for the Linux `ghostlink://oauth/callback` handler.
The handler is implemented and tested offline; nothing here is installed
automatically, and no agent runs these steps. Installing it changes the current
user's desktop configuration and is a human decision.

## What it does

`ghostlink login` renders the authorization URL, and with
`ghostlink login --callback handler` it also binds one per-attempt Unix-domain
socket inside a private directory under the user's runtime directory
(`$XDG_RUNTIME_DIR/ghostlink`). When the provider redirects the browser to
`ghostlink://oauth/callback`, the desktop entry runs
`ghostlink callback <uri>`, which validates the URI shape, resolves the
per-attempt socket, and forwards the URI once to the waiting login. The shared
`AuthorizationService` still compares the callback state, rejects replay and
expiry, and exchanges the code.

A successful `ghostlink callback` run reports delivery only. Its exit status
means the URI reached a waiting login over the rendezvous; it never means the
authorization succeeded. Only the waiting `ghostlink login` validates the state
and exchanges the code, and only it reports the authorized (or failed) result.
The handler prints no callback values; on failure it may print a sanitized error to stderr.

The default is `ghostlink login --callback paste`, which reads the callback from
the terminal and is unchanged. The handler is opt-in; the paste strategy remains
the fallback on every platform and whenever the handler is unavailable.

## Threat model

The rendezvous defends against other local users and against accidental
collisions. It does **not** defend against a hostile process already running as
the same user. Such a process can read the pointer, replace the lock or pointer
path, or repoint the pointer at a socket it controls, and could then receive or
redirect the callback. That is accepted and out of scope: the handler is no
stronger than the desktop session it runs in.

## Security model

- The rendezvous is owner-only. The socket lives under the per-user runtime
  directory in a directory and files created owner-only (`0700` directory,
  `0600` files); a rendezvous directory that is group- or world-accessible is
  refused. No other local user can read the pointer or connect to the socket.
- The per-attempt socket name carries a fresh 128-bit random token, and a fixed
  pointer names only the current attempt. The token is a uniqueness and
  anti-collision measure, **not authentication**: it is readable by this user
  through the pointer, and it does not prove that the handler reached the login
  that published it. The pointer is a symlink resolved with `readlink`, so the
  handler never opens it as a file and a prepared FIFO or symlink in the
  directory cannot make the resolution block while the one-use code sits in the
  handler's argv. Forwarding refuses a resolved name that is not exactly a valid
  per-attempt socket name, so a pointer that names anything off the rendezvous
  is refused before it is connected to.
- Only one login may hold the rendezvous per user at a time. A fixed lock file
  in that private directory is held for the whole attempt under an
  operating-system file lock, so a second concurrent login fails closed before
  it can overwrite the first attempt's pointer or steal its callback. The kernel
  releases the lock when the owning process exits, so a killed or crashed login
  never leaves a stale lock that blocks the next attempt. This guarantee is
  pathname-based: a hostile same-user process that replaces the lock path is
  outside the threat model above.
- The handler validates the URI shape, resolves the pointer without opening its
  target, and forwards once. A timeout bounds the handler's wait for the connect
  worker; a timed-out worker can remain blocked until the short-lived handler
  process exits. Socket write timeout applies per operation, not as an overall
  deadline. No strict end-to-end forwarding deadline is claimed. On failure the
  handler can write a sanitized error to stderr, never the callback value.
- The callback payload is bounded on both sides and must be exactly the
  registered custom-scheme callback; nothing is rendered, logged, or persisted,
  and forwarded bytes are never written to disk.
- The state binding is cryptographic. Even if some process reaches the socket,
  it cannot authorize anything without the pending random state, which the
  shared service compares.
- Known exposures, not claimed away:
  - A desktop `%u` argument necessarily places the one-use authorization code in
    the handler process's argv for the moment it forwards it, where the same
    user's `ps` could observe it. The handler minimizes the window by validating,
    bounding the wait for connect, forwarding once, and never printing the code;
    failure may print a sanitized error to stderr.
  - A hostile same-user process can defeat the singleton, pointer, and cleanup,
    as described in the threat model above; the design does not resist it.

## Prerequisites

- Linux with `XDG_RUNTIME_DIR` set (a normal desktop session sets it). Without
  it the handler path is unavailable and only the terminal paste works.
- A built `ghostlink` binary. Record its absolute path; desktop entries do not
  inherit the interactive shell `PATH`.

## Install (only after explicit authorization)

1. Record the association this machine already uses, so Revert can restore the
   exact previous handler instead of guessing:

   ```sh
   xdg-mime query default x-scheme-handler/ghostlink
   ```

   Keep the output. It is the desktop file this machine currently uses for
   `ghostlink://` (for example `google-chrome.desktop`), or empty when no
   handler is associated. Do not assume a particular browser: the recorded value
   is what Revert must put back.

2. Copy the template and replace `ghostlink` in `Exec` with the absolute binary
   path:

   ```sh
   mkdir -p ~/.local/share/applications
   cp packaging/ghostlink-callback.desktop ~/.local/share/applications/ghostlink-callback.desktop
   # Edit the installed copy so Exec names the absolute binary path.
   ```

   The `Exec` key is parsed by the desktop-entry rules, not by a shell, so a
   path containing spaces or reserved characters (`"` `'` `` ` `` `$` `\` `&`
   `|` `;` `<` `>` `~` `*` `?` `#` `(` `)`) must be quoted. Enclose the binary
   path in double quotes and escape `"`, `` ` ``, `$`, and `\` with a backslash
   (the general string escaping applies first, so a literal backslash inside a
   quoted argument is written with four backslashes). A literal `%` in the path
   is written `%%`, because `%` introduces field codes; the required `%u` field
   code itself stays unescaped. Prefer a binary path without spaces. Examples:

   ```ini
   Exec=/usr/local/bin/ghostlink callback %u
   Exec="/opt/ghostlink tools/bin/ghostlink" callback %u
   ```

   Validate the installed entry instead of guessing:

   ```sh
   desktop-file-validate ~/.local/share/applications/ghostlink-callback.desktop
   ```

3. Refresh the desktop database and register the scheme handler:

   ```sh
   update-desktop-database ~/.local/share/applications
   xdg-mime default ghostlink-callback.desktop x-scheme-handler/ghostlink
   ```

4. Verify the association, then authorize:

   ```sh
   xdg-mime query default x-scheme-handler/ghostlink
   ghostlink login --callback handler
   ```

## Troubleshooting: the browser opens instead of the handler

Symptom: after authorizing, the browser opens a new window or a profile
chooser, and `ghostlink login --callback handler` keeps waiting for the
callback. The callback was dispatched, but to the browser. The
`xdg-mime query` in Install step 4 can report the handler correctly while
another layer still selects the browser. Check both layers below.

1. **Contradictory `mimeapps.list` sections.** An earlier choice can leave the
   browser under `[Added Associations]` while `[Default Applications]` names
   the handler. Inspect both sections, not only the `xdg-mime` result:

   ```sh
   grep -n 'x-scheme-handler/ghostlink' ~/.config/mimeapps.list
   ```

   Every line should name `ghostlink-callback.desktop`.

2. **Remembered portal choice (Wayland).** Browsers such as Chrome on a Wayland
   session can open custom schemes through `xdg-desktop-portal`, which keeps
   its own per-application "open with" history in the permission store table
   `desktop-used-apps`. Once a choice reaches its threshold, the portal stops
   asking and launches it directly, regardless of `mimeapps.list`. Inspect it:

   ```sh
   busctl --user call org.freedesktop.impl.portal.PermissionStore \
     /org/freedesktop/impl/portal/PermissionStore \
     org.freedesktop.impl.portal.PermissionStore \
     Lookup ss desktop-used-apps x-scheme-handler/ghostlink
   ```

   A reply such as `"com.google.Chrome" 3 "google-chrome" "3" "3"` means the
   portal launches the browser. Record that reply for Revert, back up
   `~/.local/share/flatpak/db/desktop-used-apps`, then point the requesting
   application's entry at the handler (the third value is the desktop id
   without `.desktop`):

   ```sh
   busctl --user call org.freedesktop.impl.portal.PermissionStore \
     /org/freedesktop/impl/portal/PermissionStore \
     org.freedesktop.impl.portal.PermissionStore \
     SetPermission sbssas desktop-used-apps true x-scheme-handler/ghostlink \
     com.google.Chrome 3 ghostlink-callback 3 3
   ```

   Replace `com.google.Chrome` with the application id from the Lookup reply.
   To revert, run the same `SetPermission` with the recorded values.

The journal shows which desktop file actually received the callback:

```sh
journalctl --user --since "5 min ago" | grep -Ei 'ghostlink|chrome'
```

## Revert

Restore exactly the association recorded in Install step 1, then remove this
handler. Substitute the recorded value; never hardcode a browser, because the
previous handler may be any desktop file, or none at all.

```sh
# Replace <captured-handler> with the value recorded before installing.
xdg-mime default <captured-handler> x-scheme-handler/ghostlink
rm ~/.local/share/applications/ghostlink-callback.desktop
update-desktop-database ~/.local/share/applications
```

When the recorded value was empty — no handler was associated before
installation — do not name one. Remove the `x-scheme-handler/ghostlink` entry
from `~/.config/mimeapps.list` instead, then remove the desktop file and refresh
the database as above. Removing the desktop file and association leaves the
terminal paste fallback fully functional.

## Offline evidence

The handler and its IPC are covered by offline tests only:

```sh
cargo test -p destiny-adapters --all-features
cargo test -p destiny-cli --all-features
```

No live authorization, desktop registration, or provider call is performed or
claimed by this runbook.

## Human live validation

On 2026-10-01 the human operator completed `ghostlink login --callback handler`
live on KDE Plasma 6 (Wayland) with Google Chrome, after applying the portal fix
from Troubleshooting. The login reported `authorized` with an expiry and no
refresh token. That is one environment, recorded by the operator; other
desktops and browsers are unverified.
