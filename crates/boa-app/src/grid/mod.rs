//! The result grid (DataGrid.tsx): a virtualized grid on GPUI's
//! `uniform_list` that shows one page of a result. It is a pure view: its
//! owner (a console result tab, a table tab) runs the statement and hands
//! the page over with [`ResultGrid::set_content`]; the grid answers with
//! [`GridEvent`]s (another page, a server-side sort, staged edits changed,
//! submit). It never runs SQL itself.
//!
//! Behaviour follows the Tauri app's `DataGrid.tsx`: sticky header (name +
//! type, primary-key icon, sort arrows; click to sort, drag the right edge to
//! resize), sticky row numbers, `<null>` for NULL, numbers right-aligned,
//! values coloured by type (JSON token by token, with a button that opens the
//! JSON viewer), cell / range / row selection (click, drag, ⇧-click,
//! ⇧-arrows, row numbers with ⇧ / ⌘, ⌘A, Esc), ⌘C as TSV, a right-click
//! menu with Copy / Copy as › (`boa_core::grid::copy_as`) and, when the
//! content is editable, DataGrip's data editor: inline edit (double-click,
//! Enter / F2 or typing), the value editor (⇧↵, and for multi-line / long /
//! JSON values), add / delete row, set NULL, revert, submit (⌘↵). Edits are
//! staged in `boa_core::grid::grid_edits::Pending` and tinted per kind; the
//! owner reads them with [`ResultGrid::pending`] / [`ResultGrid::take_pending`].
//!
//! Only visible rows (`uniform_list`) and visible columns (computed from the
//! horizontal scroll offset) are built each frame, so 100k rows × many
//! columns cost the same as a screenful.
//!
//! Module layout: [`selection`] (selection model), [`columns`] (widths,
//! hit-testing), [`page`] (a page with its view order, staged edits and copy
//! text), [`sort`] (local sort), [`error`] (error display), `interact`
//! (mouse, keys, editing, context menu), `render` (drawing), `dev`
//! (env-driven hooks and the scroll benchmark, `dev-hooks` feature only).

pub mod columns;
// Env-driven test hooks only with the `dev-hooks` feature; otherwise no-ops.
#[cfg_attr(not(feature = "dev-hooks"), path = "dev_off.rs")]
mod dev;
pub mod error;
mod interact;
pub mod page;
mod render;
pub mod selection;
pub mod sort;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use boa_core::grid::copy_as::CopyFormat;
use boa_core::grid::grid_edits::{Editability, Pending, RowRef};
use boa_core::grid::table_query::ColumnSort;
use boa_core::model::{QueryResult, Value};
use gpui::{
    Action, App, Context, Entity, FocusHandle, Focusable, Pixels, Subscription,
    UniformListScrollHandle, Window, actions, point, px,
};
use gpui_component::input::InputState;

use crate::app_state::AppState;
use crate::models::toasts::NewToast;
use crate::views::json_viewer::JsonViewer;
use crate::views::value_editor::ValueEditor;

pub use error::RunError;
use page::{Page, plural};
use selection::{Cell, DragKind};

actions!(
    result_grid,
    [
        MoveUp,
        MoveDown,
        MoveLeft,
        MoveRight,
        SelectUp,
        SelectDown,
        SelectLeft,
        SelectRight,
        PageUp,
        PageDown,
        SelectAll,
        /// ⌘C: the selection as TSV.
        Copy,
        /// Esc: drop the range (or cancel the inline edit).
        Collapse,
        /// Enter / F2: edit the active cell (read-only: open a JSON cell in the viewer).
        Edit,
        /// ⇧↵: the value editor.
        EditLarge,
        /// ⌘⌥N: stage NULL.
        SetNull,
        /// ⌘⌥Z: revert the cell if edited, else the row.
        Revert,
        /// Context menu "Revert cell".
        RevertCell,
        /// Context menu "Revert row".
        RevertRow,
        /// ⌘⌫ / Delete: mark the row for deletion.
        DeleteRow,
        /// Esc in the inline input.
        CancelEdit,
        /// Tab in the inline input: commit, move right.
        CommitEditNext,
        /// ⇧Tab in the inline input: commit, move left.
        CommitEditPrev,
        /// The braces button of a JSON cell / context: open the active cell in the JSON viewer.
        ViewJson,
    ]
);

/// Context menu "Copy as ›": the selection in one of `CopyFormat::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Action)]
#[action(namespace = result_grid, no_json)]
pub struct CopyAs(pub CopyFormat);

/// Key context of the grid.
pub const CONTEXT: &str = "ResultGrid";
/// Key context around the inline cell input.
const EDITOR_CONTEXT: &str = "GridCellEditor";

