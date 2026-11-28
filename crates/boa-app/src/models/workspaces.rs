//! [`Workspaces`]: the tabs of each database (store.ts `workspaces`, `active`,
//! `mru`, `seen`, `bottomView`, the Output log `history`; session.ts): consoles
//! and table tabs, the active tab, preview tabs, recently opened tables, and
//! the session-only Output log of each connection.
//!
//! Tab entities ([`Console`], [`TableTab`]) are owned here; Workspaces
//! re-notifies whenever one of them notifies, so views observe only this
//! model. [`Workspaces::to_session`] / [`Workspaces::restore`] map it to
//! `boa_core::state::session::Session` (`boa.session.v1`) for Phase 7.

use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use boa_core::state::history::OutputEntry;
use boa_core::state::prefs::Scope;
use boa_core::state::session::{self as core_session, ScopeSel, shown_databases};
use gpui::{App, AppContext as _, Context, Entity, EventEmitter, SharedString, Subscription};

use super::catalogs::DbKey;
use super::connections::ConnState;
use super::console::Console;
use super::table_tab::TableTab;
use crate::app_state::AppState;

/// A tab id (session.ts: `q12` for consoles, `t3` for tables; numbers
/// continue across launches, see [`Workspaces::reserve_ids`]).
pub type TabId = SharedString;

/// At most this many Output log lines per connection (store.ts `log`).
pub const OUTPUT_CAP: usize = 200;
/// At most this many recently opened tables (store.ts `openTable`).
pub const MRU_CAP: usize = 30;

/// What a tab shows (store.ts `Tab`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabKind {
    /// A console (`kind: 'query'`).
    Query { title: SharedString },
    /// A table or view (`kind: 'table'`); a preview tab is replaced by the next one opened.
    Table {
        schema: SharedString,
        table: SharedString,
        preview: bool,
    },
}

/// The entity behind a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabContent {
    Console(Entity<Console>),
    Table(Entity<TableTab>),
}

/// A tab in the title bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    pub kind: TabKind,
    pub content: TabContent,
}

impl Tab {
    /// store.ts `tabLabel`: the console's title, or the table's name.
    pub fn label(&self) -> SharedString {
        match &self.kind {
            TabKind::Query { title } => title.clone(),
            TabKind::Table { table, .. } => table.clone(),
        }
    }

    pub fn is_query(&self) -> bool {
        matches!(self.kind, TabKind::Query { .. })
    }

    pub fn console(&self) -> Option<&Entity<Console>> {
        match &self.content {
            TabContent::Console(c) => Some(c),
            TabContent::Table(_) => None,
        }
    }

    pub fn table(&self) -> Option<&Entity<TableTab>> {
        match &self.content {
            TabContent::Table(t) => Some(t),
            TabContent::Console(_) => None,
        }
    }

    /// A statement of this tab is in flight.
    pub fn is_running(&self, cx: &App) -> bool {
        match &self.content {
            TabContent::Console(c) => c.read(cx).running.is_some(),
            TabContent::Table(t) => t.read(cx).loading,
        }
    }
}

/// One database's tabs (store.ts `Workspace`).
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
}

impl Workspace {
    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id.as_ref() == id)
    }

    /// The active tab.
    pub fn active(&self) -> Option<&Tab> {
        self.tab(self.active_tab.as_deref()?)
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id.as_ref() == id)
    }
}

/// One line of the Output log (BottomPanel `OutputLog`): the entry, plus
/// the statement line and caret under the error position, psql-style.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputLine {
    pub entry: OutputEntry,
    pub excerpt: Option<(String, String)>,
}

/// What Workspaces tells its observers besides notifying.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkspacesEvent {
    /// A line was added to a connection's Output log; `record`: a statement
    /// run that the query history keeps (page fetches are not). Phase 7's
    /// History subscribes to this (`boa_core::state::history::to_record`).
    OutputLogged {
        connection_id: String,
        entry: OutputEntry,
        record: bool,
    },
}

