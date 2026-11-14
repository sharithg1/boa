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
crates/dbclient-desktop/src/     pg.rs (sessions, introspection, queries), connections.rs (saved settings + passwords), history.rs (query history)
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
