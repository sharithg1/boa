# Phase 7d: session restore, localStorage import, introspection crash fix

Branch `p7/session`. Spec: source `docs/USAGE.md` "Session restore",
`ui/src/state/session.ts` (+ `session.test.ts`), `store.ts` (`recover`,
`connectNow`, `startSession`).

## What it does

- **One writer of session.json**: `models::session::Session`. Layout no longer
  writes the file; it exposes `restore_panels` / `panels()` and the Session
  saves its flags with everything else.
- **Restore at launch, before the window opens** (`Session::start`, called at
  the end of `AppState::init`): each database's tabs (consoles with their SQL,
  table tabs with WHERE / ORDER BY and the preview flag), the active tab, the
  selected database, MRU / seen, the panel flags. Sessions are pruned to the
  saved connections (`parse_session`, the Tauri rules). Consoles are not
  re-run; a table tab reads its first page once its connection is back (as
  any table tab does).
- **Reconnect** (`Session::reconnect`, from `app.rs` after init):
  `connections_to_restore`: the selected database's connection first (it may
  open the password prompt; once back its database is loaded), the others
  quietly (a refused login un-wants them). Each retries on `RETRY_DELAYS_MS`
  while unreachable, then shows "Could not connect to …" with Retry. Restored
  ids are `wanted` at once, so quitting before they are back keeps them.
- **Save**: 250 ms after the last change of Workspaces, Layout or Connections
  (the `wanted` set), or of a console's text (`Session::touch` from the
  editor's `Changed`); at most ~2 s while changes never pause; only when the
  JSON differs from what was last read / written; flushed on quit
  (`on_app_quit`). Written through temp file + rename, mode 0600.
- **Format**: exactly the Tauri app's `JSON.stringify(pick(state))`: keys
  `active, workspaces, mru, seen, wanted, sidebarOpen, rightPanelOpen,
  bottomOpen` in that order, nothing else (the TS dropped unknown keys too).
- **localStorage import** (`models::session::import_local_storage`, startup,
  before any model reads its file): `boa_core::state::local_storage::import_once`
  runs `import_if_needed` at most once per config dir, gated by the marker
  `.localstorage-imported` (also written when prefs.json and session.json
  both exist already). Never fails startup (panics are caught and logged).
  Source home: `$HOME` with the real config dir; with `BOA_CONFIG_DIR` set
  only `$BOA_IMPORT_HOME` (so dev runs never pull in the user's data).
- **Crash fix** (`boa-core/src/pg.rs`): introspection panicked decoding a NULL
  `pg_total_relation_size` of a table dropped while the query ran. RELATIONS
  rows now decode with `try_get`; a table / matview / partitioned table
  without a size has vanished and is skipped, other NULLs read as defaults.

## Tests

- boa-core: `live_introspection_survives_a_concurrent_drop` (deterministic:
  another session drops the table in an open transaction, introspection blocks
  on its lock, the drop commits; verified to panic before the fix),
  `live_relation_rows_with_nulls_decode`, 3 `import_once` tests.
- boa-app `models/session/tests.rs` (13 + 1 ignored): round trip through the
  real app init (save → parse with the Tauri rules → second app on the same
  dir), debounce + unchanged-skip + max delay, flush on quit, pruning of a
  deleted connection, retry schedule (1/2/4/8/15 s → connected after 30 s),
  give-up after the schedule, selected-first / quiet others, old sessions
  without `wanted`, no selection, corrupt file, Tauri key order / no unknown
  keys / 0600, scripted layout, import marker. `real_migration_copy`
  (ignored): runs on a copy of a real install (`BOA_MIGRATION_DIR`).

## Migration check (copy of the real install, counts only)

`config/` + `home/Library/WebKit/dev.boa.desktop` copied to a scratch dir:
import found the localStorage db and imported `session.json` (prefs.json
already existed in the real config dir; no `boa.csvExport` key). Restored: 3
saved connections (all with passwords), 1 database, 1 console tab, active
database set, MRU 1, seen 1, panels open; 2 connections to reconnect; prefs:
2 scopes, 3 expanded nodes. The rewritten session.json parses with the Tauri
rules. The copy was deleted afterwards.

## Screenshot

`session-restored.png`: a dev run on a temp config dir whose session.json
had a console and an `orders` table tab (WHERE `id < 50`, ORDER BY
`id desc`, active) and the right panel closed: both tabs back, the table tab
re-read its page after the connection was restored.

## Notes / gaps

- USAGE.md says restore retries "about half a minute"; the source code uses
  the same `RETRY_DELAYS_MS` as any drop (~2 min). Ported as the code does.
- `wanted` is a `BTreeSet` in Connections, so it is saved sorted, not in
  connect order (parse-compatible; only the selected one's priority matters).
- Workspaces::to_session writes workspaces and `seen` sorted by key (the
  Tauri app kept insertion order). Parse-compatible.
