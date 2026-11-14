# DB Client

A desktop database client: Tauri (Rust) shell with a React + Tailwind UI.
The chrome (sidebar, title-bar tabs, right panel, status bar, ⌘K palette,
settings) is carried over from Civet's design.

Panels follow DataGrip: a Database Explorer tree (connection › database ›
schema › tables › columns), the editor (query consoles and table data
editors), a full-width Database Output panel (result grids + Output log, ⌥J)
and breadcrumbs in the status bar.

Status: **PostgreSQL only**. Add a data source (⌘O, or + in the explorer;
a `postgresql://` URL fills the form), connect, browse schemas / tables /
columns, open table data and run statements (⌘↵) from a console. Like
DataGrip, only picked databases and schemas are shown and introspected:
click the "N of M" badge on a connection or database (defaults: the
connection's database, the `public` schema). Settings
are saved in the app config dir (`connections.json`), passwords too, in
plain text for now (the file is owner-only on Unix). In a plain browser (`dev:web`) a mock backend serves the sample
data in `ui/src/state/mock.ts`.

CSV export: the Export menu (download icon) on a result grid or table
toolbar, or "Export to CSV…" on a table / view in the explorer, asks for a
file and writes the whole result, not just the loaded page: the server
streams `COPY (<statement>) TO STDOUT WITH (FORMAT csv)` into a temporary
file that is renamed into place when done (progress and Cancel in the status
bar; a toast and an Output log line at the end). Only read-only statements
are re-run this way, in their own transaction, or under a savepoint that is
rolled back when the session is inside a user transaction (so its
uncommitted rows are included and a failure leaves it intact). Results of
statements that change data (`INSERT … RETURNING`) are never re-run: their
loaded rows are written instead. The same menu copies the loaded page as CSV
and sets the delimiter (comma, tab, semicolon) and header row. NULL is an
empty field, an empty string `""`, as in Postgres's COPY. The browser mock
builds the file from the sample rows (at most 20,000) and downloads it.

Table data is editable like DataGrip's data editor, for tables with a
primary key (views, key-less tables and console results stay read-only and
say why). Double-click, Enter or start typing to edit a cell (⇧↵ opens a
larger editor for long text / JSON), ⌘⌥N sets NULL (not the same as an
empty string), ⌘N adds a row (untouched columns are left out of the
`INSERT`, so defaults apply), ⌘⌫ deletes the row, ⌘⌥Z reverts; also from the
right-click menu. Changes are only staged, tinted per kind and kept per tab,
until Submit: the preview (⌘↵) shows the exact statements (`UPDATE` /
`DELETE` by the row's original key, `INSERT`), which then run in one
transaction (or a savepoint inside an open one). Each `UPDATE` / `DELETE`
must hit exactly one row; any error rolls everything back and points at the
failing statement. Reloading, paging or closing a tab with pending changes
asks first. In the browser mock, submitted edits persist until the page is
refreshed.

Query History (⌘⌥E, the console toolbar's history button, or "Query
History" in ⌘K): every statement run from a console is saved per connection
in the app config dir (`history-<connection id>.jsonl`, JSON lines,
owner-only; the newest 5,000 kept; deleted with the connection), failed ones
with their error. Paging through a result does not add entries. Search
matches words in the SQL as you type, with an errors-only filter and a
toggle for all connections; ⏎ inserts the statement at the console's caret,
⌘⏎ opens it in a new console on its database, ⌘C copies, ⌘⌫ deletes. Entries
can be deleted one by one or cleared, and recording turned off in Settings →
Queries. The Output log (⌥J) stays a session-only log. In the browser mock
history lives in `localStorage`.

## Layout

```
crates/dbclient-desktop/         Tauri app (Rust side: src/lib.rs)
crates/dbclient-desktop/ui/      React frontend (Vite, Tailwind v4, shadcn/ui)
  src/state/store.ts             app state: connections, tabs, panels, toasts
  src/state/mock.ts              sample connections / schemas
  src/state/mockData.ts          generated rows for the browser mock
  src/lib/backend.ts             Tauri commands (or the browser mock)
  src/state/history.ts           query history (persisted; QueryHistory.tsx is the view)
crates/dbclient-desktop/src/     pg.rs (sessions, introspection, queries), connections.rs (saved settings + passwords),
                                 edits.rs (submitting grid edits), history.rs (query history)
  src/components/                Sidebar, TitleBar, Workspace, BottomPanel, DataGrid, RightPanel, StatusBar, …
```

## Run

```sh
# UI only, in a browser (http://localhost:1420; add ?overlay to preview the macOS title-bar inset)
pnpm --dir crates/dbclient-desktop/ui install
pnpm --dir crates/dbclient-desktop/ui dev:web

# Desktop app
pnpm --dir crates/dbclient-desktop/ui dev
```

Checks: `pnpm --dir crates/dbclient-desktop/ui typecheck`, `… lint`, `… test`,
`cargo clippy --all-targets`, `cargo test`. The live Postgres test runs when
`PGHOST` (and `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`) point at a
server; it creates and drops a `dbclient_test` schema.
