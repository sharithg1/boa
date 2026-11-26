# Phase 4 (grid half): the result grid as a pure view

`crates/boa-app/src/grid/` plus `views/json_viewer.rs` and `views/value_editor.rs`.
Spec: DataGrid.tsx, JsonViewer.tsx, ValueEditor.tsx, USAGE.md "Selecting and
copying rows" and the view side of "Editing table data".

Screenshots (real window, `screencapture -l`, driven by the dev hooks below):
`grid-context-menu.png` (right-click in a range, Copy as › open; UPDATE and
IN list disabled), `json-viewer.png` (tree, two levels open, folded items),
`value-editor.png` (multi-line value of an editable table), `grid-edits.png`
(updated cells and rows tinted, NULL staged, a deleted row struck through, a
new row with `<default>` placeholders, the inline input open, "4 changes staged").

## The contract (Wave 2, `grid/mod.rs`)

The grid never runs SQL. Its owner (console result tab, table tab) runs the
statement and drives it with:

| Call | Effect |
|---|---|
| `set_content(GridContent)` | shows a page. The same `Arc<QueryResult>` again only updates offset / page size / editability and keeps selection, sort, scroll and staged edits; a new one starts them over (and emits `PendingChanged` if edits were dropped). Column widths carry over when the columns are identical. A result without columns shows "N rows affected in X ms". Clears running / error. |
| `set_running(bool)` | "Running…" in the strip (and the body when there is no page yet); editing and the pager are disabled; an open inline edit is committed. |
| `set_run_error(Option<RunError>)` | the error view (`[SQLSTATE] message`, Detail / Hint, caret line) instead of the rows. |

Events (`GridEvent`): `PageRequested { offset }` (pager « ‹ ›),
`ServerSortRequested { column, additive }` (header click / ⇧-click, table
tabs only), `PendingChanged` (every change of the staged edits, from keys,
menu, inline input or value editor), `SubmitRequested` (⌘↵ in an editable grid).

Additions (no contract signature changed, nothing added to `GridContent`):

- `set_table_context(Option<TableContext { table, pk, sort }>)`: table tabs.
  With one, header clicks emit `ServerSortRequested` and the header shows the
  sorts passed in (`table_query::sort_state(order_by)`: arrow + rank) and the
  key icon; Copy as INSERT / UPDATE target `table` and UPDATE uses `pk`.
  Kept across `set_content`. Without one (consoles) header clicks sort the
  loaded rows locally (asc → desc → none; numbers numerically, NULLs last).
- `pending()`, `take_pending()`, `clear_pending()`, `set_pending(Pending)`
  (none emits an event), `active_row() -> Option<RowRef>` (DataGrid's
  `onSelectRow`), `set_toolbar_visible(bool)` (hide the status strip when the
  owner draws its own), `header_clicked`, `copy(Option<CopyFormat>)`,
  `start_edit`, `open_json`, `open_value_editor`, readers (`page()`,
  `error()`, `is_running()`, `editability()`, `json_viewer()`,
  `value_editor()`, `editing_cell()`).
- Actions (namespace `result_grid`): `CopyAs(CopyFormat)`, `RevertCell`,
  `RevertRow`, `ViewJson`; the grid also handles `boa::AddRow` and
  `boa::SubmitEdits`.

## Behaviour (DataGrid.tsx)

- Selection: click, ⇧-click range, drag (follows the mouse outside the grid),
  ⇧-arrows, arrows / PgUp / PgDn, row numbers (click, ⇧-click run, ⌘-click
  toggle, drag), ⌘A every row, Esc back to the active cell. Right-click outside
  the selection selects the cell / row under the mouse, inside keeps it.
- ⌘C: TSV of the displayed values (one cell: its text) with a 1.6 s toast
  ("Copied value", "Copied 3 rows") through `models::toasts`.
- Context menu (gpui-component `ContextMenuExt`): Copy, Copy as › TSV, TSV with
  header, CSV, Markdown table, JSON, SQL INSERT, SQL UPDATE, SQL IN list
  (formats that don't apply are disabled; toasts "Copied 2 rows as JSON",
  "Copied 3 values as an SQL IN list", errors "Copy failed"); editable content
  adds Edit, Open in editor, Set NULL, Add row, Delete row, Revert cell,
  Revert row, Preview and submit… Shortcut hints come from the keymap.
- Values: `<null>` dim italic, `<default>` for a new row's untouched column,
  numbers right-aligned, colour by type, JSON token-coloured with a braces
  button (on hover, always on the active cell) that opens the JSON viewer.
- Header: name, type (right), key icon for PK columns (table context), sort
  arrow and rank, tooltip; drag the right edge to resize (min 48 px).
