# boa-core

Boa's backend without any UI dependency: Postgres sessions, paging,
cancel, reconnect / retry-safety rules, read-only enforcement (`pg`), one
operation at a time per session (`pg::ops`), introspection and DDL
(`pg::catalog`), CSV export (`pg::export`), transaction-state probe
(`tx`), grid edits (`edits`), saved connections (`connections`), query
history (`history`), SSH tunnels (`ssh`), errors (`error::CmdError`), the
config dir (`config`), the async bridge the GPUI app uses
(`backend::Backend`), and the Tauri UI's pure logic ported from TypeScript
(`sql`, `grid`, `state`).

The backend modules are copied from the Tauri app (`crates/boa-desktop/src`,
commit 61984bb) with the Tauri layer removed; behaviour and on-disk formats
are unchanged. `connections.rs` and `history.rs` are the Tauri files as they
are (only a `Debug` derive added), so the GPUI app reads and writes
`connections.json` and `history-<id>.jsonl` with the Tauri app's own code
(`boa-app/src/tests/migration.rs` checks the bytes). The one behaviour change
is `pg::ops` (below), which fixes a race the Tauri app also has.

## Module layout

- `model`: the catalog and result types every other module shares:
  `Schema`, `Table` (`kind: TableKind`), `Column`, `Index`, `Constraint`
  (`kind: ConstraintKind`), `ForeignRef`, `QueryResult`, `ResultColumn`,
  `Value` (`Option<String>`). What `Backend::introspect` / `run_query`
  return goes straight into completion and the grid logic. They serialize as
  the Tauri IPC did (`kind` as `"table"` / `"view"` and the `contype`
  letter). Re-exported from `pg`, `sql` and `grid`.
- Backend: `backend` (the async bridge), `pg` (sessions, paging,
  introspection; `pg::ops` the per-session operation queue and
  `CancelHandle`, `pg::catalog` indexes / constraints / DDL, `pg::export`
  CSV export), `edits` (running grid edits), `tx`, `connections`,
  `history`, `ssh`, `config`, `error`.
- `sql`: tokenizer, statement splitting, completion (over `&[Schema]`),
  write guards. Pure functions, no I/O.
- `grid`: values / JSON, formatting, CSV, copy-as, staged edits
  (`grid_edits`: statements; `edits::EditsState`: per-tab state), table
  queries and metadata, connection tags. Pure logic; `GridError` is its error.
- `state`: the UI state rules of `ui/src/state/*.ts` and
  `ui/src/lib/{history,updates}.ts`, ported with their test cases. No GPUI;
  boa-app's models are built on them.
  - `prefs` (state/prefs.ts): `Prefs` with the TS defaults, `from_json` with
    load()'s rules (wrong types keep the default, unknown fields kept in
    `extra`), `reset` (keeps the layout fields), clamps, `apply_order`;
    `load` / `save` of `prefs.json`.
  - `session` (state/session.ts): `Session` / `Tab` serialized exactly as
    `JSON.stringify(pick(state))`, `parse_session` (pruning to saved
    connections), `next_tab_number`, `connections_to_restore`, scope helpers;
    `session.json`.
  - `csv_export` (state/csvExport.ts): `parse_options` / `options_json` for
    `csv-export.json`, export sources and routes (re-run vs loaded rows),
    savepoint rules, jobs, toast texts.
  - `local_storage`: the one-time import of the Tauri app's WebKit
    localStorage (`boa.prefs.v2`, `boa.session.v1`, `boa.csvExport`, UTF-16LE
    blobs in `localstorage.sqlite3`, read from a copy) into those three files
    (`import_once`, marker `.localstorage-imported`), and
    `write_config_file` (temp file + rename, 0600).
  - `reconnect` (store.ts / links.ts rules): backoff (`RETRY_DELAYS_MS`,
    1 s … 30 s, 2 min in all), which errors mean lost, wake detection, one
    status toast per connection (`Recoveries`).
  - `tx`, `guards` (when to ask: destructive statements, quit / disconnect /
    reconnect / update with open transactions, the 3 s double-⌘Q gate),
    `history` (queryKey normalisation, search, retention, the 5,000 cap),
    `ddl`, `conn_url` (postgresql:// URL paste), `updates` (latest.json,
    version selection, the updater state machine).

