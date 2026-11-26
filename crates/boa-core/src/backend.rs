//! The async bridge between the UI and the backend.
//!
//! [`Backend`] owns one multi-thread tokio runtime (worker threads named
//! `boa-backend`) and the backend state: open Postgres sessions (`Pg`), the
//! query history lock and the config dir. It has one method per command of
//! the Tauri app (`generate_handler![…]` in its `lib.rs`, `ui/src/lib/backend.ts`).
//!
//! Every command method starts its work on the runtime **at once** and
//! returns a [`BackendTask`], a plain future (a `futures` oneshot receiver)
//! that any executor can await: GPUI's, `futures::executor::block_on`, or
//! tokio's. Dropping a `BackendTask` does not cancel the command (as with a
//! Tauri `invoke`). Statements and exports carry a [`CancelHandle`]
//! ([`BackendTask::cancel_handle`], [`Backend::cancel`]) that stops that one
//! operation, running or still waiting for its session; [`Backend::cancel_query`]
//! stops whatever statement runs on a database's session.
//!
//! One session per (connection, database) is shared by all callers, and
//! runs one operation at a time, in call order (`pg/ops.rs`): a console
//! run, a page, an export or an explorer read started while another one
//! uses the session waits for it.
//!
//! Events go out over `async-channel` receivers, also executor-agnostic:
//! - [`Backend::export_csv`] returns the export's progress channel
//!   ([`ExportProgress`], about every 250 ms; closed when the export ends);
//! - [`Backend::link_events`] subscribes to [`LinkEvent`]s: a call on an
//!   open connection failed because the server can't be reached or the
//!   connection dropped (and a transaction was lost with it). This is the
//!   UI's `trackLinks` (`ui/src/state/links.ts`), moved here so every
//!   caller reports them; the reconnect loop listens to it.
//!
//! Commands that only touch files (connections, history, CSV writes) run on
//! the runtime's blocking pool, so the UI thread never does file IO either.

use crate::connections::{self, ConnConfig, SavedConnection};
use crate::edits::{ApplyOutcome, EditStatement};
use crate::error::{CmdError, CmdResult, UNREACHABLE};
use crate::history::{self, History};
use crate::pg::{
    self, CancelHandle, ConnectInfo, CsvOptions, ExportProgress, ExportResult, Pg, PingInfo,
    QueryResult, Schema,
};
use crate::tx::TxState;
use futures_channel::oneshot;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

/// The most rows `run_query` returns in one page.
pub const MAX_PAGE_ROWS: usize = 100_000;

/// A dropped connection, noticed by a failed call (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// A call failed on the network (`unreachable`, `connection_lost`,
    /// `tx_lost`): the connection may be down; check it with `ping`.
    Lost { connection_id: String },
    /// A session dropped with a transaction open; the server rolled it back.
    /// Followed by a `Lost` for the same connection.
    TxLost {
        connection_id: String,
        database: String,
    },
}

/// The result of a backend command, awaitable from any executor. Resolves to
/// an error if the backend shut down (or the command panicked) first.
#[must_use = "the command runs anyway; await this for its result"]
#[derive(Debug)]
pub struct BackendTask<T> {
    rx: oneshot::Receiver<CmdResult<T>>,
    cancel: Option<CancelHandle>,
}

impl<T> BackendTask<T> {
    /// Cancels this command (statements and exports only; see
    /// [`Backend::cancel`]).
    pub fn cancel_handle(&self) -> Option<CancelHandle> {
        self.cancel.clone()
    }

    fn with_cancel(mut self, cancel: CancelHandle) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl<T> Future for BackendTask<T> {
    type Output = CmdResult<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.rx).poll(cx).map(|r| {
            r.unwrap_or_else(|_| Err(CmdError::msg("The backend stopped before answering")))
        })
    }
}

