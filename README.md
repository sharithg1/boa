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

Transactions and guard rails: each database session (shared by every
console and table tab on that database) is in **Tx: Auto** (each statement
commits) or **Manual** mode (a transaction is opened before the first
statement and stays open until Commit / Rollback: ⌘⇧↵ / ⌘⇧⌫, or typed
`commit` / `rollback`). After every statement the backend probes the
server for the real state (idle, open, aborted: 25P02 until rolled back),
shown in the console toolbar and the status bar; typed `begin` works in
either mode. Disconnecting, reconnecting (after editing the data source) or
quitting with a transaction open asks first, and rolls it back. Destructive
statements (UPDATE / DELETE without WHERE, TRUNCATE, DROP, ALTER TABLE …
DROP COLUMN; found with the editor's tokenizer, so comments, strings and
CTEs don't fool it) ask for confirmation; Settings can turn that off,
except on connections tagged Production. A data source can be marked
read-only (enforced by the server: `default_transaction_read_only`, and
put back if a statement turns it off) and given an environment tag and a
colour, shown on the explorer row, the editor, the console toolbar and the
status bar.

## Layout

```
crates/dbclient-desktop/         Tauri app (Rust side: src/lib.rs)
crates/dbclient-desktop/ui/      React frontend (Vite, Tailwind v4, shadcn/ui)
  src/state/store.ts             app state: connections, tabs, panels, toasts
  src/state/tx.ts, guards.ts     transaction mode / state per session; confirmations
  src/lib/guard.ts               destructive and read-only statement checks
  src/state/mock.ts              sample connections / schemas
  src/state/mockData.ts          generated rows for the browser mock
  src/lib/backend.ts             Tauri commands (or the browser mock)
crates/dbclient-desktop/src/     pg.rs (sessions, introspection, queries), tx.rs (transaction state probe), connections.rs (saved settings + passwords)
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