Glue between the two halves:

- `EditsState<QueryResult>`: `QueryResult` implements `grid::edits::PageRows`.
- `grid::grid_edits::EditStatement` → `edits::EditStatement` (what
  `Backend::apply_edits` takes) with `From` / `Into` (by value or `&`).
- `SubmitOutcome::from(&CmdResult<ApplyOutcome>)` and
  `ErrorText::from(&CmdError)` turn the `apply_edits` result into what
  `EditsState::finish_submit` takes.

## `Backend`

`Backend::new()` uses the Tauri app's config dir
(`config::config_dir()` = `~/Library/Application Support/dev.boa.desktop/`);
`Backend::with_config_dir(dir)` uses another (tests). Both start one
multi-thread tokio runtime (threads `boa-backend`). `Backend` is
`Clone + Send + Sync`; the runtime stops (and SSH tunnels are killed) when
the last clone drops. An app that keeps a clone until the process exits (the
GPUI app's `AppState` global is never dropped) must call `close_tunnels()`
when quitting.

Every command returns `BackendTask<T>`: a future of `CmdResult<T>` that
any executor can await (GPUI's `cx.spawn`, `futures::executor::block_on`).
The command starts when the method is called and runs to the end even if
the task is dropped. If the backend shuts down first, it resolves to an error.

**One operation at a time per session** (`pg::ops`). Each (connection,
database) has one Postgres session, shared by every console, table tab,
export and explorer read on it. An operation (a run with its
`begin` / `declare` / `fetch` / `close` / `commit` and the transaction
probe, a page, an export with its savepoint, an introspection) first claims
the session from a fair FIFO queue and holds it until its last step, so two
operations never interleave their steps on the server (before, a console
run during an export lost its cursor: `cursor "__boa" does not exist`).
Operations on different databases or connections still run in parallel. A
session that dropped while an operation waited is replaced, not reused.

**Cancelling.** `run_query` and `export_csv` return tasks that carry a
`pg::CancelHandle` (`BackendTask::cancel_handle()`). `Backend::cancel(&handle)`
cancels that one operation: still waiting for its session, it fails at once
with SQLSTATE 57014 without running (`pg::cancelled_error()`); running, the
server cancels it (57014 too); once it ended, nothing happens.
`cancel_query(id, database)` still cancels whatever runs on that session,
whoever started it; operations waiting behind it then run.

| Method | Args | Returns |
|---|---|---|
| `list_connections` | | `Vec<SavedConnection>` (config + `has_password`) |
| `save_connection` | `config: ConnConfig, password: Option<String>, save_password: bool` | `()`. `Some` stores the password, `None` keeps the stored one, `save_password: false` forgets it |
| `delete_connection` | `id` | `()`. Also disconnects and clears its history |
| `test_connection` | `config, password: Option<String>` (None: stored one) | server version `String` |
| `connect` | `id, password: Option<String>` (None: stored one) | `ConnectInfo { server_version, databases }` |
| `ping` | `id` | `PingInfo { lost_tx: Vec<String> }`. Errors `not_connected` / `unreachable` |
| `disconnect` | `id` | `()` |
| `set_tx_mode` | `id, database, manual: bool` | `TxState` |
| `list_schemas` | `id, database` | `Vec<String>` |
| `introspect` | `id, database, schemas: Vec<String>` | `Vec<Schema>` |
| `table_ddl` | `id, database, schema, table` | `String` |
| `redis_keys` | `id, database` (`db0`…), `limit` | `RedisKeys { keys, total, truncated }` (the first `limit`; the explorer starts with `redis::KEY_LIMIT`) |
| `run_query` | `id, database, sql, limit, offset` (limit clamped to 1..=100 000) | `QueryResult`; the task carries a `CancelHandle` |
| `cancel` | `handle: &CancelHandle` | `()`. Cancels that one statement or export, waiting or running |
| `cancel_query` | `id, database` | `()`. Cancels whatever runs on the session (also an export) |
| `apply_edits` | `id, database, statements: Vec<EditStatement>` | `ApplyOutcome` (a failing statement is in `failed`, not an error) |
| `export_csv` | `id, database, sql, path, options: CsvOptions` | `CsvExport { result: BackendTask<ExportResult>, progress }`; `result` carries a `CancelHandle` |
| `write_csv_file` | `path, contents: String` | `()` (temp file + rename) |
| `reveal_path` | `path` | `()` (Finder `open -R`) |
| `history_list` | `id, days: Option<u32>` (0 / None: all) | `Vec<history::Entry>`, oldest first |
| `history_append` | `id, entry, days: Option<u32>` | `()` |
| `history_delete` | `id, ids: Vec<String>` | `()` |
| `history_clear` | `id` | `()` |

Synchronous helpers: `config_dir()`, `close_tunnels()` (call when quitting),
`link_events()`.

Errors are `CmdError { message, code, position, detail, hint, tx }`. App
codes: `password_required`, `not_connected`, `unreachable`,
`connection_lost` (the connection dropped; also "commit unknown", whose hint
says to check the data), `tx_lost`, `read_only`, `row_count`; other codes
are SQLSTATEs.

### Events

Both are `async_channel::Receiver`s, so they can be awaited from any executor.

- **CSV export progress:** `CsvExport::progress` yields
  `ExportProgress { rows, bytes }` about every 250 ms while rows arrive and
  closes when the export ends. Was the Tauri `csv-export-progress` event.
- **Link events:** `Backend::link_events()` subscribes (one receiver per
  call, unbounded; drop it to unsubscribe) to `LinkEvent`:
  - `TxLost { connection_id, database }`: a call failed with `tx_lost`;
  - `Lost { connection_id }`: a call failed with `tx_lost`,
    `connection_lost` or `unreachable`; the connection may be down.

  Sent by `list_schemas`, `introspect`, `table_ddl`, `run_query`,
  `set_tx_mode`, `apply_edits` and `export_csv`. This was the UI's
  `trackLinks` (`ui/src/state/links.ts`); the reconnect loop (backoff, ping,
  one toast per connection) is still the UI's to build on top of it.

Not ported: `quit` and the `quit-requested` event (the GPUI app handles
quitting), updater / menu / About commands.

## Redis

Connections have a `kind` (`ConnConfig::kind`: `postgres`, the default and
never written, or `redis`). `Backend` routes Redis connections to `redis.rs`:
`connect` lists the logical databases as `db0`…`dbN`; `run_query` runs
redis-cli syntax (one command per line, the last reply as the grid);
`redis_keys` feeds the explorer; `list_schemas` / `introspect` return
nothing; `table_ddl`, `apply_edits` and `set_tx_mode` fail with code
`unsupported`; `export_csv` re-runs the (read-only) commands and writes the rows.
Read-only connections refuse commands the server flags as writing.

## Tests

`cargo test -p boa-core`. The live tests (`pg::tests::live_*`,
`pg::concurrency_tests`, `backend::tests::live_backend`) run when `PGHOST` is
set, with `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`; `live_postgres`
creates and drops a `boa_test` schema.

The live Redis tests (`redis::tests::live_*`) run when `BOA_REDIS_PORT` (no
password) and `BOA_REDIS_AUTH_PORT` (password `boa-secret`) are set, e.g.
`docker run -d --name boa-redis -p 6379:6379 redis:7` and
`docker run -d --name boa-redis-auth -p 6380:6379 redis:7 redis-server --requirepass boa-secret`;
they use db 15 and only `boa_test:*` keys.

`#[ignore]`d checks on this machine's real data (they read a copy, print
counts only): `state::local_storage::tests::real_local_storage_keys`. The
end-to-end migration test (a Tauri install → the GPUI app → files the Tauri
app still reads) is in boa-app: `tests::migration`.
