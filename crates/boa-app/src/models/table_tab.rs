//! [`TableTab`]: one table tab (Workspace.tsx `TableView`, TableEdits.tsx
//! `useTableEdits`, state/edits.ts): the page of rows it shows (`select *
//! from <table>` with the tab's WHERE / ORDER BY,
//! `boa_core::grid::table_query`), paging, header sorts, whether the rows
//! are editable (`grid_edits::editability` against the catalog), and the
//! staged edits of the shown page (`boa_core::grid::edits::EditsState`) with
//! their preview and submit (`Backend::apply_edits`: one transaction, or a
//! savepoint inside an open one).
//!
//! - The first page loads as soon as the connection is connected (the tab
//!   watches [`super::connections::Connections`]).
//! - The grid stages the edits; the owning view hands them over with
//!   [`TableTab::set_pending`] on every `GridEvent::PendingChanged`. When the
//!   model drops them (revert all, discard, a submit) it emits
//!   [`TableTabEvent::EditsCleared`] so the grid drops its own.
//! - Reloading, paging, a new filter / sort and closing the tab go through
//!   [`TableTab::request`]: with pending edits nothing happens yet and
//!   [`TableTabEvent::ConfirmDiscard`] asks ("Discard 2 pending changes?");
//!   the answer comes back through [`TableTab::resolve_discard`]. Closing
//!   asks through the Workspaces close guard, installed with the first tab.
//! - A submit logs every statement to the Output log (and the query history,
//!   `WorkspacesEvent::OutputLogged { record: true }`) with the table's name
//!   as the source, as store.ts `logRun` did, and toasts the outcome. A
//!   failure keeps the edits and reopens the preview on the failing statement.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use boa_core::edits::ApplyOutcome;
use boa_core::error::{CmdError, CmdResult};
use boa_core::grid::edits::{
    CLOSE_TAB_ACTION, DiscardPrompt, EditPage, EditsState, SubmitError, SubmitOutcome,
    SubmitReport, SubmitStart, ToastKind as ReportKind,
};
use boa_core::grid::grid_edits::{
    EditStatement, EditTarget, Editability, Pending, build_statements, editability,
};
use boa_core::grid::table_query::{ColumnSort, TableQuery, cycle_sort, sort_state, table_select};
use boa_core::model::{QueryResult, Table};
use boa_core::state::history::OutputEntry;
use gpui::{App, Context, EventEmitter, Global, Subscription, Task, WeakEntity};

use super::catalogs::DbKey;
use super::layout::PanelKind;
use super::toasts::{NewToast, ToastKind};
use super::workspaces::{TabId, now_ms};
use crate::app_state::AppState;
use crate::grid::error::RunError;

/// How long the submit success toasts show (`pushToast(…, 3000)`).
pub const SUBMITTED_TOAST_FOR: Duration = Duration::from_millis(3000);

/// What a guarded navigation does once pending edits are out of the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableNav {
    /// Reload the shown page.
    Reload,
    /// Show the page starting at this offset.
    Page(usize),
    /// A new WHERE / ORDER BY (or the same one re-run), from the first page.
    Query(TableQuery),
    /// Close the tab.
    Close,
}

impl TableNav {
    /// The prompt's "To {action}, they will be lost." (Workspace.tsx /
    /// state/edits.ts).
    pub fn action(&self) -> &'static str {
        match self {
            Self::Reload => "reload",
            Self::Page(_) => "change the page",
            Self::Query(_) => "change the filter or sort",
            Self::Close => CLOSE_TAB_ACTION,
        }
    }
}

/// What a table tab asks of the view showing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableTabEvent {
    /// Pending edits stand in the way of a reload / page / filter / close:
    /// ask (TableEdits.tsx `DiscardEditsDialog`), then answer with
    /// [`TableTab::resolve_discard`].
    ConfirmDiscard(DiscardPrompt),
    /// The model dropped the staged edits (revert all, discard, submit, a
    /// reloaded page): the grid drops its own.
    EditsCleared,
    /// Show the SQL preview (⌘↵, "Preview…", or a failed submit).
    PreviewRequested,
}

