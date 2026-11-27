//! Workspace.tsx: the editor area for the selected database (its tabs live
//! in the title bar). A console is a toolbar (run / cancel, history, the
//! transaction slot, run progress, the default-schema picker) above the SQL
//! editor, with a strip for the last failure under it; its results go to
//! the Database Output panel. A table tab is a toolbar (reload, pager, row
//! info, the edits' buttons, "Filter loaded rows", Export, DDL, "Open query
//! console") above the filter bar and the table's data grid.
//!
//! "Filter loaded rows" filters the loaded page in the grid
//! ([`ResultGrid::set_row_filter`]); like the React `TableView`'s state it
//! starts empty each time the tab is shown.
//!
//! One [`SqlEditor`] per console and one grid per table tab, created when
//! the tab is first shown and dropped with it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use boa_core::grid::table_query::table_select;
use boa_core::sql::qualified;
use boa_core::state::csv_export::table_owner;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, Hsla, Subscription, Task,
    Window, div, px,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Disableable as _, Icon, Sizable as _};

use super::chrome::{BoaIcon, IconName, icon, kbd, tooltip, vsep};
use super::csv_export::{self, MenuSource};
use super::pager::{PagerState, pager};
use super::run_progress::{run_failed, run_status};
use super::table_edits;
use super::table_filter_bar::TableFilterBar;
use super::{ActiveDb, connection_tag};
use crate::actions::ToggleQueryHistory;
use crate::app_state::AppState;
use crate::editor::{CaretPos, RunStatement, SqlEditor, SqlEditorEvent};
use crate::grid::{GridContent, GridEvent, ResultGrid, TableContext};
use crate::models::catalogs::Catalogs;
use crate::models::connections::ConnState;
use crate::models::console::{Console, ConsoleEvent};
use crate::models::exports::table_tab_source;
use crate::models::table_tab::{TableNav, TableTab, TableTabEvent};
use crate::models::workspaces::{Tab, TabContent, TabId, Workspaces};
use crate::theme::{self, ActivePalette as _};

/// Toolbar height (`h-9`).
pub const TOOLBAR_HEIGHT: gpui::Pixels = px(36.);
/// The table toolbar's filter of the loaded rows (Workspace.tsx).
pub const ROW_FILTER_PLACEHOLDER: &str = "Find in loaded rows";

/// What the workspace reports to the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// The focused console's caret (status bar), None when it has none.
    Caret(Option<CaretPos>),
}

struct ConsolePane {
    console: Entity<Console>,
    editor: Entity<SqlEditor>,
    _subscriptions: Vec<Subscription>,
}

struct TablePane {
    table: Entity<TableTab>,
    grid: Entity<ResultGrid>,
    filter_bar: Entity<TableFilterBar>,
    /// "Filter loaded rows".
    row_filter: Entity<InputState>,
    /// The page the grid shows (`TableTab::loads`).
    shown: Option<u64>,
    _subscriptions: Vec<Subscription>,
}

