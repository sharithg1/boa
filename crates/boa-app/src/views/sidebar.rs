//! Sidebar.tsx: the Database Explorer, after DataGrip's. A header row
//! (traffic-light inset, sidebar toggle, title), a toolbar (new, refresh,
//! console, collapse all, previous / next database, find table), the tree of
//! connection › database › schema › tables / views › table › columns (then
//! keys, foreign keys, indexes and checks folders), each level with its child
//! count, and a footer (Settings).
//!
//! Click selects (a table opens as a preview tab, a database becomes the
//! active one), double-click pins a table, the chevron or a double-click
//! expands. Expanding a disconnected connection connects it. Arrow keys move
//! through the tree (→ expands / enters, ← collapses / goes to the parent,
//! ↵ opens). Expansion is saved in prefs `treeExpanded` under the TS node ids
//! ([`tree`]). Right-click for the context menus; the "N of M" badges open
//! the [`ScopePicker`]; connections can be dragged to reorder them.
//!
//! The rows are built by [`tree::build`] from the models (Connections,
//! Catalogs, Workspaces, Prefs) and drawn with `uniform_list`, so thousands
//! of rows stay fast. The width (prefs `sidebarWidth`, 200–480) and the
//! resize handle on the right edge are applied by the root view.

pub mod row;
pub mod tree;

#[cfg(feature = "dev-hooks")]
pub mod dev;

#[cfg(test)]
mod tests;

use std::rc::Rc;
use std::time::{Duration, Instant};

use boa_core::sql::qualified;
use boa_core::state::ddl::{DdlAction, DdlRequest, ddl_action};
use boa_core::state::prefs::apply_order;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    FontWeight, MouseButton, Pixels, Point, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window, anchored, deferred, div, point, px, uniform_list,
};
use gpui_component::Sizable as _;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu};

use self::tree::{ActiveTable, ConnInfo, Meta, Nav, NavKey, Node, NodeKind, ScopeBadge};
use super::chrome::{self, IconName, WindowDrag, icon, icon_button, vsep};
use super::connection_dialog;
use super::scope_picker::{ScopePicker, ScopePickerEvent};
use crate::actions::{self, CollapseNode, Confirm, ExpandNode, SelectNext, SelectPrevious};
use crate::app_state::AppState;
use crate::models::catalogs::DbKey;
use crate::models::connections::ConnState;
use crate::models::exports::export_table;
use crate::models::layout::{PanelKind, SettingsPage};
use crate::models::toasts::NewToast;
use crate::models::workspaces::TabKind;
use crate::theme::{self, ActivePalette as _};

/// Row height (`h-[26px]`).
pub const ROW_HEIGHT: Pixels = px(26.);
/// How long "Copied …" shows.
const COPIED_TTL: Duration = Duration::from_millis(2400);

/// A connection being dragged to a new place.
#[derive(Debug, Clone)]
pub struct DraggedConnection {
    pub id: String,
    pub name: SharedString,
}

/// The drag preview: the connection's name.
#[derive(Debug)]
struct DragPreview(SharedString);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(theme::radius::MD)
            .bg(p.popover)
            .border_1()
            .border_color(p.border)
            .text_size(theme::text::SM)
            .text_color(p.body)
            .child(self.0.clone())
    }
}

/// The open scope picker and where it shows.
#[derive(Debug)]
struct OpenPicker {
    view: Entity<ScopePicker>,
    position: Point<Pixels>,
    _subscriptions: Vec<Subscription>,
}