/// One table tab.
pub struct TableTab {
    pub key: DbKey,
    pub tab_id: TabId,
    pub schema: String,
    pub table: String,
    /// The filter bar's applied WHERE / ORDER BY, trimmed; empty when unset.
    pub where_clause: String,
    pub order_by: String,
    /// Rows skipped before the shown page.
    pub offset: usize,
    /// `prefs.rowLimit` the page was fetched with.
    pub page_size: usize,
    /// The statement paged through.
    pub sql: String,
    /// The shown page (None while the last load failed).
    pub result: Option<Arc<QueryResult>>,
    /// A page is loading.
    pub loading: bool,
    /// The last load failed (shown in place of the rows): the server's error,
    /// or why a clause was refused (`WHERE: Only one condition…`).
    pub error: Option<String>,
    /// Bumped per loaded page (the grid shows a page once).
    pub loads: u64,
    /// The SQL preview is open (TableEdits.tsx `preview`).
    pub preview: bool,
    edits: EditsState<QueryResult>,
    /// What to do once the open discard prompt is answered with Discard.
    after_discard: Option<TableNav>,
    task: Option<Task<()>>,
    submit_task: Option<Task<()>>,
    _connection: Option<Subscription>,
}

impl std::fmt::Debug for TableTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TableTab")
            .field("key", &self.key)
            .field("tab_id", &self.tab_id)
            .field("schema", &self.schema)
            .field("table", &self.table)
            .field("where_clause", &self.where_clause)
            .field("order_by", &self.order_by)
            .field("offset", &self.offset)
            .field("loading", &self.loading)
            .field("error", &self.error)
            .field("preview", &self.preview)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<TableTabEvent> for TableTab {}

/// Every live table tab by id, for the Workspaces close guard (which runs
/// while Workspaces is being updated, so it can't look the tab up there).
#[derive(Default)]
struct Registry {
    tabs: HashMap<TabId, WeakEntity<TableTab>>,
    guard_installed: bool,
}

impl Global for Registry {}

/// The live table tab with this id, if any.
pub fn find(tab_id: &str, cx: &App) -> Option<gpui::Entity<TableTab>> {
    cx.try_global::<Registry>()?
        .tabs
        .get(tab_id)
        .and_then(WeakEntity::upgrade)
}

/// Installs the Workspaces close guard (state/edits.ts `setCloseGuard`):
/// a table tab with pending edits asks before it closes, and closes itself
/// after a Discard. Done once, when the first table tab is created.
fn install_close_guard(cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let registry = cx.default_global::<Registry>();
    if registry.guard_installed {
        return;
    }
    registry.guard_installed = true;
    state.workspaces.update(cx, |w, _| {
        w.set_close_guard(Some(Rc::new(|_key, id, cx| match find(id, cx) {
            Some(table) => table.update(cx, |t, cx| t.close_guard(cx)),
            None => true,
        })));
    });
}

impl TableTab {
    pub fn new(
        key: DbKey,
        tab_id: TabId,
        schema: impl Into<String>,
        table: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let connections = AppState::try_global(cx).map(|s| s.connections.clone());
        let subscription = connections.map(|c| {
            cx.observe(&c, |this: &mut Self, _, cx| {
                if this.result.is_none() && !this.loading && this.error.is_none() {
                    this.reload(cx);
                }
            })
        });
        let weak = cx.weak_entity();
        let registry = cx.default_global::<Registry>();
        registry.tabs.retain(|_, t| t.upgrade().is_some());
        registry.tabs.insert(tab_id.clone(), weak.clone());
        if !registry.guard_installed {
            // Workspaces is being updated (it creates the tab): later.
            cx.defer(install_close_guard);
        }
        let mut this = Self {
            key,
            tab_id,
            schema: schema.into(),
            table: table.into(),
            where_clause: String::new(),
            order_by: String::new(),
            offset: 0,
            page_size: 0,
            sql: String::new(),
            result: None,
            loading: false,
            error: None,
            loads: 0,
            preview: false,
            edits: EditsState::new(),
            after_discard: None,
            task: None,
            submit_task: None,
            _connection: subscription,
        };
        this.sql = this.select().sql;
        // After the entity exists (and a restored tab's filter is set).
        cx.defer(move |cx| {
            weak.update(cx, |this, cx| this.reload(cx)).ok();
        });
        this
    }