/// The centre of the main window.
pub struct Workspace {
    workspaces: Entity<Workspaces>,
    catalogs: Entity<Catalogs>,
    consoles: HashMap<EntityId, ConsolePane>,
    tables: HashMap<EntityId, TablePane>,
    /// The active tab the last sync saw (focus moves when it changes).
    active: Option<TabId>,
    ticker: Option<Task<()>>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("consoles", &self.consoles.len())
            .field("tables", &self.tables.len())
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Workspace {
    /// Needs [`AppState`] (panics without it).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = AppState::global(cx).clone();
        let subscriptions = vec![
            cx.observe_in(&state.workspaces, window, |this, _, window, cx| {
                this.sync(window, cx)
            }),
            cx.observe(&state.catalogs, |this, _, cx| this.push_schemas(cx)),
            cx.observe(&state.catalogs, |this, _, cx| this.sync_table_grids(cx)),
            cx.observe(&state.connections, |_, _, cx| cx.notify()),
            cx.observe(&state.transactions, |_, _, cx| cx.notify()),
            cx.observe(&state.exports, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            workspaces: state.workspaces,
            catalogs: state.catalogs,
            consoles: HashMap::new(),
            tables: HashMap::new(),
            active: None,
            ticker: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        #[cfg(feature = "dev-hooks")]
        dev::start(window, cx);
        #[cfg(feature = "dev-hooks")]
        table_edits::dev::start(window, cx);
        this
    }

    fn active_tab(&self, cx: &App) -> Option<Tab> {
        self.workspaces.read(cx).active_tab().cloned()
    }

    /// The active console's editor, if the active tab is a console.
    pub fn editor(&self, cx: &App) -> Option<Entity<SqlEditor>> {
        let tab = self.active_tab(cx)?;
        let console = tab.console()?;
        self.consoles
            .get(&console.entity_id())
            .map(|p| p.editor.clone())
    }

    /// The editor of console `console`, if it has been shown.
    pub fn editor_of(&self, console: &Entity<Console>) -> Option<Entity<SqlEditor>> {
        self.consoles
            .get(&console.entity_id())
            .map(|p| p.editor.clone())
    }

    /// The grid of table tab `table`, if it has been shown.
    pub fn grid_of(&self, table: &Entity<TableTab>) -> Option<Entity<ResultGrid>> {
        self.tables.get(&table.entity_id()).map(|p| p.grid.clone())
    }

    /// The "Filter loaded rows" input of table tab `table`, if it has been shown.
    pub fn row_filter_of(&self, table: &Entity<TableTab>) -> Option<Entity<InputState>> {
        self.tables
            .get(&table.entity_id())
            .map(|p| p.row_filter.clone())
    }

    /// The filter bar of table tab `table`, if it has been shown.
    pub fn filter_bar_of(&self, table: &Entity<TableTab>) -> Option<Entity<TableFilterBar>> {
        self.tables
            .get(&table.entity_id())
            .map(|p| p.filter_bar.clone())
    }

    /// Creates the active tab's pane, drops closed tabs' panes, and moves
    /// the focus into a newly activated tab.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (live, active) = {
            let ws = self.workspaces.read(cx);
            let live: HashSet<EntityId> = ws
                .workspaces
                .values()
                .flat_map(|w| w.tabs.iter())
                .map(|t| match &t.content {
                    TabContent::Console(c) => c.entity_id(),
                    TabContent::Table(t) => t.entity_id(),
                })
                .collect();
            (live, ws.active_tab().cloned())
        };
        self.consoles.retain(|id, _| live.contains(id));
        self.tables.retain(|id, _| live.contains(id));
        match &active {
            Some(tab) => {
                let shown_before = self.active.as_ref() == Some(&tab.id);
                match &tab.content {
                    TabContent::Console(console) => self.ensure_console(console, window, cx),
                    TabContent::Table(table) => {
                        if !shown_before {
                            // Workspace.tsx mounts `TableView` afresh per shown
                            // tab: its "Filter loaded rows" starts empty.
                            self.clear_row_filter(table, window, cx);
                        }
                        self.ensure_table(table, window, cx);
                    }
                }
                if !shown_before {
                    self.active = Some(tab.id.clone());
                    self.focus_active(window, cx);
                    let caret = tab
                        .console()
                        .and_then(|c| self.consoles.get(&c.entity_id()))
                        .and_then(|p| p.editor.read(cx).caret());
                    cx.emit(WorkspaceEvent::Caret(caret));
                }
            }
            None => {
                if self.active.take().is_some() {
                    // Keep the focus in the window, so the shortcuts still work.
                    window.focus(&self.focus_handle, cx);
                    cx.emit(WorkspaceEvent::Caret(None));
                }
            }
        }
        cx.notify();
    }

    /// Empties a table tab's "Filter loaded rows" (shows every loaded row).
    fn clear_row_filter(
        &mut self,
        table: &Entity<TableTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.tables.get(&table.entity_id()) else {
            return;
        };
        if !pane.row_filter.read(cx).value().is_empty() {
            pane.row_filter
                .update(cx, |i, cx| i.set_value("", window, cx));
        }
        pane.grid.update(cx, |g, cx| g.set_row_filter("", cx));
    }

