# Phase 7c: command palette, Settings, About / updates

Branch `p7/palette-settings`. Spec: CommandPalette.tsx, SettingsView.tsx,
AboutWindow.tsx, App.tsx (⌘K / ⌘P / ⌘,), state/prefs.ts, lib/format.ts
(fuzzy), lib/updates.ts, src/updater.rs, StatusBar.tsx `UpdatePill`.

## What's built

- **Command palette** (`views/command_palette.rs`, ⌘K / ⌘P toggle, also the title
  bar chip and the sidebar's Search). Empty query: "Recent" (Workspaces MRU,
  prefs `recentCount`) then "All tables" (untitled when nothing is recent);
  otherwise fuzzy matches (`boa_core::grid::format::fuzzy`) on the name (+5,
  highlighted) or "schema.table database connection", best first. "Actions"
  (Query History, ⌘⌥E) follow when every typed word starts one of its words.
  Rows: table / view icon, name, "view" badge, connection tile
  (`oklch(0.76 0.09 hue)`), `db/schema`, "5m · 100k rows", ⏎ on the selection.
  ↑ ↓ Tab ⇧Tab move (wrapping), ⏎ opens the table pinned or runs the action,
  Esc / a click outside closes. Root rules: ⌘K closes Settings; opening
  Settings and ⌘⌥E close the palette; while it is open ⌘O and the workspace
  shortcuts (⌘T, ⌘W, ⌥ keys) pass, as App.tsx's `overlay` check.
- **Settings**: every General pref with the source's labels, help texts and
  choices; switches redrawn with white thumbs (Phase 1 deviation fixed);
  Connections page now lists every saved connection with its state dot,
  `PostgreSQL · postgresql://… · 12 ms` and Connect / Disconnect / Retry now;
  typing in the search from another page no longer loses the focus.
- **Updates** (`models/updates.rs`) on boa-core's `Updater`: fetches
  `latest.json` from `BOA_UPDATE_ENDPOINT` only (file://, http://, https://
  with redirects), `select_update` for this Mac, events for About and the pill.
  No endpoint: "Update check failed: not available in this build yet".
  Download / install: Phase 8 (a newer release ends with "Boa X is available,
  but this build can't download updates yet"; a pluggable downloader exists
  for tests / dev; "Restart to update" shows a "not available yet" toast).
  Periodic checks: release builds with an endpoint, unless BOA_NO_UPDATE_CHECK.
- **About** uses `boa_core::state::updates` (AppInfo, version line, date
  label, status lines, busy), spinner while checking, "Restart to Update"
  when ready. **Status bar** shows "Restart to update to X" when ready.

## Screenshots

| File | What |
|---|---|
| `palette.png` | ⌘K, empty query: Recent + All tables (82 tables, views badged) |
| `palette-search.png` | "ord": name matches first, highlighted |
| `settings-general.png` | General page, white switch thumbs |
| `settings-search.png` | search "history" |
| `settings-connections.png` | Connections page with the live dev connection |
| `about.png` | About after "Check for Updates…" on a same-version feed |
| `status-bar-update.png` | the "Restart to update to 99.0.0" pill |

Dev hooks (feature `dev-hooks`): `BOA_DEV_PALETTE=open|<query>`,
`BOA_DEV_PALETTE_RECENT=public.orders,…`, `BOA_DEV_PALETTE_ALL_SCHEMAS=1`,
`BOA_DEV_SETTINGS=search:<text>`, `BOA_DEV_ABOUT=check`, `BOA_DEV_UPDATE=ready`,
and `BOA_SCREENSHOT_WINDOW=about` (screenshot the About window).

## Tests

Palette: groups / recents / ranking / highlight / actions / wrap unit tests;
window tests for ⌘K ⌘P Esc, Settings interplay, typing + ↑ ↓ Tab ⇧Tab, ⏎ on a
table (pinned, MRU, Recent, recentCount) and on the action. Settings: search,
every switch (clicked) and every choice writing prefs.json, reset keeping the
layout, Connections page. Updates: URL / HTTP parsing; checks against a local
HTTP server (newer via a redirect, downloaded → pill, same, malformed, bad
version, 404) and a file:// feed; no endpoint.

## Not done / for the lead

- Download, signature check, install, relaunch: Phase 8 (needs the user's OK).
  The real feed (`UPDATE_ENDPOINT`) is not used by default yet.
- Opening a table from the palette moves the focus to its grid; without a
  server (tests) the grid isn't drawn, so the focus leaves the dispatch tree
  (pre-existing Workspace behaviour).
- Seen during dev runs: "Could not read boa_dev: current transaction is
  aborted" when a scope change reloads the catalog while table tabs fetch.
