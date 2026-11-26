# Phase 2: connections

Branch `p2/connections`. Spec: source `docs/USAGE.md` (Data sources, SSH tunnel,
Staying connected), `ConnectionDialog.tsx` (+ test), `ConnectionTag.tsx`,
`StatusDot.tsx`, `store.ts` (connection + reconnect parts), `links.ts`,
`reconnect.test.ts`, `connTag.ts`, commit 61984bb (one status toast per connection).

## What's built

| Piece | Where |
|---|---|
| `Connections` model: saved list in `connectionOrder`, live state, connect / disconnect / `ensure_connected` (joins a connect in flight), save / delete / test / reorder, password prompt state, reconnect loop (boa-core `Recoveries` driven by GPUI timers on `Backend::link_events` + `ping`), one keyed status toast per connection, tx-lost toasts, `describe_error` (tx lost / commit unknown), `restore()` (Phase 7), close-guard hook (Phase 6) | `crates/boa-app/src/models/connections.rs` |
| macOS did-wake (NSWorkspace) + network change (SystemConfiguration dynamic store) → check connections; plus the 5 s wake tick of the Tauri app | `crates/boa-app/src/platform/` (the only module with a scoped `#[allow(unsafe_code)]`) |
| Connection dialog: URL paste, Name / Host / Port / User / Password / Database, SSL mode, environment + colour, save password, read-only, SSH tunnel (agent or key file, with a Browse… file picker), Test connection, Save (and connect), edit, Delete with second-click confirm; `confirm_delete` alert for the explorer; ⌘O | `crates/boa-app/src/views/connection_dialog.rs` |
| Password prompt (opens on the model's request, closes when connected, Save password) | `crates/boa-app/src/views/password_prompt.rs` |
| ConnectionTag (env badge + lock), colour stripe, status dot per state | `crates/boa-app/src/views/conn_tag.rs` |
| Dev hooks (feature `dev-hooks`): `BOA_DEV_CONNECTION=1` seeds "Local dev" (localhost boa/boa boa_dev, password saved) into an empty `BOA_CONFIG_DIR` and connects at launch; `BOA_DEV_RECONNECT=1` adds "Unreachable" (localhost:1) and recovers it; `BOA_DEV_DIALOG=new\|url\|ssh\|edit\|prompt` | models/connections.rs `dev`, connection_dialog.rs `dev` |

The source dialog has no tabs: General and SSH are one form, the SSH fields
show under the "SSH tunnel" checkbox. The GPUI dialog follows it.

## Screenshots (real window, `screencapture -l`)

- `dialog-general.png`: new data source.
- `dialog-url-pasted.png`: `postgresql://analyst:s3cret@db.internal.example.com:6432/warehouse?sslmode=require` pasted.
- `dialog-ssh.png`: SSH tunnel with key file, Production tag (note shown); the form scrolls when taller than the window.
- `dialog-edit.png`: editing "Local dev" (`<stored>` password, Delete).
- `password-prompt.png`: the prompt with a refused-password error.
- `lost-connection-toast.png`: the real end of the reconnect loop: "Unreachable" retried for ~2 min (1, 2, 4, 8, 15, 30, 30, 30 s), then failed with the keyed "Lost connection" toast and Retry.

The explorer / status bar in these shots are still Phase 1 placeholders (the
spike connection); showing per-connection state there is p3/explorer's.

## Tests

- `models/connections/tests.rs`: every store-level case of `reconnect.test.ts`
  (incl. "a keyed toast replacing a timed one keeps its own lifetime" and "running a
  query connects first"), password prompt flow, refused logins (28P01 / 28000 /
  password_required), one toast per connection, save / delete / reorder / close
  guard / restore / network debounce; live: connect to boa_dev, refused login →
  prompt, `pg_terminate_backend` of our session mid-query → `LinkEvent::Lost` →
  recovery, read-only enforced by the server (`read_only` flag and role `boa_ro`).
- `views/connection_dialog/tests.rs`: ⌘O, URL paste (typed), validation, save writes
  `connections.json` (password in the file, camelCase fields), save without password,
  edit keeps the stored password, delete confirm, password prompt open / Esc / close
  / save + connect.

## Manual checks for the user

1. **SSH tunnel with a real server**: add a data source with "SSH tunnel" (agent, then
   key file via Browse…), Test connection, Save and connect, run a query. Remote Login
   is off on this machine, so no automated end-to-end SSH test exists.
2. **Sleep / wake**: connect, put the Mac to sleep for a minute, wake it: the
   connection is checked at once (log line "checking connections" with
   `BOA_LOG=info`); a dropped session is reopened, an open transaction lost with it
   shows "Transaction rolled back".
3. **Network change**: connect, turn Wi-Fi off and on: ~2 s after the change the
   connections are checked; while the server is unreachable the connection shows as
   reconnecting, then "Reconnected to …".
4. **Wrong password**: the local server trusts localhost, so a refused password
   (28P01) was only tested on a scripted backend; try one against a server with
   password auth: the prompt shows the server's message.