    /// Focuses the active tab: its editor, or its grid.
    pub fn focus_active(&self, window: &mut Window, cx: &mut App) {
        let Some(tab) = self.active_tab(cx) else {
            return;
        };
        let handle = match &tab.content {
            TabContent::Console(c) => self
                .consoles
                .get(&c.entity_id())
                .map(|p| p.editor.focus_handle(cx)),
            TabContent::Table(t) => self
                .tables
                .get(&t.entity_id())
                .map(|p| p.grid.focus_handle(cx)),
        };
        if let Some(handle) = handle {
            window.focus(&handle, cx);
        }
    }

    fn ensure_console(
        &mut self,
        console: &Entity<Console>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.consoles.contains_key(&console.entity_id()) {
            return;
        }
        let (sql, key, default_schema) = {
            let c = console.read(cx);
            (c.sql.clone(), c.key.clone(), c.default_schema.clone())
        };
        let editor = cx.new(|cx| SqlEditor::with_text(sql, window, cx));
        let schemas = self.catalogs.read(cx).get(&key).map(|c| c.schemas.clone());
        editor.update(cx, |e, cx| {
            e.set_default_schema(default_schema);
            if let Some(schemas) = schemas {
                e.set_schemas(schemas, cx);
            }
        });
        let run_console = console.clone();
        let text_console = console.clone();
        let subscriptions = vec![
            cx.subscribe(&editor, move |_, _, event: &RunStatement, cx| {
                let sql = event.sql.clone();
                run_console.update(cx, |c, cx| c.run(sql, cx));
            }),
            cx.subscribe(
                &editor,
                move |_, editor, event: &SqlEditorEvent, cx| match event {
                    SqlEditorEvent::Changed => {
                        let text = editor.read(cx).text(cx).to_string();
                        text_console.update(cx, |c, _| c.set_sql(text));
                        // The models aren't notified per keystroke: save the session soon.
                        if let Some(session) = AppState::try_global(cx).map(|s| s.session.clone()) {
                            session.update(cx, |s, cx| s.touch(cx));
                        }
                    }
                    SqlEditorEvent::Caret(caret) => cx.emit(WorkspaceEvent::Caret(*caret)),
                },
            ),
            cx.subscribe(console, |_, console, event: &ConsoleEvent, cx| {
                let ConsoleEvent::ConfirmRun {
                    sql,
                    risks,
                    production,
                } = event;
                super::guard_dialogs::confirm_run(
                    console,
                    sql.clone(),
                    risks.clone(),
                    *production,
                    cx,
                );
            }),
            cx.observe(console, |this, _, cx| this.tick_while_running(cx)),
        ];
        self.consoles.insert(
            console.entity_id(),
            ConsolePane {
                console: console.clone(),
                editor,
                _subscriptions: subscriptions,
            },
        );
    }