/// Row height (`h-7`).
pub const ROW_HEIGHT: f32 = 28.;
/// Header height (`h-8`).
pub const HEADER_HEIGHT: f32 = 32.;
/// The status strip above the grid (BottomPanel's `h-9` header).
const STATUS_HEIGHT: f32 = 36.;
/// Horizontal cell padding (`px-2.5`).
const CELL_PAD: f32 = 10.;
/// JSON cells colour at most this many chars (`JsonInline`).
const JSON_HEAD: usize = 400;
/// How long the copy toasts show (`pushToast(…, 1600)`).
const COPY_TOAST_FOR: Duration = Duration::from_millis(1600);
/// Values that open in the value editor instead of the inline input (`v.length > 200`).
const LONG_VALUE: usize = 200;

/// Binds the grid's keys (with all of Boa's: `crate::keymap`, once per app).
fn bind_keys(cx: &mut App) {
    crate::keymap::init(cx);
}

// ── Wave 2 contract: the grid as a pure view ──────────────────────────────

/// What a console result tab or a table tab hands the grid to show.
#[derive(Debug, Clone)]
pub struct GridContent {
    /// The statement that produced the page (breadcrumbs, copy-as INSERT/UPDATE target).
    pub sql: String,
    pub result: Arc<QueryResult>,
    /// Row offset of this page (row numbers start at offset + 1).
    pub offset: usize,
    /// `prefs.rowLimit` the page was fetched with.
    pub page_size: usize,
    /// Table tabs of tables with a PK only; consoles are always read-only.
    pub editability: Editability,
}

/// What the grid asks its owner to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridEvent {
    /// The pager asked for another page.
    PageRequested { offset: usize },
    /// Header click on a table tab (server ORDER BY; ⇧ adds a column). Consoles sort locally and never emit this.
    ServerSortRequested { column: usize, additive: bool },
    /// The staged edits changed (Phase 5).
    PendingChanged,
    /// ⌘↵ in the grid: preview / submit the staged edits (Phase 5).
    SubmitRequested,
}

impl gpui::EventEmitter<GridEvent> for ResultGrid {}

/// Table-tab extras (DataGrid's `table`, `pk` and `serverSort` props), set
/// with [`ResultGrid::set_table_context`]. With one, header clicks emit
/// [`GridEvent::ServerSortRequested`] instead of sorting locally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableContext {
    /// Qualified, quoted table name (`qualified(schema, table)`): the target
    /// of Copy as INSERT / UPDATE.
    pub table: String,
    /// Primary-key column names (key icon in the header; Copy as UPDATE).
    pub pk: Vec<String>,
    /// The ORDER BY's sorts by column name
    /// (`boa_core::grid::table_query::sort_state`): arrows and ranks in the header.
    pub sort: Vec<(String, ColumnSort)>,
    /// Columns that have a default (catalog `Column::default`): a new row
    /// shows `<default>` in them until set; its other columns show NULL
    /// (TableEdits.tsx `placeholder`).
    pub defaults: Vec<String>,
}

// ── State ─────────────────────────────────────────────────────────────────

/// The inline cell input.
struct CellEditor {
    cell: Cell,
    input: Entity<InputState>,
    /// The text changed (or editing started by typing): only then is it staged.
    touched: bool,
    _events: Subscription,
}

/// A modal over the window: the JSON viewer or the value editor.
enum Overlay {
    Json {
        viewer: Entity<JsonViewer>,
        _events: Subscription,
    },
    Value {
        editor: Entity<ValueEditor>,
        row: RowRef,
        col: usize,
        /// The value shown when it opened (`<default>` as NULL): applying it unchanged stages nothing.
        original: Value,
        _events: Subscription,
    },
}

/// A column-edge drag.
#[derive(Debug, Clone, Copy)]
struct ColumnResize {
    col: usize,
    start_x: Pixels,
    start_width: f32,
}

/// Scroll geometry read at the start of each render.
#[derive(Debug, Clone, Copy, Default)]
struct Frame {
    /// Horizontal scroll (>= 0: how far the content moved left).
    scroll_x: f32,
    viewport_width: f32,
    viewport_height: f32,
}

/// What a mouse position is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    RowNumber(usize),
    Cell(Cell),
}

/// The result grid.
pub struct ResultGrid {
    focus_handle: FocusHandle,
    page: Option<Page>,
    /// The statement of the content shown (or of the dev hook's, while it loads).
    last_sql: Option<String>,
    /// The owner's statement is running.
    running: bool,
    error: Option<RunError>,
    /// Outcome of a statement without rows ("3 rows affected in 4 ms").
    notice: Option<String>,
    editability: Editability,
    table: Option<TableContext>,
    /// The owner's "Filter loaded rows" (table tabs), applied to every page.
    row_filter: String,
    /// Show the status strip (run state, staged changes, pager).
    toolbar: bool,
    scroll: UniformListScrollHandle,
    editor: Option<CellEditor>,
    overlay: Option<Overlay>,
    drag: Option<DragKind>,
    resize: Option<ColumnResize>,
    frame: Frame,
    /// Set when content arrives; the next frame logs time to first paint.
    paint_probe: Option<Instant>,
    dev: dev::DevState,
}