/// Asked before a tab closes (store.ts `setCloseGuard`; Phase 5: pending
/// edits); false keeps it open (the guard may close it later with
/// [`Workspaces::close_tab_forced`]).
pub type CloseGuard = Rc<dyn Fn(&DbKey, &TabId, &mut App) -> bool>;

/// Every database's tabs, the selected database, the Output logs.
pub struct Workspaces {
    pub workspaces: HashMap<DbKey, Workspace>,
    pub active: Option<DbKey>,
    /// Recently opened tables, newest first (`tableKey`s).
    pub mru: Vec<String>,
    /// When each table was last opened (ms since the epoch), by `tableKey`.
    pub seen: HashMap<String, i64>,
    /// What the Database Output panel shows: a console's result (by tab id),
    /// or the Output log (None). store.ts `bottomView`.
    pub bottom_view: Option<TabId>,
    /// Output log per connection id, newest first.
    output: HashMap<String, Vec<OutputLine>>,
    next_id: u64,
    next_output_id: u64,
    close_guard: Option<CloseGuard>,
    tab_subscriptions: HashMap<TabId, Subscription>,
}

impl fmt::Debug for Workspaces {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workspaces")
            .field("workspaces", &self.workspaces)
            .field("active", &self.active)
            .field("mru", &self.mru)
            .field("bottom_view", &self.bottom_view)
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Default for Workspaces {
    fn default() -> Self {
        Self {
            workspaces: HashMap::new(),
            active: None,
            mru: Vec::new(),
            seen: HashMap::new(),
            bottom_view: None,
            output: HashMap::new(),
            next_id: 1,
            next_output_id: 1,
            close_guard: None,
            tab_subscriptions: HashMap::new(),
        }
    }
}

impl EventEmitter<WorkspacesEvent> for Workspaces {}

/// Milliseconds since the epoch, as the Tauri app's `Date.now()`.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |d| d.as_millis() as f64)
}