/// The Database Explorer view.
pub struct Sidebar {
    drag: WindowDrag,
    focus_handle: FocusHandle,
    /// The keyboard-focused row (`aria-selected`), by node id.
    focused: Option<String>,
    /// The rows of the last render.
    nodes: Rc<Vec<Node>>,
    scroll: UniformListScrollHandle,
    picker: Option<OpenPicker>,
    /// The picker last closed by a click outside, and when: a click on its
    /// own badge then closes it instead of reopening it.
    picker_closed: Option<(String, Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Sidebar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidebar")
            .field("focused", &self.focused)
            .field("rows", &self.nodes.len())
            .finish_non_exhaustive()
    }
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The connections in sidebar order, with their live state.
pub fn connection_infos(cx: &App) -> Vec<ConnInfo> {
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let connections = state.connections.read(cx);
    let prefs = state.prefs.read(cx).get();
    let ids: Vec<&str> = connections
        .saved
        .iter()
        .map(|c| c.config.id.as_str())
        .collect();
    apply_order(&ids, &prefs.connection_order)
        .iter()
        .filter_map(|id| connections.get(id))
        .map(|c| ConnInfo::new(c, connections.state_of(&c.config.id)))
        .collect()
}

/// Where "New query" goes: the selected database, else the first connected
/// one (Sidebar.tsx `newQueryTarget`).
pub fn new_query_target(cx: &App) -> Option<DbKey> {
    let state = AppState::try_global(cx)?;
    let connections = state.connections.read(cx);
    if let Some(active) = state.workspaces.read(cx).active.clone()
        && connections.is_connected(&active.connection_id)
    {
        return Some(active);
    }
    connection_infos(cx).into_iter().find_map(|c| {
        c.databases()
            .first()
            .map(|db| DbKey::new(c.id.clone(), db.clone()))
    })
}

/// Copies `text` and says so (Sidebar.tsx `copy`).
pub fn copy_text(text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    if let Some(state) = AppState::try_global(cx) {
        let toasts = state.toasts.clone();
        toasts.update(cx, |t, cx| {
            t.push_for(
                NewToast::info(format!("Copied {text}")),
                Some(COPIED_TTL),
                cx,
            );
        });
    }
}

/// "Show DDL": asks the server for the relation's DDL and opens it in a new
/// console on that database (state/ddl.ts `openDdl`).
pub fn show_ddl(key: DbKey, schema: String, table: String, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let req = DdlRequest {
        connection_id: key.connection_id.clone(),
        database: key.database.clone(),
        schema,
        table,
    };
    let task = state
        .backend
        .table_ddl(&req.connection_id, &req.database, &req.schema, &req.table);
    cx.spawn(async move |cx| {
        let result = task.await;
        let action = ddl_action(&req, result.as_ref().cloned());
        cx.update(|cx| match action {
            DdlAction::OpenConsole {
                connection_id,
                database,
                sql,
            } => {
                let workspaces = state.workspaces.clone();
                workspaces.update(cx, |w, cx| {
                    w.new_query_tab_with(DbKey::new(connection_id, database), sql, cx)
                });
            }
            DdlAction::Toast(toast) => {
                let mut t = NewToast::error(toast.title);
                if let Some(body) = toast.body {
                    t = t.body(body);
                }
                state.toasts.update(cx, |toasts, cx| {
                    toasts.push(t, cx);
                });
            }
        });
    })
    .detach();
}

impl Sidebar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(state) = AppState::try_global(cx).cloned() {
            subscriptions.push(cx.observe(&state.prefs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.connections, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.catalogs, |_, _, cx| cx.notify()));
            subscriptions.push(cx.observe(&state.workspaces, |_, _, cx| cx.notify()));
        }
        Self {
            drag: WindowDrag::new(),
            focus_handle: cx.focus_handle(),
            focused: None,
            nodes: Rc::new(Vec::new()),
            scroll: UniformListScrollHandle::new(),
            picker: None,
            picker_closed: None,
            _subscriptions: subscriptions,
        }
    }

    /// The keyboard-focused row's id.
    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    /// The rows as last rendered.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The open scope picker, if any.
    pub fn picker(&self) -> Option<&Entity<ScopePicker>> {
        self.picker.as_ref().map(|p| &p.view)
    }

    /// Builds the rows from the models.
    pub fn build_nodes(cx: &App) -> Vec<Node> {
        let Some(state) = AppState::try_global(cx) else {
            return Vec::new();
        };
        let conns = connection_infos(cx);
        let prefs = state.prefs.read(cx).get();
        let catalogs = state.catalogs.read(cx);
        let workspaces = state.workspaces.read(cx);
        let active = workspaces.active.as_ref();
        let active_tab = workspaces.active_workspace().and_then(|ws| {
            let id = ws.active_tab.as_ref()?;
            ws.tabs.iter().find(|t| &t.id == id)
        });
        let active_table = active_tab.and_then(|t| match &t.kind {
            TabKind::Table {
                schema,
                table,
                preview,
            } => Some(ActiveTable {
                schema: schema.as_ref(),
                table: table.as_ref(),
                preview: *preview,
            }),
            TabKind::Query { .. } => None,
        });
        tree::build(&tree::TreeInput {
            connections: &conns,
            prefs,
            catalogs,
            active,
            active_table,
        })
    }

    fn node(&self, id: &str) -> Option<Node> {
        self.nodes.iter().find(|n| n.id == id).cloned()
    }

    fn is_expanded(id: &str, cx: &App) -> bool {
        AppState::try_global(cx).is_some_and(|s| s.prefs.read(cx).get().is_expanded(id))
    }

    /// Expands or collapses a row (`expand`): expanding a connection connects
    /// it, expanding a database loads it; the state is saved in the prefs.
    pub fn set_expanded(&mut self, node: &Node, open: bool, cx: &mut Context<Self>) {
        if !node.expandable {
            return;
        }
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        if open {
            match &node.kind {
                NodeKind::Connection(conn) => {
                    if !matches!(
                        conn.state,
                        ConnState::Connected { .. } | ConnState::Connecting
                    ) {
                        let id = conn.id.clone();
                        state.connections.update(cx, |c, cx| c.connect(&id, cx));
                    }
                }
                NodeKind::Database { key, .. } => {
                    let key = key.clone();
                    state.catalogs.update(cx, |c, cx| c.load(key, false, cx));
                }
                _ => {}
            }
        }
        let id = node.id.clone();
        state
            .prefs
            .update(cx, |p, cx| p.update(cx, |p| p.set_expanded(&id, open)));
    }

    fn toggle(&mut self, node: &Node, cx: &mut Context<Self>) {
        let open = !Self::is_expanded(&node.id, cx);
        self.set_expanded(node, open, cx);
    }

    /// Click / ↵ without an open action: a table opens as a preview tab;
    /// databases and schemas select their database (and load it).
    pub fn select(&mut self, node: &Node, cx: &mut Context<Self>) {
        let Some(state) = AppState::try_global(cx).cloned() else {
            return;
        };
        match &node.kind {
            NodeKind::Database { key, .. } | NodeKind::Schema { key, .. } => {
                let key = key.clone();
                state
                    .workspaces
                    .update(cx, |w, cx| w.select_database(key.clone(), cx));
                state.catalogs.update(cx, |c, cx| c.load(key, false, cx));
            }
            NodeKind::Table {
                key, schema, table, ..
            } => {
                let key = key.clone();
                state
                    .workspaces
                    .update(cx, |w, cx| w.open_table(key, schema, table, true, cx));
            }
            _ => {}
        }
    }

    /// Double-click / ↵: a table opens pinned; anything else toggles.
    pub fn open(&mut self, node: &Node, cx: &mut Context<Self>) {
        match &node.kind {
            NodeKind::Table {
                key, schema, table, ..
            } => {
                if let Some(state) = AppState::try_global(cx).cloned() {
                    let key = key.clone();
                    state
                        .workspaces
                        .update(cx, |w, cx| w.open_table(key, schema, table, false, cx));
                }
            }
            _ => self.toggle(node, cx),
        }
    }

    fn focus_row(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(ix) = self.nodes.iter().position(|n| n.id == id) {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
        self.focused = Some(id);
        cx.notify();
    }

    /// The tree's key handling (Sidebar.tsx `onKeyDown`).
    pub fn navigate(&mut self, key: NavKey, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        self.nodes = Rc::new(Self::build_nodes(cx));
        let nav = {
            let prefs = AppState::try_global(cx).map(|s| s.prefs.read(cx).get().clone());
            tree::nav(&self.nodes, self.focused.as_deref(), key, |id| {
                prefs.as_ref().is_some_and(|p| p.is_expanded(id))
            })
        };
        match nav {
            Nav::None => {}
            Nav::Focus(id) => self.focus_row(id, cx),
            Nav::Expand(id) | Nav::Collapse(id) | Nav::Toggle(id) => {
                if let Some(node) = self.node(&id) {
                    let open = !Self::is_expanded(&id, cx);
                    self.set_expanded(&node, open, cx);
                }
            }
            Nav::Open(id) => {
                if let Some(node) = self.node(&id) {
                    self.open(&node, cx);
                }
            }
            Nav::Select(id) => {
                if let Some(node) = self.node(&id) {
                    self.select(&node, cx);
                }
            }
        }
    }

    fn on_row_click(
        &mut self,
        ix: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.nodes.get(ix).cloned() else {
            return;
        };
        window.focus(&self.focus_handle, cx);
        if event.click_count() >= 2 {
            self.open(&node, cx);
            return;
        }
        self.focused = Some(node.id.clone());
        self.select(&node, cx);
        cx.notify();
    }

    // ── Scope picker ─────────────────────────────────────────────────────

    /// Opens the scope picker for `badge` at `position` (window coordinates).
    pub fn open_picker(
        &mut self,
        badge: &ScopeBadge,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((key, at)) = &self.picker_closed
            && *key == badge.key
            && at.elapsed() < Duration::from_millis(400)
        {
            self.picker_closed = None;
            return;
        }
        let view = cx.new(|cx| ScopePicker::new(badge, window, cx));
        let key = badge.key.clone();
        let subscriptions = vec![
            cx.subscribe(&view, move |_, _, event: &ScopePickerEvent, cx| {
                if let ScopePickerEvent::Apply(scope) = event
                    && let Some(state) = AppState::try_global(cx).cloned()
                {
                    let (key, scope) = (key.clone(), scope.clone());
                    state.prefs.update(cx, |p, cx| {
                        p.update(cx, |p| {
                            p.scopes.insert(key, scope);
                        })
                    });
                }
            }),
            cx.subscribe_in(
                &view,
                window,
                |this, view, _: &gpui::DismissEvent, window, cx| {
                    let key = view.read(cx).key().to_owned();
                    this.picker = None;
                    this.picker_closed = Some((key, Instant::now()));
                    window.focus(&this.focus_handle, cx);
                    cx.notify();
                },
            ),
        ];
        view.update(cx, |v, cx| v.focus_filter(window, cx));
        self.picker = Some(OpenPicker {
            view,
            position: point(position.x - px(12.), position.y + px(10.)),
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    fn scope_badge(&self, ix: usize, badge: &ScopeBadge, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let open = self
            .picker
            .as_ref()
            .is_some_and(|o| o.view.read(cx).key() == badge.key);
        let badge_c = badge.clone();
        div()
            .id(("scope-badge", ix))
            .flex_none()
            .px(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(if open { p.dim } else { p.border_strong })
            .text_size(theme::text::XS)
            .line_height(px(16.))
            .text_color(if open { p.soft } else { p.dim })
            .cursor_pointer()
            .hover(|s| s.border_color(p.dim).text_color(p.soft))
            .tooltip(chrome::tooltip(format!(
                "Choose {} to show",
                badge.kind.noun()
            )))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.open_picker(&badge_c, event.position(), window, cx);
            }))
            .child(format!("{} of {}", badge.selected.len(), badge.items.len()))
            .into_any_element()
    }

    // ── Context menus ────────────────────────────────────────────────────

    fn context_menu(
        node: Node,
        weak: WeakEntity<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        move |menu, _, cx| Self::build_menu(menu, &node, &weak, cx)
    }

    fn build_menu(menu: PopupMenu, node: &Node, weak: &WeakEntity<Self>, cx: &App) -> PopupMenu {
        // chrome::menu_row: the app's menu rows in px (the stock item sizes in rems).
        let item = |label: &'static str, icon: IconName| chrome::menu_row(icon, label, "");
        let copy = |label: &'static str, icon: IconName, text: String| {
            item(label, icon).on_click(move |_, _, cx| copy_text(text.clone(), cx))
        };
        match &node.kind {
            NodeKind::Connection(conn) => {
                let connected = conn.connected();
                let id = conn.id.clone();
                let mut menu = if connected {
                    let id = id.clone();
                    menu.item(
                        item("Disconnect", IconName::Unplug).on_click(move |_, _, cx| {
                            if let Some(s) = AppState::try_global(cx).cloned() {
                                s.connections.update(cx, |c, cx| c.disconnect(&id, cx));
                            }
                        }),
                    )
                } else {
                    let id = id.clone();
                    menu.item(
                        item("Connect", IconName::Plug)
                            .disabled(matches!(conn.state, ConnState::Connecting))
                            .on_click(move |_, _, cx| {
                                if let Some(s) = AppState::try_global(cx).cloned() {
                                    s.connections.update(cx, |c, cx| c.connect(&id, cx));
                                }
                            }),
                    )
                };
                let first_db = conn
                    .databases()
                    .iter()
                    .find(|d| **d == conn.database)
                    .or(conn.databases().first())
                    .cloned();
                let console_key = first_db.map(|db| DbKey::new(id.clone(), db));
                menu = menu
                    .item(
                        item("New query console", IconName::SquareTerminal)
                            .disabled(console_key.is_none())
                            .on_click({
                                let key = console_key.clone();
                                move |_, _, cx| {
                                    if let (Some(key), Some(s)) =
                                        (key.clone(), AppState::try_global(cx).cloned())
                                    {
                                        s.workspaces.update(cx, |w, cx| w.new_query_tab(key, cx));
                                    }
                                }
                            }),
                    )
                    .item(
                        item("Refresh", IconName::RefreshCw)
                            .disabled(!connected)
                            .on_click({
                                let id = id.clone();
                                move |_, _, cx| refresh_connection(&id, cx)
                            }),
                    )
                    .item(copy(
                        "Copy connection URL",
                        IconName::Copy,
                        tree::connection_string(conn, None),
                    ))
                    .separator()
                    .item(item("Edit…", IconName::Pencil).on_click({
                        let id = id.clone();
                        move |_, window, cx| connection_dialog::open(Some(id.clone()), window, cx)
                    }))
                    .item(item("Delete…", IconName::Trash).on_click({
                        let id = id.clone();
                        move |_, window, cx| {
                            connection_dialog::confirm_delete(id.clone(), window, cx)
                        }
                    }));
                menu
            }
            NodeKind::Database { key, .. } => {
                let (k1, k2) = (key.clone(), key.clone());
                menu.item(
                    item("New query console", IconName::SquareTerminal).on_click(
                        move |_, _, cx| {
                            if let Some(s) = AppState::try_global(cx).cloned() {
                                let key = k1.clone();
                                s.workspaces.update(cx, |w, cx| w.new_query_tab(key, cx));
                            }
                        },
                    ),
                )
                .item(
                    item("Refresh", IconName::RefreshCw).on_click(move |_, _, cx| {
                        if let Some(s) = AppState::try_global(cx).cloned() {
                            let key = k2.clone();
                            s.catalogs.update(cx, |c, cx| c.refresh(key, cx));
                        }
                    }),
                )
            }
            NodeKind::Table {
                key, schema, table, ..
            } => {
                let select = format!("select * from {};", qualified(schema, table));
                let (key, schema, table) = (key.clone(), schema.clone(), table.clone());
                let open = {
                    let (key, schema, table) = (key.clone(), schema.clone(), table.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(s) = AppState::try_global(cx).cloned() {
                            let key = key.clone();
                            s.workspaces
                                .update(cx, |w, cx| w.open_table(key, &schema, &table, false, cx));
                        }
                    }
                };
                let console = {
                    let key = key.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(s) = AppState::try_global(cx).cloned() {
                            let (key, sql) = (key.clone(), select.clone());
                            s.workspaces
                                .update(cx, |w, cx| w.new_query_tab_with(key, sql, cx));
                        }
                    }
                };
                let ddl = {
                    let (key, schema, table) = (key.clone(), schema.clone(), table.clone());
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        show_ddl(key.clone(), schema.clone(), table.clone(), cx)
                    }
                };
                let refresh = {
                    let key = key.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        if let Some(s) = AppState::try_global(cx).cloned() {
                            let key = key.clone();
                            s.catalogs.update(cx, |c, cx| c.refresh(key, cx));
                        }
                    }
                };
                menu.item(item("Open in new tab", IconName::Table2).on_click(open))
                    .item(item("New console: select *", IconName::FileCode).on_click(console))
                    .item(item("Show DDL", IconName::ScrollText).on_click(ddl))
                    .item(item("Export to CSV…", IconName::FileDown).on_click({
                        let (key, schema, table) = (key.clone(), schema.clone(), table.clone());
                        move |_, _, cx| export_table(&key, &schema, &table, cx)
                    }))
                    .separator()
                    .item(copy("Copy name", IconName::Copy, table.clone()))
                    .item(copy(
                        "Copy qualified name",
                        IconName::Copy,
                        format!("{schema}.{table}"),
                    ))
                    .separator()
                    .item(item("Refresh", IconName::RefreshCw).on_click(refresh))
            }
            NodeKind::Column { name, .. } => {
                menu.item(copy("Copy name", IconName::Copy, name.clone()))
            }
            NodeKind::MetaItem {
                name, definition, ..
            } => menu
                .item(copy("Copy name", IconName::Copy, name.clone()))
                .item(copy(
                    "Copy definition",
                    IconName::FileCode,
                    definition.clone(),
                )),
            _ => {
                let _ = (weak, cx);
                menu
            }
        }
    }

    // ── Rows ─────────────────────────────────────────────────────────────

    fn render_rows(
        &mut self,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let nodes = self.nodes.clone();
        range
            .filter_map(|ix| nodes.get(ix).map(|n| self.render_row(ix, n, cx)))
            .collect()
    }

    fn render_row(&self, ix: usize, node: &Node, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        let focused = self.focused.as_deref() == Some(node.id.as_str());
        let open = Self::is_expanded(&node.id, cx);
        let surface = if focused {
            p.muted
        } else if node.current {
            // hover is translucent: the cutout shows the sidebar behind it.
            p.sidebar
        } else {
            p.sidebar
        };
        let chevron = div()
            .id(("chevron", ix))
            .flex_none()
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .text_color(p.dim)
            .when(node.expandable, |d| {
                d.hover(|s| s.text_color(p.body))
                    .child(
                        gpui_component::Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(px(14.)),
                    )
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        // Sidebar.tsx: the chevron toggles without the row's
                        // click (no focus move, no select); the tree keeps
                        // the keyboard focus.
                        cx.stop_propagation();
                        window.focus(&this.focus_handle, cx);
                        if let Some(node) = this.nodes.get(ix).cloned() {
                            this.toggle(&node, cx);
                        }
                    }))
            });
        let meta: Option<AnyElement> = match &node.meta {
            Meta::Scope(badge) => Some(self.scope_badge(ix, badge, cx)),
            Meta::Database { badge, loading } => Some(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .children(badge.as_ref().map(|b| self.scope_badge(ix, b, cx)))
                    .when(*loading, |d| d.child(row::meta_text("loading…", cx)))
                    .into_any_element(),
            ),
            other => row::node_meta(other, cx).map(IntoElement::into_any_element),
        };
        let mut el = div()
            .id(gpui::ElementId::Name(format!("node:{}", node.id).into()))
            .w_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(ROW_HEIGHT)
            .pl(px(6. + node.depth as f32 * 16.))
            .pr(px(8.))
            .rounded(theme::radius::MD)
            .text_size(theme::text::SM)
            .whitespace_nowrap()
            .cursor_default()
            .when(focused, |d| d.bg(p.muted).text_color(p.body))
            .when(!focused, |d| d.hover(|s| s.bg(p.hover)))
            .when(node.current && !focused, |d| {
                d.bg(p.hover).text_color(p.body)
            })
            .when(!focused && !node.current, |d| d.text_color(p.soft))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.on_row_click(ix, event, window, cx)
            }))
            .child(chevron)
            .children(row::node_icon(node, surface, cx))
            .child(row::node_label(node, cx).min_w_0())
            .children(meta);
        if let NodeKind::Connection(conn) = &node.kind {
            let target = conn.id.clone();
            let dragged = DraggedConnection {
                id: conn.id.clone(),
                name: conn.name.clone().into(),
            };
            el = el
                .on_drag(dragged, |d, _, _, cx| {
                    cx.new(|_| DragPreview(d.name.clone()))
                })
                .drag_over::<DraggedConnection>(move |s, _, _, _| s.bg(p.hover_strong))
                .on_drop(cx.listener(move |this, d: &DraggedConnection, _, cx| {
                    this.reorder(&d.id, &target, cx)
                }));
        }
        let has_menu = !matches!(
            node.kind,
            NodeKind::Status
                | NodeKind::NoDatabases
                | NodeKind::Schema { .. }
                | NodeKind::Group { .. }
                | NodeKind::Folder(_)
        );
        let el = if has_menu {
            el.context_menu(Self::context_menu(node.clone(), cx.entity().downgrade()))
                .into_any_element()
        } else {
            el.into_any_element()
        };
        // §mx-1.5 min-w-max§: the row spans the list (or its widest row).
        div().w_full().px(px(6.)).child(el).into_any_element()
    }

    /// Drops connection `dragged` on `target`'s place (Connections::reorder).
    pub fn reorder(&mut self, dragged: &str, target: &str, cx: &mut Context<Self>) {
        let mut ids: Vec<String> = connection_infos(cx).into_iter().map(|c| c.id).collect();
        let (Some(from), Some(to)) = (
            ids.iter().position(|i| i == dragged),
            ids.iter().position(|i| i == target),
        ) else {
            return;
        };
        if from == to {
            return;
        }
        let id = ids.remove(from);
        ids.insert(to, id);
        if let Some(state) = AppState::try_global(cx).cloned() {
            state.connections.update(cx, |c, cx| c.reorder(ids, cx));
        }
    }

    // ── Chrome ───────────────────────────────────────────────────────────

    fn render_top(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let row = div()
            .id("sidebar-top")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(40.))
            .pr(px(8.))
            .pl(chrome::traffic_lights_inset(window).max(px(8.)))
            .child(
                icon_button(
                    "sidebar-toggle",
                    IconName::PanelLeft,
                    16.,
                    "Hide sidebar (⌥B)",
                    cx,
                )
                .on_click(|_, _, cx| {
                    if let Some(state) = AppState::try_global(cx) {
                        let layout = state.layout.clone();
                        layout.update(cx, |l, cx| l.toggle(PanelKind::Sidebar, cx));
                    }
                }),
            )
            .child(
                div()
                    .ml(px(4.))
                    .truncate()
                    .text_size(theme::text::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.body)
                    .child("Database Explorer"),
            );
        self.drag.apply(row)
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette();
        let any_connected = connection_infos(cx).iter().any(ConnInfo::connected);
        // Sidebar.tsx: a `size-6` trigger with a `size-4` Plus in
        // text-muted-foreground (a button sized 16 / 0.75 gives its icon 16 px).
        let new_button = Button::new("explorer-new")
            .ghost()
            .with_size(px(16. / 0.75))
            .size(px(24.))
            .text_color(p.quiet)
            .icon(IconName::Plus)
            .dropdown_menu(move |menu, _, _| {
                // The title bar's "+" rows (chrome::menu_row), so both menus match.
                menu.min_w(px(220.))
                    .item(
                        chrome::menu_row(IconName::SquareTerminal, "Query console", "⌥N")
                            .disabled(!any_connected)
                            .on_click(|_, _, cx| new_console(cx)),
                    )
                    .item(
                        chrome::menu_row(IconName::Plug, "Data source", "⌘O")
                            .on_click(|_, window, cx| connection_dialog::open(None, window, cx)),
                    )
            });
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .h(px(32.))
            .px(px(8.))
            .border_b_1()
            .border_color(p.border)
            .child(div().id("explorer-new-wrap").child(new_button))
            .child(
                icon_button(
                    "explorer-refresh",
                    IconName::RefreshCw,
                    14.,
                    "Refresh selected database",
                    cx,
                )
                .on_click(|_, _, cx| refresh_active(cx)),
            )
            .child(
                icon_button(
                    "explorer-console",
                    IconName::SquareTerminal,
                    14.,
                    "Query console (⌥N)",
                    cx,
                )
                .when(!any_connected, |b| b.opacity(0.5).cursor_default())
                .on_click(move |_, _, cx| {
                    if any_connected {
                        new_console(cx)
                    }
                }),
            )
            .child(vsep(cx))
            .child(
                icon_button(
                    "explorer-collapse",
                    IconName::ChevronsDownUp,
                    14.,
                    "Collapse all",
                    cx,
                )
                .on_click(|_, _, cx| {
                    if let Some(state) = AppState::try_global(cx) {
                        let prefs = state.prefs.clone();
                        prefs.update(cx, |p, cx| p.update(cx, |p| p.tree_expanded.clear()));
                    }
                }),
            )
            .child(div().flex_1())
            .child(
                icon_button(
                    "explorer-prev-db",
                    IconName::ArrowLeft,
                    14.,
                    "Previous database (⌥↑)",
                    cx,
                )
                .on_click(|_, _, cx| cycle_database(-1, cx)),
            )
            .child(
                icon_button(
                    "explorer-next-db",
                    IconName::ArrowRight,
                    14.,
                    "Next database (⌥↓)",
                    cx,
                )
                .on_click(|_, _, cx| cycle_database(1, cx)),
            )
            .child(
                icon_button(
                    "explorer-find",
                    IconName::Search,
                    14.,
                    "Find table (⌘K)",
                    cx,
                )
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(actions::TogglePalette), cx)
                }),
            )
    }

    fn render_tree(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = cx.palette().clone();
        if self.nodes.is_empty() {
            return div()
                .id("explorer-tree")
                .flex_1()
                .min_h_0()
                .py(px(6.))
                .track_focus(&self.focus_handle)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .px(px(16.))
                        .py(px(32.))
                        .text_size(theme::text::SM)
                        .text_color(p.quiet)
                        .child("No data sources")
                        .child(
                            div()
                                .id("explorer-add-postgres")
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .h(px(28.))
                                .px(px(10.))
                                .rounded(theme::radius::MD)
                                .border_1()
                                .border_color(p.input)
                                .bg(p.card)
                                .text_color(p.soft)
                                .cursor_pointer()
                                .hover(|s| s.bg(p.hover_strong).text_color(p.body))
                                .on_click(|_, window, cx| connection_dialog::open(None, window, cx))
                                .child(icon(IconName::Plus, 14., p.soft))
                                .child("Add PostgreSQL"),
                        ),
                )
                .into_any_element();
        }
        let count = self.nodes.len();
        // The list is as wide as its widest row (measured from this one).
        let widest = self
            .nodes
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| {
                let meta = match &n.meta {
                    Meta::Text(t) | Meta::Detail(t) => t.chars().count(),
                    Meta::Column { ty, .. } => ty.chars().count() + 9,
                    _ => 8,
                };
                n.depth * 16 + (n.label.chars().count() + meta) * 8
            })
            .map(|(ix, _)| ix);
        div()
            .id("explorer-tree")
            .flex_1()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .child(
                uniform_list("explorer-rows", count, cx.processor(Self::render_rows))
                    .track_scroll(&self.scroll)
                    .with_width_from_item(widest)
                    .with_horizontal_sizing_behavior(
                        gpui::ListHorizontalSizingBehavior::Unconstrained,
                    )
                    .py(px(6.))
                    .size_full(),
            )
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.))
            .h(px(44.))
            .px(px(8.))
            .border_t_1()
            .border_color(p.border)
            .child(
                div()
                    .id("sidebar-settings")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(px(10.))
                    .h(px(30.))
                    .px(px(8.))
                    .rounded(theme::radius::MD)
                    .cursor_pointer()
                    .text_size(theme::text::BASE)
                    .text_color(p.soft)
                    .hover(|s| s.bg(p.hover).text_color(p.body))
                    .tooltip(chrome::tooltip("Settings (⌘,)"))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(|_, _, cx| {
                        if let Some(state) = AppState::try_global(cx) {
                            let layout = state.layout.clone();
                            layout.update(cx, |l, cx| {
                                l.set_settings(Some(SettingsPage::General), cx)
                            });
                        }
                    })
                    .child(icon(IconName::Settings, 16., p.quiet))
                    .child(div().truncate().child("Settings")),
            )
    }
}