impl std::fmt::Debug for ResultGrid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultGrid")
            .field("last_sql", &self.last_sql)
            .field("rows", &self.page.as_ref().map(Page::row_count))
            .field("running", &self.running)
            .field("error", &self.error)
            .field("editability", &self.editability)
            .finish_non_exhaustive()
    }
}

impl ResultGrid {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let mut grid = Self {
            focus_handle: cx.focus_handle(),
            page: None,
            last_sql: None,
            running: false,
            error: None,
            notice: None,
            editability: Editability::ReadOnly { reason: "" },
            table: None,
            row_filter: String::new(),
            toolbar: true,
            scroll: UniformListScrollHandle::new(),
            editor: None,
            overlay: None,
            drag: None,
            resize: None,
            frame: Frame::default(),
            paint_probe: None,
            dev: dev::DevState::from_env(),
        };
        dev::schedule_quit(cx);
        dev::start(&mut grid, window, cx);
        grid
    }

    // ── Contract ──────────────────────────────────────────────────────────

    /// Shows a page, replacing whatever was shown. The same result (`Arc`)
    /// again only updates the offset / page size / editability and keeps
    /// selection, sort, scroll and staged edits; a new one starts them over
    /// (column widths carry over when the columns are the same). A result
    /// without columns shows its "N rows affected" notice.
    pub fn set_content(&mut self, content: GridContent, cx: &mut Context<Self>) {
        let GridContent {
            sql,
            result,
            offset,
            page_size,
            editability,
        } = content;
        self.running = false;
        self.error = None;
        self.last_sql = Some(sql.clone());
        self.editability = editability;
        if !self.can_edit_any() {
            self.close_editor(false, cx);
        }
        let same = self
            .page
            .as_ref()
            .is_some_and(|p| Arc::ptr_eq(&p.result, &result));
        if same {
            if let Some(page) = &mut self.page {
                page.sql = sql;
                page.offset = offset;
                page.page_size = page_size;
                page.layout.fit_row_numbers(offset + page.row_count());
            }
            cx.notify();
            return;
        }
        let had_pending = self.page.as_ref().is_some_and(|p| !p.pending.is_empty());
        self.close_editor(false, cx);
        self.overlay = None;
        self.drag = None;
        self.resize = None;
        if result.columns.is_empty() {
            self.notice = Some(match result.affected {
                Some(n) => format!("{} affected in {} ms", plural(n as usize, "row"), result.ms),
                None => format!("Completed in {} ms", result.ms),
            });
            self.page = None;
        } else {
            self.notice = None;
            let mut page = Page::new(sql, result, offset, page_size);
            self.prepare(&mut page);
            if let Some(old) = &self.page
                && old.same_columns(page.columns())
            {
                page.layout = old.layout.clone();
                page.layout.fit_row_numbers(offset + page.row_count());
            }
            tracing::info!(
                "grid: {} rows × {} columns",
                page.row_count(),
                page.col_count()
            );
            self.page = Some(page);
            self.paint_probe = Some(Instant::now());
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(0.)));
            dev::after_load(self);
        }
        if had_pending {
            cx.emit(GridEvent::PendingChanged);
        }
        cx.notify();
    }

    /// The owner's statement is running (shows the run state; ignores edits).
    pub fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        self.running = running;
        if running {
            self.close_editor(true, cx);
        }
        cx.notify();
    }

    /// Shows a run error (with position caret) instead of rows; `None` clears it.
    pub fn set_run_error(&mut self, error: Option<RunError>, cx: &mut Context<Self>) {
        if error.is_some() {
            self.running = false;
            self.close_editor(false, cx);
        }
        self.error = error;
        cx.notify();
    }

    // ── Additions for owners ──────────────────────────────────────────────

    /// Table-tab extras (None for consoles): the Copy as INSERT / UPDATE
    /// target, the key icon and server-side sorting. Kept across
    /// [`Self::set_content`]; set it again when the ORDER BY changes.
    pub fn set_table_context(&mut self, table: Option<TableContext>, cx: &mut Context<Self>) {
        if table.is_some()
            && let Some(page) = &mut self.page
            && page.sort().is_some()
        {
            page.set_sort(None);
        }
        self.table = table;
        if let Some(mut page) = self.page.take() {
            self.prepare(&mut page);
            self.page = Some(page);
        }
        cx.notify();
    }

    pub fn table_context(&self) -> Option<&TableContext> {
        self.table.as_ref()
    }

    /// Table tabs' "Filter loaded rows" (Workspace.tsx): shows only the
    /// loaded rows with a value containing `filter` (ignoring case; staged
    /// values count, new rows always show), numbered from 1. Kept across
    /// [`Self::set_content`]; empty shows every row.
    pub fn set_row_filter(&mut self, filter: &str, cx: &mut Context<Self>) {
        if self.row_filter == filter {
            return;
        }
        self.row_filter = filter.to_owned();
        if let Some(page) = &mut self.page {
            page.set_filter(filter);
        }
        cx.notify();
    }

    pub fn row_filter(&self) -> &str {
        &self.row_filter
    }

    /// Applies the table context's defaults and the row filter to a page.
    fn prepare(&self, page: &mut Page) {
        page.set_column_defaults(self.table.as_ref().map_or(&[], |t| &t.defaults));
        page.set_filter(&self.row_filter);
    }

    /// Shows or hides the status strip above the grid (owners with their
    /// own toolbar hide it).
    pub fn set_toolbar_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        self.toolbar = visible;
        cx.notify();
    }

    /// The staged edits of the page shown (empty without one).
    pub fn pending(&self) -> Pending {
        self.page
            .as_ref()
            .map(|p| p.pending.clone())
            .unwrap_or_default()
    }

    /// Takes the staged edits, leaving none (no event).
    pub fn take_pending(&mut self, cx: &mut Context<Self>) -> Pending {
        let taken = self
            .page
            .as_mut()
            .map(Page::take_pending)
            .unwrap_or_default();
        cx.notify();
        taken
    }

    /// Drops the staged edits (no event).
    pub fn clear_pending(&mut self, cx: &mut Context<Self>) {
        self.take_pending(cx);
    }

    /// Restores staged edits for the page shown (e.g. switching back to a
    /// tab whose edits the owner kept). No event.
    pub fn set_pending(&mut self, pending: Pending, cx: &mut Context<Self>) {
        if let Some(page) = &mut self.page {
            page.set_pending(pending);
            cx.notify();
        }
    }

    // ── Reading ───────────────────────────────────────────────────────────

    /// The statement of the content shown.
    pub fn last_sql(&self) -> Option<&str> {
        self.last_sql.as_deref()
    }

    /// The page shown, if any.
    pub fn page(&self) -> Option<&Page> {
        self.page.as_ref()
    }

    /// The error shown, if the last statement failed.
    pub fn error(&self) -> Option<&RunError> {
        self.error.as_ref()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn editability(&self) -> &Editability {
        &self.editability
    }

    /// The JSON viewer is open.
    pub fn json_viewer(&self) -> Option<&Entity<JsonViewer>> {
        match &self.overlay {
            Some(Overlay::Json { viewer, .. }) => Some(viewer),
            _ => None,
        }
    }

    /// The value editor is open.
    pub fn value_editor(&self) -> Option<&Entity<ValueEditor>> {
        match &self.overlay {
            Some(Overlay::Value { editor, .. }) => Some(editor),
            _ => None,
        }
    }

    /// The selected row (DataGrid's `onSelectRow`, e.g. for the right panel):
    /// the active cell's row as the staged edits address it.
    pub fn active_row(&self) -> Option<RowRef> {
        let page = self.page.as_ref()?;
        page.row_ref(page.selection.active()?.row)
    }

    /// The cell the inline input is open on.
    pub fn editing_cell(&self) -> Option<Cell> {
        self.editor.as_ref().map(|e| e.cell)
    }

    /// Rows can be edited now (editable content, nothing running).
    fn can_edit_any(&self) -> bool {
        matches!(self.editability, Editability::Editable { .. }) && !self.running
    }

    /// The reason the status strip shows: a table tab's only. Console
    /// results are read-only without saying why, in both apps (DataGrid.tsx
    /// gets no `editing` for them, and BottomPanel.tsx shows no reason).
    fn shown_read_only_reason(&self) -> Option<&'static str> {
        self.table
            .as_ref()
            .and(self.page.as_ref())
            .and(self.read_only_reason())
    }

    fn read_only_reason(&self) -> Option<&'static str> {
        match self.editability {
            Editability::ReadOnly { reason } if !reason.is_empty() => Some(reason),
            _ => None,
        }
    }

    /// Shows a toast through the app's [`crate::models::toasts::Toasts`].
    fn toast(toast: NewToast, ttl: Option<Duration>, cx: &mut App) {
        let Some(state) = AppState::try_global(cx) else {
            tracing::info!("grid toast (no AppState): {}", toast.title);
            return;
        };
        let toasts = state.toasts.clone();
        toasts.update(cx, |t, cx| {
            t.push_for(toast, ttl.or(Some(crate::models::toasts::DEFAULT_TTL)), cx);
        });
    }
}

impl Focusable for ResultGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
