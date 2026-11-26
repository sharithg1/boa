# Phase 5: table data editing (p5/table-edits)

Code: `crates/boa-app/src/models/table_tab.rs` (the tab: page, filter / sort,
staged edits via `boa_core::grid::edits::EditsState`, guarded navigation,
submit), `views/table_filter_bar.rs` (TableFilterBar.tsx),
`views/table_edits.rs` (TableEdits.tsx: toolbar controls, SQL preview,
discard prompt), the table-tab part of `views/workspace.rs` (wiring grid ⇄ tab).

| Screenshot | What it shows |
|---|---|
| [filter-bar.png](filter-bar.png) | WHERE `price < 50 and note is null` and ORDER BY `stock desc, id` applied on the server (25 of ~60 rows); the header arrows with the rank of the second sort column |
| [staged-edits.png](staged-edits.png) | Staged, not submitted: two edited cells (tinted), a row marked for deletion (red, struck through), a new row with `<default>` placeholders; "4 changes · Preview… · Submit" and the add / delete / revert-all buttons in the toolbar |
| [sql-preview.png](sql-preview.png) | The preview (⌘↵ in the grid, or Preview…): the exact statements, DELETE first, then UPDATE by the original key, then INSERT of the touched columns only; Copy, Open in console, ↵ submit |
| [submit-error.png](submit-error.png) | A submit that hit a unique violation: everything rolled back, the preview reopened with the failing statement highlighted and the server's error + detail under it; the Output log shows the statement, the error and "Statement 3 of 3 failed; all changes were rolled back." |
| [where-error.png](where-error.png) | A bad WHERE: the server's error (and hint) in place of the rows, the typed text kept in the bar |

Captured with the `screenshot` feature (GPUI `render_to_image`) on a temp
`BOA_CONFIG_DIR` against the local server, scripted by
`views/table_edits/dev.rs`: `BOA_DEV_CONNECTION=1 BOA_DEV_PANELS=sidebar
BOA_DEV_TABLE=boa_p5_shots.products` plus `BOA_DEV_TABLE_WHERE` /
`BOA_DEV_TABLE_ORDER` / `BOA_DEV_TABLE_EDITS="0,2,Cold brew jar XL;1,3,12.50;del,3;add;new,1,SKU-0999;new,2,Moka pot"`
/ `BOA_DEV_TABLE_PREVIEW=1` / `BOA_DEV_TABLE_SUBMIT=1` (the fixture schema
`boa_p5_shots` was created for the shots and dropped afterwards).

## Built

- **Filter bar**: WHERE and ORDER BY fields (mono, `WHERE` / `ORDER BY`
  labels, placeholders as the source). Enter applies both (trimmed, stored on
  the tab and in the session), Esc puts back the field's applied text, ×
  empties the field and applies. Text typed but not applied is tinted.
  One-clause validation (`table_select`) refuses `;`, unbalanced parentheses,
  unterminated strings / comments before anything runs; server errors show
  in place of the rows; both start again from the first page.
- **Header sorts**: a header click on a table tab rewrites the ORDER BY
  (`cycle_sort`: asc → desc → off; ⇧ appends), shown in the bar and as
  arrows + ranks in the header (`ResultGrid::set_table_context`).
- **Staged edits**: the grid stages (inline edit, ⇧↵ value editor, ⌘⌥N,
  ⌘N / Alt+Insert, ⌘⌫ / Delete, ⌘⌥Z, context menu); every
  `GridEvent::PendingChanged` hands them to the tab's `EditsState` (per page,
  kept across tab switches). Toolbar: add row, delete selected row, revert
  all, "N changes", Preview…, Submit — or the read-only reason (views, no
  primary key, key not in the result, table not introspected).
- **Preview and submit**: ⌘↵ in the grid / Preview… opens the preview;
  Submit or ↵ runs `Backend::apply_edits` (one transaction, a savepoint inside
  an open one). Success: toast "Submitted N changes" (or "applied in the open
  transaction"), edits cleared, page reloaded. Failure: edits kept, preview
  (re)opened on the failing statement, error toast, Output log opened. Every
  statement is logged to the Output log with the table as source and
  `record: true` (query history, Phase 7).
- **Discard prompts**: reload, page change, new filter / sort, header click
  and closing the tab (Workspaces close guard) with pending edits ask
  "Discard N pending changes?" / "They have not been submitted. To …, they
  will be lost." (Keep editing / Discard).

## Tests

`views/table_edits/tests.rs`: 11 (5 without a server, 6 live with `PGHOST`
on their own `boa_p5_edits` schema, serialized).