/// A new console on [`new_query_target`] (the toolbar button, the + menu).
pub fn new_console(cx: &mut App) {
    if let (Some(key), Some(state)) = (new_query_target(cx), AppState::try_global(cx).cloned()) {
        state
            .workspaces
            .update(cx, |w, cx| w.new_query_tab(key, cx));
    }
}

/// The toolbar's Refresh: re-reads the selected database, or connects its
/// connection (Sidebar.tsx `refresh`).
pub fn refresh_active(cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let Some(active) = state.workspaces.read(cx).active.clone() else {
        return;
    };
    if state
        .connections
        .read(cx)
        .is_connected(&active.connection_id)
    {
        state.catalogs.update(cx, |c, cx| c.refresh(active, cx));
    } else {
        state
            .connections
            .update(cx, |c, cx| c.connect(&active.connection_id, cx));
    }
}

/// A connection's Refresh: re-reads its loaded databases.
pub fn refresh_connection(id: &str, cx: &mut App) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let keys: Vec<DbKey> = state
        .catalogs
        .read(cx)
        .catalogs
        .keys()
        .filter(|k| k.connection_id == id)
        .cloned()
        .collect();
    state.catalogs.update(cx, |c, cx| {
        for key in keys {
            c.refresh(key, cx);
        }
    });
}

