---
name: run-dbclient-desktop
description: Run, start, drive or screenshot the DB Client desktop UI (crates/dbclient-desktop/ui) against its mock backend in headless Chrome — connect a data source, expand the explorer, open tables, type and run SQL in a console, read completions / result grids / the output log, take screenshots. Use to verify a UI change in the running app, not just in unit tests.
---

# Run the DB Client UI

The UI is a React + Vite app. In a plain browser (`isTauri()` false) it runs on
the **mock backend** (`src/lib/backend.ts` → `mockBackend()`, sample data in
`src/state/mock.ts` / `mockData.ts`), so no Rust, Tauri or Postgres is needed
to exercise UI changes. `driver.mjs` loads it in headless Chrome via
`playwright-core` and runs a script of commands from stdin.

Paths below are relative to the repo root. Verified on macOS.

## Prerequisites

- Node from nvm (no `node` on the default PATH):
  `export PATH="$HOME/.nvm/versions/node/v24.18.0/bin:$PATH"` — `pnpm` lives there too.
- Google Chrome in `/Applications` (the driver uses `channel: 'chrome'`, so
  no Playwright browser download).

## Setup (once per worktree)

```sh
export PATH="$HOME/.nvm/versions/node/v24.18.0/bin:$PATH"
pnpm --dir crates/dbclient-desktop/ui install --frozen-lockfile
npm install --prefix .claude/skills/run-dbclient-desktop   # playwright-core for the driver
```

## Run (agent path)

1. Start Vite in the background (Bash `run_in_background: true`). Not on 1420
   (Tauri's `devUrl`; a `pnpm dev` may hold it) and not 5199 / 5299 (the Civet
   skill and stray Vites use those). In a worktree, pick your own port so two
   agents don't share one.

   ```sh
   cd crates/dbclient-desktop/ui && PATH="$HOME/.nvm/versions/node/v24.18.0/bin:$PATH" pnpm exec vite --port 5273 --strictPort
   ```

   "Port 5273 is already in use": check whose it is before reusing it
   (`lsof -a -p $(lsof -tiTCP:5273 -sTCP:LISTEN) -d cwd -Fn`); a Vite of
   another checkout serves that checkout's code. Otherwise take another port
   and pass the URL to the driver.

2. Wait for it (up to ~30 s; stop if it never answers), then pipe a script to
   the driver. Screenshots go to `$SHOTS` (default `./shots`) — point it at
   your scratchpad.

   ```sh
   curl -sf --retry 30 --retry-connrefused --retry-delay 1 -o /dev/null localhost:5273 || { echo "Vite is not up"; exit 1; }
   SHOTS=<scratchpad>/shots node .claude/skills/run-dbclient-desktop/driver.mjs <<'EOF'
   connect Production
   expand app_production
   expand prod-pg/app_production/public
   expand tables
   tree
   node users
   tabs
   grid 3
   opt n
   sql-type select * from us
   completions
   key Enter
   sql-type  where id < 5;
   run
   grid 3
   output
   ss console
   EOF
   ```

   Output: `tabs ["users*<"]`, the grid as tab-separated rows, `screenshot
   <path>`. Read the PNG to look at it. Exit code 1 and `FAILED <line>:
   <reason>` on the first step that fails; a page exception is printed as
   `pageerror during \`<step>\`: …` and also makes it exit 1. The driver
   refuses a page whose title isn't `DB Client` (wrong port).

   Another port or path: `node driver.mjs http://localhost:5280/`.

3. Stop Vite: `pkill -f "vite --port 5273"`.

### Commands (one per line, `#` comments allowed)

Explorer rows are named by their label (`users`; the first match wins) or by
node id when the name contains a `/` (`prod-pg/analytics/public`, ids are
printed by `tree`). Connection ids: `prod-pg` (Production), `staging-mysql`
(Staging), `local-sqlite` (todos.db).

