//! [`Console`]: one console tab (Workspace.tsx `QueryView`, store.ts
//! `runQuery` / `pageResult` / `cancelQuery` / `results` / `running` /
//! `failed`): its SQL, run / cancel, its latest result (one result tab per
//! console in the Database Output panel), paging, and the Output log lines
//! it writes. Created per tab by [`super::workspaces::Workspaces`].
//!
//! Running: connect first when needed ([`Connections::ensure_connected`]),
//! then the guard rails (`boa_core::state::guards::check_run`: read-only
//! escape → not run; destructive → [`ConsoleEvent::ConfirmRun`], the view
//! asks and calls [`Console::execute`]), then `Backend::run_query` with
//! `prefs.rowLimit` rows.
//!
//! [`Connections::ensure_connected`]: super::connections::Connections::ensure_connected

use std::sync::Arc;
use std::time::Instant;

use boa_core::error::CmdError;
use boa_core::grid::conn_tag::is_production;
use boa_core::model::QueryResult;
use boa_core::pg::CancelHandle;
use boa_core::sql::guard::Risk;
use boa_core::state::guards::{RunContext, RunGuard, check_redis_run, check_run};
use boa_core::state::history::OutputEntry;
use gpui::{Context, EventEmitter, SharedString, Task};

use super::catalogs::DbKey;
use super::layout::PanelKind;
use super::toasts::NewToast;
use super::workspaces::{TabId, now_ms};
use crate::app_state::AppState;
use crate::grid::error::RunError;

/// The latest result of a console (store.ts `ResultSet`).
#[derive(Debug, Clone)]
pub struct ConsoleResult {
    pub sql: String,
    pub result: Arc<QueryResult>,
    /// Rows skipped before this page.
    pub offset: usize,
    /// `prefs.rowLimit` when it ran.
    pub page_size: usize,
    pub ms: u64,
    /// Which run produced it (pages of one run keep it; store.ts `at`).
    pub run: u64,
}

/// What a console asks of its view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsoleEvent {
    /// A destructive statement: ask first ("Run a destructive statement?"),
    /// then [`Console::execute`] it.
    ConfirmRun {
        sql: String,
        risks: Vec<Risk>,
        production: bool,
    },
}

/// One console.
#[derive(Debug)]
pub struct Console {
    pub key: DbKey,
    pub tab_id: TabId,
    pub title: SharedString,
    pub sql: String,
    /// The toolbar's default schema (completion ranks it first); None: the
    /// database's first schema in scope.
    pub default_schema: Option<String>,
    pub result: Option<ConsoleResult>,
    /// When the statement (or page) in flight started.
    pub running: Option<Instant>,
    /// The last statement failed (the strip under the editor), until the next run.
    pub failed: Option<RunError>,
    runs: u64,
    /// Connecting before a run.
    connect: Option<Task<()>>,
    /// The statement or page in flight.
    task: Option<Task<()>>,
    /// Cancels it (also while it waits behind another operation on the session).
    cancel: Option<CancelHandle>,
}

impl EventEmitter<ConsoleEvent> for Console {}

impl Console {
    pub fn new(key: DbKey, tab_id: TabId, title: SharedString, sql: impl Into<String>) -> Self {
        Self {
            key,
            tab_id,
            title,
            sql: sql.into(),
            default_schema: None,
            result: None,
            running: None,
            failed: None,
            runs: 0,
            connect: None,
            task: None,
            cancel: None,
        }
    }

    /// The editor's text changed.
    pub fn set_sql(&mut self, sql: String) {
        self.sql = sql;
    }

    pub fn set_default_schema(&mut self, schema: Option<String>, cx: &mut Context<Self>) {
        self.default_schema = schema;
        cx.notify();
    }

    /// Drops the failure strip (its ×).
    pub fn dismiss_failure(&mut self, cx: &mut Context<Self>) {
        if self.failed.take().is_some() {
            cx.notify();
        }
    }

    /// Drops the result (closing its result tab).
    pub fn close_result(&mut self, cx: &mut Context<Self>) {
        if self.result.take().is_some() {
            if let Some(state) = AppState::try_global(cx).cloned() {
                let id = self.tab_id.clone();
                state.workspaces.update(cx, |w, cx| {
                    if w.bottom_view.as_ref() == Some(&id) {
                        w.set_bottom_view(None, cx);
                    }
                });
            }
            cx.notify();
        }
    }