- Editing (only with `Editability::Editable`, never while running): Enter / F2
  / typing / double-click edit inline; multi-line, > 200 chars and JSON values
  open the value editor instead; ⇧↵ opens it (also from the inline input,
  dropping its text); in the input Enter / blur commit, Esc cancels, Tab / ⇧Tab
  commit and move. ⌘N / Alt+Insert add a row (selected, works without a
  selection), ⌘⌫ / Delete delete the active row (a new row just goes away),
  ⌘⌥N NULL (also from the input), ⌘⌥Z reverts the cell, else the row, ⌘↵ emits
  `SubmitRequested`. Tints: edited cells `status-run/20`, new rows
  `status-ok/10` with a `+` row number, deleted rows `destructive/10` struck
  through, row numbers coloured per kind. Read-only content ignores all of it
  (Enter on a JSON cell opens the viewer instead); with a table context the
  strip shows the read-only reason.
- JSON viewer (modal over the window, Esc / × / click outside closes): tree
  with the first two levels open, fold / unfold per node with key / item
  counts, Expand all / Collapse all, Tree / Text (pretty-printed, coloured),
  Copy (pretty text, "Copied JSON"), "Not valid JSON: …" falls back to text.
  Virtualized (`uniform_list`); lines don't wrap but scroll sideways.
- Value editor (modal): column, row, type, "N chars" / "NULL"; Format (JSON),
  Set NULL; ⌘↵ or Apply stages the value if it changed (NULL until typed into,
  so NULL ≠ ''), Esc / × cancel.

## Dev hooks (`dev-hooks` feature, `grid/dev.rs`)

`BOA_GRID_SQL` now fetches through `AppState`'s backend on a saved connection
of `BOA_CONFIG_DIR` (`BOA_GRID_CONNECTION`, `BOA_GRID_DATABASE`) and feeds
`set_content`. New: `BOA_GRID_EDITABLE=pk,…` (+ `BOA_GRID_TABLE`),
`BOA_GRID_ADD_ROWS=n`, `BOA_GRID_DELETE=r,…`, `BOA_GRID_JSON=r,c`,
`BOA_GRID_VALUE=r,c`, `BOA_GRID_MENU=r,c` (+ `BOA_GRID_MENU_KEYS="down down"`).
Unchanged: `BOA_GRID_SELECT`, `_STAGE`, `_EDIT`, `_SCROLL`, `_PAGE_SIZE`,
`_BENCH`, `_QUIT_AFTER_MS`.

```sh
dir=$(mktemp -d)   # write a connections.json for the local server into it
BOA_CONFIG_DIR=$dir BOA_GRID_SQL="select * from orders limit 200" \
  BOA_GRID_EDITABLE=id BOA_GRID_TABLE=public.orders BOA_GRID_ADD_ROWS=1 \
  cargo run -p boa-app --features dev-hooks
```

## Tests

`cargo test -p boa-app grid` and `json_viewer`: 18 `#[gpui::test]` view tests in
`grid/tests.rs` (selection kinds, ⌘A / Esc, ⌘C TSV + toast, every Copy-as format
through its `CopyAs` action, the menu opened by a right-click and Copy run from
it, local sort cycle, `ServerSortRequested`, JSON viewer open / fold / copy /
Esc, value editor ⌘↵ / Set NULL / Esc / auto-open, every editing shortcut's
`Pending` change and events, read-only blocking, the `set_content` /
`set_running` / `set_run_error` / pending contract, pager events) plus unit
tests (`page`, `sort`, `json_viewer` tree flattening).

## Bench (5,000-row page)

Release build, `BOA_GRID_SQL="select * from orders limit 5000" BOA_GRID_BENCH=1`
(Apple M5 Pro, 120 Hz display, grid viewport 1280×325 px ≈ 12 rows × 7 columns),
first paint 10 ms after the content arrived:

| Phase | Interval p50 / p95 / max (ms) | Frames > 1.5× median | render→paint p50 / p95 / max (ms) |
|---|---|---|---|
| scroll 40 px/frame (600) | 8.33 / 8.37 / 8.50 | 0 | 1.65 / 2.73 / 3.58 |
| fling 400 px/frame (300) | 8.33 / 8.35 / 8.37 | 0 | 2.75 / 3.16 / 3.93 |
| scroll right 30 px/frame (300) | 8.33 / 8.37 / 8.52 | 0 | 1.17 / 2.18 / 2.77 |
| jump to random rows (120) | 8.33 / 8.36 / 8.38 | 0 | 3.23 / 3.94 / 5.58 |

Still the display rate. Rows now come from the owner's `Arc<QueryResult>`
(no copy of the page). The 100k-row bench was not re-run to the end: the
window could not be kept in front on a shared screen and macOS stops frames
of occluded windows; `paging_down_scrolls_100k_rows` still passes.

## Not done / deviations

- No breadcrumbs: DataGrid.tsx has none (the statement source is the owner's).
- The modals are the grid's own overlay (scrim + centred panel, no open /
  close animation), not gpui-component's `Dialog`, so tests need no
  timing and keys inside never reach the grid's bindings.
- Menu items that don't apply are disabled without the TS tooltip giving the
  reason (gpui-component's `PopupMenu` has no per-item tooltip).
- JSON viewer lines don't wrap (TS wraps long strings); the tree guides use
  `border-subtle`, which is barely visible on the popover background (as in TS).
- Local text sort approximates `localeCompare` (case-insensitive, then by case).
- Cancel is the owner's (the grid strip has no Cancel button any more).
