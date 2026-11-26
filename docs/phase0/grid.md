# Phase 0: result grid spike

`crates/boa-app/src/grid/`: a virtualized result grid on GPUI's
`uniform_list` (not gpui-component's `DataTable`, see widgets.md), wired to
the spike connection. Verdict: **GPUI is a good fit for the grid.** 100k rows ×
100 columns scroll at the display's full 120 Hz with 2–4 ms of CPU per frame, and
the selection / copy / edit model ports cleanly. The cost is that we write the
grid ourselves: hit-testing, horizontal virtualization, sticky parts and drag
handling are most of the ~1,500 lines of `grid/mod.rs` (plus ~900 lines of
pure, unit-tested logic in `selection` / `columns` / `page` / `error`); a DOM
table gave the React app much of that for free.

Screenshots (`--features screenshot`, see "Dev hooks" below):
`grid-100k-range.png` (100k orders, range selected), `grid-edit.png` (three
staged edits tinted, inline input open), `grid-wide.png` (100 columns, scrolled
right: sticky row numbers, header in sync), `grid-error.png` (server error with
the position caret).

## What works

| Behaviour (USAGE.md "Selecting and copying rows", DataGrid.tsx) | How | Verified by |
|---|---|---|
| Virtualized rows | `uniform_list`, 28 px rows | bench, screenshot, test (`paging_down_scrolls_100k_rows`) |
| Virtualized columns | the row closure builds only the columns the horizontal offset shows (`ColumnLayout::visible`) | bench (100 columns), screenshot |
| Sticky header (name + type) and row numbers | header outside the list, shifted by the list's scroll offset; row numbers absolutely placed at the viewport's left edge in each row. Row-number column widens for 6-digit numbers | screenshots |
| Values per `boa_core::grid::values` | NULL `<null>` dim italic, numbers right-aligned, colour per `ValueKind::style`, JSON token-coloured (first 400 chars, `json_tokens`), line breaks shown as spaces | screenshots |
| Column resize | drag the 6 px handle on a header edge, min 48 px (no max), initial widths = `initialWidth` | test (`column_resize_by_dragging_the_edge_clamps`), unit tests |
| Cell / range selection | click, ⇧-click, drag (follows the mouse outside the grid and auto-scrolls), ⇧-arrows, arrows, PageUp/PageDown | tests (click, shift-click, drag, keys) |
| Row selection | click a row number, ⇧-click a run, ⌘-click to toggle, drag over numbers | test (`row_numbers_pick_rows`) + unit tests |
| ⌘A / Esc | ⌘A picks every row (kept symbolic, O(1) for 100k), Esc back to the active cell | tests |
| ⌘C | TSV through `copy_as::copy_plain` of displayed values (staged edits included); one cell = its text; "Copied value" / "Copied N rows" | tests (clipboard contents) |
| Inline edit | Enter / F2 / ⇧↵ / double-click / typing a character; Enter or blur commits, Esc cancels, Tab / ⇧Tab commit and move; unchanged text stages nothing | tests (`enter_edits…`, `double_click_and_typing…`), screenshot |
| Staging | `grid_edits::Pending` (`set_cell` / `revert_*` / `delete_row`); edited cells tinted `status-run/20`, row numbers coloured per `RowMark`, deleted rows struck through; "N changes staged" in the strip | tests, screenshot |
| ⌘⌥N set NULL, ⌘⌥Z revert, ⌘⌫ / Delete delete row | as in DataGrid.tsx | test |
| Run states | Connecting (waits for the spike connection), Running… + Cancel (`cancel_query`), result, "N rows affected", error: `[SQLSTATE] message`, Detail / Hint lines, and the statement line with a caret at `position` (psql style) | screenshots (result, error); Cancel **not** exercised |
| Paging | Pager.tsx: « ‹ `1–500 …` ›, re-runs with an offset; a failed page keeps the rows and shows the message | screenshot of the pager (`BOA_GRID_PAGE_SIZE=500`); clicking a page **not** exercised |

Tests: 38 in `grid` (28 unit tests of the pure parts: `selection`, `columns`,
`page`, `error`, `dev`; 10 `#[gpui::test]` view tests driving real mouse and
key events through `VisualTestContext` against an injected result).

## Numbers

Apple M5 Pro, 120 Hz display, release build (`thin` LTO), local Postgres 17.
The window is the default 1280×820; the grid pane under the 360 px editor shows
~13 rows (the "big window" runs: 2400 px wide, ~18 rows × 15 columns).

**Load to first paint** (`BOA_LOG=info` lines `loaded in` / `first paint`):

| Query | Data in the grid | First frame drawn |
|---|---|---|
| `select * from orders` (100k × 7) | 72–77 ms | 86–94 ms after the run started |
| 100k × 100 columns | 766–776 ms | 780–793 ms |
| `select * from customers` (500 rows, page size 500) | 1 ms | 13 ms |

The first frame comes 10–15 ms after the data: building the `Page` (column
widths from the first 50 rows) and one frame. Nearly all the time is the
backend fetching and converting rows to strings (`run_query`), not the UI.
Debug builds: 100k × 7 in ~180 ms / first paint ~200 ms.

**Scrolling** (`BOA_GRID_BENCH=1`: scripted scrolling, one step per frame,
from `on_next_frame`). "Interval" is what the display link delivered,
"render→paint" the CPU time from the grid's `render` to the end of its paint
(element build + layout + text shaping + prepaint + paint; GPU work excluded).

| Run | Phase | Interval p50 / p95 / max (ms) | Frames > 1.5× median | render→paint p50 / p95 / max (ms) |
|---|---|---|---|---|
| 100k × 7 | scroll 40 px/frame (600) | 8.33 / 8.36 / 8.73 | 0 | 1.90 / 3.07 / 3.64 |
| | fling 400 px/frame (300) | 8.33 / 8.36 / 8.45 | 0 | 2.74 / 4.26 / 6.03 |
| | scroll right 30 px/frame (300) | 8.33 / 8.37 / 8.40 | 0 | 1.66 / 1.97 / 2.06 |
| | jump to random rows (120) | 8.33 / 8.36 / 8.38 | 0 | 3.28 / 3.76 / 4.84 |
| 100k × 100 | scroll 40 px/frame | 8.33 / 8.37 / 8.57 | 0 | 1.73 / 2.83 / 3.82 |
| | fling 400 px/frame | 8.33 / 8.36 / 8.41 | 0 | 3.41 / 3.68 / 6.66 |
| | scroll right 30 px/frame | 8.33 / 8.36 / 8.47 | 0 | 1.46 / 2.15 / 2.68 |
| | jump to random rows | 8.33 / 8.39 / 8.46 | 0 | 1.75 / 2.65 / 2.86 |
| 100k × 100, big window | scroll 40 px/frame | 8.33 / 8.36 / 15.29 | 1 | 2.32 / 2.84 / 3.73 |
| | fling 400 px/frame | 8.33 / 8.37 / 8.41 | 0 | 4.00 / 4.35 / 5.10 |
| | scroll right 30 px/frame | 8.33 / 8.39 / 8.48 | 0 | 1.91 / 2.57 / 3.12 |
| | jump to random rows | 8.33 / 8.37 / 8.40 | 0 | 2.32 / 3.10 / 3.82 |

Steady 120 Hz, with half or more of the 8.3 ms budget left even when every
frame shows entirely new rows (fling / jump: all text re-shaped). Element
building (`render` + visible rows) is 0.05–0.3 ms; the rest is GPUI's layout,
text shaping and paint.

Caveat: other agents' windows were opening on the same screen. When the window
was covered macOS throttled it to 30 Hz (intervals of 33.3 ms, and CPU times
2–3× longer, presumably on lower clocks); one run stalled entirely while hidden.
Those runs are left out above. Scrolling feel with a real trackpad (momentum,
scrollbar dragging) was not checked by a person.

**Memory** (`footprint <pid>`, 15 s after launch, release, `BOA_GRID_QUIT_AFTER_MS`):

| State | Footprint | Of which malloc | RSS |
|---|---|---|---|
| Idle, no result (this build) | 68 MB | 14 MB | 100 MB |
| 100k × 7 (`orders`, 700k cells) | 168 MB | 54 MB small (26 MB reclaimable) + 10 MB large | 178 MB |
| 100k × 100 (10M cells) | 627 MB | 547 MB small (384 MB reclaimable) + 10 MB large | 1.03 GB |

The rows are `Vec<Vec<Option<String>>>` straight from `QueryResult`: 24 bytes
per cell plus a heap allocation per value, ~60–80 bytes per cell live, and the
backend's conversion leaves freed pages behind (the "reclaimable" part). IOSurface
(Metal drawables) also varied between 33 and 82 MB from run to run. For comparison
the Tauri app caps a page at 5,000 rows (Settings; default 500) and its React grid
puts every row in the DOM (no virtualization), so it never holds 100k rows.
A compact page (one string buffer + offsets per page, ~10–20 bytes per cell) would
cut the 100k × 7 case to ~15 MB; worth doing in Phase 4 if 100k-row pages stay.

## What was painful

- **No grid widget, and `uniform_list` virtualizes rows only.** Columns are
  virtualized by hand from the scroll offset; the header and the sticky row
  numbers are positioned from the list's `ScrollHandle` (read in `render`, so
  the header can lag the list by a clamp for one frame). Hit-testing (which cell
  is under the mouse) is our own arithmetic on the handle's bounds and offset.
