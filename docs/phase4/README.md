# Phase 4: consoles and results

## Consoles (p4/consoles)

Code: `crates/boa-app/src/models/{workspaces,console,table_tab}.rs`,
`editor/`, `views/{workspace,title_bar,bottom_panel,status_bar,root,
statement_chooser,completion_popup,run_progress,pager}.rs`.

| Screenshot | What it shows |
|---|---|
| [console-results.png](console-results.png) | A console run (⌘↵ arms, ⌘↵ runs): toolbar, outlined statement, result tab "console_1" in Database Output with source / time and the pager (500 = prefs.rowLimit), status bar crumbs, caret, UTF-8, last run |
| [console-output-log.png](console-output-log.png) | A failed statement: the strip under the editor and the Output log line with `[42P01]` and the psql-style `LINE 2:` + caret; the Output tab turns red |
| [statement-chooser.png](statement-chooser.png) | ⌘↵ with the caret in a subquery: the statement is filled and the "Statements" chooser offers the whole statement or the subquery |
| [completion-popup.png](completion-popup.png) | Our own completion popup after `o.`: per-kind icons (yellow key for the PK), detail on the right, anchored at the word |
| [run-progress.png](run-progress.png) | `select boa_fx.slow(8)` in flight: spinner on the tab, "Executing… 5.0 s" in the toolbar, Cancel enabled, the sweeping bar under the Database Output header |
| [multiple-tabs.png](multiple-tabs.png) | Consoles and table tabs in the title bar, a production connection (red stripe, PROD tag in the toolbar and status bar) |

All captured from the real window (`screencapture -l`) of a dev build
(`--features dev-hooks`) on a temp `BOA_CONFIG_DIR` against the local
server; the scripted session is `views/workspace/dev.rs` (`BOA_DEV_LIVE=1`,
`BOA_EDITOR_TEXT`, `BOA_DEV_CARET`, `BOA_DEV_ARM` / `BOA_DEV_RUN`,
`BOA_EDITOR_TYPE`, `BOA_DEV_TABLES`, `BOA_DEV_CONSOLES`, `BOA_DEV_ENV`,
`BOA_DEV_OUTPUT`). It saves and connects the connection itself and fills
`Catalogs` directly, because Connections / Catalogs are still stubs on this
branch. (The grid's own "Results" strip with a second pager in these shots
belongs to the Phase 0 grid; p4/grid's `set_toolbar_visible(false)` should
hide it once merged.)

### Built

- **Workspaces**: tabs per database (Console / TableTab entities), active
  tab, lowest free `console_N`, preview tabs (replaced until pinned by double
  click / opening pinned), drag reorder, MRU (30) + seen, ⌘T / ⌥N (selected
  database, else the first connected one), ⌘W (right neighbour, else left),
  ⌥1–9, ⌥↑ / ⌥↓ over every shown database (prefs `scopes`), `bottomView`,
  the per-connection Output log (200 lines). `to_session` / `restore` map
  it to `boa.session.v1` (tested byte-for-byte on the tab objects).
- **Console**: run = `ensure_connected` → `guards::check_run` (read-only
  escape refused and logged; destructive → confirmation dialog) →
  `run_query(limit = prefs.rowLimit)`; one result per console (= one result
  tab); paging re-runs with an offset (Output line, not recorded); cancel via
  `cancel_query`; failures in a strip under the editor and in the Output log
  with the error position; DDL reloads the catalog.
- **Editor**: arm-then-run, statement chooser, our completion popup (Tab
  whole word, Enter up to the caret, PgUp/PgDn ±9, sticky explicit mode,
  `count(|)`, schema re-open), caret line:col, schemas from `Catalogs`,
  default-schema picker in the toolbar.
- **TableTab** (basic): first page via `grid::table_query`, paging,
  editability from the catalog, reload, "Open query console".
- Title bar tabs, Database Output (result tabs, Output log, pager, run bar,
  clear), status bar, run progress, pager.

### Not done / deviations

- Comments aren't italic: the Tauri app ships no italic Geist Mono either
  (the browser slants it); none is bundled.
- Query history recording is left to Phase 7 (`WorkspacesEvent::OutputLogged
  { record }` is the hook); the transaction controls are a "Tx: Auto" slot.
- `select_database` opens no console (the Tauri app doesn't either).