    fn ensure_table(
        &mut self,
        table: &Entity<TableTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tables.contains_key(&table.entity_id()) {
            return;
        }
        let grid = cx.new(|cx| ResultGrid::new(window, cx));
        // The tab's toolbar has the pager and the run state.
        grid.update(cx, |g, cx| g.set_toolbar_visible(false, cx));
        let filter_bar = cx.new(|cx| TableFilterBar::new(table.clone(), window, cx));
        let row_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(ROW_FILTER_PLACEHOLDER));
        let events_table = table.clone();
        let filter_grid = grid.clone();
        let subscriptions = vec![
            cx.subscribe(&row_filter, move |_, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let text = input.read(cx).value();
                    filter_grid.update(cx, |g, cx| g.set_row_filter(&text, cx));
                }
            }),
            cx.observe(table, |this, table, cx| this.sync_table_grid(&table, cx)),
            cx.subscribe(&grid, move |_, grid, event: &GridEvent, cx| {
                let table = events_table.clone();
                match event {
                    GridEvent::PageRequested { offset } => {
                        let offset = *offset;
                        table.update(cx, |t, cx| t.request(TableNav::Page(offset), cx));
                    }
                    GridEvent::ServerSortRequested { column, additive } => {
                        let name = table
                            .read(cx)
                            .result
                            .as_ref()
                            .and_then(|r| r.columns.get(*column))
                            .map(|c| c.name.clone());
                        if let Some(name) = name {
                            let additive = *additive;
                            table.update(cx, |t, cx| t.sort_by(&name, additive, cx));
                        }
                    }
                    GridEvent::PendingChanged => {
                        let pending = grid.read(cx).pending();
                        table.update(cx, |t, cx| t.set_pending(pending, cx));
                    }
                    GridEvent::SubmitRequested => {
                        table.update(cx, |t, cx| t.open_preview(cx));
                    }
                }
            }),
            cx.subscribe_in(
                table,
                window,
                |this, table, event: &TableTabEvent, window, cx| match event {
                    TableTabEvent::ConfirmDiscard(prompt) => {
                        table_edits::confirm_discard(table.clone(), prompt, window, cx);
                    }
                    TableTabEvent::EditsCleared => {
                        if let Some(pane) = this.tables.get(&table.entity_id()) {
                            pane.grid.update(cx, |g, cx| g.clear_pending(cx));
                        }
                    }
                    TableTabEvent::PreviewRequested => {
                        table_edits::open_preview(table.clone(), window, cx);
                    }
                },
            ),
        ];
        self.tables.insert(
            table.entity_id(),
            TablePane {
                table: table.clone(),
                grid,
                filter_bar,
                row_filter,
                shown: None,
                _subscriptions: subscriptions,
            },
        );
        self.sync_table_grid(table, cx);
    }

    /// Hands a newly loaded page to the table tab's grid, with its
    /// editability (again when the catalog arrives) and the header sorts.
    fn sync_table_grid(&mut self, table: &Entity<TableTab>, cx: &mut Context<Self>) {
        let Some(pane) = self.tables.get_mut(&table.entity_id()) else {
            return;
        };
        let t = table.read(cx);
        let loading = t.loading;
        let editability = t.editability(cx);
        let context = TableContext {
            table: qualified(&t.schema, &t.table),
            pk: t
                .meta(cx)
                .map(|m| {
                    m.columns
                        .iter()
                        .filter(|c| c.pk)
                        .map(|c| c.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            sort: t.sort_state(),
            defaults: t
                .meta(cx)
                .map(|m| {
                    m.columns
                        .iter()
                        .filter(|c| c.default.as_deref().is_some_and(|d| !d.is_empty()))
                        .map(|c| c.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
        };
        if let Some(result) = t.result.clone() {
            let fresh = pane.shown != Some(t.loads);
            if fresh || pane.grid.read(cx).editability() != &editability {
                pane.shown = Some(t.loads);
                let content = GridContent {
                    sql: t.sql.clone(),
                    result,
                    offset: t.offset,
                    page_size: t.page_size,
                    editability,
                };
                pane.grid.update(cx, |g, cx| g.set_content(content, cx));
            }
        }
        if pane.grid.read(cx).table_context() != Some(&context) {
            pane.grid
                .update(cx, |g, cx| g.set_table_context(Some(context), cx));
        }
        pane.grid.update(cx, |g, cx| g.set_running(loading, cx));
        cx.notify();
    }

    /// The catalog changed: table grids get their editability and keys.
    fn sync_table_grids(&mut self, cx: &mut Context<Self>) {
        let tables: Vec<Entity<TableTab>> = self.tables.values().map(|p| p.table.clone()).collect();
        for table in tables {
            self.sync_table_grid(&table, cx);
        }
    }

    /// The catalog changed: consoles get their database's schemas.
    fn push_schemas(&mut self, cx: &mut Context<Self>) {
        for pane in self.consoles.values() {
            let key = pane.console.read(cx).key.clone();
            let Some(schemas) = self.catalogs.read(cx).get(&key).map(|c| c.schemas.clone()) else {
                continue;
            };
            pane.editor.update(cx, |e, cx| e.set_schemas(schemas, cx));
        }
        cx.notify();
    }

    /// Re-renders every 100 ms while the active console runs ("Executing… 1.2 s").
    fn tick_while_running(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.ticker.is_some() || !self.active_running(cx) {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let go_on = this
                    .update(cx, |this, cx| {
                        cx.notify();
                        let running = this.active_running(cx);
                        if !running {
                            this.ticker = None;
                        }
                        running
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        }));
    }

    fn active_running(&self, cx: &App) -> bool {
        self.active_tab(cx)
            .and_then(|t| t.console().cloned())
            .is_some_and(|c| c.read(cx).running.is_some())
    }

    // ── Rendering ─────────────────────────────────────────────────────────

    fn empty(title: &str, hint: Vec<gpui::AnyElement>, cx: &App) -> gpui::Div {
        let p = cx.palette();
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_size(theme::text::SM)
            .text_color(p.quiet)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.))
                    .child(title.to_owned())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_color(p.dim)
                            .children(hint),
                    ),
            )
    }

    fn render_console(
        &self,
        pane: &ConsolePane,
        active: &ActiveDb,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = cx.palette().clone();
        let c = pane.console.read(cx);
        let running = c.running;
        let connecting = matches!(active.status, ConnState::Connecting);
        let can_run = !connecting && !c.sql.trim().is_empty() && running.is_none();
        let failed = c.failed.clone();
        let default_schema = c.default_schema.clone();
        let editor = pane.editor.clone();
        let console = pane.console.clone();
        let schema_names: Vec<String> = self
            .catalogs
            .read(cx)
            .get(&active.key())
            .map(|cat| cat.schemas.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default();
        let schema = default_schema
            .clone()
            .or_else(|| schema_names.first().cloned())
            .unwrap_or_else(|| "public".to_owned());
        let run_editor = editor.clone();
        let cancel_console = console.clone();
        let picker_console = console.clone();
        let picker_editor = editor.clone();
        let database = active.database.clone();
        let picker = tool_menu_trigger(
            "console-schema",
            format!("{}.{schema}", active.database),
            true,
            true,
            cx,
        )
        .tooltip("Default schema")
        .dropdown_menu(move |menu, _, _| {
            let mut menu = menu
                .min_w(px(208.))
                .item(PopupMenuItem::label(database.clone()));
            menu = menu.separator();
            for name in &schema_names {
                let (console, editor, name) =
                    (picker_console.clone(), picker_editor.clone(), name.clone());
                menu = menu.item(
                    PopupMenuItem::new(name.clone())
                        .checked(name == schema)
                        .on_click(move |_, _, cx| {
                            let pick = Some(name.clone());
                            console.update(cx, |c, cx| c.set_default_schema(pick.clone(), cx));
                            editor.update(cx, |e, _| e.set_default_schema(pick.clone()));
                        }),
                );
            }
            menu
        });
        let toolbar = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(TOOLBAR_HEIGHT)
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(
                tool_button(
                    "console-run",
                    IconName::Play,
                    "Execute (⌘↵)",
                    can_run,
                    Some(p.status_ok),
                    cx,
                )
                .on_click(move |_, window, cx| {
                    if can_run {
                        run_editor.update(cx, |e, cx| e.run_or_arm(true, window, cx));
                    }
                }),
            )
            .child(
                tool_button(
                    "console-cancel",
                    IconName::Square,
                    "Cancel running statement",
                    running.is_some(),
                    None,
                    cx,
                )
                .on_click(move |_, _, cx| cancel_console.update(cx, |c, cx| c.cancel(cx))),
            )
            .child(
                tool_button_icon(
                    "console-history",
                    BoaIcon::History.icon(),
                    "Query history (⌘⌥E)",
                    true,
                    None,
                    cx,
                )
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleQueryHistory), cx)),
            )
            .child(vsep(cx).mx(px(6.)))
            // TxControls.tsx: the transaction mode and state, then the connection tag.
            .children(super::tx_controls::render(
                &active.key(),
                active.is_connected(),
                running.is_some(),
                cx,
            ))
            .children(connection_tag(active.saved.as_ref(), cx).map(|t| t.ml(px(6.))))
            .children(running.map(|since| run_status(since, cx).ml(px(8.))))
            .child(div().flex_1())
            .child(picker);
        let dismiss = console.clone();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().child(editor))
            .when_some(failed.filter(|_| running.is_none()), |d, failed| {
                d.child(run_failed(
                    &failed,
                    move |_, _, cx| dismiss.update(cx, |c, cx| c.dismiss_failure(cx)),
                    cx,
                ))
            })
    }

    fn render_table(
        &self,
        pane: &TablePane,
        active: &ActiveDb,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = cx.palette().clone();
        let t = pane.table.read(cx);
        let connected = active.is_connected();
        let meta = t.meta(cx);
        let info = match (&t.result, t.loading) {
            (Some(r), _) => {
                format!(
                    "{} loaded · {} ms",
                    crate::grid::page::grouped(r.rows.len()),
                    r.ms
                )
            }
            (None, true) if connected => "Loading…".to_owned(),
            _ => String::new(),
        };
        let total = meta
            .as_ref()
            .filter(|m| m.rows > 0)
            .map(|m| {
                format!(
                    " · ~{} in table",
                    crate::grid::page::grouped(m.rows as usize)
                )
            })
            .unwrap_or_default();
        let pager_state = t.result.as_ref().map(|r| PagerState {
            offset: t.offset,
            count: r.rows.len(),
            page_size: t.page_size,
            more: r.truncated,
            busy: t.loading,
        });
        let (reload_table, page_table) = (pane.table.clone(), pane.table.clone());
        let can_reload = connected && !t.loading;
        let key = t.key.clone();
        let select = format!("{};", t.sql);
        let edits = table_edits::toolbar(&pane.table, &pane.grid, cx);
        let pending_bar = table_edits::pending_bar(&pane.table, cx);
        // CsvExport.tsx ExportMenu: the whole table with the tab's filter and
        // sort; "Copy page as CSV" copies the rows the grid shows (the row
        // filter and staged edits applied: TableEdits.tsx `rows`). Disabled
        // only while disconnected or when a clause is refused before it runs
        // (`clauseError`); after a server error the export still runs.
        let clause_error = table_select(&t.schema, &t.table, &t.query())
            .error
            .is_some();
        let export_el = csv_export::export_menu(
            "table-export",
            table_owner(&key.connection_id, &key.database, &t.schema, &t.table),
            Some(MenuSource {
                source: table_tab_source(&key, &t.schema, &t.table, &t.sql, None),
                page: t.result.clone(),
                grid: Some(pane.grid.downgrade()),
            }),
            export_disabled(connected, clause_error),
            cx,
        );
        // The DDL button, for a table the catalog knows (state/ddl.ts `openDdl`).
        let ddl = meta.is_some().then(|| {
            let (key, schema, table) = (key.clone(), t.schema.clone(), t.table.clone());
            // `TOOL_BTN`: h-6 px-1.5 text-xs text-quiet, 40 % when disabled.
            let (hover_bg, hover_fg) = (p.hover_strong, p.body);
            div()
                .id("table-ddl")
                .debug_selector(|| "table-ddl".to_owned())
                .flex()
                .flex_none()
                .items_center()
                .h(px(24.))
                .px(px(6.))
                .rounded(theme::radius::MD)
                .text_size(theme::text::XS)
                .text_color(p.quiet)
                .child("DDL")
                .when(connected, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                })
                .when(!connected, |d| d.opacity(0.4))
                .on_click(move |_, _, cx| {
                    if connected {
                        super::sidebar::show_ddl(key.clone(), schema.clone(), table.clone(), cx);
                    }
                })
        });
        let row_filter = div()
            .id("table-row-filter")
            .debug_selector(|| "table-row-filter".to_owned())
            .flex()
            .flex_1()
            .min_w(px(200.))
            .max_w(px(320.))
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(8.))
            .rounded(theme::radius::MD)
            .border_1()
            .border_color(p.input)
            .bg(p.field)
            .child(icon(IconName::Search, 12., p.dim))
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&pane.row_filter)
                        .appearance(false)
                        .px_0()
                        .py_0()
                        .h(px(22.))
                        .font_family(theme::MONO_FONT)
                        .text_size(theme::text::XS)
                        .text_color(p.body),
                ),
            );
        let toolbar = div()
            .id("table-toolbar")
            .debug_selector(|| "table-toolbar".to_owned())
            .flex()
            .flex_none()
            .flex_wrap()
            .items_center()
            .gap(px(2.))
            .min_h(TOOLBAR_HEIGHT)
            .py(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(
                tool_button(
                    "table-reload",
                    IconName::RefreshCw,
                    "Reload",
                    can_reload,
                    None,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    if can_reload {
                        reload_table.update(cx, |t, cx| t.request(TableNav::Reload, cx));
                    }
                }),
            )
            .children(pager_state.and_then(|s| {
                pager(
                    "table-pager",
                    s,
                    move |offset, _, cx| {
                        page_table.update(cx, |t, cx| t.request(TableNav::Page(offset), cx));
                    },
                    cx,
                )
            }))
            .children(edits)
            .child(vsep(cx).mx(px(6.)))
            .child(row_filter)
            .child(div().flex_1())
            .child(export_el)
            .children(ddl)
            .child(
                tool_button(
                    "table-console",
                    IconName::FileCode,
                    "Open query console",
                    true,
                    None,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    let (key, sql) = (key.clone(), select.clone());
                    if let Some(state) = AppState::try_global(cx).cloned() {
                        state
                            .workspaces
                            .update(cx, |w, cx| w.new_query_tab_with(key, sql, cx));
                    }
                }),
            );
        let body = if !connected {
            not_connected(active, cx)
        } else if let Some(error) = t.error.clone() {
            // A bad condition: the server's error in place of the rows (the
            // filter bar keeps what was typed).
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .px(px(24.))
                .text_center()
                .text_size(theme::text::SM)
                .text_color(p.status_err_text)
                .child(error)
        } else if t.result.is_none() {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Spinner::new().small().color(p.quiet))
        } else {
            div().flex_1().min_h_0().child(pane.grid.clone())
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(toolbar)
            .children(pending_bar)
            .child(pane.filter_bar.clone())
            .child(body)
            .child(
                div()
                    .flex_none()
                    .px(px(8.))
                    .py(px(4.))
                    .border_t_1()
                    .border_color(p.border)
                    .text_size(theme::text::XS)
                    .text_color(p.quiet)
                    .child(format!("{info}{total}")),
            )
    }
}