fn cycle_database(step: isize, cx: &mut App) {
    if let Some(state) = AppState::try_global(cx).cloned() {
        state
            .workspaces
            .update(cx, |w, cx| w.cycle_database(step, cx));
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.palette().clone();
        self.nodes = Rc::new(Self::build_nodes(cx));
        if let Some(f) = &self.focused
            && !self.nodes.iter().any(|n| &n.id == f)
        {
            self.focused = None;
        }
        #[cfg(feature = "dev-hooks")]
        dev::on_render(self, window, cx);
        let picker = self.picker.as_ref().map(|o| {
            deferred(
                anchored()
                    .position(o.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(o.view.clone()),
            )
            .with_priority(1)
        });
        div()
            .key_context(crate::keymap::SIDEBAR)
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.navigate(NavKey::Up, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.navigate(NavKey::Down, cx)))
            .on_action(cx.listener(|this, _: &ExpandNode, _, cx| this.navigate(NavKey::Right, cx)))
            .on_action(cx.listener(|this, _: &CollapseNode, _, cx| this.navigate(NavKey::Left, cx)))
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.navigate(NavKey::Enter, cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.sidebar)
            .text_color(p.soft)
            .border_r_1()
            .border_color(p.border)
            .child(self.render_top(window, cx))
            .child(self.render_toolbar(cx))
            .child(self.render_tree(cx))
            .child(self.render_footer(cx))
            .children(picker)
    }
}