    /// Runs one statement (the armed one, or the selection): connects first
    /// when needed, checks the guard rails, then executes. Nothing while
    /// another statement of this console runs.
    pub fn run(&mut self, sql: String, cx: &mut Context<Self>) {
        let text = sql.trim().to_owned();
        if text.is_empty() || self.running.is_some() {
            return;
        }
        self.failed = None;
        cx.notify();
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let id = self.key.connection_id.clone();
        let connect = if state.connections.read(cx).is_connected(&id) {
            None
        } else {
            Some(
                state
                    .connections
                    .update(cx, |c, cx| c.ensure_connected(&id, cx)),
            )
        };
        self.connect = Some(cx.spawn(async move |this, cx| {
            if let Some(connect) = connect {
                let _ = connect.await;
            }
            this.update(cx, |this, cx| this.after_connect(text, cx))
                .ok();
        }));
    }

    fn after_connect(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let id = self.key.connection_id.clone();
        let (connected, saved) = {
            let conns = state.connections.read(cx);
            (conns.is_connected(&id), conns.get(&id).cloned())
        };
        let at = now_ms();
        if !connected {
            let name = saved
                .as_ref()
                .map_or("Connection", |s| s.config.name.as_str());
            let error = RunError::plain(format!("{name} is not connected"));
            self.fail(text, at, error, cx);
            return;
        }
        let ctx = RunContext {
            read_only: saved.as_ref().is_some_and(|s| s.config.read_only),
            production: saved
                .as_ref()
                .is_some_and(|s| is_production(crate::views::env_tag(s.config.environment))),
            confirm_destructive: state.prefs.read(cx).get().confirm_destructive,
        };
        let redis = saved.as_ref().is_some_and(|s| s.config.kind.is_redis());
        let guard = if redis {
            check_redis_run(&text, ctx)
        } else {
            check_run(&text, ctx)
        };
        match guard {
            RunGuard::Go => self.execute(text, cx),
            RunGuard::Blocked(message) => self.fail(text, at, RunError::plain(message), cx),
            RunGuard::Confirm(risks) => cx.emit(ConsoleEvent::ConfirmRun {
                sql: text,
                risks,
                production: ctx.production,
            }),
        }
    }

