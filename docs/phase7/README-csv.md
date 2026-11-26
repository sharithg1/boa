# Phase 7b — CSV export

Spec: source `docs/USAGE.md` "CSV export"; `CsvExport.tsx`, `state/csvExport.ts`,
`lib/exportBackend.ts`, `StatusBar.tsx` (`ExportStatus`), `BottomPanel.tsx` /
`Workspace.tsx` / `Sidebar.tsx` (where exports start), `pg/export.rs` (savepoint rules).

## What was built

| Piece | File | Source |
|---|---|---|
| `Exports` model | `crates/boa-app/src/models/exports.rs` | state/csvExport.ts |
| Export menu + status bar progress | `crates/boa-app/src/views/csv_export.rs` | CsvExport.tsx |
| Entry points | `views/bottom_panel.rs` (result toolbar), `views/workspace.rs` (table tab toolbar), `views/sidebar.rs` (explorer "Export to CSV…"), `views/status_bar.rs` | BottomPanel / Workspace / Sidebar / StatusBar |

The rules were already in boa-core (`state::csv_export`: options JSON, routes, job list,
texts; `pg::export`: the COPY, temp file + rename, savepoint); the model does the I/O.

### Model

- **Options**: delimiter (comma / tab / semicolon) and header row, read from
  `<config dir>/csv-export.json` at launch (`Exports::load`) and written on every change,
  byte-identical to the Tauri app's `boa.csvExport` JSON (`{"delimiter":",","header":true}`).
- **`Exports::export(src, owner, cx)`** (`exportCsv`): a read-only source connects first
  (`Connections::ensure_connected`; still closed → "Export failed: X is not connected"), then
  the save panel (`cx.prompt_for_new_path`, suggested name from `csv_file_name`, opening in
  the last folder used this session, else ~/Downloads), then `export_to`.
- **`export_to(src, owner, path, cx)`**: read-only statements and tables stream through
  `Backend::export_csv` as a job (progress every 250 ms; the menu shows "Exporting…" for its
  owner — the console's tab id or the table's `tableKey`, shared by the explorer menu and
  the table tab); statements that change data (`INSERT … RETURNING`, `pageable: false`) are
  never re-run: their loaded rows are written with `to_csv` (temp file + rename, on GPUI's
  background executor).
- **Endings** (as `csvExport.ts`): an Output log line (recorded in the history too, as
  `logOutput` does) and a toast — success "Exported N rows" (`path · size`, or the
  loaded-rows text) with **Reveal in Finder**, 10 s; 57014 → "Export cancelled" (info, note in
  the log); anything else → "Export failed" with `[SQLSTATE] message`, and
  `Export failed: …` + detail in the log.
- **Cancel** (`cancelExport`): `Backend::cancel_query` on the job's database; a failure toasts
  "Cancel failed".
- **Savepoint rules**: the backend runs the COPY in its own `begin … commit`, or, when the
  session is inside a user transaction, under a savepoint that is always rolled back — so
  uncommitted rows are exported and a failure or cancel leaves the transaction usable.
  A live test checks both.

### Views

- **Export menu** (download icon) in the Database Output header (for the shown result) and in
  table tab toolbars (the table with the tab's WHERE / ORDER BY; disabled while disconnected or
  the filter is invalid): Export to CSV…, Copy page / rows as CSV (with the loaded count),
  the "statement changes data" note, Delimiter (radio), Header row (check). A popover with
  menu rows, so options keep it open as Radix's `preventDefault` did.
- While its export runs, the menu is replaced by "Exporting…" + Cancel.
- **Status bar**: the first running export — spinner, "Exporting `file`", "· N rows · size"
  once progress arrived, "(+n)" for more, Cancel; tooltip with the full path.
- Explorer table/view context menu "Export to CSV…" → `exports::export_table`.

## Tests (12)

`models/exports/tests.rs` (10, three of them live) and `views/csv_export/tests.rs` (2):

- options read from / saved to csv-export.json (exact JSON), corrupt → defaults;
- loaded-rows route: file content, Output log note, toast + Reveal action, no job, no `.part`;
- save panel: cancelled → nothing; chosen file written; next panel opens in that folder;
- closed connection → "Connection is not connected" toast, no panel; failed write → log + toast;
- jobs by owner; table tab sources;
- UI (real window, mouse): menu opens, delimiter + header keep it open and persist, Copy page
  as CSV puts `1;\n2;x\n` on the clipboard + toast and closes it, Export to CSV… opens the
  save panel; a running job swaps the menu for "Exporting…" + Cancel and shows in the status bar;
- **live** (PGHOST): explorer export of public.orders through the save panel (header = the
  table's columns, 100,000 rows, toast/log texts, no `.part`), semicolon + no header;
  cancel after the first progress → "Export cancelled", no file, no `.part`; inside a manual
  transaction: uncommitted rows exported, `1/0` fails without aborting the transaction,
  a data-modifying `WITH` is refused ("…this statement modifies data"), rows still there.

## Screenshots

Captured with `--features screenshot`, `BOA_DEV_CONNECTION=1` and the new
`BOA_DEV_EXPORT=menu|progress|done` (+ `BOA_DEV_EXPORT_FILE`) hook:

- `csv-export-menu.png` — the Export menu on a result.
- `csv-export-progress.png` — "Exporting…" in the result toolbar, file / rows / size + Cancel in the status bar.
- `csv-export-done.png` — the "Exported 100,000 rows" toast with Reveal in Finder.

## Not verified / deviations

- The native save panel itself and Reveal in Finder were not clicked through (tests use GPUI's
  simulated save panel); the panel has no csv/tsv/txt type filter (GPUI's API has none).
- The save panel opens in the last folder used (session only) or ~/Downloads; the Tauri dialog
  let macOS choose.
- Quitting during an export leaves the hidden `.name.<pid>-<nanos>.part` file (as the Tauri app).
- Backend gotcha (not changed here): an export shares the database's session with consoles;
  starting one while a console statement runs on the same database interleaves them
  (the console's cursor is closed: "cursor __boa does not exist"). The Tauri app has the same
  shape; worth a per-session queue in boa-core.