- **Drags that leave the element**: GPUI delivers mouse moves to hovered
  elements only; following a selection or column drag across the window needs a
  `canvas` that registers window-level `MouseMove` / `MouseUp` listeners while
  dragging (Zed's editor does the same).
- **Focus fights**: a `track_focus` parent refocuses itself on mouse down
  *after* the child's handler, so focusing the inline input from a double-click
  silently lost focus until `window.prevent_default()`.
- **Key precedence for the inline input**: gpui-component's `Input` binds Tab
  (indent) and Escape itself; Tab / Esc for the cell editor needed bindings in a
  `GridCellEditor > Input` context (same depth, bound later, so they win).
  Typing a character to start editing must stop propagation, or the key is
  lost / duplicated when focus moves to the new input mid-keystroke.
- **gpui-component's `Input`** keeps its padding and size with
  `appearance(false)`; it needed `px_0 / py_0 / h_full / text_size` overrides to
  sit inside a 28 px cell. Toasts (`push_notification`) panic without a `Root`
  window root (tests), so "Copied …" shows in the row-count pill for 1.6 s instead.
- **Verification without a screen**: no Screen Recording permission, so all visual
  checks are `render_to_image` screenshots driven by env hooks, and interaction is
  tested with `VisualTestContext`. That worked well (real mouse / key / input events
  through the dispatch tree) but cannot judge feel.

What was easy: the selection / copy / staging logic is plain Rust over
`boa_core::grid` (already ported) and unit-testable; GPUI's text rendering and
`StyledText` highlights made value colouring trivial; frame times needed no
optimisation at all.

## Gaps vs. the Tauri app

Not done in the spike (Phase 4 / 5 items):

- Header click sort (local cycle for console results, server ORDER BY with ⇧
  multi-sort for table tabs), sort arrows, primary-key icon, header tooltip.
- Right-click menu: Copy, **Copy as ›** (all `copy_as` formats exist in boa-core),
  edit items. gpui-component's `ContextMenuExt` should fit.
- JSON viewer (Enter / double-click / hover button on JSON cells) and the larger
  value editor (⇧↵, and automatically for multi-line, >200 chars or JSON values).
  Here ⇧↵ and long values open the single-line inline input.
- Add row (⌘N, Alt+Insert) and new-row placeholders (`<default>`), Submit (⌘↵:
  preview + `apply_edits`), "pending changes" prompts before paging / re-running.
  Editing is allowed on every result here; the Tauri app allows it only on table
  tabs of tables with a primary key (`grid_edits::editability`).
- Real toasts (Phase 1), the Output log, result tabs, "row limit reached".
- Page size is the backend maximum (100,000) instead of the Settings value
  (default 500; `BOA_GRID_PAGE_SIZE` overrides).
- `onSelectRow` (feeds the right panel), focus ring / a11y semantics.

## Estimate to parity for this pane

About **2–2.5 weeks**: sort + header details 2 d, context menu with Copy as 1–2 d,
JSON viewer 2–3 d, value editor dialog 1–2 d, add row / editability / submit
preview / pending-change prompts 3–4 d, compact row storage 1–2 d, results model
(tabs, Output log, toasts, paging prompts) shared with Phase 4.

## Dev hooks (`grid/dev.rs`)

`BOA_GRID_SQL` (run at startup), `BOA_GRID_SELECT=r0,c0:r1,c1`,
`BOA_GRID_STAGE=r,c=text;r,c=NULL`, `BOA_GRID_EDIT=r,c`, `BOA_GRID_SCROLL=x,y`,
`BOA_GRID_PAGE_SIZE`, `BOA_GRID_BENCH=1` (+ `BOA_GRID_BENCH_QUIT`,
`BOA_GRID_BENCH_WINDOW=WxH`), `BOA_GRID_QUIT_AFTER_MS`, and in `app.rs`
`BOA_SCREENSHOT_DELAY_MS` for the screenshot hook. Example:

```sh
BOA_GRID_SQL="select * from orders" BOA_GRID_SELECT="2,1:8,4" \
  BOA_SCREENSHOT=out.png BOA_SCREENSHOT_DELAY_MS=4000 \
  cargo run -p boa-app --features screenshot
BOA_LOG=info BOA_GRID_SQL="select * from orders" BOA_GRID_BENCH=1 BOA_GRID_BENCH_QUIT=1 \
  cargo run --release -p boa-app
```