/// The table toolbar's Export menu is disabled (Workspace.tsx
/// `!connected || !!clauseError`): not for a failed load.
pub fn export_disabled(connected: bool, clause_error: bool) -> bool {
    !connected || clause_error
}

/// IconButton.tsx with a disabled state (40 % opacity, no hover) and an
/// optional colour (Run is green).
pub fn tool_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    enabled: bool,
    color: Option<Hsla>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    tool_button_icon(
        id,
        gpui_component::Icon::new(icon),
        label,
        enabled,
        color,
        cx,
    )
}

pub fn tool_button_icon(
    id: &'static str,
    icon: gpui_component::Icon,
    label: &'static str,
    enabled: bool,
    color: Option<Hsla>,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let p = cx.palette();
    let (fg, hover_fg, hover_bg) = (
        color.unwrap_or(p.quiet),
        color.unwrap_or(p.body),
        p.hover_strong,
    );
    div()
        .id(id)
        .flex_none()
        .size(px(24.))
        .rounded(theme::radius::MD)
        .flex()
        .items_center()
        .justify_center()
        .text_color(fg)
        .tooltip(tooltip(label))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        })
        .when(!enabled, |d| d.opacity(0.4))
        .child(icon.size(px(14.)))
}

/// A menu trigger in a toolbar (Workspace.tsx `TOOL_BTN`: h-6 px-1.5
/// gap-1 text-xs text-quiet, the foreground on hover, 40 % when disabled):
/// the label, then a 12 px chevron (`ChevronDown size-3`). `mono`: the
/// label in Geist Mono (the default-schema picker).
pub fn tool_menu_trigger(
    id: &'static str,
    label: impl Into<gpui::SharedString>,
    mono: bool,
    enabled: bool,
    cx: &App,
) -> Button {
    let p = cx.palette();
    Button::new(id)
        .ghost()
        .h(px(24.))
        .px(px(6.))
        .rounded(theme::radius::MD)
        .text_color(p.quiet)
        .disabled(!enabled)
        .when(!enabled, |b| b.opacity(0.4))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .text_size(theme::text::XS)
                .line_height(theme::text::XS_LINE)
                .when(mono, |d| d.font_family(theme::MONO_FONT))
                .child(label.into())
                .child(Icon::new(IconName::ChevronDown).size(px(12.))),
        )
}