impl Workspaces {
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected database's tabs.
    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active.as_ref()?)
    }

    pub fn workspace(&self, key: &DbKey) -> Option<&Workspace> {
        self.workspaces.get(key)
    }

    /// The selected database's active tab.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active_workspace()?.active()
    }

    /// The tab `id` of any database, with its database.
    pub fn find_tab(&self, id: &str) -> Option<(&DbKey, &Tab)> {
        self.workspaces
            .iter()
            .find_map(|(k, ws)| ws.tab(id).map(|t| (k, t)))
    }

    /// The selected database's console tabs that have a result, in tab order
    /// (BottomPanel's result tabs).
    pub fn result_tabs(&self, cx: &App) -> Vec<Tab> {
        self.active_workspace()
            .map(|ws| {
                ws.tabs
                    .iter()
                    .filter(|t| t.console().is_some_and(|c| c.read(cx).result.is_some()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The console whose result the bottom panel shows, if any.
    pub fn shown_result(&self, cx: &App) -> Option<Entity<Console>> {
        let id = self.bottom_view.as_ref()?;
        let tab = self.active_workspace()?.tab(id)?;
        let console = tab.console()?;
        console.read(cx).result.is_some().then(|| console.clone())
    }

    /// Keeps new ids clear of restored ones (store.ts `reserveIds`).
    pub fn reserve_ids(&mut self, from: u64) {
        self.next_id = self.next_id.max(from);
    }

    fn new_id(&mut self, prefix: &str) -> TabId {
        let id = format!("{prefix}{}", self.next_id);
        self.next_id += 1;
        id.into()
    }

    /// Installs the close guard (Phase 5: pending edits).
    pub fn set_close_guard(&mut self, guard: Option<CloseGuard>) {
        self.close_guard = guard;
    }

    fn watch_tab(&mut self, id: &TabId, content: &TabContent, cx: &mut Context<Self>) {
        let sub = match content {
            TabContent::Console(c) => cx.observe(c, |_, _, cx| cx.notify()),
            TabContent::Table(t) => cx.observe(t, |_, _, cx| cx.notify()),
        };
        self.tab_subscriptions.insert(id.clone(), sub);
    }

    // ── Selection and tabs (store.ts) ─────────────────────────────────────

    /// Makes `key` the selected database, connecting its connection if it
    /// isn't (as DataGrip does) and loading its catalog (store.ts
    /// `selectDatabase`). Like the Tauri app it opens no tab by itself: an
    /// empty workspace shows "No tabs open".
    pub fn select_database(&mut self, key: DbKey, cx: &mut Context<Self>) {
        self.workspaces.entry(key.clone()).or_default();
        self.active = Some(key.clone());
        connect_and_load(key, cx);
        cx.notify();
    }

    /// A new console on `key` (⌘T / ⌥N), named after the lowest free
    /// `console_N`; selects the database and the tab.
    pub fn new_query_tab(&mut self, key: DbKey, cx: &mut Context<Self>) {
        self.new_query_tab_with(key, String::new(), cx);
    }

    /// A new console on `key` holding `sql` (Show DDL, history "open in new
    /// console", a table tab's "Open query console"); selects it.
    pub fn new_query_tab_with(&mut self, key: DbKey, sql: String, cx: &mut Context<Self>) {
        self.select_database(key.clone(), cx);
        let ws = self.workspaces.entry(key.clone()).or_default();
        let title: SharedString = next_console_title(ws).into();
        let id = self.new_id("q");
        let console = {
            let (key, id, title) = (key.clone(), id.clone(), title.clone());
            cx.new(|_| Console::new(key, id, title, sql))
        };
        self.add_tab(
            &key,
            Tab {
                id,
                kind: TabKind::Query { title },
                content: TabContent::Console(console),
            },
            None,
            cx,
        );
    }

    /// A new console on `key` holding `sql`, which it runs at once (a Redis
    /// key opened from the explorer).
    pub fn new_query_tab_running(&mut self, key: DbKey, sql: String, cx: &mut Context<Self>) {
        self.new_query_tab_with(key.clone(), sql.clone(), cx);
        let console = self
            .workspace(&key)
            .and_then(|ws| ws.active())
            .and_then(|t| t.console())
            .cloned();
        if let Some(console) = console {
            console.update(cx, |c, cx| c.run(sql, cx));
        }
    }

    /// Opens a table / view in a tab, selecting its database (store.ts
    /// `openTable`): an explorer click opens a preview tab (replacing the
    /// previous preview), a double click / Enter (`preview: false`) pins it.
    pub fn open_table(
        &mut self,
        key: DbKey,
        schema: &str,
        table: &str,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
        self.select_database(key.clone(), cx);
        let tk = core_session::table_key(&key.connection_id, &key.database, schema, table);
        self.mru.retain(|k| k != &tk);
        self.mru.insert(0, tk.clone());
        self.mru.truncate(MRU_CAP);
        self.seen.insert(tk, now_ms() as i64);
        let ws = self.workspaces.entry(key.clone()).or_default();
        let existing = ws.tabs.iter_mut().find(|t| {
            matches!(&t.kind, TabKind::Table { schema: s, table: n, .. } if s.as_ref() == schema && n.as_ref() == table)
        });
        if let Some(tab) = existing {
            if !preview && let TabKind::Table { preview: p, .. } = &mut tab.kind {
                *p = false;
            }
            ws.active_tab = Some(tab.id.clone());
            cx.notify();
            return;
        }
        let replace = if preview {
            ws.tabs
                .iter()
                .position(|t| matches!(t.kind, TabKind::Table { preview: true, .. }))
        } else {
            None
        };
        let id = self.new_id("t");
        let entity = {
            let (key, id) = (key.clone(), id.clone());
            let (schema, table) = (schema.to_owned(), table.to_owned());
            cx.new(|cx| TableTab::new(key, id, schema, table, cx))
        };
        self.add_tab(
            &key,
            Tab {
                id,
                kind: TabKind::Table {
                    schema: schema.to_owned().into(),
                    table: table.to_owned().into(),
                    preview,
                },
                content: TabContent::Table(entity),
            },
            replace,
            cx,
        );
    }

    /// Adds `tab` to `key`'s workspace (at `replace`, dropping the tab
    /// there) and activates it.
    fn add_tab(&mut self, key: &DbKey, tab: Tab, replace: Option<usize>, cx: &mut Context<Self>) {
        self.watch_tab(&tab.id, &tab.content, cx);
        let id = tab.id.clone();
        let ws = self.workspaces.entry(key.clone()).or_default();
        match replace {
            Some(i) if i < ws.tabs.len() => {
                let old = std::mem::replace(&mut ws.tabs[i], tab);
                self.tab_subscriptions.remove(&old.id);
                if self.bottom_view.as_ref() == Some(&old.id) {
                    self.bottom_view = None;
                }
            }
            _ => ws.tabs.push(tab),
        }
        if let Some(ws) = self.workspaces.get_mut(key) {
            ws.active_tab = Some(id);
        }
        cx.notify();
    }

    /// Activates tab `id` of `key` (a click on it).
    pub fn activate(&mut self, key: &DbKey, id: &str, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(key)
            && let Some(tab) = ws.tab(id)
        {
            ws.active_tab = Some(tab.id.clone());
            cx.notify();
        }
    }

    /// Activates the n-th tab (0-based) of the selected database (⌥1–⌥9);
    /// false when there is none.
    pub fn activate_index(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active.clone() else {
            return false;
        };
        let Some(id) = self
            .workspaces
            .get(&key)
            .and_then(|ws| ws.tabs.get(index))
            .map(|t| t.id.clone())
        else {
            return false;
        };
        self.activate(&key, &id, cx);
        true
    }

    /// Pins a preview tab (double click on it).
    pub fn pin_tab(&mut self, key: &DbKey, id: &str, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspaces.get_mut(key)
            && let Some(tab) = ws.tabs.iter_mut().find(|t| t.id.as_ref() == id)
            && let TabKind::Table { preview, .. } = &mut tab.kind
        {
            *preview = false;
            cx.notify();
        }
    }

    /// Moves tab `id` to index `to` (dragging it in the strip).
    pub fn move_tab(&mut self, key: &DbKey, id: &str, to: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get_mut(key) else {
            return;
        };
        let Some(from) = ws.index_of(id) else { return };
        if to >= ws.tabs.len() || from == to {
            return;
        }
        let tab = ws.tabs.remove(from);
        ws.tabs.insert(to, tab);
        cx.notify();
    }

    /// Closes a tab (×, middle click, ⌘W), asking the close guard first
    /// (Phase 5: pending edits). Its running statement is cancelled and its
    /// result goes; the tab to its right (else left) becomes active.
    pub fn close_tab(&mut self, key: DbKey, tab: TabId, cx: &mut Context<Self>) {
        if let Some(guard) = self.close_guard.clone()
            && !guard(&key, &tab, cx)
        {
            return;
        }
        self.close_tab_forced(key, tab, cx);
    }

    /// Closes a tab without asking the guard.
    pub fn close_tab_forced(&mut self, key: DbKey, tab: TabId, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get_mut(&key) else {
            return;
        };
        let Some(i) = ws.index_of(&tab) else { return };
        let removed = ws.tabs.remove(i);
        if ws.active_tab.as_ref() == Some(&tab) {
            ws.active_tab = ws
                .tabs
                .get(i.min(ws.tabs.len().saturating_sub(1)))
                .map(|t| t.id.clone());
        }
        if self.bottom_view.as_ref() == Some(&tab) {
            self.bottom_view = None;
        }
        self.tab_subscriptions.remove(&tab);
        if let TabContent::Console(console) = &removed.content {
            console.update(cx, |c, cx| c.cancel(cx));
        }
        cx.notify();
    }

    /// Drops a deleted connection's workspaces (store.ts `deleteConnection`):
    /// their tabs go (running statements cancelled), and the selection when
    /// it was one of its databases. Wired to `ConnectionsEvent::Deleted` in
    /// `AppState::init`.
    pub fn forget_connection(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        let keys: Vec<DbKey> = self
            .workspaces
            .keys()
            .filter(|k| k.connection_id == connection_id)
            .cloned()
            .collect();
        let was_active = self
            .active
            .as_ref()
            .is_some_and(|a| a.connection_id == connection_id);
        if keys.is_empty() && !was_active {
            return;
        }
        for key in keys {
            let Some(ws) = self.workspaces.remove(&key) else {
                continue;
            };
            for tab in ws.tabs {
                self.tab_subscriptions.remove(&tab.id);
                if self.bottom_view.as_ref() == Some(&tab.id) {
                    self.bottom_view = None;
                }
                if let TabContent::Console(console) = &tab.content {
                    console.update(cx, |c, cx| c.cancel(cx));
                }
            }
        }
        if was_active {
            self.active = None;
        }
        cx.notify();
    }

    /// Closes the selected database's active tab (⌘W); false when there is none.
    pub fn close_active(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.active.clone() else {
            return false;
        };
        let Some(id) = self
            .workspaces
            .get(&key)
            .and_then(|ws| ws.active_tab.clone())
        else {
            return false;
        };
        self.close_tab(key, id, cx);
        true
    }

    /// Selects the previous / next database (⌥↑ / ⌥↓) among every shown
    /// database of every connection, in sidebar order (store.ts `cycleDatabase`).
    pub fn cycle_database(&mut self, step: isize, cx: &mut Context<Self>) {
        let list = all_databases(cx);
        if list.is_empty() {
            return;
        }
        let n = list.len() as isize;
        let i = self
            .active
            .as_ref()
            .and_then(|a| list.iter().position(|k| k == a))
            .map_or(-1, |i| i as isize);
        let next = (i + step).rem_euclid(n) as usize;
        self.select_database(list[next].clone(), cx);
    }

    /// Where "New query" (⌥N) goes: the selected database if its connection
    /// is connected, else the first connected connection's first database
    /// (Sidebar.tsx `newQueryTarget`).
    pub fn new_query_target(&self, cx: &App) -> Option<DbKey> {
        let state = AppState::try_global(cx)?;
        let conns = state.connections.read(cx);
        if let Some(active) = &self.active
            && conns.is_connected(&active.connection_id)
        {
            return Some(active.clone());
        }
        conns.saved.iter().find_map(|c| {
            let id = &c.config.id;
            match conns.state_of(id) {
                ConnState::Connected { databases, .. } => databases.first().map(|d| DbKey {
                    connection_id: id.clone(),
                    database: d.clone(),
                }),
                _ => None,
            }
        })
    }

    // ── Database Output ───────────────────────────────────────────────────

    /// Shows a console's result (`Some(tab id)`) or the Output log in the
    /// Database Output panel.
    pub fn set_bottom_view(&mut self, view: Option<TabId>, cx: &mut Context<Self>) {
        if self.bottom_view != view {
            self.bottom_view = view;
            cx.notify();
        }
    }

    /// The connection's Output log, newest first.
    pub fn output(&self, connection_id: &str) -> &[OutputLine] {
        self.output.get(connection_id).map_or(&[], Vec::as_slice)
    }

    /// Adds a line to a connection's Output log (store.ts `log`; at most
    /// [`OUTPUT_CAP`]); `record`: also for the saved query history (not
    /// for page fetches). The entry's id is assigned here.
    pub fn log(
        &mut self,
        connection_id: &str,
        mut entry: OutputEntry,
        excerpt: Option<(String, String)>,
        record: bool,
        cx: &mut Context<Self>,
    ) {
        entry.id = self.next_output_id;
        self.next_output_id += 1;
        let lines = self.output.entry(connection_id.to_owned()).or_default();
        lines.insert(
            0,
            OutputLine {
                entry: entry.clone(),
                excerpt,
            },
        );
        lines.truncate(OUTPUT_CAP);
        cx.emit(WorkspacesEvent::OutputLogged {
            connection_id: connection_id.to_owned(),
            entry,
            record,
        });
        cx.notify();
    }

    /// "Clear output" in the Output log.
    pub fn clear_output(&mut self, connection_id: &str, cx: &mut Context<Self>) {
        self.output.remove(connection_id);
        cx.notify();
    }

    // ── Session (Phase 7) ─────────────────────────────────────────────────

    /// The tabs as `session.json` stores them (`boa.session.v1`: active,
    /// workspaces with each console's SQL and each table tab's filter / sort,
    /// MRU, seen). Panel flags and `wanted` are the Session model's.
    pub fn to_session(&self, cx: &App) -> core_session::Session {
        let mut s = core_session::Session {
            active: self.active.as_ref().map(|k| core_session::Active {
                connection_id: k.connection_id.clone(),
                database: k.database.clone(),
            }),
            mru: self.mru.clone(),
            ..Default::default()
        };
        let mut keys: Vec<&DbKey> = self.workspaces.keys().collect();
        keys.sort_by(|a, b| (&a.connection_id, &a.database).cmp(&(&b.connection_id, &b.database)));
        for key in keys {
            let ws = &self.workspaces[key];
            let tabs = ws
                .tabs
                .iter()
                .map(|t| match (&t.kind, &t.content) {
                    (TabKind::Query { title }, TabContent::Console(c)) => {
                        core_session::Tab::Query(core_session::QueryTab {
                            id: t.id.to_string(),
                            title: title.to_string(),
                            sql: c.read(cx).sql.clone(),
                        })
                    }
                    (
                        TabKind::Table {
                            schema,
                            table,
                            preview,
                        },
                        content,
                    ) => {
                        let (w, o) = match content {
                            TabContent::Table(tt) => {
                                let tt = tt.read(cx);
                                (tt.where_clause.clone(), tt.order_by.clone())
                            }
                            TabContent::Console(_) => (String::new(), String::new()),
                        };
                        let some = |s: String| (!s.trim().is_empty()).then(|| s.trim().to_owned());
                        core_session::Tab::Table(core_session::TableTab {
                            id: t.id.to_string(),
                            schema: schema.to_string(),
                            table: table.to_string(),
                            preview: *preview,
                            r#where: some(w),
                            order_by: some(o),
                        })
                    }
                    (TabKind::Query { title }, TabContent::Table(_)) => {
                        core_session::Tab::Query(core_session::QueryTab {
                            id: t.id.to_string(),
                            title: title.to_string(),
                            sql: String::new(),
                        })
                    }
                })
                .collect();
            s.workspaces.insert(
                core_session::db_key(&key.connection_id, &key.database),
                core_session::Workspace {
                    tabs,
                    active_tab: ws.active_tab.as_ref().map(|t| t.to_string()),
                },
            );
        }
        let mut seen: Vec<(&String, &i64)> = self.seen.iter().collect();
        seen.sort();
        for (k, v) in seen {
            s.seen.insert(k.clone(), *v);
        }
        s
    }

    /// Restores the tabs of a saved session (results are not re-run); new
    /// tab ids continue after the restored ones (`next_tab_number`).
    pub fn restore(&mut self, session: &core_session::Session, cx: &mut Context<Self>) {
        self.reserve_ids(core_session::next_tab_number(session));
        for (db, saved) in &session.workspaces {
            let mut parts = db.split('\u{0}');
            let key = DbKey {
                connection_id: parts.next().unwrap_or_default().to_owned(),
                database: parts.next().unwrap_or_default().to_owned(),
            };
            for t in &saved.tabs {
                let tab = match t {
                    core_session::Tab::Query(q) => {
                        let (k, id, title) = (
                            key.clone(),
                            SharedString::from(q.id.clone()),
                            SharedString::from(q.title.clone()),
                        );
                        let sql = q.sql.clone();
                        let console = {
                            let (id, title) = (id.clone(), title.clone());
                            cx.new(|_| Console::new(k, id, title, sql))
                        };
                        Tab {
                            id,
                            kind: TabKind::Query { title },
                            content: TabContent::Console(console),
                        }
                    }
                    core_session::Tab::Table(tt) => {
                        let id = SharedString::from(tt.id.clone());
                        let entity = {
                            let (k, id) = (key.clone(), id.clone());
                            let (schema, table) = (tt.schema.clone(), tt.table.clone());
                            let (w, o) = (
                                tt.r#where.clone().unwrap_or_default(),
                                tt.order_by.clone().unwrap_or_default(),
                            );
                            cx.new(|cx| {
                                let mut t = TableTab::new(k, id, schema, table, cx);
                                t.where_clause = w;
                                t.order_by = o;
                                t
                            })
                        };
                        Tab {
                            id,
                            kind: TabKind::Table {
                                schema: tt.schema.clone().into(),
                                table: tt.table.clone().into(),
                                preview: tt.preview,
                            },
                            content: TabContent::Table(entity),
                        }
                    }
                };
                self.watch_tab(&tab.id, &tab.content, cx);
                self.workspaces
                    .entry(key.clone())
                    .or_default()
                    .tabs
                    .push(tab);
            }
            let ws = self.workspaces.entry(key.clone()).or_default();
            ws.active_tab = saved
                .active_tab
                .as_ref()
                .and_then(|id| ws.tab(id).map(|t| t.id.clone()));
        }
        self.active = session.active.as_ref().map(|a| DbKey {
            connection_id: a.connection_id.clone(),
            database: a.database.clone(),
        });
        self.mru = session.mru.clone();
        self.seen = session.seen.iter().map(|(k, v)| (k.clone(), *v)).collect();
        cx.notify();
    }
}

/// The lowest free `console_N` of a workspace (store.ts `newQueryTab`).
pub fn next_console_title(ws: &Workspace) -> String {
    let used: Vec<&str> = ws
        .tabs
        .iter()
        .filter_map(|t| match &t.kind {
            TabKind::Query { title } => Some(title.as_ref()),
            TabKind::Table { .. } => None,
        })
        .collect();
    let mut n = 1;
    while used.contains(&format!("console_{n}").as_str()) {
        n += 1;
    }
    format!("console_{n}")
}

/// Every shown database of every connection, in sidebar order (store.ts
/// `allDatabases`: prefs `scopes`, default the connection's own database).
pub fn all_databases(cx: &App) -> Vec<DbKey> {
    let Some(state) = AppState::try_global(cx) else {
        return Vec::new();
    };
    let conns = state.connections.read(cx);
    let prefs = state.prefs.read(cx).get();
    conns
        .saved
        .iter()
        .flat_map(|c| {
            let id = &c.config.id;
            let names = conns.databases(id);
            let pick = prefs.scopes.get(id).map(|s| match s {
                Scope::All => ScopeSel::All,
                Scope::Some(v) => ScopeSel::Names(v),
            });
            shown_databases(pick, &c.config.default_database(), &names)
                .into_iter()
                .map(|d| DbKey {
                    connection_id: id.clone(),
                    database: d,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Connects `key`'s connection when it is disconnected or failed, then
/// loads the database's catalog (store.ts `selectDatabase`).
fn connect_and_load(key: DbKey, cx: &mut Context<Workspaces>) {
    let Some(state) = AppState::try_global(cx).cloned() else {
        return;
    };
    let id = key.connection_id.clone();
    let needs_connect = matches!(
        state.connections.read(cx).state_of(&id),
        ConnState::Disconnected | ConnState::Failed { .. }
    );
    if !needs_connect {
        state.catalogs.update(cx, |c, cx| c.load(key, false, cx));
        return;
    }
    let connect = state
        .connections
        .update(cx, |c, cx| c.ensure_connected(&id, cx));
    let catalogs = state.catalogs.clone();
    cx.spawn(async move |_, cx| {
        if connect.await.is_ok() {
            catalogs.update(cx, |c, cx| c.load(key, false, cx));
        }
    })
    .detach();
}

#[cfg(test)]
mod tests;