/// A running CSV export (see [`Backend::export_csv`]).
#[derive(Debug)]
pub struct CsvExport {
    /// Rows and bytes written, and the time it took.
    pub result: BackendTask<ExportResult>,
    /// Progress while rows arrive; closed when the export ends.
    pub progress: async_channel::Receiver<ExportProgress>,
}

/// State shared by the commands running on the runtime.
#[derive(Debug)]
struct Inner {
    pg: Pg,
    history: History,
    config_dir: PathBuf,
    links: Mutex<Vec<async_channel::Sender<LinkEvent>>>,
}

impl Inner {
    /// Reports a failure of a call on connection `id` that means it dropped.
    fn watch<T>(&self, id: &str, database: &str, r: &CmdResult<T>) {
        let Err(e) = r else { return };
        let code = e.code.as_deref();
        if code == Some("tx_lost") {
            self.emit(LinkEvent::TxLost {
                connection_id: id.to_owned(),
                database: database.to_owned(),
            });
        }
        if matches!(code, Some("tx_lost" | "connection_lost" | UNREACHABLE)) {
            self.emit(LinkEvent::Lost {
                connection_id: id.to_owned(),
            });
        }
    }

    fn emit(&self, e: LinkEvent) {
        self.links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.try_send(e.clone()).is_ok());
    }
}

/// Owns the runtime; dropped with the last `Backend` clone (tasks only hold `Inner`).
#[derive(Debug)]
struct Runtime {
    runtime: Option<tokio::runtime::Runtime>,
    inner: Arc<Inner>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // The process may exit without dropping running tasks' state: kill
        // the SSH tunnels now. Shutting down in the background is allowed
        // from async code too (the last clone may drop on any thread).
        self.inner.pg.close_tunnels();
        if let Some(rt) = self.runtime.take() {
            rt.shutdown_background();
        }
    }
}

/// Handle to the backend: cheap to clone, `Send + Sync`. See the module docs.
#[derive(Debug, Clone)]
pub struct Backend {
    inner: Arc<Inner>,
    handle: tokio::runtime::Handle,
    _runtime: Arc<Runtime>,
}

/// Start of a query history retention period of `days` (0 or absent: no limit).
pub fn history_cutoff(days: Option<u32>) -> Option<f64> {
    let days = days.filter(|d| *d > 0)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    Some(now.as_secs_f64() * 1000.0 - f64::from(days) * 86_400_000.0)
}

impl Backend {
    /// A backend keeping its files in the app's config dir
    /// (`crate::config::config_dir`, shared with the Tauri app).
    pub fn new() -> CmdResult<Self> {
        let dir = crate::config::config_dir()
            .ok_or_else(|| CmdError::msg("Could not find the home directory"))?;
        Self::with_config_dir(dir)
    }