/// Stands in for a table's rows while its connection isn't open, with a way to open it.
fn not_connected(active: &ActiveDb, cx: &App) -> gpui::Div {
    let p = cx.palette();
    let busy = matches!(active.status, ConnState::Connecting);
    let reconnecting = matches!(active.status, ConnState::Reconnecting { .. });
    let label = if busy {
        "Connecting…"
    } else if reconnecting {
        "Reconnecting…"
    } else {
        "Not connected"
    };
    let id = active.connection_id.to_string();
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .text_size(theme::text::SM)
        .text_color(p.quiet)
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .child(label)
                .when(!busy, |d| {
                    d.child(
                        div()
                            .id("table-connect")
                            .rounded(theme::radius::MD)
                            .border_1()
                            .border_color(p.border)
                            .px(px(10.))
                            .py(px(4.))
                            .text_size(theme::text::XS)
                            .text_color(p.body)
                            .cursor_pointer()
                            .hover(|s| s.bg(p.hover))
                            .child(if reconnecting { "Retry now" } else { "Connect" })
                            .on_click(move |_, _, cx| {
                                if let Some(state) = AppState::try_global(cx).cloned() {
                                    state.connections.update(cx, |c, cx| c.connect(&id, cx));
                                }
                            }),
                    )
                }),
        )
}