    /// The tab's WHERE / ORDER BY as a [`TableQuery`].
    pub fn query(&self) -> TableQuery {
        TableQuery {
            filter: self.where_clause.clone(),
            order_by: self.order_by.clone(),
        }
    }

    fn select(&self) -> boa_core::grid::table_query::TableSelect {
        table_select(&self.schema, &self.table, &self.query())
    }

    /// Whether the tab's connection is connected.
    pub fn connected(&self, cx: &App) -> bool {
        AppState::try_global(cx)
            .is_some_and(|s| s.connections.read(cx).is_connected(&self.key.connection_id))
    }

    // ── Loading ───────────────────────────────────────────────────────────

    /// (Re)loads the current page without asking about pending edits (a new
    /// page size starts over). Buttons go through [`Self::request`].
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let select = self.select();
        self.sql = select.sql.clone();
        if let Some(error) = select.error {
            // Refused before anything runs (Workspace.tsx `clauseError`).
            self.task = None;
            self.error = Some(error);
            self.result = None;
            self.loading = false;
            cx.notify();
            return;
        }
        if !state
            .connections
            .read(cx)
            .is_connected(&self.key.connection_id)
        {
            cx.notify();
            return;
        }
        let page_size = state.prefs.read(cx).get().page_size() as usize;
        if page_size != self.page_size {
            self.offset = 0;
        }
        self.page_size = page_size;
        self.loading = true;
        cx.notify();
        let query = state.backend.run_query(
            &self.key.connection_id,
            &self.key.database,
            &select.sql,
            page_size,
            self.offset,
        );
        let sql = select.sql;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = query.await;
            this.update(cx, |this, cx| this.finish(sql, result, cx))
                .ok();
        }));
    }

    fn finish(
        &mut self,
        sql: String,
        result: Result<QueryResult, CmdError>,
        cx: &mut Context<Self>,
    ) {
        self.loading = false;
        // Phase 6: in Manual mode this select opens the session's transaction.
        super::transactions::report(&self.key, &result, cx);
        match result {
            Ok(res) => {
                // The page was replaced under edits (TableEdits.tsx): they
                // no longer apply.
                if self.edits.tab(&self.tab_id).is_some() {
                    let had = self.edits.pending_count(&self.tab_id) > 0;
                    self.edits.clear_edits(&self.tab_id);
                    cx.emit(TableTabEvent::EditsCleared);
                    if had {
                        toast(
                            NewToast::info("Pending changes discarded")
                                .body("The rows were reloaded."),
                            None,
                            cx,
                        );
                    }
                }
                self.result = Some(Arc::new(res));
                self.error = None;
                self.loads += 1;
            }
            Err(err) => {
                let e = RunError::from_cmd(&err, &sql);
                self.result = None;
                self.error = Some(match e.detail {
                    Some(detail) => format!("{}\n{detail}", e.message),
                    None => e.message,
                });
            }
        }
        cx.notify();
    }

    /// Shows the page starting at `offset` without asking (see [`Self::request`]).
    pub fn page(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.offset = offset;
        self.reload(cx);
    }

    /// Sets the WHERE / ORDER BY (trimmed; store.ts `setTableQuery`) and
    /// loads the first page, without asking (see [`Self::request`]).
    pub fn set_query(&mut self, q: &TableQuery, cx: &mut Context<Self>) {
        self.where_clause = q.filter.trim().to_owned();
        self.order_by = q.order_by.trim().to_owned();
        self.offset = 0;
        self.reload(cx);
    }

    // ── Guarded navigation ────────────────────────────────────────────────

    /// Reload / page / filter / close, after making sure no pending edits
    /// are lost (TableEdits.tsx `guard`): true when it went ahead now
    /// (for [`TableNav::Close`]: may close now, the caller closes); false
    /// when the user is asked first ([`TableTabEvent::ConfirmDiscard`]).
    pub fn request(&mut self, nav: TableNav, cx: &mut Context<Self>) -> bool {
        if self.edits.confirm_discard(&self.tab_id, nav.action()) {
            if nav != TableNav::Close {
                self.go(nav, cx);
            }
            return true;
        }
        if let Some(prompt) = self.edits.discard_prompt().cloned() {
            self.after_discard = Some(nav);
            cx.emit(TableTabEvent::ConfirmDiscard(prompt));
        }
        cx.notify();
        false
    }

    /// A new WHERE / ORDER BY from the filter bar (Enter, ×), guarded.
    pub fn apply_query(&mut self, q: TableQuery, cx: &mut Context<Self>) -> bool {
        self.request(TableNav::Query(q), cx)
    }

    /// A header click on `column` (⇧: `additive`): rewrites the ORDER BY
    /// (`cycle_sort`), guarded.
    pub fn sort_by(&mut self, column: &str, additive: bool, cx: &mut Context<Self>) -> bool {
        let q = TableQuery {
            filter: self.where_clause.clone(),
            order_by: cycle_sort(&self.order_by, column, additive),
        };
        self.request(TableNav::Query(q), cx)
    }

    /// The header arrows: the ORDER BY's sorts by column name.
    pub fn sort_state(&self) -> Vec<(String, ColumnSort)> {
        sort_state(&self.order_by)
    }

    /// The open "Discard N pending changes?" prompt of this tab.
    pub fn discard_prompt(&self) -> Option<&DiscardPrompt> {
        self.edits.discard_prompt()
    }

    /// The prompt was answered: Discard (`ok`) drops the edits and goes on
    /// with what asked; Keep editing does nothing. Only the first answer counts.
    pub fn resolve_discard(&mut self, ok: bool, cx: &mut Context<Self>) {
        if self.edits.resolve_discard(ok).is_none() {
            return;
        }
        let nav = self.after_discard.take();
        if ok {
            cx.emit(TableTabEvent::EditsCleared);
            if let Some(nav) = nav {
                self.go(nav, cx);
            }
        }
        cx.notify();
    }

    /// The Workspaces close guard: may the tab close now?
    pub fn close_guard(&mut self, cx: &mut Context<Self>) -> bool {
        if self.edits.tab(&self.tab_id).is_none() {
            return true;
        }
        self.request(TableNav::Close, cx)
    }

    fn go(&mut self, nav: TableNav, cx: &mut Context<Self>) {
        match nav {
            TableNav::Reload => self.reload(cx),
            TableNav::Page(offset) => self.page(offset, cx),
            TableNav::Query(q) => self.set_query(&q, cx),
            TableNav::Close => {
                if let Some(state) = AppState::try_global(cx).cloned() {
                    let (key, id) = (self.key.clone(), self.tab_id.clone());
                    state
                        .workspaces
                        .update(cx, |w, cx| w.close_tab_forced(key, id, cx));
                }
            }
        }
    }

    // ── Catalog ───────────────────────────────────────────────────────────

    /// The table in the catalog, if introspected (row estimate, view or table, PK).
    pub fn meta(&self, cx: &App) -> Option<Table> {
        let state = AppState::try_global(cx)?;
        state
            .catalogs
            .read(cx)
            .find_table(&self.key, &self.schema, &self.table)
            .cloned()
    }

    /// Whether the shown rows can be edited (tables with a primary key, all
    /// of it in the result), else why not.
    pub fn editability(&self, cx: &App) -> Editability {
        let columns = self.result.as_ref().map_or(&[][..], |r| &r.columns[..]);
        editability(self.meta(cx).as_ref(), columns)
    }

    /// What the statements are generated for, when the rows are editable.
    pub fn edit_target(&self, cx: &App) -> Option<EditTarget> {
        let result = self.result.as_ref()?;
        match self.editability(cx) {
            Editability::Editable { pk } => Some(EditTarget {
                schema: self.schema.clone(),
                table: self.table.clone(),
                columns: result.columns.iter().map(|c| c.name.clone()).collect(),
                pk,
            }),
            Editability::ReadOnly { .. } => None,
        }
    }

    // ── Staged edits ──────────────────────────────────────────────────────

    /// The staged edits of the shown page (None without any).
    pub fn pending(&self) -> Option<&Pending> {
        let result = self.result.as_ref()?;
        self.edits.pending_for(&self.tab_id, result)
    }

    /// Changed rows of the shown page ("N changes").
    pub fn pending_count(&self) -> usize {
        self.pending().map_or(0, Pending::change_count)
    }

    /// The grid's staged edits changed: keep them for the shown page.
    pub fn set_pending(&mut self, pending: Pending, cx: &mut Context<Self>) {
        let Some(result) = self.result.clone() else {
            return;
        };
        let same = match self.pending() {
            Some(p) => *p == pending,
            None => pending.is_empty(),
        };
        if same {
            return;
        }
        let page = EditPage {
            result,
            offset: self.offset as u64,
            row_limit: self.page_size as u64,
        };
        self.edits
            .update_pending(&self.tab_id, page, |p| *p = pending);
        cx.notify();
    }

    /// "Revert all changes".
    pub fn revert_all(&mut self, cx: &mut Context<Self>) {
        if self.submitting() {
            return;
        }
        self.edits.clear_edits(&self.tab_id);
        cx.emit(TableTabEvent::EditsCleared);
        cx.notify();
    }

    /// A submit is running.
    pub fn submitting(&self) -> bool {
        self.edits.is_submitting(&self.tab_id)
    }

    /// The last submit's failure (the preview points at the statement).
    pub fn submit_error(&self) -> Option<&SubmitError> {
        self.edits.error(&self.tab_id)
    }

    /// The statements a submit would run now (the preview's list).
    pub fn statements(&self, cx: &App) -> Vec<EditStatement> {
        let (Some(target), Some(result), Some(pending)) =
            (self.edit_target(cx), self.result.as_ref(), self.pending())
        else {
            return Vec::new();
        };
        build_statements(&target, &result.rows, pending).unwrap_or_default()
    }

    /// Opens the SQL preview (⌘↵ in the grid, "Preview…"), if anything changed.
    pub fn open_preview(&mut self, cx: &mut Context<Self>) {
        if self.pending_count() == 0 {
            return;
        }
        self.preview = true;
        cx.emit(TableTabEvent::PreviewRequested);
        cx.notify();
    }

    /// The preview was closed (Cancel, Esc, "Open in console").
    pub fn close_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview {
            self.preview = false;
            cx.notify();
        }
    }

    /// Submits the staged edits (state/edits.ts `submitEdits`): the
    /// statements run as one unit; then the page reloads, or on failure the
    /// edits stay and the preview shows the failing statement.
    pub fn submit_edits(&mut self, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        let Some(target) = self.edit_target(cx) else {
            return;
        };
        let start = match self.edits.begin_submit(&self.tab_id, &target) {
            Ok(start) => start,
            Err(e) => {
                toast(
                    NewToast::error("Changes not submitted").body(e.message),
                    None,
                    cx,
                );
                return;
            }
        };
        match start {
            SubmitStart::Nothing => {}
            SubmitStart::Empty => {
                self.preview = false;
                cx.emit(TableTabEvent::EditsCleared);
                self.reload(cx);
            }
            SubmitStart::Run(statements) => {
                let at = now_ms();
                let task = state.backend.apply_edits(
                    &self.key.connection_id,
                    &self.key.database,
                    statements.iter().map(Into::into).collect(),
                );
                cx.notify();
                self.submit_task = Some(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    this.update(cx, |this, cx| {
                        this.finish_submit(statements, at, result, cx)
                    })
                    .ok();
                }));
            }
        }
    }

    fn finish_submit(
        &mut self,
        statements: Vec<EditStatement>,
        at: f64,
        result: CmdResult<ApplyOutcome>,
        cx: &mut Context<Self>,
    ) {
        self.submit_task = None;
        // The session's transaction state after the submit (manual mode keeps it open).
        super::transactions::report(&self.key, &result, cx);
        let outcome = SubmitOutcome::from(&result);
        let report = self
            .edits
            .finish_submit(&self.tab_id, statements.len(), outcome.clone());
        let failed = matches!(outcome, SubmitOutcome::Failed { .. });
        let entries = self.log_entries(&statements, at, &result, &outcome, &report);
        if let Some(state) = AppState::try_global(cx).cloned() {
            let id = self.key.connection_id.clone();
            state.workspaces.update(cx, |w, cx| {
                for entry in entries {
                    w.log(&id, entry, None, true, cx);
                }
                if failed {
                    // The Output log shows the failure.
                    w.set_bottom_view(None, cx);
                }
            });
            if failed {
                state
                    .layout
                    .update(cx, |l, cx| l.set_open(PanelKind::Bottom, true, cx));
            }
        }
        let kind = match report.kind {
            ReportKind::Success => ToastKind::Success,
            ReportKind::Info => ToastKind::Info,
            ReportKind::Error => ToastKind::Error,
        };
        let mut new = NewToast::new(kind, report.title.clone());
        if let Some(body) = report.body.clone() {
            new = new.body(body);
        }
        toast(new, report.ok.then_some(SUBMITTED_TOAST_FOR), cx);
        if report.ok {
            self.preview = false;
            cx.emit(TableTabEvent::EditsCleared);
            self.reload(cx);
        } else {
            self.preview = true;
            cx.emit(TableTabEvent::PreviewRequested);
        }
        cx.notify();
    }

    /// The Output log entries of a submit (state/edits.ts `logRun` calls):
    /// each statement with its time and affected rows; the failing one with
    /// the error and "Statement i of n failed…"; a failed call as one entry.
    fn log_entries(
        &self,
        statements: &[EditStatement],
        at: f64,
        result: &CmdResult<ApplyOutcome>,
        outcome: &SubmitOutcome,
        report: &SubmitReport,
    ) -> Vec<OutputEntry> {
        let base = |sql: String| OutputEntry {
            sql,
            database: self.key.database.clone(),
            at,
            source: Some(self.table.clone()),
            ..OutputEntry::default()
        };
        let join = |parts: Vec<Option<String>>| {
            let lines: Vec<String> = parts.into_iter().flatten().collect();
            (!lines.is_empty()).then(|| lines.join("\n"))
        };
        match (result, outcome) {
            (Ok(out), SubmitOutcome::Failed { index, error }) => {
                let mut e = base(
                    statements
                        .get(*index)
                        .map(|s| s.sql.clone())
                        .unwrap_or_default(),
                );
                e.ms = out.ms as f64;
                e.error = Some(error.message.clone());
                e.detail = join(vec![report.note.clone(), error.detail.clone()]);
                vec![e]
            }
            (Ok(out), _) => statements
                .iter()
                .enumerate()
                .map(|(i, st)| {
                    let run = out.runs.get(i);
                    let mut e = base(st.sql.clone());
                    e.ms = run.map_or(0., |r| r.ms as f64);
                    e.affected = Some(run.map_or(0., |r| r.affected as f64));
                    e
                })
                .collect(),
            (Err(_), _) => {
                let error = match outcome {
                    SubmitOutcome::Error(e) => Some(e.clone()),
                    _ => None,
                };
                let mut e = base(statements_text(statements));
                e.ms = (now_ms() - at).max(0.);
                e.error = error.as_ref().map(|e| e.message.clone());
                e.detail = error.and_then(|e| e.detail);
                vec![e]
            }
        }
    }
}

/// The statements as one script (`statementsText`: "…;" per line), for
/// Copy and "Open in console".
pub fn statements_text(statements: &[EditStatement]) -> String {
    statements
        .iter()
        .map(|s| format!("{};", s.sql))
        .collect::<Vec<_>>()
        .join("\n")
}

fn toast(new: NewToast, ttl: Option<Duration>, cx: &mut App) {
    if let Some(state) = AppState::try_global(cx).cloned() {
        state.toasts.update(cx, |t, cx| match ttl {
            Some(ttl) => {
                t.push_for(new, Some(ttl), cx);
            }
            None => {
                t.push(new, cx);
            }
        });
    }
}