    /// Runs `sql` now (after the guard rails said go, or the user confirmed).
    pub fn execute(&mut self, sql: String, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let page_size = state.prefs.read(cx).get().page_size() as usize;
        let (id, db) = (self.key.connection_id.clone(), self.key.database.clone());
        self.failed = None;
        self.running = Some(Instant::now());
        cx.notify();
        let at = now_ms();
        let query = state.backend.run_query(&id, &db, &sql, page_size, 0);
        self.cancel = query.cancel_handle();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = query.await;
            this.update(cx, |this, cx| {
                this.finish_run(sql, at, page_size, result, cx)
            })
            .ok();
        }));
    }

    fn base_entry(&self, sql: &str, at: f64) -> OutputEntry {
        OutputEntry {
            sql: sql.to_owned(),
            database: self.key.database.clone(),
            at,
            source: Some(self.title.to_string()),
            ..OutputEntry::default()
        }
    }

    fn finish_run(
        &mut self,
        sql: String,
        at: f64,
        page_size: usize,
        result: Result<QueryResult, CmdError>,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
        // Phase 6: every outcome reports the session's transaction state.
        super::transactions::report(&self.key, &result, cx);
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let id = self.key.connection_id.clone();
        match result {
            Ok(res) => {
                let mut entry = self.base_entry(&sql, at);
                entry.ms = res.ms as f64;
                let has_rows = !res.columns.is_empty();
                if has_rows {
                    entry.rows = Some(res.rows.len() as f64);
                    entry.truncated = res.truncated;
                    self.runs += 1;
                    self.result = Some(ConsoleResult {
                        sql: sql.clone(),
                        ms: res.ms,
                        result: Arc::new(res),
                        offset: 0,
                        page_size,
                        run: self.runs,
                    });
                } else {
                    entry.affected = Some(res.affected.unwrap_or(0) as f64);
                }
                let view = has_rows.then(|| self.tab_id.clone());
                state.workspaces.update(cx, |w, cx| {
                    w.log(&id, entry, None, true, cx);
                    w.set_bottom_view(view, cx);
                });
                // DDL (Redis: writes) may have changed the tree.
                let redis = state
                    .connections
                    .read(cx)
                    .get(&id)
                    .is_some_and(|c| c.config.kind.is_redis());
                let changed = if redis {
                    boa_core::redis::changes_keys(&sql)
                } else {
                    is_ddl(&sql)
                };
                if changed {
                    let key = self.key.clone();
                    state.catalogs.update(cx, |c, cx| c.load(key, true, cx));
                }
            }
            Err(err) => {
                let error = RunError::from_cmd(&err, &sql);
                self.fail_with(sql, at, now_ms() - at, error, cx);
                return;
            }
        }
        state
            .layout
            .update(cx, |l, cx| l.set_open(PanelKind::Bottom, true, cx));
        cx.notify();
    }

    fn fail(&mut self, sql: String, at: f64, error: RunError, cx: &mut Context<Self>) {
        self.fail_with(sql, at, 0., error, cx);
    }

    /// Logs a failed run and shows it: the strip under the editor, the
    /// Output log in the Database Output panel.
    fn fail_with(
        &mut self,
        sql: String,
        at: f64,
        ms: f64,
        error: RunError,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
        if let Some(state) = AppState::try_global(cx).cloned() {
            let mut entry = self.base_entry(&sql, at);
            entry.ms = ms.max(0.);
            entry.error = Some(error.message.clone());
            entry.detail = error.detail.clone();
            let id = self.key.connection_id.clone();
            let excerpt = error.excerpt.clone();
            state.workspaces.update(cx, |w, cx| {
                w.log(&id, entry, excerpt, true, cx);
                w.set_bottom_view(None, cx);
            });
            state
                .layout
                .update(cx, |l, cx| l.set_open(PanelKind::Bottom, true, cx));
        }
        self.failed = Some(error);
        cx.notify();
    }

    /// Asks the server to cancel this console's running statement (it then
    /// fails with 57014; store.ts `cancelQuery`).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.running.is_none() {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let cancel = match &self.cancel {
            Some(handle) => state.backend.cancel(handle),
            None => state
                .backend
                .cancel_query(&self.key.connection_id, &self.key.database),
        };
        let toasts = state.toasts.clone();
        cx.spawn(async move |_, cx| {
            if let Err(err) = cancel.await {
                toasts.update(cx, |t, cx| {
                    t.push(NewToast::error("Cancel failed").body(err.message), cx);
                });
            }
        })
        .detach();
    }

    /// Shows another page of the result by re-running its statement with an
    /// offset (store.ts `pageResult`: stateless LIMIT/OFFSET paging; only
    /// pageable results; the Output log gets a line, the history doesn't).
    pub fn page(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(current) = self.result.clone() else {
            return;
        };
        if !current.result.pageable || self.running.is_some() {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let (id, db) = (self.key.connection_id.clone(), self.key.database.clone());
        self.running = Some(Instant::now());
        cx.notify();
        let at = now_ms();
        let query = state
            .backend
            .run_query(&id, &db, &current.sql, current.page_size, offset);
        self.cancel = query.cancel_handle();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = query.await;
            this.update(cx, |this, cx| {
                this.finish_page(current, offset, at, result, cx)
            })
            .ok();
        }));
    }

    fn finish_page(
        &mut self,
        page: ConsoleResult,
        offset: usize,
        at: f64,
        result: Result<QueryResult, CmdError>,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
        super::transactions::report(&self.key, &result, cx);
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let id = self.key.connection_id.clone();
        let mut entry = self.base_entry(&page.sql, at);
        entry.offset = Some(offset as f64);
        match result {
            Ok(res) => {
                entry.ms = res.ms as f64;
                entry.rows = Some(res.rows.len() as f64);
                entry.truncated = res.truncated;
                state
                    .workspaces
                    .update(cx, |w, cx| w.log(&id, entry, None, false, cx));
                // Closed or re-run meanwhile: drop this page.
                if self.result.as_ref().is_some_and(|r| r.run == page.run) {
                    self.result = Some(ConsoleResult {
                        ms: res.ms,
                        result: Arc::new(res),
                        offset,
                        ..page
                    });
                }
            }
            Err(err) => {
                let error = RunError::from_cmd(&err, &page.sql);
                entry.ms = (now_ms() - at).max(0.);
                entry.error = Some(error.message.clone());
                entry.detail = error.detail.clone();
                state.workspaces.update(cx, |w, cx| {
                    w.log(&id, entry, error.excerpt.clone(), false, cx)
                });
                state.toasts.update(cx, |t, cx| {
                    t.push(
                        NewToast::error("Could not load page").body(error.message),
                        cx,
                    );
                });
            }
        }
        cx.notify();
    }
}

/// `/^\s*(create|drop|alter)\b/i`: the statement may have changed the tree.
pub fn is_ddl(sql: &str) -> bool {
    let word: String = sql
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(word.as_str(), "create" | "drop" | "alter")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_statements_reload_the_catalog() {
        assert!(is_ddl("create table x (a int)"));
        assert!(is_ddl("  DROP view v"));
        assert!(is_ddl("\nAlter table t add c int"));
        assert!(!is_ddl("select 1"));
        assert!(!is_ddl("created_at"));
        assert!(!is_ddl("-- create\nselect 1"));
    }
}