impl Focusable for Workspace {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        let tab = self.active_tab(cx);
        let handle = tab.and_then(|tab| match &tab.content {
            TabContent::Console(c) => self
                .consoles
                .get(&c.entity_id())
                .map(|p| p.editor.focus_handle(cx)),
            TabContent::Table(t) => self
                .tables
                .get(&t.entity_id())
                .map(|p| p.grid.focus_handle(cx)),
        });
        handle.unwrap_or_else(|| self.focus_handle.clone())
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        let root = div()
            .id("workspace")
            .track_focus(&self.focus_handle)
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(p.background);
        let Some(active) = ActiveDb::get(cx) else {
            return root.child(Self::empty(
                "Select a database",
                vec![
                    kbd("⌘K", cx).into_any_element(),
                    "to search tables".into_any_element(),
                ],
                cx,
            ));
        };
        let stripe = active.color().map(|c| div().flex_none().h(px(2.)).bg(c));
        let tab = self.active_tab(cx);
        let body = match tab.as_ref().map(|t| &t.content) {
            Some(TabContent::Console(c)) => match self.consoles.get(&c.entity_id()) {
                Some(pane) => self.render_console(pane, &active, cx),
                None => div(),
            },
            Some(TabContent::Table(t)) => match self.tables.get(&t.entity_id()) {
                Some(pane) => self.render_table(pane, &active, cx),
                None => div(),
            },
            None => Self::empty(
                "No tabs open",
                vec![
                    kbd("⌘T", cx).into_any_element(),
                    "new console ·".into_any_element(),
                    kbd("⌘K", cx).into_any_element(),
                    "open a table".into_any_element(),
                ],
                cx,
            ),
        };
        root.children(stripe).child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(body),
        )
    }
}

#[cfg(feature = "dev-hooks")]
mod dev;
#[cfg(test)]
mod tests;