| Command | Does |
|---|---|
| `connect <row>` | Expand a connection row (that connects it) and wait until it is connected. |
| `tree` | Print the visible explorer rows: indent, `+` collapsed / `-` expanded, label, `(meta)`, `[node id]`. |
| `expand` / `collapse <row>` | Open / close a row; no-op when it already is. |
| `node <row>` | Click a row: a database becomes the active one, a table opens as a preview tab. |
| `dblclick-node` / `rclick-node` / `hover-node <row>` | Double-click (a table: pinned tab), right-click (context menu, then `menu-item`), hover. |
| `sql <text>` | Replace the active console's text (`\n` = newline); caret at the end. Does not open completion. |
| `sql-type <text>` | Focus the console and type at the caret key by key (opens completion). Spaces are kept as written. |
| `caret <n>` | Put the console caret at offset `n` (negative: from the end). |
| `run` | ⌘↵ twice / ⌘↵ + Enter: run the statement at the caret (the whole statement when the caret is in a subquery), and wait until it finishes. |
| `completions` | Print the open completion list, `>` = selected. Accept with `key Enter` (insert) or `key Tab` (replace the word). |
| `grid [n]` | Print the last result grid on screen (a table tab's data, or the Database Output result): header, first `n` rows (default 10), row count. |
| `cell` / `dblclick-cell <row:col>` | Click / double-click a grid cell (0-based, the `data-cell` attribute). |
| `output` | Switch the Database Output panel to its Output log and print it. |
| `output-tab <text>` | Click a tab of the Database Output panel (`Output`, or a result tab). |
| `right <structure\|info\|history>` | Switch the right panel tab. |
| `click-tab` / `dblclick-tab` / `close-tab <label>` | An editor tab in the title bar. |
| `tabs` | Print the editor tab strip: `*` = italic (preview tab), `<` = active. |
| `click-button` / `hover-button <name>` | A button by its exact accessible name (icon buttons: the tooltip label, e.g. `Next page`, `Query history`, `Collapse all`). |
| `menu-item <text>` | An item of the open dropdown / context menu, by the start of its text. |
| `fill <label> = <value>` | Fill a field by its label, aria-label or placeholder (`fill Host = db.example.com`, `fill Filter loaded rows = paid`). |
| `click-text` / `rclick` / `hover-text <text>` | Click / right-click / hover any element by exact visible text. |
| `click <selector>` | Click the first match of a CSS / Playwright selector (`click [title="Default schema"]`, `click text=Tx: Auto`). |
| `type <text>` | Type into the focused element; spaces kept as written. |
| `key <combo>` | `page.keyboard.press` (`Escape`, `Meta+k` palette, `Meta+o` new data source, `Meta+,` settings, `Control+Space` completion). Not for ⌥ shortcuts — use `opt`. |
| `opt <key>` | An ⌥ shortcut: `opt n` new console, `opt j` Database Output, `opt b` sidebar, `opt e` right panel, `opt 2` second tab, `opt ArrowDown` next database. |
| `drag-separator <label> <d>` / `dblclick-separator <label>` | Drag a resize handle (`Resize sidebar`, `Resize right panel`, `Resize output panel`) `d` px along its axis (negative = left / up), or double-click it (reset). |
| `scroll <dy> <selector>` | Mouse wheel over an element. |
| `text [selector]` | Print the visible text of every match (body by default, first 3000 chars). `text aside >> nth=1` is the right panel. |
| `eval <js>` | Evaluate an expression in the page and print it. |
| `js <expr>` | Run JS in the page with the app's modules in scope: `store` (state/store.ts), `prefs` (state/prefs.ts), `s` (current store state). Opens anything directly: `js store.useStore.setState({ settings: 'keyboard' })`, `js store.pushToast({ kind: 'error', title: 'x', body: 'y' }, 0)`, `js prefs.setPrefs({ rowLimit: 50 })`. |
| `ss [name] [-- desc]` | Screenshot to `$SHOTS/<name>.png`; a description is recorded in `manifest.json`. |
| `ss-el <name> <selector> [-- desc]` | Screenshot one element: `aside` = explorer, `aside >> nth=1` = right panel, `section` = Database Output. |
| `viewport [WxH]` | Resize the window mid-script; bare `viewport` restores the run's size (`VIEWPORT`). |
| `goto <path>` | Load a URL relative to the start URL (`goto /?overlay` previews the macOS title-bar inset). Resets app state. |
| `fresh` | Clear localStorage (prefs, tree state, scopes) and reload. |
| `toasts off` / `toasts on` | Hide / show toast notifications. |
| `unhover` | Move the mouse to the corner (drop hover / tooltip). |
| `wait [ms]` | Sleep (default 300). Every other command already waits 150 ms after. |

Add a command to `driver.mjs` when you need a new interaction; keep it small.

Driver env: `SHOTS`, `SCALE` (default 2), `VIEWPORT` (`1400x900`),
`KEEP_GOING=1` (log a failed step and go on, 5 s timeout), `HEADED=1`.

## Design review (screenshot tour, before/after)

`tour.txt` walks every screen, panel, menu and dialog (~47: explorer, scope
pickers, context menus, table data, console, completion, statement chooser,
results, output log, right panel, palette, dialogs, settings pages, hidden
panels, narrow window, toast). `shoot.sh` runs it at each window size, at 2×
scale, on its own Vite. With `--base <ref>` it also runs the tour against
`<ref>` in a temporary git worktree and writes a before | after composite
(changed pixels tinted magenta) for each screen that changed.

```sh
.claude/skills/run-dbclient-desktop/shoot.sh <scratchpad>/review --base main
# options: --sizes "1400x900 1100x720" (default)  --tour <file>
# ports: PORT (5274, this checkout), BASE_PORT (5275, the base ref)
```

Output: `<out>/after/<size>/NN-name.png`, and with `--base` also
`<out>/before/<size>/` and `<out>/compare/<size>/` plus a `same` /
`N.NN% changed` line per screen. Read the PNGs to review them.

- The tour runs with `KEEP_GOING=1`: a step that fails (UI that doesn't exist
  on the base ref yet) is logged as `FAILED` and the tour goes on. Screens
  after a failed step may show a different state.
- Small diffs on result / output screens are mock noise: query timings and
  log timestamps change between runs.
- Add a screen by adding steps + an `ss NN-name -- what it shows` line to
  `tour.txt`; keep the names numbered and stable so before/after pairs line up.

`catalog.sh [dir] [size]` captures the tour into `<dir>/screenshots/` with a
`README.md` index and review brief (default `docs/ui-review`, which is not
gitignored here — pass a scratchpad dir unless the catalog is meant to be
committed).

## Run (human path)

`pnpm --dir crates/dbclient-desktop/ui dev:web`, then open
`http://localhost:1420`. The real app (`… dev`) needs the Rust toolchain and a
Postgres server; not covered here.

## Test

```sh
pnpm --dir crates/dbclient-desktop/ui typecheck && pnpm --dir crates/dbclient-desktop/ui lint && pnpm --dir crates/dbclient-desktop/ui test
```

## Gotchas

- Every data source starts **offline**; nothing is selectable until
  `connect Production`. `Staging` and `todos.db` connect too (sample MySQL /
  SQLite data); a data source you add in the dialog fails to connect ("No
  server here").
- Only picked databases / schemas are shown: Production lists
  `app_production` only (1 of 2) and its `public` schema (1 of 2). `audit`
  and `analytics` need the "N of M" badge (`click [title="Choose schemas to
  show"]`). A console can still query them: `sql select * from audit.events;`
  (the only table with a `jsonb` column, for the JSON viewer).
- The mock runs no SQL: `select … from <table>` returns that table's sample
  rows whatever the `where` / `join` / `limit`, and expressions like
  `count(*)` fail with "column does not exist". An unknown table fails with
  `42P01`. Assert on the UI, not on row contents; timings are random.
- `sql` uses `fill`, which does not fire the per-key input the completion
  popup needs — use `sql-type` to see completions.
- `⌘↵` only arms the statement at the caret (outline → fill); a second `⌘↵`
  or `Enter` runs it. `run` does both. To see the armed state or the
  statement / subquery chooser, use `key Meta+Enter` and `ss`.
- Title-bar editor tabs are the `role="tab"` elements with `data-tab-id`; the
  right panel's and the Database Output panel's tabs are `role="tab"` too.
  Don't select `[role="tab"]` alone.
- `key Alt+j` does nothing on macOS: the app takes ⌥+key only when the
  event's `key` is non-ASCII (what Option composes), and Playwright sends a
  plain `j`. `opt j` dispatches the Mac-shaped event instead.
- Each driver run is a fresh browser profile (empty localStorage, default
  prefs); nothing carries over between runs.
- A `pageerror` about `useState` / `useEffect` of null, or a run that
  suddenly loses its connection, means Vite hot-reloaded mid-run (files
  changed under it: a branch switch, a merge, another agent). Re-run.
- `node node_modules/.bin/vite` fails (`SyntaxError`): pnpm's `.bin` entries
  are shell scripts. Use `pnpm exec vite`.
- Without the nvm PATH export, `pnpm` / `node` are "command not found".

## Troubleshooting

- `page.goto: net::ERR_CONNECTION_REFUSED` — Vite isn't up yet (or died); run
  the `curl` wait first, check the Vite task output.
- `FAILED: … is not the DB Client UI (title "…")` — another app's dev server
  is on that port; start this repo's Vite on a free one.