    /// A backend keeping `connections.json` and the history files in `dir`
    /// (created on first write). Starts the runtime's threads.
    pub fn with_config_dir(dir: impl Into<PathBuf>) -> CmdResult<Self> {
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4));
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(workers)
            .max_blocking_threads(16)
            .thread_name("boa-backend")
            .enable_all()
            .build()?;
        let inner = Arc::new(Inner {
            pg: Pg::default(),
            history: History::default(),
            config_dir: dir.into(),
            links: Mutex::new(Vec::new()),
        });
        Ok(Self {
            handle: runtime.handle().clone(),
            _runtime: Arc::new(Runtime {
                runtime: Some(runtime),
                inner: inner.clone(),
            }),
            inner,
        })
    }

    /// Where `connections.json` and `history-<id>.jsonl` live.
    pub fn config_dir(&self) -> &Path {
        &self.inner.config_dir
    }

    /// Runs `f`'s future on the runtime.
    fn spawn<T, Fut>(&self, f: impl FnOnce(Arc<Inner>) -> Fut) -> BackendTask<T>
    where
        T: Send + 'static,
        Fut: Future<Output = CmdResult<T>> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let fut = f(self.inner.clone());
        self.handle.spawn(async move {
            let _ = tx.send(fut.await);
        });
        BackendTask { rx, cancel: None }
    }

    /// Runs blocking `f` (file IO) on the runtime's blocking pool.
    fn spawn_blocking<T>(
        &self,
        f: impl FnOnce(&Inner) -> CmdResult<T> + Send + 'static,
    ) -> BackendTask<T>
    where
        T: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let inner = self.inner.clone();
        self.handle.spawn_blocking(move || {
            let _ = tx.send(f(&inner));
        });
        BackendTask { rx, cancel: None }
    }

    /// Subscribes to [`LinkEvent`]s. Each receiver gets every event from now
    /// on; dropping it unsubscribes.
    pub fn link_events(&self) -> async_channel::Receiver<LinkEvent> {
        let (tx, rx) = async_channel::unbounded();
        self.inner
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    /// Kills every SSH tunnel at once (call when quitting: the process may
    /// exit without dropping the backend).
    pub fn close_tunnels(&self) {
        self.inner.pg.close_tunnels();
    }

    // ── Saved connections ────────────────────────────────────────────────

    /// The saved connections, with whether a password is stored.
    pub fn list_connections(&self) -> BackendTask<Vec<SavedConnection>> {
        self.spawn_blocking(|i| connections::list(&i.config_dir))
    }

    /// Saves a connection (by `config.id`). `password`: `Some` stores it
    /// (when `save_password`), `None` keeps the stored one;
    /// `save_password: false` forgets it.
    pub fn save_connection(
        &self,
        config: ConnConfig,
        password: Option<String>,
        save_password: bool,
    ) -> BackendTask<()> {
        self.spawn_blocking(move |i| {
            connections::upsert(&i.config_dir, config, password, save_password)
        })
    }

    /// Disconnects, then deletes the saved connection and its query history.
    pub fn delete_connection(&self, id: &str) -> BackendTask<()> {
        let id = id.to_owned();
        self.spawn(|i| async move {
            i.pg.disconnect(&id).await;
            connections::remove(&i.config_dir, &id)?;
            i.history.clear(&i.config_dir, &id)
        })
    }

    /// Connects with unsaved settings (through a tunnel of its own) and
    /// returns the server version (`PostgreSQL 17.2`). `password: None` uses
    /// the stored one of `config.id`, if any.
    pub fn test_connection(
        &self,
        config: ConnConfig,
        password: Option<String>,
    ) -> BackendTask<String> {
        self.spawn(|i| async move {
            let password = match password {
                Some(p) => Some(p),
                None => connections::password(&i.config_dir, &config.id)?,
            };
            Pg::test(&config, password.as_deref()).await
        })
    }

    // ── Connections and sessions ─────────────────────────────────────────

    /// Connects a saved connection, with `password` or else the stored one.
    /// Fails with code `password_required` when the server wants one.
    pub fn connect(&self, id: &str, password: Option<String>) -> BackendTask<ConnectInfo> {
        let id = id.to_owned();
        self.spawn(|i| async move {
            let (config, stored) = connections::get(&i.config_dir, &id)?
                .ok_or_else(|| CmdError::msg("Unknown connection"))?;
            i.pg.connect(config, password.or(stored)).await
        })
    }

    /// Checks an open connection without disturbing it, reopening dropped
    /// sessions; reports the transactions lost with them. Fails with
    /// `not_connected` if it isn't open, `unreachable` if the server doesn't answer.
    pub fn ping(&self, id: &str) -> BackendTask<PingInfo> {
        let id = id.to_owned();
        self.spawn(|i| async move { i.pg.ping(&id).await })
    }

    /// Closes the connection's sessions (rolling back open transactions) and its tunnel.
    pub fn disconnect(&self, id: &str) -> BackendTask<()> {
        let id = id.to_owned();
        self.spawn(|i| async move {
            i.pg.disconnect(&id).await;
            Ok(())
        })
    }

    /// Auto (`manual: false`) or Manual transaction mode for one session;
    /// returns its transaction state.
    pub fn set_tx_mode(&self, id: &str, database: &str, manual: bool) -> BackendTask<TxState> {
        let (id, database) = (id.to_owned(), database.to_owned());
        self.spawn(move |i| async move {
            let r = i.pg.set_tx_mode(&id, &database, manual).await;
            i.watch(&id, &database, &r);
            r
        })
    }

    // ── Introspection ────────────────────────────────────────────────────

    /// Every schema name in the database (cheap: for the schema picker).
    pub fn list_schemas(&self, id: &str, database: &str) -> BackendTask<Vec<String>> {
        let (id, database) = (id.to_owned(), database.to_owned());
        self.spawn(|i| async move {
            let r = i.pg.list_schemas(&id, &database).await;
            i.watch(&id, &database, &r);
            r
        })
    }

    /// Tables, views, columns, indexes and constraints of `schemas` only.
    pub fn introspect(
        &self,
        id: &str,
        database: &str,
        schemas: Vec<String>,
    ) -> BackendTask<Vec<Schema>> {
        let (id, database) = (id.to_owned(), database.to_owned());
        self.spawn(|i| async move {
            let r = i.pg.introspect(&id, &database, &schemas).await;
            i.watch(&id, &database, &r);
            r
        })
    }

    /// DDL of one table / view, assembled from the server's own renderings.
    pub fn table_ddl(
        &self,
        id: &str,
        database: &str,
        schema: &str,
        table: &str,
    ) -> BackendTask<String> {
        let (id, database) = (id.to_owned(), database.to_owned());
        let (schema, table) = (schema.to_owned(), table.to_owned());
        self.spawn(|i| async move {
            let r = i.pg.table_ddl(&id, &database, &schema, &table).await;
            i.watch(&id, &database, &r);
            r
        })
    }

    // ── Statements ───────────────────────────────────────────────────────

    /// Runs one statement (or a batch); row-returning ones return `limit`
    /// rows (clamped to 1..=[`MAX_PAGE_ROWS`]) after the first `offset`
    /// (pageable statements only). Errors carry the transaction state.
    /// Waits for the session while another operation uses it; cancel it
    /// with its [`BackendTask::cancel_handle`].
    pub fn run_query(
        &self,
        id: &str,
        database: &str,
        sql: &str,
        limit: usize,
        offset: usize,
    ) -> BackendTask<QueryResult> {
        let (id, database, sql) = (id.to_owned(), database.to_owned(), sql.to_owned());
        let cancel = CancelHandle::new();
        let handle = cancel.clone();
        self.spawn(move |i| async move {
            let limit = limit.clamp(1, MAX_PAGE_ROWS);
            let r =
                i.pg.query_with(&id, &database, &sql, limit, offset, &handle)
                    .await;
            i.watch(&id, &database, &r);
            r
        })
        .with_cancel(cancel)
    }

    /// Cancels the statement (or export) running on the database's session,
    /// whichever caller started it; ones waiting for the session still run.
    pub fn cancel_query(&self, id: &str, database: &str) -> BackendTask<()> {
        let (id, database) = (id.to_owned(), database.to_owned());
        self.spawn(|i| async move { i.pg.cancel(&id, &database).await })
    }

    /// Cancels one statement or export (its [`BackendTask::cancel_handle`]):
    /// still waiting for its session, it fails at once with 57014 without
    /// running; running, the server cancels it (57014 too). A no-op once it ended.
    pub fn cancel(&self, handle: &CancelHandle) -> BackendTask<()> {
        let handle = handle.clone();
        self.spawn(|_| async move { handle.cancel().await })
    }

    /// Runs the data editor's statements as one unit: all or nothing, each
    /// `exactly_one` statement must affect exactly one row (in a savepoint
    /// inside an open transaction). A failing statement is reported in the
    /// outcome, not as an error.
    pub fn apply_edits(
        &self,
        id: &str,
        database: &str,
        statements: Vec<EditStatement>,
    ) -> BackendTask<ApplyOutcome> {
        let (id, database) = (id.to_owned(), database.to_owned());
        self.spawn(|i| async move {
            let r = i.pg.apply_edits(&id, &database, &statements).await;
            i.watch(&id, &database, &r);
            r
        })
    }

    // ── CSV export ───────────────────────────────────────────────────────

    /// Streams the whole result of a read-only statement to `path` as CSV
    /// (server-side `COPY`, through a temporary file renamed over `path`).
    /// Cancelled with `result`'s [`BackendTask::cancel_handle`] (or
    /// `cancel_query` on the same database while it runs).
    pub fn export_csv(
        &self,
        id: &str,
        database: &str,
        sql: &str,
        path: impl Into<PathBuf>,
        options: CsvOptions,
    ) -> CsvExport {
        let (id, database, sql) = (id.to_owned(), database.to_owned(), sql.to_owned());
        let path = path.into();
        let (tx, progress) = async_channel::unbounded();
        let cancel = CancelHandle::new();
        let handle = cancel.clone();
        let result = self
            .spawn(move |i| async move {
                let send = |p| {
                    let _ = tx.try_send(p);
                };
                let r =
                    i.pg.export_csv_with(&id, &database, &sql, &path, options, send, &handle)
                        .await;
                i.watch(&id, &database, &r);
                r
            })
            .with_cancel(cancel);
        CsvExport { result, progress }
    }

    /// Writes CSV built by the UI (rows of a result that must not be re-run),
    /// through a temporary file and a rename.
    pub fn write_csv_file(&self, path: impl Into<PathBuf>, contents: String) -> BackendTask<()> {
        let path = path.into();
        self.spawn_blocking(move |_| pg::write_file_atomic(&path, contents.as_bytes()))
    }

    /// Shows the file in Finder (the folder elsewhere).
    pub fn reveal_path(&self, path: impl Into<PathBuf>) -> BackendTask<()> {
        let path = path.into();
        self.spawn_blocking(move |_| {
            let mut cmd;
            if cfg!(target_os = "macos") {
                cmd = std::process::Command::new("open");
                cmd.arg("-R").arg(&path);
            } else if cfg!(target_os = "windows") {
                cmd = std::process::Command::new("explorer");
                cmd.arg(format!("/select,{}", path.display()));
            } else {
                cmd = std::process::Command::new("xdg-open");
                cmd.arg(path.parent().unwrap_or(&path));
            }
            cmd.spawn()?;
            Ok(())
        })
    }

    // ── Query history ────────────────────────────────────────────────────

    /// The connection's query history of the last `days` (0 or None: all), oldest first.
    pub fn history_list(&self, id: &str, days: Option<u32>) -> BackendTask<Vec<history::Entry>> {
        let id = id.to_owned();
        self.spawn_blocking(move |i| i.history.list(&i.config_dir, &id, history_cutoff(days)))
    }

    /// Saves a run in place of the same query's earlier one; drops runs
    /// older than `days` (0 or None: none) and the oldest past `history::CAP`.
    pub fn history_append(
        &self,
        id: &str,
        entry: history::Entry,
        days: Option<u32>,
    ) -> BackendTask<()> {
        let id = id.to_owned();
        self.spawn_blocking(move |i| {
            i.history
                .append(&i.config_dir, &id, &entry, history_cutoff(days))
        })
    }

    /// Removes the entries with these ids.
    pub fn history_delete(&self, id: &str, ids: Vec<String>) -> BackendTask<()> {
        let id = id.to_owned();
        self.spawn_blocking(move |i| i.history.delete(&i.config_dir, &id, &ids))
    }

    /// Deletes the connection's history file.
    pub fn history_clear(&self, id: &str) -> BackendTask<()> {
        let id = id.to_owned();
        self.spawn_blocking(move |i| i.history.clear(&i.config_dir, &id))
    }
}

#[cfg(test)]
mod tests;
