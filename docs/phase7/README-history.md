# Phase 7a — query history

Spec: source `docs/USAGE.md` "Query history"; `QueryHistory.tsx`, `RightPanel.tsx`
(History tab), `state/history.ts`, `lib/history.ts`, `store.ts` (`log` / `recordHistory`).

## What was built

| Piece | File | Source |
|---|---|---|
| `History` model | `crates/boa-app/src/models/history.rs` | state/history.ts |
| Query History overlay (⌘⌥E) | `crates/boa-app/src/views/query_history.rs` (`QueryHistoryView`) | QueryHistory.tsx |
| Right panel History tab | same file (`HistoryPanel`), hosted by `views/right_panel.rs` | RightPanel.tsx `HistoryTab` |
| Wiring | `app_state.rs` (create + `watch`), `views/root.rs` (⌘⌥E, overlay, insert), `actions.rs` / `keymap.rs` (⌘↵, ⌘C) | App.tsx |

The rules (same-query key, dedupe, merge, retention cut-off, 5,000 cap, search,
display helpers, the per-connection list state machine) were already in
`boa_core::state::history` with the TS tests ported; the model only does the I/O
that state machine asks for.

### Recording

- Every console run and failure arrives as `WorkspacesEvent::OutputLogged { record: true }`
  (Console logs page re-fetches with `record: false`, as `pageResult` does), and is
  recorded unless Settings → Queries → "Save query history" is off.
- Appends go to `history-<connection id>.jsonl` in the config dir through
  boa-core's `history` store (the code behind `Backend::history_*`): an append replaces
  the earlier run of the same query (`query_key`), drops runs older than `historyDays`
  and keeps the newest 5,000. The format is unchanged (camelCase, one JSON object per
  line, owner-only); a test checks the exact objects written and reads them back with
  the store the Tauri backend uses.
- File operations run one at a time in request order on GPUI's background executor
  (not the backend's tokio runtime: no foreign wake-ups, so `#[gpui::test]`s that run
  statements stay deterministic without `allow_parking`).
- Lists load lazily (the overlay's scope, or the History tab's connection), then follow
  new runs in memory; records made during a load are merged in (`merge_loaded`).
  A new retention period re-reads every loaded list. Legacy key-less duplicates are
  deleted on load / record (as the TS does). Delete / clear update the list at once,
  then the file; on error a toast ("Could not delete history entry" / "Could not clear
  query history") and a re-read. A failing append toasts once per failure streak.
- A deleted connection's list is forgotten (the backend deletes its file).

### Query History overlay

⌘⌥E (also the console toolbar's history button; ⌘K's "Query History" can call
`History::set_open(true)`) toggles it, closing Settings first. Over the window
at 8 % from the top, `min(1000, vw − 32) × min(680, 84 % vh)`:

- search field (focused; every word in the SQL, any order), scope pills (the active
  connection / All connections), "Errors only", count ("12" or "3 of 12");
- virtualized list (64 px rows: two highlighted SQL lines + "conn · db · ago ·
  duration · outcome", error icon), detail pane (connection / database · source, local
  time · duration · outcome, Insert / New console / Copy / Delete, error + detail, the
  whole statement highlighted);
- footer hints, "Recording is off" when it is, "Clear history…" (alert dialog:
  "Deletes the N saved statements of <scope>. This cannot be undone."), esc.

Keys (bound in `QueryHistory > Input` and `QueryHistory`): ↑ ↓, PgUp / PgDn (a page
less one row), ⏎ insert at the console's caret (`insertion_text`: on its own line next
to other text; one undoable edit; the editor gets the focus back) or, when the active
tab isn't a console, open in a new console; ⌘⏎ new console on the entry's database
(`Workspaces::new_query_tab_with`); ⌘C copy ("Statement copied"; copies the field's
selection instead when it has one); ⌘⌫ delete; Esc / a click on the scrim closes.
Click selects, double click = ⏎.

### Right panel History tab

The active connection's list (lazy load), "Search history" field with a clear (×)
button, Esc clears it (`ClearPanelSearch` in `RightPanel > Input`), "Recent N" /
"Matches N", "No queries match “…”.", rows with up to 3 highlighted SQL lines and
"db · ago · duration · outcome"; a click opens the statement in a new console; the
tooltip is the full SQL. Empty states as in the source ("Loading history…",
"Queries you run on this connection show up here.", "Query history is turned off in
Settings → Queries."). The tab choice is prefs `rightTab` (already persisted).

## Tests (20)

- `models/history/tests.rs` (9): records runs and failures from the Output log, not
  page fetches; the exact JSONL written (+ read back by the Tauri store, mode 0600);
  nothing when recording is off; same-query replace (memory and disk); retention
  (30 days default, forever, 7 days, the next append prunes the file); the 5,000 cap
  (load and append); delete / clear / all-connections merge / forget; open / toggle
  events; and the store-level case **"records console runs and failures, not page
  re-fetches"** live against boa_dev (one-row pages of `generate_series`, two page
  fetches, a failing `select * from bad`: two entries saved with `rows: 1`,
  `truncated: true`, `source: "console_1"` and the `[42P01]` error; the Output log has 4).
- `views/query_history/tests.rs` (9): page step / clamped moves; count and empty texts,
  meta line; ⌘⌥E toggles, focus, Esc closes and refocuses the editor, ⌘⌥E over
  Settings; ↑ ↓ PgUp PgDn, search, errors only; ⏎ inserts at the caret (console SQL
  follows); ⌘C copies + toast, ⌘⏎ opens console_2; ⌘⌫ deletes (memory and disk);
  all connections merged newest first; the right panel tab's search, Esc, live update.

## Screenshots

- `query-history.png` — the overlay on "Local dev" (seeded history, an error entry,
  a truncated one, the detail pane).
- `right-panel-history.png` — the right panel's History tab.

`render_to_image` captures (feature `screenshot`, `BOA_DEV_CONNECTION=1`,
`BOA_DEV_HISTORY=overlay|panel`, a seeded `history-local-dev.jsonl` in a temp
`BOA_CONFIG_DIR`).

## Deviations / not done

- The overlay is drawn by the root view (a scrim + panel), not a gpui-component
  Dialog; the "Clear history…" confirmation is gpui-component's alert dialog.
- Rows are not text-selectable (GPUI text); Copy / ⌘C copy the statement.
- The list's PgUp / PgDn page is computed from the dialog height, not measured.
- The right panel's list isn't keyboard-navigable (it isn't in the source either).
